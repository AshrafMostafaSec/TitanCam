use anyhow::{Result, ensure};
use serde::Serialize;
use std::{
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
    },
    time::{Duration, Instant},
};
use titan_media::{AudioDecoder, Microphone, Video};
use titan_protocol::{Control, Reassembler, StreamConfig, Unit};
use tokio::sync::mpsc;
#[derive(Clone)]
pub struct OutputOptions {
    pub preview: bool,
    pub webcam: Option<String>,
    pub software: bool,
    pub mute: bool,
}
#[derive(Default, Serialize, Clone)]
pub struct Stats {
    pub video: u64,
    pub audio: u64,
    pub bytes: u64,
    pub expired: u64,
    pub dropped: u64,
    pub recoveries: u64,
    pub reference_discarded: u64,
    pub missing_units: u64,
    pub audio_plc: u64,
    pub microphone_underruns: u64,
    pub decoder: String,
    pub profile: String,
    pub clock_rtt_ms: f64,
    pub clock_offset_ns: f64,
    pub audio_queue_frames: u32,
}
pub fn now_ns() -> u64 {
    use std::sync::OnceLock;
    static ORIGIN: OnceLock<Instant> = OnceLock::new();
    ORIGIN.get_or_init(Instant::now).elapsed().as_nanos() as u64
}
pub struct Context {
    pub created: Instant,
    pub id: [u8; 16],
    pub token: [u8; 32],
    pub config: Mutex<StreamConfig>,
    pub command: mpsc::Sender<Control>,
    pub units: mpsc::Sender<Unit>,
    pub bound: AtomicBool,
    pub live: AtomicBool,
    pub cancelled: AtomicBool,
    pub stats: Mutex<Stats>,
    pub host_epoch: Mutex<Option<u64>>,
    pub clock: Mutex<Option<(f64, f64)>>,
    pub last_media: Mutex<Instant>,
}
impl Context {
    pub fn request(&self, kind: &str, body: serde_json::Value) {
        let _ = self
            .command
            .try_send(Control::new(kind, &hex::encode(self.id), body));
    }
    pub fn accept(&self, unit: Unit) {
        if !self.live.load(Ordering::Acquire) {
            return;
        }
        *self.last_media.lock().unwrap() = Instant::now();
        if self.units.try_send(unit).is_err() {
            self.stats.lock().unwrap().dropped += 1;
            self.request(
                "RequestIDR",
                serde_json::json!({"reason":"bounded_ingress_overrun"}),
            );
        }
    }
    pub fn update_clock(&self, body: &serde_json::Value) {
        let parse = |k: &str| body[k].as_str().and_then(|s| s.parse::<f64>().ok());
        if let (Some(r1), Some(s2), Some(s3)) = (parse("r1"), parse("s2"), parse("s3")) {
            let r4 = now_ns() as f64;
            let rtt = (r4 - r1) - (s3 - s2);
            if !(0.0..=500_000_000.0).contains(&rtt) {
                return;
            }
            let offset = ((s2 - r1) + (s3 - r4)) / 2.0;
            let mut c = self.clock.lock().unwrap();
            if c.is_none_or(|(_, best)| rtt < best * 1.5) {
                *c = Some((offset, rtt));
                let mut stats = self.stats.lock().unwrap();
                stats.clock_rtt_ms = rtt / 1e6;
                stats.clock_offset_ns = offset;
            }
        }
    }
}
pub fn create(
    config: StreamConfig,
    options: OutputOptions,
) -> (Arc<Context>, mpsc::Receiver<Control>) {
    let (commands, rx) = mpsc::channel(32);
    let (units, input) = mpsc::channel(16);
    let ctx = Arc::new(Context {
        created: Instant::now(),
        id: titan_transport::random(),
        token: titan_transport::random(),
        config: Mutex::new(config),
        command: commands,
        units,
        bound: AtomicBool::new(false),
        live: AtomicBool::new(false),
        cancelled: AtomicBool::new(false),
        stats: Mutex::new(Stats::default()),
        host_epoch: Mutex::new(None),
        clock: Mutex::new(None),
        last_media: Mutex::new(Instant::now()),
    });
    let c = ctx.clone();
    tokio::spawn(async move {
        if let Err(e) = media_worker(c.clone(), input, options).await {
            tracing::error!("media stopped: {e}");
            c.live.store(false, Ordering::Release);
            c.request(
                "Error",
                serde_json::json!({"reason":"media_output_failure"}),
            );
        }
    });
    (ctx, rx)
}
async fn media_worker(
    ctx: Arc<Context>,
    mut input: mpsc::Receiver<Unit>,
    options: OutputOptions,
) -> Result<()> {
    let mut video: Option<Video> = None;
    let mut mic: Option<Microphone> = None;
    let mut opus: Option<AudioDecoder> = None;
    let mut waiting_idr = true;
    let mut video_sequence = None;
    let mut audio_sequence = None;
    let mut fallback_anchor: Option<(u64, u64)> = None;
    let mut video_anchor: Option<(u64, u64)> = None;
    let mut active_config = None;
    let mut idr_at = Instant::now() - Duration::from_secs(1);
    loop {
        let unit = tokio::select! {unit=input.recv()=>match unit{Some(u)=>u,None=>break},_=tokio::time::sleep(Duration::from_millis(250))=>{if ctx.cancelled.load(Ordering::Acquire){break}continue}};
        if !ctx.live.load(Ordering::Acquire) {
            video = None;
            mic = None;
            opus = None;
            waiting_idr = true;
            continue;
        }
        let config = ctx.config.lock().unwrap().clone();
        ensure!(config.validate(), "invalid effective configuration");
        if unit.header.config != config.config_id || unit.header.session != ctx.id {
            continue;
        }
        if active_config != Some(config.config_id) {
            video = None;
            mic = None;
            opus = None;
            video_sequence = None;
            audio_sequence = None;
            video_anchor = None;
            waiting_idr = true;
            active_config = Some(config.config_id);
        }
        let now = now_ns();
        let target = if let (Some(host), Some((offset, _))) =
            (*ctx.host_epoch.lock().unwrap(), *ctx.clock.lock().unwrap())
        {
            ((host as f64 + unit.header.pts as f64 - offset).max(0.0) as u64)
                .saturating_add(config.playout_ms as u64 * 1_000_000)
        } else {
            let (p, r) = *fallback_anchor.get_or_insert((unit.header.pts, now));
            r.saturating_add(unit.header.pts.saturating_sub(p))
                .saturating_add(config.playout_ms as u64 * 1_000_000)
        };
        if now > target.saturating_add(120_000_000) {
            ctx.stats.lock().unwrap().dropped += 1;
            if unit.header.kind == 1 {
                waiting_idr = true;
            }
            continue;
        }
        if unit.header.kind == 1 {
            if video_sequence.is_some_and(|n| unit.header.sequence <= n) {
                continue;
            }
            if video_sequence.is_some_and(|n| unit.header.sequence != n + 1) {
                waiting_idr = true;
                ctx.stats.lock().unwrap().missing_units += unit
                    .header
                    .sequence
                    .saturating_sub(video_sequence.unwrap() + 1);
            }
            video_sequence = Some(unit.header.sequence);
            if waiting_idr && !unit.header.independent() {
                let mut stats = ctx.stats.lock().unwrap();
                stats.reference_discarded += 1;
                stats.dropped += 1;
                drop(stats);
                if idr_at.elapsed() > Duration::from_millis(250) {
                    ctx.request("RequestIDR", serde_json::json!({"reason":"reference_loss"}));
                    idr_at = Instant::now();
                }
                continue;
            }
            if video.is_none() {
                let v = Video::new(
                    &config.codec,
                    options.preview,
                    options.webcam.as_deref(),
                    options.software,
                )?;
                ctx.stats.lock().unwrap().decoder = v.decoder.clone();
                video = Some(v);
                video_anchor = None;
            }
            let v = video.as_ref().unwrap();
            if let Some(e) = v.poll_error() {
                tracing::warn!("decoder error: {e}");
                video = Some(Video::new(
                    &config.codec,
                    options.preview,
                    options.webcam.as_deref(),
                    true,
                )?);
                video_anchor = None;
                waiting_idr = true;
                ctx.request("RequestIDR", serde_json::json!({"reason":"decoder_reset"}));
                continue;
            }
            if waiting_idr {
                v.reset()?;
                video_anchor = None;
                ctx.stats.lock().unwrap().recoveries += 1;
                waiting_idr = false;
            }
            let (receiver_start, pipeline_start) =
                *video_anchor.get_or_insert((now, v.running_time()));
            let pts = pipeline_start.saturating_add(target.saturating_sub(receiver_start));
            if v.push(unit.data, pts, unit.header.duration).is_err() {
                waiting_idr = true;
                ctx.stats.lock().unwrap().dropped += 1;
                ctx.request(
                    "RequestIDR",
                    serde_json::json!({"reason":"decoder_backpressure"}),
                );
                continue;
            }
            let mut stats = ctx.stats.lock().unwrap();
            stats.video += 1;
            stats.profile = config.profile.clone();
        } else {
            if audio_sequence.is_some_and(|n| unit.header.sequence <= n) {
                continue;
            }
            if options.mute {
                continue;
            }
            if mic.is_none() {
                mic = Some(Microphone::new(config.audio_channels)?);
                opus = Some(AudioDecoder::new(config.audio_channels)?);
            }
            let frames = (unit.header.duration as u64 * 48_000 / 1_000_000_000) as u32;
            let mut pcm = if unit.header.kind == 3 {
                if let Some(last) = audio_sequence {
                    let missing = unit.header.sequence.saturating_sub(last + 1).min(3);
                    for _ in 0..missing {
                        let concealed = opus.as_mut().unwrap().decode(None, frames)?;
                        mic.as_mut().unwrap().push(concealed);
                        ctx.stats.lock().unwrap().audio_plc += 1;
                    }
                }
                opus.as_mut()
                    .unwrap()
                    .decode(Some(&unit.data), frames)?
                    .to_vec()
            } else {
                ensure!(unit.data.len() % 2 == 0, "invalid PCM alignment");
                unit.data
                    .as_chunks::<2>()
                    .0
                    .iter()
                    .map(|b| i16::from_le_bytes([b[0], b[1]]))
                    .collect()
            };
            audio_sequence = Some(unit.header.sequence);
            // Small fill-driven resampling corrects audio-clock drift without changing global PipeWire settings.
            let queued = mic.as_ref().unwrap().queued();
            if queued > 4800 {
                ctx.stats.lock().unwrap().dropped += 1;
                continue;
            }
            let correction = ((queued as f64 - 480.0) / 4800.0 * 0.0003).clamp(-0.0003, 0.0003);
            pcm = resample(&pcm, config.audio_channels as usize, 1.0 + correction);
            let delay = target.saturating_sub(now_ns());
            if delay > 0 && delay < 100_000_000 {
                tokio::time::sleep(Duration::from_nanos(delay)).await;
            }
            let m = mic.as_mut().unwrap();
            if m.push(&pcm) == 0 {
                ctx.stats.lock().unwrap().dropped += 1;
            }
            let mut stats = ctx.stats.lock().unwrap();
            stats.audio += 1;
            stats.microphone_underruns = m.underruns();
            stats.audio_queue_frames = m.queued();
        }
    }
    Ok(())
}
fn resample(input: &[i16], channels: usize, ratio: f64) -> Vec<i16> {
    let frames = input.len() / channels;
    if frames < 2 {
        return input.to_vec();
    }
    let output = (frames as f64 / ratio).round() as usize;
    let mut pcm = Vec::with_capacity(output * channels);
    for i in 0..output {
        let pos = (i as f64 * ratio).min((frames - 1) as f64);
        let a = pos.floor() as usize;
        let b = (a + 1).min(frames - 1);
        let fraction = pos - a as f64;
        for c in 0..channels {
            pcm.push(
                (input[a * channels + c] as f64 * (1.0 - fraction)
                    + input[b * channels + c] as f64 * fraction) as i16,
            )
        }
    }
    pcm
}
pub struct Lease(pub Option<Arc<Context>>);
impl Drop for Lease {
    fn drop(&mut self) {
        if let Some(c) = &self.0 {
            c.live.store(false, Ordering::Release);
            c.cancelled.store(true, Ordering::Release);
        }
    }
}
pub async fn datagrams(ctx: Arc<Context>, connection: quinn::Connection) -> Result<()> {
    let mut config_id = ctx.config.lock().unwrap().config_id;
    let mut assembler = Reassembler::new(ctx.id, 1, config_id, Duration::from_millis(60));
    let mut tick = tokio::time::interval(Duration::from_millis(10));
    loop {
        tokio::select! {b=connection.read_datagram()=>{let b=b?;if titan_protocol::check_binding(&b,ctx.id,1,ctx.token){continue}ctx.stats.lock().unwrap().bytes+=b.len()as u64;let active=ctx.config.lock().unwrap().config_id;if active!=config_id{config_id=active;assembler=Reassembler::new(ctx.id,1,config_id,Duration::from_millis(60));}match assembler.push(&b,Instant::now()){Ok(Some(unit))=>{ctx.accept(unit)},Ok(None)=>(),Err(_)=>{ctx.stats.lock().unwrap().dropped+=1;}}},_=tick.tick()=>{if assembler.expire(Instant::now()){ctx.request("RequestIDR",serde_json::json!({"reason":"fragment_deadline"}));}ctx.stats.lock().unwrap().expired=assembler.expired;if ctx.cancelled.load(Ordering::Acquire){connection.close(0u8.into(),b"session ended");break}}}
    }
    Ok(())
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn drift_resampling_preserves_channels() {
        let i = vec![1000, -1000, 2000, -2000, 3000, -3000];
        let o = resample(&i, 2, 1.0);
        assert_eq!(i, o);
    }
}
