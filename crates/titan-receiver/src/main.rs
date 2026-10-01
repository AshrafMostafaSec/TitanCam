mod clock;
mod control_api;
mod local;
mod reorder;
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
    net::UnixStream,
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
            let config = StreamConfig::profile(&profile, false);
            let hub = control_api::Hub::new(config.clone(), mute);
            let receiver = local::run(
                bind,
                advertise,
                config,
                OutputOptions {
                    preview,
                    webcam,
                    software,
                },
                !no_usb,
                hub.clone(),
            );
            tokio::select! {
                result=receiver=>result,
                result=control_api::serve(hub)=>result,
            }
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
    let mut saved_at = Instant::now();
    let mut feedback = tokio::time::interval(Duration::from_millis(100));
    let mut clock = tokio::time::interval(Duration::from_secs(1));
    let mut last_control = Instant::now();
    let mut previous_decode = (Instant::now(), 0u64);
    loop {
        tokio::select! {
                msg=incoming.recv()=>{
                    let msg=msg.context("control reader ended")??;
                    ensure!(msg.session_id==hex::encode(ctx.id)&&msg.transport_epoch==1,"stale control message");
                    last_control=Instant::now();
                    match msg.kind.as_str() {
                        "StartAck" => {
                            let host=msg.body["host_epoch_ns"].as_str().and_then(|s|s.parse().ok()).context("invalid capture clock epoch")?;
                            *ctx.host_epoch.lock().unwrap()=Some(host);ctx.live.store(true,Ordering::Release);
                            write_control(&mut writer,&Control::new("StreamingReady",&hex::encode(ctx.id),serde_json::json!({}))).await?;
                        },
                        "ConfigureAck" => {
                            let effective:StreamConfig=serde_json::from_value(msg.body.clone())?;
                            ensure!(effective.validate(),"invalid updated configuration");
                            let is_current = {
                                let current=ctx.config.lock().unwrap();
                                effective.config_id >= current.config_id
                            };
                            if !is_current { continue; }
                            *ctx.config.lock().unwrap()=effective.clone();
                            if let Some(capabilities)=msg.body.get("capabilities") { *ctx.capabilities.lock().unwrap()=capabilities.clone(); }
                            let mut applied=Control::new("ConfigureApplied",&hex::encode(ctx.id),serde_json::json!({"config_id":effective.config_id}));
                            applied.request_id=msg.request_id.clone();
                            write_control(&mut writer,&applied).await?;
                            let mut pending=ctx.pending_config.lock().unwrap();
                            if pending.as_ref().is_some_and(|(id,_)| id==&msg.request_id) && let Some((_,sender))=pending.take() { let _=sender.send(Ok(effective)); }
                        },
                        "ConfigureError" => {
                            let mut pending=ctx.pending_config.lock().unwrap();
                            if pending.as_ref().is_some_and(|(id,_)| id==&msg.request_id) && let Some((_,sender))=pending.take() { let _=sender.send(Err(msg.body["reason"].as_str().unwrap_or("configuration rejected").to_owned())); }
                        },
                        "Capabilities"=>*ctx.capabilities.lock().unwrap()=msg.body,
                        "ClockPong"=>ctx.update_clock(&msg.body),
                        "Heartbeat"=>(),
                        "Error"|"Stop"=>{
                            ctx.live.store(false,Ordering::Release);
                            ctx.stats.lock().unwrap().last_error=msg.body["reason"].as_str().unwrap_or("phone interrupted capture").to_owned();
                            anyhow::bail!("phone interrupted capture");
                        },
                        _=>(),
                    }
                }
                msg=commands.recv()=>{if let Some(msg)=msg{tokio::time::timeout(Duration::from_secs(2),write_control(&mut writer,&msg)).await??}else{break}},
                _=feedback.tick()=>{ensure!(!ctx.cancelled.load(Ordering::Acquire),"media session cancelled");if last_control.elapsed()>Duration::from_secs(3){ctx.live.store(false,Ordering::Release);anyhow::bail!("control heartbeat timeout")}let stats={
            let mut stats=ctx.stats.lock().unwrap();
            if let Some((base,counter))=&*ctx.decoded_counter.lock().unwrap(){stats.decoded_video=base+counter.load(Ordering::Relaxed);}
            let elapsed=previous_decode.0.elapsed().as_secs_f64();
            if elapsed>=1.0 { stats.decoded_fps=stats.decoded_video.saturating_sub(previous_decode.1) as f64/elapsed; previous_decode=(Instant::now(),stats.decoded_video); }
            stats.clone()
        };let message=Control::new("Feedback",&hex::encode(ctx.id),serde_json::to_value(&stats)?);tokio::time::timeout(Duration::from_secs(2),write_control(&mut writer,&message)).await??;if ctx.live.load(Ordering::Acquire)&&ctx.last_media.lock().unwrap().elapsed()>Duration::from_millis(500){ctx.request("RequestIDR",serde_json::json!({"reason":"media_watchdog"}));}
        if saved_at.elapsed()>=Duration::from_secs(1){let directory=titan_transport::state_dir();
            let snapshot:Result<()> = async {
                use std::os::unix::fs::PermissionsExt;
                tokio::fs::create_dir_all(&directory).await?;
                tokio::fs::set_permissions(&directory,std::fs::Permissions::from_mode(0o700)).await?;
                tokio::fs::write(directory.join("stats.json.tmp"),serde_json::to_vec(&stats)?).await?;
                tokio::fs::rename(directory.join("stats.json.tmp"),directory.join("stats.json")).await?;
                Ok(())
            }.await;
            // Diagnostic storage failures must not interrupt capture or outputs.
            if let Err(error)=snapshot {tracing::warn!("stats snapshot unavailable: {error}");}
            saved_at=Instant::now();}},
                _=clock.tick()=>{let msg=Control::new("ClockPing",&hex::encode(ctx.id),serde_json::json!({"r1":session::now_ns().to_string()}));tokio::time::timeout(Duration::from_secs(2),write_control(&mut writer,&msg)).await??;}
                }
    }
    ctx.live.store(false, Ordering::Release);
    Ok(())
}
async fn usb_stream(device: &str, port: u16) -> Result<UnixStream> {
    let d = device.to_string();
    let owned: OwnedFd =
        tokio::task::spawn_blocking(move || titan_usb::connect(&d, port)).await??;
    usb_stream_from_fd(owned)
}
fn usb_stream_from_fd(owned: OwnedFd) -> Result<UnixStream> {
    // libusbmuxd's local daemon connection is AF_UNIX. The device-side TCP
    // tunnel does not make this descriptor a TCP socket; TCP_NODELAY fails.
    let stream = std::os::unix::net::UnixStream::from(owned);
    stream.set_nonblocking(true)?;
    Ok(UnixStream::from_std(stream)?)
}

#[cfg(test)]
mod usb_adapter_tests {
    use super::*;
    #[tokio::test]
    async fn unix_daemon_fd_transfers_framed_control_without_tcp_socket_options() {
        let (client, server) = std::os::unix::net::UnixStream::pair().unwrap();
        let mut client = usb_stream_from_fd(client.into()).unwrap();
        let mut server = usb_stream_from_fd(server.into()).unwrap();
        let message = Control::new(
            "Hello",
            &"11".repeat(16),
            serde_json::json!({"transport_version": 2}),
        );
        write_control(&mut server, &message).await.unwrap();
        let received = tokio::time::timeout(Duration::from_secs(1), read_control(&mut client))
            .await
            .unwrap()
            .unwrap();
        assert_eq!(received.kind, "Hello");
        assert_eq!(received.body["transport_version"], 2);
    }
}
