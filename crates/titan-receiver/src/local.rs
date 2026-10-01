//! Local v2 transport: clear TCP control/USB and deadline-bounded UDP media.
use super::session::{self, Context, OutputOptions};
use anyhow::{Context as _, Result, ensure};
use serde::Deserialize;
use std::{
    collections::HashMap,
    net::{IpAddr, SocketAddr},
    process::Stdio,
    sync::{Arc, Mutex, atomic::Ordering},
    time::{Duration, Instant},
};
use titan_protocol::{Control, MediaHeader, Reassembler, StreamConfig, Unit};
use titan_transport::{read_control, write_control};
use tokio::{
    io::{AsyncRead, AsyncReadExt, AsyncWrite},
    net::{TcpListener, UdpSocket},
    process::{Child, Command},
    sync::{Semaphore, mpsc},
};
const BASES: [u16; 3] = [43052, 43062, 43072];
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Bootstrap {
    version: u8,
    base: u16,
}
impl Bootstrap {
    fn parse(bytes: &[u8], expected: u16) -> Result<Self> {
        ensure!(bytes.len() <= 256, "USB metadata too large");
        let value: Self = serde_json::from_slice(bytes)?;
        ensure!(
            value.version == 2 && value.base == expected && BASES.contains(&value.base),
            "incompatible USB metadata"
        );
        Ok(value)
    }
}
async fn probe(device: &str) -> Result<Bootstrap> {
    for base in BASES {
        let result: Result<Bootstrap> = async {
            let stream = super::usb_stream(device, base - 1).await?;
            let mut bytes = Vec::new();
            stream.take(257).read_to_end(&mut bytes).await?;
            Bootstrap::parse(&bytes, base)
        }
        .await;
        if let Ok(metadata) = result {
            return Ok(metadata);
        }
    }
    anyhow::bail!("phone is not ready for USB")
}
fn advertise(host: &str) -> Result<Child> {
    let address: IpAddr = host.parse()?;
    ensure!(
        !address.is_unspecified() && !address.is_loopback() && !address.is_multicast(),
        "discovery needs a LAN address"
    );
    let hostname = std::fs::read_to_string("/etc/hostname").unwrap_or_else(|_| "Linux".into());
    let name = format!(
        "TitanCam — {}",
        hostname.trim().chars().take(40).collect::<String>()
    );
    Ok(Command::new("avahi-publish-service")
        .args([
            "--no-fail",
            &name,
            "_titancam._tcp",
            "49160",
            "v=2",
            "mode=plain",
            "media=49161",
        ])
        .arg(format!("host={host}"))
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .kill_on_drop(true)
        .spawn()?)
}
struct Peer {
    ctx: Arc<Context>,
    ip: IpAddr,
    address: Option<SocketAddr>,
    assembler: Reassembler,
    config: u32,
    reorder: crate::reorder::Reorder,
    last_repair: Instant,
}
type Peers = Arc<Mutex<HashMap<[u8; 16], Peer>>>;
type Split<S> = (
    tokio::io::ReadHalf<S>,
    (tokio::io::WriteHalf<S>, mpsc::Receiver<Control>),
);
async fn negotiate<S: AsyncRead + AsyncWrite + Unpin + Send + 'static>(
    mut stream: S,
    cfg: StreamConfig,
    outputs: OutputOptions,
    slots: Arc<Semaphore>,
    hub: Arc<crate::control_api::Hub>,
) -> Result<(Arc<Context>, Split<S>)> {
    let usb = cfg.audio_codec == "pcm";
    let mut cfg = hub.desired.lock().unwrap().clone();
    cfg.config_id = 1;
    cfg.audio_codec = if usb { "pcm" } else { "opus" }.into();
    if usb {
        cfg.audio_packet_ms = 5;
        cfg.playout_ms = 35;
    }
    let permit = slots
        .try_acquire_owned()
        .context("receiver busy with another session")?;
    let (ctx, commands) = session::create(cfg.clone(), outputs, permit, hub.controls.clone());
    let mut lease = session::Lease(Some(ctx.clone()));
    let sid = hex::encode(ctx.id);
    write_control(&mut stream, &Control::new("Hello", &sid, serde_json::json!({"transport_version":2,"mode":"plain","media_token":hex::encode(ctx.token),"media_port":49161}))).await?;
    let ack = tokio::time::timeout(Duration::from_secs(5), read_control(&mut stream)).await??;
    ensure!(
        ack.kind == "HelloAck"
            && ack.session_id == sid
            && ack.transport_epoch == 1
            && ack.body["transport_version"] == 2,
        "incompatible sender; install local v2 on both ends"
    );
    write_control(
        &mut stream,
        &Control::new("Configure", &sid, serde_json::to_value(cfg)?),
    )
    .await?;
    let ack = tokio::time::timeout(Duration::from_secs(30), async {
        loop {
            let message = read_control(&mut stream).await?;
            ensure!(
                message.session_id == sid && message.transport_epoch == 1,
                "stale setup message"
            );
            if message.kind == "Heartbeat" {
                continue;
            }
            break Ok::<_, anyhow::Error>(message);
        }
    })
    .await??;
    ensure!(
        ack.kind == "ConfigureAck" && ack.session_id == sid && ack.transport_epoch == 1,
        "configuration not acknowledged"
    );
    let effective: StreamConfig = serde_json::from_value(ack.body.clone())?;
    ensure!(effective.validate(), "invalid effective configuration");
    *ctx.config.lock().unwrap() = effective;
    *ctx.capabilities.lock().unwrap() = ack
        .body
        .get("capabilities")
        .cloned()
        .unwrap_or_else(|| serde_json::json!({}));
    *hub.active.lock().unwrap() = Arc::downgrade(&ctx);
    write_control(
        &mut stream,
        &Control::new("MediaReady", &sid, serde_json::json!({})),
    )
    .await?;
    lease.0 = None;
    let (read, write) = tokio::io::split(stream);
    Ok((ctx, (read, (write, commands))))
}
pub async fn run(
    bind: IpAddr,
    host: Option<String>,
    cfg: StreamConfig,
    outputs: OutputOptions,
    usb: bool,
    hub: Arc<crate::control_api::Hub>,
) -> Result<()> {
    let listener = TcpListener::bind(SocketAddr::new(bind, 49160)).await?;
    let socket = Arc::new(UdpSocket::bind(SocketAddr::new(bind, 49161)).await?);
    let mut announcement = if let Some(host) = host {
        Some(advertise(&host)?)
    } else {
        None
    };
    let slots = Arc::new(Semaphore::new(1));
    let handshake_slots = Arc::new(Semaphore::new(4));
    let peers: Peers = Arc::new(Mutex::new(HashMap::new()));
    let udp_task = tokio::spawn(udp_loop(socket, peers.clone()));
    let usb_task = if usb {
        Some(tokio::spawn(usb_monitor(
            StreamConfig::profile(&cfg.profile, true),
            outputs.clone(),
            slots.clone(),
            hub.clone(),
        )))
    } else {
        None
    };
    tracing::info!(
        "listening on {bind}:49160/TCP and :49161/UDP; local v2 ready, waiting for sender"
    );
    loop {
        tokio::select! {
            _=tokio::signal::ctrl_c()=>break,
            result=listener.accept()=>{
                let (stream, address)=result?; stream.set_nodelay(true)?;
                let Ok(handshake)=handshake_slots.clone().try_acquire_owned() else {continue};
                let slots=slots.clone(); let cfg=cfg.clone(); let outputs=outputs.clone(); let peers=peers.clone(); let hub=hub.clone();
                tokio::spawn(async move {
                    let _handshake=handshake;
                    let result:Result<()> = async {
                        let (ctx, commands)=negotiate(stream,cfg,outputs,slots,hub).await?;
                        let _lease=session::Lease(Some(ctx.clone()));
                        ctx.stats.lock().unwrap().transport="Wi-Fi".into(); let id=ctx.id; let config=ctx.config.lock().unwrap().config_id;
                        peers.lock().unwrap().insert(id,Peer { ctx:ctx.clone(),ip:address.ip(),address:None,assembler:Reassembler::new(id,1,config,Duration::from_millis(60)),config,reorder:crate::reorder::Reorder::new(),last_repair:Instant::now() });
                        let result=super::control_session(ctx,commands.0,commands.1).await;
                        peers.lock().unwrap().remove(&id); result
                    }.await;
                    if let Err(e)=result {tracing::warn!("sender disconnected: {e}");}
                });
            }
        }
    }
    udp_task.abort();
    let _ = udp_task.await;
    if let Some(task) = usb_task {
        task.abort();
        let _ = task.await;
    }
    if let Some(child) = announcement.as_mut() {
        let _ = child.kill().await;
    }
    Ok(())
}
async fn udp_loop(socket: Arc<UdpSocket>, peers: Peers) -> Result<()> {
    let mut bytes = [0u8; 1201];
    let mut tick = tokio::time::interval(Duration::from_millis(10));
    loop {
        tokio::select! {
            received=socket.recv_from(&mut bytes)=>{
                let (length, source)=received?; let packet=&bytes[..length];
                if length==57 && &packet[..4]==b"TCMB" {
                    let id:[u8;16]=packet[5..21].try_into()?;
                    let mut peers=peers.lock().unwrap();
                    if let Some(peer)=peers.get_mut(&id)
                        && source.ip()==peer.ip && !peer.ctx.cancelled.load(Ordering::Acquire) && titan_protocol::check_binding(packet,id,1,peer.ctx.token) {
                            if peer.address.is_none() && peer.ctx.created.elapsed()<Duration::from_secs(35) {
                                peer.address=Some(source); peer.ctx.bound.store(true,Ordering::Release);
                                peer.ctx.request("MediaBound",serde_json::json!({})); peer.ctx.request("Start",serde_json::json!({}));
                            } else if peer.address==Some(source) {peer.ctx.request("MediaBound",serde_json::json!({}));}
                    }
                } else if length<=1100 {
                    let Ok(header)=MediaHeader::parse(packet) else {continue};
                    let mut peers=peers.lock().unwrap();
                    if let Some(peer)=peers.get_mut(&header.session) {
                        if peer.address!=Some(source) || peer.ctx.cancelled.load(Ordering::Acquire) {continue;}
                        let config=peer.ctx.config.lock().unwrap().config_id;
                        if peer.config!=config {peer.config=config;peer.reorder=crate::reorder::Reorder::new();peer.assembler=Reassembler::new(peer.ctx.id,1,config,Duration::from_millis(60));}
                        peer.ctx.stats.lock().unwrap().bytes+=length as u64;
                        match peer.assembler.push(packet,Instant::now()) {Ok(Some(unit))=>{if unit.header.kind==1 {for ready in peer.reorder.push(unit,Instant::now()){peer.ctx.accept(ready);}} else {peer.ctx.accept(unit);}},Ok(None)=>(),Err(_)=>peer.ctx.stats.lock().unwrap().dropped+=1}
                    }
                }
            }
            _=tick.tick()=>{
                let mut peers=peers.lock().unwrap();
                for peer in peers.values_mut() {
                    let now=Instant::now();
                    for unit in peer.reorder.drain(now) { peer.ctx.accept(unit); }
                    if peer.last_repair.elapsed() >= Duration::from_millis(15) && peer.ctx.capabilities.lock().unwrap()["selective_repair"]==true {
                        let rtt=peer.ctx.stats.lock().unwrap().clock_rtt_ms;
                        for (header,missing,remaining) in peer.assembler.missing(now) {
                            if rtt+10.0 < remaining as f64 {
                                peer.ctx.request("Repair",serde_json::json!({"config_id":header.config,"sequence":header.sequence.to_string(),"missing":missing,"remaining_ms":remaining}));
                            }
                        }
                        peer.last_repair=now;
                    }
                    if peer.assembler.expire(Instant::now()) {peer.ctx.request("RequestIDR",serde_json::json!({"reason":"fragment_deadline"}));}
                    peer.ctx.stats.lock().unwrap().expired=peer.assembler.expired;
                }
            }
        }
    }
}
async fn usb_session(
    device: &str,
    base: u16,
    cfg: StreamConfig,
    outputs: OutputOptions,
    slots: Arc<Semaphore>,
    hub: Arc<crate::control_api::Hub>,
) -> Result<()> {
    let (ctx, commands) = negotiate(
        super::usb_stream(device, base).await?,
        cfg,
        outputs,
        slots,
        hub,
    )
    .await?;
    let _lease = session::Lease(Some(ctx.clone()));
    ctx.stats.lock().unwrap().transport = "USB".into();
    for (port, role) in [(base + 1, "video"), (base + 2, "audio")] {
        let mut media = super::usb_stream(device, port).await?;
        write_control(
            &mut media,
            &Control::new(
                "MediaBind",
                &hex::encode(ctx.id),
                serde_json::json!({"token":hex::encode(ctx.token),"role":role}),
            ),
        )
        .await?;
        let c = ctx.clone();
        tokio::spawn(async move {
            loop {
                if c.cancelled.load(Ordering::Acquire) {
                    break;
                }
                match tokio::time::timeout(
                    Duration::from_secs(2),
                    titan_transport::read_media(&mut media),
                )
                .await
                {
                    Ok(Ok(bytes)) => {
                        if let Ok(header) = MediaHeader::parse(&bytes) {
                            c.accept(Unit {
                                header,
                                data: bytes[64..].to_vec(),
                            });
                        }
                    }
                    _ => {
                        c.cancelled.store(true, Ordering::Release);
                        c.live.store(false, Ordering::Release);
                        break;
                    }
                }
            }
        });
    }
    ctx.request("Start", serde_json::json!({}));
    super::control_session(ctx, commands.0, commands.1).await
}
async fn usb_monitor(
    cfg: StreamConfig,
    outputs: OutputOptions,
    slots: Arc<Semaphore>,
    hub: Arc<crate::control_api::Hub>,
) {
    loop {
        if slots.available_permits() > 0
            && let Ok(Ok(devices)) = tokio::task::spawn_blocking(titan_usb::devices).await
        {
            for device in devices.into_iter().take(8) {
                if let Ok(Ok(metadata)) =
                    tokio::time::timeout(Duration::from_secs(3), probe(&device)).await
                {
                    if let Err(e) = usb_session(
                        &device,
                        metadata.base,
                        cfg.clone(),
                        outputs.clone(),
                        slots.clone(),
                        hub.clone(),
                    )
                    .await
                    {
                        tracing::warn!("USB session ended: {e}");
                    }
                    break;
                }
            }
        }
        tokio::time::sleep(Duration::from_secs(1)).await;
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn bootstrap_rejects_oversize_and_incompatible_sender() {
        assert!(Bootstrap::parse(br#"{"version":2,"base":43052}"#, 43052).is_ok());
        assert!(Bootstrap::parse(br#"{"version":1,"base":43052}"#, 43052).is_err());
        assert!(Bootstrap::parse(br#"{"version":2,"base":43062}"#, 43052).is_err());
        assert!(Bootstrap::parse(&[b' '; 257], 43052).is_err());
    }
    #[tokio::test]
    async fn plain_handshake_and_single_output_lease() {
        let slots = Arc::new(Semaphore::new(1));
        let (server, mut phone) = tokio::io::duplex(65536);
        let options = OutputOptions {
            preview: false,
            webcam: None,
            software: true,
        };
        let task = tokio::spawn(negotiate(
            server,
            StreamConfig::profile("saver", false),
            options,
            slots.clone(),
            crate::control_api::Hub::new(StreamConfig::profile("saver", false), true),
        ));
        let hello = read_control(&mut phone).await.unwrap();
        assert_eq!(hello.kind, "Hello");
        assert_eq!(hello.body["mode"], "plain");
        assert!(hello.body.get("receiver_public").is_none());
        write_control(
            &mut phone,
            &Control::new(
                "HelloAck",
                &hello.session_id,
                serde_json::json!({"transport_version":2}),
            ),
        )
        .await
        .unwrap();
        let cfg = read_control(&mut phone).await.unwrap();
        assert_eq!(cfg.kind, "Configure");
        write_control(
            &mut phone,
            &Control::new("ConfigureAck", &hello.session_id, cfg.body),
        )
        .await
        .unwrap();
        assert_eq!(read_control(&mut phone).await.unwrap().kind, "MediaReady");
        let (ctx, _) = task.await.unwrap().unwrap();
        assert_eq!(slots.available_permits(), 0);
        drop(session::Lease(Some(ctx)));
        let _permit = tokio::time::timeout(Duration::from_secs(1), slots.acquire())
            .await
            .unwrap()
            .unwrap();
    }
}
