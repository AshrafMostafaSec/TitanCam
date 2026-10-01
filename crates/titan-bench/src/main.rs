//! Synthetic peer: plain TCP/UDP, generated H.264 GOP, intentional reference loss.
use anyhow::{Context, Result, ensure};
use clap::Parser;
use gstreamer::{self as gst, prelude::*};
use std::{
    path::{Path, PathBuf},
    time::{Duration, Instant},
};
use titan_protocol::{Control, MediaHeader, StreamConfig};
use titan_transport::{read_control, write_control};
use tokio::io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt};
#[derive(Parser)]
struct Args {
    /// Run an uninterrupted stream instead of the intentional GOP-loss fixture.
    #[arg(long)]
    healthy: bool,
    /// Omit one fragment and exercise the receiver's deadline-aware repair request.
    #[arg(long, conflicts_with = "healthy")]
    repair: bool,
    /// Publish a generated PCM tone through the real local PipeWire source.
    #[arg(long)]
    audio: bool,
    /// Exercise the same private gain/mute/transform commands used by GTK.
    #[arg(long, requires = "audio")]
    controls: bool,
    #[arg(long, default_value_t = 30, value_parser = clap::value_parser!(u32).range(24..=60))]
    fps: u32,
    #[arg(long)]
    hardware: bool,
    #[arg(long)]
    preview: bool,
    #[arg(long)]
    webcam: Option<String>,
    #[arg(long, default_value = "target/release/titan-receiver")]
    receiver: PathBuf,
}
struct Temp(PathBuf);
impl Drop for Temp {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}
async fn gui(runtime: &Path, command: &str, body: serde_json::Value) -> Result<serde_json::Value> {
    let mut socket = tokio::net::UnixStream::connect(runtime.join("titancam/control.sock")).await?;
    let id = hex::encode(titan_transport::random::<8>());
    let mut request = serde_json::to_vec(
        &serde_json::json!({"version":1,"id":id,"command":command,"body":body}),
    )?;
    request.push(b'\n');
    socket.write_all(&request).await?;
    let mut response = Vec::new();
    tokio::time::timeout(
        Duration::from_secs(2),
        tokio::io::BufReader::new(socket)
            .take(65537)
            .read_until(b'\n', &mut response),
    )
    .await??;
    ensure!(
        response.len() <= 65536 && response.last() == Some(&b'\n'),
        "invalid GUI response size"
    );
    let response: serde_json::Value = serde_json::from_slice(&response)?;
    ensure!(
        response["ok"] == true && response["id"] == id,
        "GUI command rejected: {response}"
    );
    Ok(response["body"].clone())
}
fn frames(count: usize, fps: u32) -> Result<Vec<(Vec<u8>, bool)>> {
    gst::init()?;
    let encoder = if gst::ElementFactory::find("x264enc").is_some() {
        "x264enc tune=zerolatency speed-preset=ultrafast key-int-max=30 byte-stream=true"
    } else {
        "openh264enc gop-size=30"
    };
    let pipeline = gst::parse::launch(&format!("videotestsrc num-buffers={count} pattern=ball ! video/x-raw,format=I420,width=320,height=180,framerate={fps}/1 ! {encoder} ! h264parse ! video/x-h264,stream-format=byte-stream,alignment=au ! appsink name=out sync=false max-buffers=1"))?.downcast::<gst::Pipeline>().map_err(|_|anyhow::anyhow!("fixture pipeline"))?;
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
    ensure!(out.len() == count, "failed to generate all fixture frames");
    Ok(out)
}
#[tokio::main]
async fn main() -> Result<()> {
    let args = Args::parse();
    let recovery = !args.healthy && !args.repair;
    let count = if recovery { 90 } else { 300 };
    let fps = args.fps;
    let fixture = tokio::task::spawn_blocking(move || frames(count, fps)).await??;
    let directory = Temp(std::env::temp_dir().join(format!(
        "titancam-bench-{}",
        hex::encode(titan_transport::random::<8>())
    )));
    let mut command = tokio::process::Command::new(&args.receiver);
    command.args(["local", "--bind", "127.0.0.1", "--no-usb", "--mute"]);
    if args.preview {
        command.arg("--preview");
    }
    if let Some(webcam) = &args.webcam {
        command.args(["--webcam", webcam]);
    }
    if !args.hardware {
        command.arg("--software");
    }
    // Isolate TitanCam's control socket while still using the user's audio server.
    if std::env::var_os("PIPEWIRE_RUNTIME_DIR").is_none()
        && let Some(runtime) = std::env::var_os("XDG_RUNTIME_DIR")
    {
        command.env("PIPEWIRE_RUNTIME_DIR", runtime);
    }
    let mut process = command
        .env("XDG_STATE_HOME", directory.0.join("state"))
        .env("XDG_RUNTIME_DIR", directory.0.join("runtime"))
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::inherit())
        .kill_on_drop(true)
        .spawn()?;
    let mut control = tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            if let Ok(stream) = tokio::net::TcpStream::connect("127.0.0.1:49160").await {
                break stream;
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
    })
    .await?;
    control.set_nodelay(true)?;
    let hello = read_control(&mut control).await?;
    ensure!(
        hello.kind == "Hello"
            && hello.body["mode"] == "plain"
            && hello.body["transport_version"] == 2,
        "wrong receiver transport"
    );
    let sid: [u8; 16] = hex::decode(&hello.session_id)?.try_into().unwrap();
    let token: [u8; 32] = hex::decode(hello.body["media_token"].as_str().unwrap())?
        .try_into()
        .unwrap();
    write_control(
        &mut control,
        &Control::new(
            "HelloAck",
            &hello.session_id,
            serde_json::json!({"transport_version":2}),
        ),
    )
    .await?;
    let requested = read_control(&mut control).await?;
    ensure!(requested.kind == "Configure", "missing configuration");
    let mut config: StreamConfig = serde_json::from_value(requested.body)?;
    config.width = 320;
    config.height = 180;
    config.fps = fps;
    if args.audio {
        config.audio_codec = "pcm".into();
        config.audio_channels = 1;
        config.audio_packet_ms = 10;
    }
    let mut acknowledgement = serde_json::to_value(&config)?;
    acknowledgement["capabilities"] = serde_json::json!({"selective_repair":args.repair});
    write_control(
        &mut control,
        &Control::new("ConfigureAck", &hello.session_id, acknowledgement),
    )
    .await?;
    let ready = read_control(&mut control).await?;
    ensure!(ready.kind == "MediaReady", "missing media-ready barrier");
    let connection = tokio::net::UdpSocket::bind("127.0.0.1:0").await?;
    connection.connect("127.0.0.1:49161").await?;
    connection
        .send(&titan_protocol::media_binding(sid, 1, token))
        .await?;
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
    let mut repair_requests = 0;
    let mut repair_packets: Vec<Vec<u8>> = Vec::new();
    let mut baseline = None;
    let mut baseline_audio_sent = 0;
    let mut audio_sent = 0u64;
    let mut meter_verified = false;
    let latest;
    let duration = 1_000_000_000 / fps;
    let mut tick = tokio::time::interval(Duration::from_nanos(duration as u64));
    let mut heartbeat = tokio::time::interval(Duration::from_millis(500));
    let mut audio_tick = tokio::time::interval(Duration::from_millis(10));
    let deadline = Instant::now() + Duration::from_secs(20);
    let mut finished_at = None;
    loop {
        ensure!(Instant::now() < deadline, "synthetic streaming timed out");
        tokio::select! {
            message=rx.recv()=>{
                let msg=message.context("control reader ended")??;
                match msg.kind.as_str(){
                    "ClockPing"=>{let t=now().to_string();write_control(&mut writer,&Control::new("ClockPong",&hello.session_id,serde_json::json!({"r1":msg.body["r1"],"s2":t,"s3":now().to_string()}))).await?;},
                    "ConfigureApplied"=>pending_config=false,
                    "RequestIDR"=>idr_requests+=1,
                    "Repair"=>{
                        repair_requests+=1;
                        ensure!(args.repair && msg.body["sequence"]=="120", "unexpected repair request");
                        for missing in msg.body["missing"].as_array().context("missing repair indices")? {
                            let packet=repair_packets.get(missing.as_u64().context("invalid repair index")? as usize).context("repair index outside fixture")?;
                            connection.send(packet).await?;
                        }
                    },
                    "Feedback"=>{
                        if args.controls && (145..220).contains(&index) && (0.05..0.15).contains(&msg.body["audio_peak"].as_f64().unwrap_or(0.0)) { meter_verified=true; }
                        if !recovery && baseline.is_none() && (45..110).contains(&index) { baseline=Some(msg.body.clone()); baseline_audio_sent=audio_sent; }
                        if index==fixture.len() && finished_at.is_some_and(|at: Instant| at.elapsed()>Duration::from_millis(100)) {
                            latest=msg.body;break
                        }
                    },
                    "Error"=>anyhow::bail!("receiver reported media error"),_=>()
                }
            }
            _=audio_tick.tick(), if args.audio && index<fixture.len()=>{
                let pcm:Vec<u8>=(0..480).flat_map(|n| ((((audio_sent*480+n) as f64*std::f64::consts::TAU*440.0/48000.0).sin()*8192.0).round() as i16).to_le_bytes()).collect();
                let header=MediaHeader{kind:2,flags:0,session:sid,epoch:1,config:config.config_id,sequence:audio_sent,pts:now(),duration:10_000_000,unit_len:pcm.len() as u32,index:0,count:1,offset:0};
                let mut packet=header.encode().to_vec();packet.extend(pcm);connection.send(&packet).await?;audio_sent+=1;
            }
            _ = heartbeat.tick() => {
                write_control(&mut writer, &Control::new("Heartbeat", &hello.session_id, serde_json::json!({}))).await?;
            }
            _ = tick.tick() => {
                if pending_config { continue; }
                if index == fixture.len() {
                    finished_at.get_or_insert_with(Instant::now);
                    continue;
                }
                let i = index;
                index += 1;
                if args.controls && i == 140 {
                    gui(&directory.0.join("runtime"),"SetAudio",serde_json::json!({"gain_db":-6.0,"mute":false})).await?;
                    gui(&directory.0.join("runtime"),"SetTransform",serde_json::json!({"mirror":true,"flip":true})).await?;
                }
                if args.controls && i == 220 {
                    gui(&directory.0.join("runtime"),"SetAudio",serde_json::json!({"mute":true})).await?;
                }
                if recovery && i == 5 { continue; }
                if recovery && i == 60 && config.config_id == 1 {
                    config.config_id = 2;
                    pending_config = true;
                    index -= 1;
                    write_control(&mut writer, &Control::new("ConfigureAck", &hello.session_id, serde_json::to_value(&config)?)).await?;
                    continue;
                }
                let (data, independent) = &fixture[i];
                let pts = now();
                let chunk_size = if args.repair && i == 120 { 64 } else { 1036 };
                let count = data.len().div_ceil(chunk_size);
                if args.repair && i == 120 { ensure!(count > 1, "repair fixture needs multiple fragments"); }
                for (n, chunk) in data.chunks(chunk_size).enumerate() {
                    let header = MediaHeader {
                        kind: 1, flags: u16::from(*independent), session: sid, epoch: 1,
                        config: config.config_id, sequence: i as u64, pts, duration,
                        unit_len: data.len() as u32, index: n as u16, count: count as u16,
                        offset: (n * chunk_size) as u32,
                    };
                    let mut packet = header.encode().to_vec();
                    packet.extend(chunk);
                    if args.repair && i == 120 {
                        repair_packets.push(packet.clone());
                        if n == count - 1 { continue; }
                    }
                    connection.send(&packet).await?;
                }
                sent += 1;
            }
        }
    }
    read_task.abort();
    if args.controls {
        let status = gui(
            &directory.0.join("runtime"),
            "GetStatus",
            serde_json::json!({}),
        )
        .await?;
        ensure!(
            status["mirror"] == true
                && status["flip"] == true
                && status["mute"] == true
                && status["gain_db"] == -6.0,
            "effective GUI state mismatch"
        );
        ensure!(
            meter_verified && latest["audio_peak"] == 0.0,
            "gain and mute meter behavior failed"
        );
    }
    process.kill().await?;
    eprintln!("Synthetic receiver counters: {latest}");
    if recovery {
        ensure!(idr_requests > 0, "reference loss did not request recovery");
        ensure!(
            latest["recoveries"].as_u64().unwrap_or(0) >= 3,
            "recovery/reconfiguration not exercised"
        );
    } else {
        let baseline = baseline.as_ref().context("missing steady-state baseline")?;
        for counter in [
            "late_video",
            "reference_discarded",
            "missing_units",
            "expired",
        ] {
            ensure!(
                baseline[counter] == latest[counter],
                "steady-state {counter} increased: baseline {}, final {}",
                baseline[counter],
                latest[counter]
            );
        }
        ensure!(
            latest["decoded_video"].as_u64().unwrap_or(0) >= count as u64 - 35,
            "insufficient decoded frames"
        );
        if args.repair {
            ensure!(repair_requests > 0, "repair request was not exercised");
        }
        if args.audio {
            ensure!(
                baseline["late_audio"] == latest["late_audio"],
                "steady-state audio became late"
            );
            ensure!(
                latest["audio"]
                    .as_u64()
                    .unwrap_or(0)
                    .saturating_sub(baseline["audio"].as_u64().unwrap_or(0))
                    >= audio_sent
                        .saturating_sub(baseline_audio_sent)
                        .saturating_sub(2),
                "steady-state audio delivery failed"
            );
        }
    }
    if args.audio {
        ensure!(
            latest["audio"].as_u64().unwrap_or(0) > audio_sent.saturating_sub(35),
            "audio delivery failed"
        );
    }
    if args.hardware {
        ensure!(
            latest["decoder"].as_str() == Some("nvh264dec"),
            "hardware pipeline fell back to CPU"
        );
    }
    println!(
        "{}",
        serde_json::json!({"synthetic_only":true,"transport":"plain TCP+UDP localhost","fixture_resolution":"320x180","fixture_fps":fps,"generated_frames":fixture.len(),"sent_frames":sent,"audio_packets_sent":audio_sent,"local_controls_exercised":args.controls,"tone_meter_verified":meter_verified,"steady_state_baseline":baseline,"intentional_reference_loss":u32::from(recovery),"configurations":if recovery{2}else{1},"repair_requests":repair_requests,"idr_requests":idr_requests,"receiver":latest,"physical_iphone_test":false})
    );
    Ok(())
}
