//! Private, bounded GUI control. No additional LAN listener.
use crate::session::Context;
use anyhow::{Result, ensure};
use serde::{Deserialize, Serialize};
use std::{
    path::PathBuf,
    sync::{Arc, Mutex, Weak},
    time::Duration,
};
use titan_media::dsp::Controls;
use titan_protocol::{Control, StreamConfig};
use tokio::{
    io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader},
    net::{UnixListener, UnixStream},
    sync::Semaphore,
};

pub struct Hub {
    pub active: Mutex<Weak<Context>>,
    pub desired: Mutex<StreamConfig>,
    pub controls: Arc<Mutex<Controls>>,
    apply: tokio::sync::Mutex<()>,
}
impl Hub {
    pub fn new(config: StreamConfig, mute: bool) -> Arc<Self> {
        Arc::new(Self {
            active: Mutex::new(Weak::new()),
            desired: Mutex::new(config),
            controls: Arc::new(Mutex::new(Controls {
                mute,
                ..Default::default()
            })),
            apply: tokio::sync::Mutex::new(()),
        })
    }
    pub fn context(&self) -> Option<Arc<Context>> {
        self.active
            .lock()
            .unwrap()
            .upgrade()
            .filter(|ctx| !ctx.cancelled.load(std::sync::atomic::Ordering::Acquire))
    }
    pub fn status(&self) -> serde_json::Value {
        let controls = self.controls.lock().unwrap().clone();
        if let Some(ctx) = self.context() {
            serde_json::json!({"connected":ctx.live.load(std::sync::atomic::Ordering::Acquire),"session":hex::encode(ctx.id),"config":*ctx.config.lock().unwrap(),"capabilities":*ctx.capabilities.lock().unwrap(),"stats":*ctx.stats.lock().unwrap(),"gain_db":controls.gain_db,"mute":controls.mute,"mirror":controls.mirror,"flip":controls.flip})
        } else {
            serde_json::json!({"connected":false,"config":*self.desired.lock().unwrap(),"capabilities":{},"gain_db":controls.gain_db,"mute":controls.mute,"mirror":controls.mirror,"flip":controls.flip})
        }
    }
    async fn configure(&self, patch: serde_json::Value) -> Result<serde_json::Value> {
        let _guard = self
            .apply
            .try_lock()
            .map_err(|_| anyhow::anyhow!("configuration busy; retry after acknowledgement"))?;
        let ctx = self
            .context()
            .ok_or_else(|| anyhow::anyhow!("connect the phone before changing capture settings"))?;
        ensure!(
            ctx.live.load(std::sync::atomic::Ordering::Acquire),
            "phone capture is not ready"
        );
        ensure!(
            ctx.capabilities.lock().unwrap()["live_controls"] == true,
            "phone update required for live controls"
        );
        ensure!(
            ctx.pending_config.lock().unwrap().is_none(),
            "configuration already pending"
        );
        let mut config = ctx.config.lock().unwrap().clone();
        let mut value = serde_json::to_value(&config)?;
        let fields = patch
            .as_object()
            .ok_or_else(|| anyhow::anyhow!("configuration must be an object"))?;
        for (key, input) in fields {
            ensure!(
                [
                    "camera_id",
                    "audio_input_id",
                    "audio_data_source",
                    "width",
                    "height",
                    "fps",
                    "bitrate",
                    "codec",
                    "profile",
                    "playout_ms"
                ]
                .contains(&key.as_str()),
                "unsupported configuration field {key}"
            );
            value[key] = input.clone();
        }
        config = serde_json::from_value(value)?;
        config.config_id = config
            .config_id
            .checked_add(1)
            .ok_or_else(|| anyhow::anyhow!("configuration generation exhausted"))?;
        config.fallback_reason = None;
        ensure!(config.validate(), "configuration outside supported bounds");
        let id = hex::encode(titan_transport::random::<8>());
        let (sender, receiver) = tokio::sync::oneshot::channel();
        *ctx.pending_config.lock().unwrap() = Some((id.clone(), sender));
        let mut command = Control::new(
            "Configure",
            &hex::encode(ctx.id),
            serde_json::to_value(config)?,
        );
        command.request_id = id.clone();
        if ctx.command.try_send(command).is_err() {
            ctx.pending_config.lock().unwrap().take();
            anyhow::bail!("control queue full");
        }
        let response = tokio::time::timeout(Duration::from_secs(15), receiver).await;
        let effective = match response {
            Ok(Ok(Ok(config))) => config,
            Ok(Ok(Err(error))) => anyhow::bail!("{error}"),
            _ => {
                let mut pending = ctx.pending_config.lock().unwrap();
                if pending.as_ref().is_some_and(|(request, _)| request == &id) {
                    pending.take();
                }
                anyhow::bail!(
                    "configuration acknowledgement timed out; inspect effective status before retrying"
                );
            }
        };
        *self.desired.lock().unwrap() = effective.clone();
        Ok(serde_json::to_value(effective)?)
    }
    pub async fn handle(&self, request: Request) -> Response {
        let result: Result<serde_json::Value> = async {
            ensure!(request.version == 1 && !request.id.is_empty() && request.id.len() <= 64, "invalid control envelope");
            match request.command.as_str() {
                "GetStatus" | "GetCapabilities" => Ok(self.status()),
                "SetConfig" => self.configure(request.body).await,
                "SetProfile" => {
                    let name = request.body["profile"].as_str().unwrap_or("");
                    ensure!(["saver", "balanced", "maximum"].contains(&name), "unknown profile");
                    let mut cfg = StreamConfig::profile(name, false);
                    if self.context().is_some_and(|ctx| ctx.config.lock().unwrap().codec == "hevc") {
                        cfg.bitrate = match name { "saver"=>3_000_000,"maximum"=>40_000_000,_=>9_000_000 };
                    }
                    self.configure(serde_json::json!({"profile":name,"width":cfg.width,"height":cfg.height,"fps":cfg.fps,"bitrate":cfg.bitrate,"playout_ms":cfg.playout_ms})).await
                }
                "SetAudio" => {
                    let mut controls = self.controls.lock().unwrap();
                    let mut next = controls.clone();
                    if let Some(value) = request.body.get("gain_db") { next.gain_db = value.as_f64().ok_or_else(|| anyhow::anyhow!("invalid gain"))?; }
                    if let Some(value) = request.body.get("mute") { next.mute = value.as_bool().ok_or_else(|| anyhow::anyhow!("invalid mute"))?; }
                    next.validate()?; *controls = next;
                    Ok(serde_json::json!({"gain_db":controls.gain_db,"mute":controls.mute}))
                }
                "SetTransform" => {
                    let mirror = request.body["mirror"].as_bool().ok_or_else(|| anyhow::anyhow!("invalid mirror"))?;
                    let flip = request.body["flip"].as_bool().ok_or_else(|| anyhow::anyhow!("invalid flip"))?;
                    let mut controls = self.controls.lock().unwrap(); controls.mirror = mirror; controls.flip = flip;
                    Ok(serde_json::json!({"mirror":mirror,"flip":flip}))
                }
                "Reconnect" => {
                    let ctx = self.context().ok_or_else(|| anyhow::anyhow!("no active phone session"))?;
                    ctx.cancelled.store(true, std::sync::atomic::Ordering::Release);
                    Ok(serde_json::json!({"reconnecting":true}))
                }
                _ => anyhow::bail!("unsupported local command"),
            }
        }.await;
        match result {
            Ok(body) => Response {
                version: 1,
                id: request.id,
                ok: true,
                body,
                error: None,
            },
            Err(error) => Response {
                version: 1,
                id: request.id,
                ok: false,
                body: serde_json::Value::Null,
                error: Some(error.to_string()),
            },
        }
    }
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Request {
    pub version: u32,
    pub id: String,
    pub command: String,
    #[serde(default)]
    pub body: serde_json::Value,
}
#[derive(Serialize)]
pub struct Response {
    version: u32,
    id: String,
    ok: bool,
    body: serde_json::Value,
    error: Option<String>,
}
pub fn socket_path() -> PathBuf {
    std::env::var_os("XDG_RUNTIME_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(titan_transport::state_dir)
        .join("titancam/control.sock")
}
pub async fn serve(hub: Arc<Hub>) -> Result<()> {
    use std::os::unix::fs::PermissionsExt;
    let path = socket_path();
    let parent = path.parent().unwrap();
    std::fs::create_dir_all(parent)?;
    std::fs::set_permissions(parent, std::fs::Permissions::from_mode(0o700))?;
    if path.exists() {
        ensure!(
            UnixStream::connect(&path).await.is_err(),
            "another receiver owns the control socket"
        );
        std::fs::remove_file(&path)?;
    }
    let listener = UnixListener::bind(&path)?;
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600))?;
    struct Cleanup(PathBuf);
    impl Drop for Cleanup {
        fn drop(&mut self) {
            let _ = std::fs::remove_file(&self.0);
        }
    }
    let _cleanup = Cleanup(path);
    let slots = Arc::new(Semaphore::new(8));
    loop {
        let (stream, _) = listener.accept().await?;
        let Ok(permit) = slots.clone().try_acquire_owned() else {
            continue;
        };
        let hub = hub.clone();
        tokio::spawn(async move {
            let _permit = permit;
            let result: Result<()> = async {
                let (read, mut write) = stream.into_split();
                let mut reader = BufReader::new(read).take(65537);
                let mut bytes = Vec::new();
                tokio::time::timeout(Duration::from_secs(3), reader.read_until(b'\n', &mut bytes))
                    .await??;
                ensure!(
                    bytes.len() <= 65536 && bytes.last() == Some(&b'\n'),
                    "oversized or unfinished request"
                );
                let response = hub.handle(serde_json::from_slice(&bytes)?).await;
                let mut data = serde_json::to_vec(&response)?;
                data.push(b'\n');
                ensure!(data.len() <= 65536, "response too large");
                tokio::time::timeout(Duration::from_secs(3), write.write_all(&data)).await??;
                Ok(())
            }
            .await;
            if let Err(error) = result {
                tracing::debug!("local control: {error}");
            }
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[tokio::test]
    async fn invalid_gain_is_atomic_and_disconnected_config_is_explicit() {
        let hub = Hub::new(StreamConfig::profile("balanced", false), false);
        let request = |command: &str, body| Request {
            version: 1,
            id: "test".into(),
            command: command.into(),
            body,
        };
        assert!(
            !hub.handle(request(
                "SetAudio",
                serde_json::json!({"gain_db":13,"mute":true})
            ))
            .await
            .ok
        );
        assert!(!hub.controls.lock().unwrap().mute);
        assert!(
            hub.handle(request("SetAudio", serde_json::json!({"gain_db":-6})))
                .await
                .ok
        );
        assert_eq!(hub.controls.lock().unwrap().gain_db, -6.0);
        assert!(
            !hub.handle(request("SetConfig", serde_json::json!({"fps":60})))
                .await
                .ok
        );
    }
}
