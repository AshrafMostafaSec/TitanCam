mod session;
use anyhow::{Context as _, Result, ensure};
use clap::{Parser, Subcommand};
use session::{Context, OutputOptions};
use std::{
    collections::HashMap,
    net::{IpAddr, SocketAddr},
    os::fd::OwnedFd,
    sync::{Arc, Mutex, atomic::Ordering},
    time::{Duration, Instant},
};
use titan_protocol::{Control, MediaHeader, StreamConfig, Unit};
use titan_transport::{Identity, hex_array, read_control, write_control};
use tokio::{
    io::{AsyncRead, AsyncWrite},
    net::{TcpListener, TcpStream},
    sync::{Semaphore, mpsc},
};
#[derive(Parser)]
#[command(version, about = "TitanCam authenticated iPhone camera receiver")]
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
    Run {
        #[arg(long, default_value = "127.0.0.1")]
        bind: IpAddr,
        #[arg(long)]
        advertise: Option<String>,
        #[arg(long,default_value="balanced",value_parser=["saver","balanced","maximum"])]
        profile: String,
        #[arg(long)]
        pairing: bool,
        #[arg(long)]
        preview: bool,
        #[arg(long)]
        webcam: Option<String>,
        #[arg(long)]
        software: bool,
        #[arg(long)]
        mute: bool,
    },
    Usb {
        #[arg(long)]
        udid: Option<String>,
        #[arg(long)]
        phone_pin: String,
        #[arg(long)]
        pair_token: Option<String>,
        #[arg(long,default_value="balanced",value_parser=["saver","balanced","maximum"])]
        profile: String,
        #[arg(long)]
        preview: bool,
        #[arg(long)]
        webcam: Option<String>,
        #[arg(long)]
        software: bool,
        #[arg(long)]
        mute: bool,
    },
    Revoke {
        public_key: String,
    },
    Stats,
}
#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env().unwrap_or_else(|_| "info".into()),
        )
        .init();
    let _ = rustls::crypto::ring::default_provider().install_default();
    match Cli::parse().command {
        Commands::Doctor { decode, microphone } => {
            for p in ["gstreamer-1.0", "libpipewire-0.3", "libusbmuxd-2.0", "opus"] {
                let o = std::process::Command::new("pkg-config")
                    .args(["--modversion", p])
                    .output()?;
                ensure!(o.status.success(), "missing {p}");
                println!("{p}: {}", String::from_utf8_lossy(&o.stdout).trim());
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
        Commands::Run {
            bind,
            advertise,
            profile,
            pairing,
            preview,
            webcam,
            software,
            mute,
        } => {
            run(
                bind,
                advertise,
                StreamConfig::profile(&profile, false),
                pairing,
                OutputOptions {
                    preview,
                    webcam,
                    software,
                    mute,
                },
            )
            .await
        }
        Commands::Usb {
            udid,
            phone_pin,
            pair_token,
            profile,
            preview,
            webcam,
            software,
            mute,
        } => {
            let id = Identity::load(titan_transport::state_dir())?;
            let devices = titan_usb::devices()?;
            let device = udid
                .or_else(|| {
                    if devices.len() == 1 {
                        devices.first().cloned()
                    } else {
                        None
                    }
                })
                .context("specify --udid when zero or multiple phones are connected")?;
            usb_loop(
                device,
                phone_pin,
                pair_token.unwrap_or_default(),
                Arc::new(id),
                StreamConfig::profile(&profile, true),
                OutputOptions {
                    preview,
                    webcam,
                    software,
                    mute,
                },
            )
            .await
        }
        Commands::Revoke { public_key } => {
            Identity::load(titan_transport::state_dir())?.revoke(&public_key)
        }
        Commands::Stats => {
            let p = titan_transport::state_dir().join("stats.json");
            println!(
                "{}",
                std::fs::read_to_string(p).context("no active receiver statistics yet")?
            );
            Ok(())
        }
    }
}
type Registry = Arc<Mutex<HashMap<[u8; 16], Arc<Context>>>>;
async fn run(
    bind: IpAddr,
    advertise: Option<String>,
    cfg: StreamConfig,
    pairing: bool,
    outputs: OutputOptions,
) -> Result<()> {
    let identity = Arc::new(Identity::load(titan_transport::state_dir())?);
    let token = hex::encode(titan_transport::random::<32>());
    let token_created = Instant::now();
    let has_peers = std::fs::read_dir(&identity.directory)?
        .filter_map(Result::ok)
        .any(|p| p.file_name().to_string_lossy().starts_with("peer-"));
    let allow_pair = pairing || !has_peers;
    let acceptor =
        tokio_rustls::TlsAcceptor::from(Arc::new(identity.tls_server(b"titancam-control/1")?));
    let listener = TcpListener::bind(SocketAddr::new(bind, 49160)).await?;
    let mut qs = quinn::ServerConfig::with_crypto(Arc::new(
        quinn::crypto::rustls::QuicServerConfig::try_from(
            identity.tls_server(b"titancam-media/1")?,
        )?,
    ));
    let mut qt = quinn::TransportConfig::default();
    qt.datagram_receive_buffer_size(Some(4 * 1024 * 1024));
    qt.datagram_send_buffer_size(256 * 1024);
    qt.max_concurrent_bidi_streams(0u8.into());
    qt.max_concurrent_uni_streams(0u8.into());
    qt.max_idle_timeout(Some(Duration::from_secs(5).try_into()?));
    qs.transport_config(Arc::new(qt));
    let endpoint = quinn::Endpoint::server(qs, SocketAddr::new(bind, 49161))?;
    if allow_pair {
        let host = advertise.unwrap_or(bind.to_string());
        ensure!(
            host != "0.0.0.0" && host != "::",
            "use --advertise with an address reachable by the phone"
        );
        let uri = format!(
            "titancam://pair?host={host}&port=49160&media=49161&cert={}&receiver={}&token={token}",
            titan_transport::fingerprint(&identity.cert),
            hex::encode(identity.public())
        );
        println!(
            "Pairing is open for 120 seconds. Paste this URI into TitanCam on the phone:\n{uri}"
        );
    }
    tracing::info!("listening on {bind}:49160/TCP and :49161/UDP; media waits for authentication");
    let registry: Registry = Arc::new(Mutex::new(HashMap::new()));
    let qr = registry.clone();
    let media_slots = Arc::new(Semaphore::new(4));
    let ms = media_slots.clone();
    tokio::spawn(async move {
        while let Some(incoming) = endpoint.accept().await {
            let Ok(permit) = ms.clone().try_acquire_owned() else {
                incoming.refuse();
                continue;
            };
            let registry = qr.clone();
            tokio::spawn(async move {
                let _permit = permit;
                let result: Result<()> = async {
                    let connection =
                        tokio::time::timeout(Duration::from_secs(5), incoming).await??;
                    let bytes =
                        tokio::time::timeout(Duration::from_secs(5), connection.read_datagram())
                            .await??;
                    ensure!(bytes.len() == 57, "invalid media binding");
                    let id: [u8; 16] = bytes[5..21].try_into()?;
                    let ctx = registry
                        .lock()
                        .unwrap()
                        .get(&id)
                        .cloned()
                        .context("no authenticated session")?;
                    ensure!(
                        titan_protocol::check_binding(&bytes, id, 1, ctx.token),
                        "invalid media token"
                    );
                    ensure!(
                        !ctx.bound.swap(true, Ordering::AcqRel),
                        "media token already used"
                    );
                    ensure!(
                        ctx.created.elapsed() < Duration::from_secs(10)
                            && !ctx.cancelled.load(Ordering::Acquire),
                        "expired session binding"
                    );
                    ctx.request("MediaBound", serde_json::json!({}));
                    ctx.request("Start", serde_json::json!({}));
                    let r = session::datagrams(ctx.clone(), connection).await;
                    ctx.bound.store(false, Ordering::Release);
                    r
                }
                .await;
                if let Err(e) = result {
                    tracing::warn!("media connection ended: {e}")
                }
            });
        }
    });
    let slots = Arc::new(Semaphore::new(4));
    loop {
        tokio::select! {_=tokio::signal::ctrl_c()=>break,r=listener.accept()=>{let(tcp,_)=r?;let Ok(permit)=slots.clone().try_acquire_owned()else{continue};tcp.set_nodelay(true)?;let acceptor=acceptor.clone();let id=identity.clone();let cfg=cfg.clone();let options=outputs.clone();let reg=registry.clone();let pair=if allow_pair&&token_created.elapsed()<Duration::from_secs(120){Some(token.clone())}else{None};tokio::spawn(async move{let _permit=permit;let result:Result<()>=async{let tls=tokio::time::timeout(Duration::from_secs(5),acceptor.accept(tcp)).await??;let(ctx,commands)=authenticate(tls,id,cfg,options,pair,Some(reg.clone())).await?;let session_id=ctx.id;let result=control_session(ctx,commands.0,commands.1).await;reg.lock().unwrap().remove(&session_id);result}.await;if let Err(e)=result{tracing::warn!("control session ended: {e}")}});}}
    }
    Ok(())
}
async fn authenticate<S: AsyncRead + AsyncWrite + Unpin + Send + 'static>(
    mut stream: S,
    id: Arc<Identity>,
    cfg: StreamConfig,
    options: OutputOptions,
    pair: Option<String>,
    registry: Option<Registry>,
) -> Result<(
    Arc<Context>,
    (
        tokio::io::ReadHalf<S>,
        (tokio::io::WriteHalf<S>, mpsc::Receiver<Control>),
    ),
)> {
    let (ctx, commands) = session::create(cfg.clone(), options);
    let mut lease = session::Lease(Some(ctx.clone()));
    let sid = hex::encode(ctx.id);
    let nonce = titan_transport::random::<32>();
    write_control(&mut stream,&Control::new("Hello",&sid,serde_json::json!({"receiver_public":hex::encode(id.public()),"nonce":hex::encode(nonce),"media_token":hex::encode(ctx.token),"media_port":49161,"pair_token":if registry.is_none(){pair.clone().unwrap_or_default()}else{String::new()}}))).await?;
    let proof = tokio::time::timeout(Duration::from_secs(5), read_control(&mut stream)).await??;
    ensure!(
        proof.kind == "AuthProof" && proof.session_id == sid,
        "expected authentication proof"
    );
    let public = hex_array::<32>(
        proof.body["phone_public"]
            .as_str()
            .context("missing phone identity")?,
    )?;
    titan_transport::verify(
        &public,
        proof.body["signature"]
            .as_str()
            .context("missing signature")?,
        1,
        &public,
        &id.public(),
        &nonce,
        &ctx.id,
    )?;
    if !id.known(&public) {
        let token = proof.body["pair_token"].as_str().unwrap_or("");
        ensure!(
            pair.is_some_and(|expected| titan_transport::secret_matches(token, &expected)),
            "unknown peer or expired pairing token"
        );
        id.remember(&public)?;
    }
    write_control(&mut stream,&Control::new("AuthOk",&sid,serde_json::json!({"signature":titan_transport::sign(&id.signing,2,&public,&id.public(),&nonce,&ctx.id)}))).await?;
    write_control(
        &mut stream,
        &Control::new("Configure", &sid, serde_json::to_value(&cfg)?),
    )
    .await?;
    let ack = tokio::time::timeout(Duration::from_secs(5), read_control(&mut stream)).await??;
    ensure!(
        ack.kind == "ConfigureAck" && ack.session_id == sid,
        "configuration not acknowledged"
    );
    let effective: StreamConfig = serde_json::from_value(ack.body)?;
    ensure!(effective.validate(), "invalid effective configuration");
    *ctx.config.lock().unwrap() = effective;
    if let Some(r) = registry {
        r.lock().unwrap().insert(ctx.id, ctx.clone());
    }
    lease.0 = None;
    let (read, write) = tokio::io::split(stream);
    Ok((ctx, (read, (write, commands))))
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
                msg=incoming.recv()=>{let msg=msg.context("control reader ended")??;ensure!(msg.session_id==hex::encode(ctx.id)&&msg.transport_epoch==1,"stale control message");last_control=Instant::now();match msg.kind.as_str(){"StartAck"=>{let host=msg.body["host_epoch_ns"].as_str().and_then(|s|s.parse().ok()).context("invalid capture clock epoch")?;*ctx.host_epoch.lock().unwrap()=Some(host);ctx.live.store(true,Ordering::Release);},"ConfigureAck"=>{let effective:StreamConfig=serde_json::from_value(msg.body)?;ensure!(effective.validate(),"invalid updated configuration");*ctx.config.lock().unwrap()=effective;},"ClockPong"=>ctx.update_clock(&msg.body),"Heartbeat"|"Capabilities"=>(),"Error"|"Stop"=>{ctx.live.store(false,Ordering::Release);tracing::warn!("phone interrupted capture");},_=>()}}
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
async fn usb_loop(
    device: String,
    pin: String,
    token: String,
    id: Arc<Identity>,
    cfg: StreamConfig,
    options: OutputOptions,
) -> Result<()> {
    let tls = tokio_rustls::TlsConnector::from(Arc::new(titan_transport::tls_client(
        &pin,
        b"titancam-control/1",
    )?));
    let mut delay = 100;
    loop {
        let run = async {
            let stream = tokio::time::timeout(
                Duration::from_secs(5),
                tls.connect(
                    rustls::pki_types::ServerName::try_from("titancam.local")?,
                    usb_tcp(&device, 49152).await?,
                ),
            )
            .await??;
            let (ctx, commands) = authenticate(
                stream,
                id.clone(),
                cfg.clone(),
                options.clone(),
                Some(token.clone()),
                None,
            )
            .await?;
            let _usb_lease = session::Lease(Some(ctx.clone()));
            // USB's one-time phone token is validated by the phone; its cert is explicitly pinned here.
            for (port, role) in [(49153, "video"), (49154, "audio")] {
                let connector = tokio_rustls::TlsConnector::from(Arc::new(
                    titan_transport::tls_client(&pin, b"titancam-media/1")?,
                ));
                let mut media = tokio::time::timeout(
                    Duration::from_secs(5),
                    connector.connect(
                        rustls::pki_types::ServerName::try_from("titancam.local")?,
                        usb_tcp(&device, port).await?,
                    ),
                )
                .await??;
                write_control(&mut media,&Control::new("MediaBind",&hex::encode(ctx.id),serde_json::json!({"token":hex::encode(ctx.token),"role":role,"pair_token":token}))).await?;
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
                            Ok(Ok(b)) => {
                                let h = MediaHeader::parse(&b).unwrap();
                                c.accept(Unit {
                                    header: h,
                                    data: b[64..].to_vec(),
                                });
                            }
                            _ => {
                                c.live.store(false, Ordering::Release);
                                c.cancelled.store(true, Ordering::Release);
                                c.request("Error",serde_json::json!({"reason":"USB channel disconnected; reconnect"}));
                                break;
                            }
                        }
                    }
                });
            }
            ctx.request("Start", serde_json::json!({}));
            control_session(ctx, commands.0, commands.1).await
        };
        let run: Result<()> = tokio::select! {_=tokio::signal::ctrl_c()=>break,result=run=>result};
        tokio::select! {_=tokio::signal::ctrl_c()=>break,_=tokio::time::sleep(Duration::from_millis(delay))=>()};
        if let Err(e) = run {
            tracing::warn!("USB reconnect: {e}")
        }
        delay = (delay * 2).min(2000);
    }
    Ok(())
}
