mod local;
mod session;
use anyhow::{Context as _, Result, ensure};
use clap::{Parser, Subcommand};
use session::{Context, OutputOptions};
use std::{
    net::IpAddr,
    os::fd::OwnedFd,
    sync::{Arc, atomic::Ordering},
    time::{Duration, Instant},
};
use titan_protocol::{Control, StreamConfig};
use titan_transport::{read_control, write_control};
use tokio::{
    io::{AsyncRead, AsyncWrite},
    net::TcpStream,
    sync::mpsc,
};
#[derive(Parser)]
#[command(version, about = "TitanCam local iPhone camera receiver")]
struct Cli {
    #[command(subcommand)]
    command: Commands,
}
#[derive(Subcommand)]
enum Commands {
    Doctor {
        #[arg(long)]
        decode: bool,
        #[arg(long)]
        microphone: bool,
    },
    Devices,
    /// Plain local TCP/UDP receiver with automatic USB and Bonjour discovery.
    #[command(alias = "run")]
    Local {
        #[arg(long, default_value = "0.0.0.0")]
        bind: IpAddr,
        #[arg(long)]
        advertise: Option<String>,
        #[arg(long, default_value="saver", value_parser=["saver","balanced","maximum"])]
        profile: String,
        #[arg(long)]
        no_usb: bool,
        #[arg(long)]
        preview: bool,
        #[arg(long)]
        webcam: Option<String>,
        #[arg(long)]
        software: bool,
        #[arg(long)]
        mute: bool,
    },
    Stats,
}
#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_writer(std::io::stderr)
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env().unwrap_or_else(|_| "info".into()),
        )
        .init();
    let state = titan_transport::state_dir();
    std::fs::create_dir_all(&state)?;
    use std::os::unix::fs::PermissionsExt;
    std::fs::set_permissions(state, std::fs::Permissions::from_mode(0o700))?;
    match Cli::parse().command {
        Commands::Doctor { decode, microphone } => {
            for p in ["gstreamer-1.0", "libpipewire-0.3", "libusbmuxd-2.0", "opus"] {
                match std::process::Command::new("pkg-config")
                    .args(["--modversion", p])
                    .output()
                {
                    Ok(o) if o.status.success() => {
                        println!("{p}: {}", String::from_utf8_lossy(&o.stdout).trim());
                    }
                    _ => println!(
                        "{p}: development version metadata unavailable; use runtime probes below"
                    ),
                }
            }
            match titan_usb::devices() {
                Ok(devices) => println!("USB devices: {}", devices.len()),
                Err(_) => println!("USB enumeration unavailable; check usbmuxd and device trust"),
            };
            if decode {
                println!("{}", titan_media::decode_probe()?)
            }
            if microphone {
                println!("{}", titan_media::microphone_probe()?)
            }
            Ok(())
        }
        Commands::Devices => {
            for (id, device) in titan_usb::devices()?.iter().enumerate() {
                println!("{}: {}", id + 1, device)
            }
            Ok(())
        }
        Commands::Local {
            bind,
            advertise,
            profile,
            no_usb,
            preview,
            webcam,
            software,
            mute,
        } => {
            local::run(
                bind,
                advertise,
                StreamConfig::profile(&profile, false),
                OutputOptions {
                    preview,
                    webcam,
                    software,
                    mute,
                },
                !no_usb,
            )
            .await
        }
        Commands::Stats => {
            println!(
                "{}",
                std::fs::read_to_string(titan_transport::state_dir().join("stats.json"))
                    .context("no active receiver statistics yet")?
            );
            Ok(())
        }
    }
}
async fn control_session<R: AsyncRead + Unpin + Send + 'static, W: AsyncWrite + Unpin>(
    ctx: Arc<Context>,
    mut reader: R,
    (mut writer, mut commands): (W, mpsc::Receiver<Control>),
) -> Result<()> {
    let _lease = session::Lease(Some(ctx.clone()));
    // A persistent reader preserves partially read records across timer events.
    let (messages, mut incoming) = mpsc::channel(32);
    let task = tokio::spawn(async move {
        loop {
            let message = read_control(&mut reader).await;
            let failed = message.is_err();
            if messages.send(message).await.is_err() || failed {
                break;
            }
        }
    });
    struct ReaderGuard(tokio::task::JoinHandle<()>);
    impl Drop for ReaderGuard {
        fn drop(&mut self) {
            self.0.abort();
        }
    }
    let _reader_guard = ReaderGuard(task);
    let mut saved_at = Instant::now() - Duration::from_secs(1);
    let mut feedback = tokio::time::interval(Duration::from_millis(100));
    let mut clock = tokio::time::interval(Duration::from_secs(1));
    let mut last_control = Instant::now();
    loop {
        tokio::select! {
                msg=incoming.recv()=>{let msg=msg.context("control reader ended")??;ensure!(msg.session_id==hex::encode(ctx.id)&&msg.transport_epoch==1,"stale control message");last_control=Instant::now();match msg.kind.as_str(){"StartAck"=>{let host=msg.body["host_epoch_ns"].as_str().and_then(|s|s.parse().ok()).context("invalid capture clock epoch")?;*ctx.host_epoch.lock().unwrap()=Some(host);ctx.live.store(true,Ordering::Release);write_control(&mut writer,&Control::new("StreamingReady",&hex::encode(ctx.id),serde_json::json!({}))).await?;},"ConfigureAck"=>{let effective:StreamConfig=serde_json::from_value(msg.body)?;ensure!(effective.validate(),"invalid updated configuration");*ctx.config.lock().unwrap()=effective;write_control(&mut writer,&Control::new("ConfigureApplied",&hex::encode(ctx.id),serde_json::json!({}))).await?;},"ClockPong"=>ctx.update_clock(&msg.body),"Heartbeat"|"Capabilities"=>(),"Error"|"Stop"=>{ctx.live.store(false,Ordering::Release);tracing::warn!("phone interrupted capture");},_=>()}}
                msg=commands.recv()=>{if let Some(msg)=msg{tokio::time::timeout(Duration::from_secs(2),write_control(&mut writer,&msg)).await??}else{break}},
                _=feedback.tick()=>{ensure!(!ctx.cancelled.load(Ordering::Acquire),"media session cancelled");if last_control.elapsed()>Duration::from_secs(3){ctx.live.store(false,Ordering::Release);anyhow::bail!("control heartbeat timeout")}let stats=ctx.stats.lock().unwrap().clone();let message=Control::new("Feedback",&hex::encode(ctx.id),serde_json::to_value(&stats)?);tokio::time::timeout(Duration::from_secs(2),write_control(&mut writer,&message)).await??;if ctx.live.load(Ordering::Acquire)&&ctx.last_media.lock().unwrap().elapsed()>Duration::from_millis(500){ctx.request("RequestIDR",serde_json::json!({"reason":"media_watchdog"}));}
        if saved_at.elapsed()>=Duration::from_secs(1){tokio::fs::write(titan_transport::state_dir().join("stats.json"),serde_json::to_vec(&stats)?).await?;saved_at=Instant::now();}},
                _=clock.tick()=>{let msg=Control::new("ClockPing",&hex::encode(ctx.id),serde_json::json!({"r1":session::now_ns().to_string()}));tokio::time::timeout(Duration::from_secs(2),write_control(&mut writer,&msg)).await??;}
                }
    }
    ctx.live.store(false, Ordering::Release);
    Ok(())
}
async fn usb_tcp(device: &str, port: u16) -> Result<TcpStream> {
    let d = device.to_string();
    let owned: OwnedFd =
        tokio::task::spawn_blocking(move || titan_usb::connect(&d, port)).await??;
    let stream = std::net::TcpStream::from(owned);
    stream.set_nonblocking(true)?;
    stream.set_nodelay(true)?;
    Ok(TcpStream::from_std(stream)?)
}
