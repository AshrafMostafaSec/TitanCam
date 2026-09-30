//! Synthetic peer: real TLS/QUIC, generated H.264 GOP, intentional reference loss.
use anyhow::{Context, Result, ensure};
use clap::Parser;
use gstreamer::{self as gst, prelude::*};
use std::{
    path::PathBuf,
    time::{Duration, Instant},
};
use titan_protocol::{Control, MediaHeader, StreamConfig};
use titan_transport::{Identity, read_control, write_control};
use tokio::io::{AsyncBufReadExt, BufReader};
#[derive(Parser)]
struct Args {
    #[arg(long)]
    hardware: bool,
    #[arg(long, default_value = "target/release/titan-receiver")]
    receiver: PathBuf,
}
struct Temp(PathBuf);
impl Drop for Temp {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}
fn frames() -> Result<Vec<(Vec<u8>, bool)>> {
    gst::init()?;
    let encoder = if gst::ElementFactory::find("x264enc").is_some() {
        "x264enc tune=zerolatency speed-preset=ultrafast key-int-max=30 byte-stream=true"
    } else {
        "openh264enc gop-size=30"
    };
    let pipeline = gst::parse::launch(&format!("videotestsrc num-buffers=90 pattern=ball ! video/x-raw,format=I420,width=320,height=180,framerate=30/1 ! {encoder} ! h264parse ! video/x-h264,stream-format=byte-stream,alignment=au ! appsink name=out sync=false max-buffers=1"))?.downcast::<gst::Pipeline>().map_err(|_|anyhow::anyhow!("fixture pipeline"))?;
    let sink = pipeline
        .by_name("out")
        .unwrap()
        .downcast::<gstreamer_app::AppSink>()
        .unwrap();
    pipeline.set_state(gst::State::Playing)?;
    let mut out = Vec::new();
    while let Some(sample) = sink.try_pull_sample(gst::ClockTime::from_seconds(5)) {
        let b = sample.buffer().context("sample buffer")?;
        out.push((
            b.map_readable()?.as_slice().to_vec(),
            !b.flags().contains(gst::BufferFlags::DELTA_UNIT),
        ));
    }
    pipeline.set_state(gst::State::Null)?;
    ensure!(out.len() == 90, "failed to generate all 90 fixture frames");
    Ok(out)
}
#[tokio::main]
async fn main() -> Result<()> {
    let args = Args::parse();
    let _ = rustls::crypto::ring::default_provider().install_default();
    let fixture = tokio::task::spawn_blocking(frames).await??;
    let directory = Temp(std::env::temp_dir().join(format!(
        "titancam-bench-{}",
        hex::encode(titan_transport::random::<8>())
    )));
    let phone = Identity::load(directory.0.join("phone"))?;
    let mut command = tokio::process::Command::new(&args.receiver);
    command.args(["run", "--pairing", "--mute"]);
    if !args.hardware {
        command.arg("--software");
    }
    let mut process = command
        .env("XDG_STATE_HOME", directory.0.join("state"))
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::inherit())
        .kill_on_drop(true)
        .spawn()?;
    let mut lines = BufReader::new(process.stdout.take().unwrap()).lines();
    let uri = tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            let line = lines
                .next_line()
                .await?
                .context("receiver stopped before pairing")?;
            if line.starts_with("titancam://pair?") {
                return Ok::<_, anyhow::Error>(line);
            }
        }
    })
    .await??;
    // URI contains a temporary secret: consume it internally, never log it.
    let fields: std::collections::HashMap<_, _> = uri
        .split_once('?')
        .unwrap()
        .1
        .split('&')
        .filter_map(|s| s.split_once('='))
        .collect();
    let connector = tokio_rustls::TlsConnector::from(std::sync::Arc::new(
        titan_transport::tls_client(fields["cert"], b"titancam-control/1")?,
    ));
    let mut control = connector
        .connect(
            rustls::pki_types::ServerName::try_from("titancam.local")?,
            tokio::net::TcpStream::connect("127.0.0.1:49160").await?,
        )
        .await?;
    let hello = read_control(&mut control).await?;
    ensure!(
        hello.kind == "Hello" && hello.body["pair_token"].as_str() == Some(""),
        "server leaked pairing secret"
    );
    let sid: [u8; 16] = hex::decode(&hello.session_id)?.try_into().unwrap();
    let nonce: [u8; 32] = hex::decode(hello.body["nonce"].as_str().unwrap())?
        .try_into()
        .unwrap();
    let public: [u8; 32] = hex::decode(hello.body["receiver_public"].as_str().unwrap())?
        .try_into()
        .unwrap();
    let token: [u8; 32] = hex::decode(hello.body["media_token"].as_str().unwrap())?
        .try_into()
        .unwrap();
    write_control(&mut control,&Control::new("AuthProof",&hello.session_id,serde_json::json!({"phone_public":hex::encode(phone.public()),"signature":titan_transport::sign(&phone.signing,1,&phone.public(),&public,&nonce,&sid),"pair_token":fields["token"]}))).await?;
    let auth = read_control(&mut control).await?;
    ensure!(auth.kind == "AuthOk", "authentication rejected");
    titan_transport::verify(
        &public,
        auth.body["signature"].as_str().unwrap(),
        2,
        &phone.public(),
        &public,
        &nonce,
        &sid,
    )?;
    let requested = read_control(&mut control).await?;
    ensure!(requested.kind == "Configure", "missing configuration");
    let mut config: StreamConfig = serde_json::from_value(requested.body)?;
    config.width = 320;
    config.height = 180;
    config.fps = 30;
    write_control(
        &mut control,
        &Control::new(
            "ConfigureAck",
            &hello.session_id,
            serde_json::to_value(&config)?,
        ),
    )
    .await?;
    let ready = read_control(&mut control).await?;
    ensure!(ready.kind == "MediaReady", "missing media-ready barrier");
    let mut endpoint = quinn::Endpoint::client("127.0.0.1:0".parse()?)?;
    let crypto = quinn::crypto::rustls::QuicClientConfig::try_from(titan_transport::tls_client(
        fields["cert"],
        b"titancam-media/1",
    )?)?;
    endpoint.set_default_client_config(quinn::ClientConfig::new(std::sync::Arc::new(crypto)));
    let connection = endpoint
        .connect("127.0.0.1:49161".parse()?, "titancam.local")?
        .await?;
    connection.send_datagram(titan_protocol::media_binding(sid, 1, token).into())?;
    let origin = Instant::now();
    let now = || origin.elapsed().as_nanos() as u64;
    loop {
        let msg = read_control(&mut control).await?;
        if msg.kind == "Start" {
            write_control(
                &mut control,
                &Control::new(
                    "StartAck",
                    &hello.session_id,
                    serde_json::json!({"host_epoch_ns":"0"}),
                ),
            )
            .await?;
        } else if msg.kind == "StreamingReady" {
            break;
        }
    }
    let (mut reader, mut writer) = tokio::io::split(control);
    let (tx, mut rx) = tokio::sync::mpsc::channel(32);
    let read_task = tokio::spawn(async move {
        loop {
            let msg = read_control(&mut reader).await;
            if tx.send(msg).await.is_err() {
                break;
            }
        }
    });
    let mut pending_config = false;
    let mut index = 0usize;
    let mut sent = 0;
    let mut idr_requests = 0;
    let latest;
    let mut tick = tokio::time::interval(Duration::from_nanos(33_333_333));
    let mut heartbeat = tokio::time::interval(Duration::from_millis(500));
    let deadline = Instant::now() + Duration::from_secs(8);
    let finished_at = std::sync::Arc::new(std::sync::Mutex::new(None::<Instant>));
    loop {
        ensure!(Instant::now() < deadline, "synthetic streaming timed out");
        tokio::select! {
                          message=rx.recv()=>{let msg=message.context("control reader ended")??;match msg.kind.as_str(){"ClockPing"=>{let t=now().to_string();write_control(&mut writer,&Control::new("ClockPong",&hello.session_id,serde_json::json!({"r1":msg.body["r1"],"s2":t,"s3":now().to_string()}))).await?;},"ConfigureApplied"=>pending_config=false,"RequestIDR"=>idr_requests+=1,"Feedback"=>{if index==fixture.len()&&msg.body["video"].as_u64().unwrap_or(0)>40{latest=msg.body;break}},"Error"=>anyhow::bail!("receiver reported media error"),_=>()}}
                          _=heartbeat.tick()=>{write_control(&mut writer,&Control::new("Heartbeat",&hello.session_id,serde_json::json!({}))).await?;}
                          _=tick.tick()=>{if pending_config{continue}
        if index==fixture.len(){if finished_at.lock().unwrap().is_none(){*finished_at.lock().unwrap()=Some(Instant::now());}continue}let i=index;index+=1;if i==5{continue}
                if i==60&&config.config_id==1{config.config_id=2;pending_config=true;index-=1;write_control(&mut writer,&Control::new("ConfigureAck",&hello.session_id,serde_json::to_value(&config)?)).await?;continue}let(data,independent)=&fixture[i];let pts=now();let count=data.len().div_ceil(1036);for (n,chunk) in data.chunks(1036).enumerate(){let h=MediaHeader{kind:1,flags:u16::from(*independent),session:sid,epoch:1,config:config.config_id,sequence:i as u64,pts,duration:33_333_333,unit_len:data.len()as u32,index:n as u16,count:count as u16,offset:(n*1036)as u32};let mut packet=h.encode().to_vec();packet.extend(chunk);connection.send_datagram(packet.into())?;}sent+=1;}
                        }
    }
    read_task.abort();
    connection.close(0u8.into(), b"benchmark complete");
    process.kill().await?;
    eprintln!("Synthetic receiver counters: {latest}");
    ensure!(idr_requests > 0, "reference loss did not request recovery");
    ensure!(
        latest["recoveries"].as_u64().unwrap_or(0) >= 3,
        "recovery/reconfiguration not exercised"
    );
    if args.hardware {
        ensure!(
            latest["decoder"].as_str() == Some("nvh264dec"),
            "hardware pipeline fell back to CPU"
        );
    }
    println!(
        "{}",
        serde_json::json!({"synthetic_only":true,"transport":"TLS+QUIC localhost","generated_frames":fixture.len(),"sent_frames":sent,"intentional_reference_loss":1,"configurations":2,"idr_requests":idr_requests,"receiver":latest,"physical_iphone_test":false})
    );
    Ok(())
}
