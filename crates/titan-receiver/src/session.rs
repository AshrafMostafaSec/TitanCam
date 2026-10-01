use anyhow::{Result, ensure};
use serde::Serialize;
use std::{
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, AtomicUsize, Ordering},
    },
    time::{Duration, Instant},
};
use titan_media::dsp::{Controls, DriftResampler, Gain};
use titan_media::{AudioDecoder, Microphone, Video};
use titan_protocol::{Control, StreamConfig, Unit};
use tokio::sync::mpsc;
#[derive(Clone)]
pub struct OutputOptions {
    pub preview: bool,
    pub webcam: Option<String>,
    pub software: bool,
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
    pub decoded_video: u64,
    pub decoded_fps: f64,
    pub gain_db: f64,
    pub muted: bool,
    pub audio_peak: f64,
    pub audio_rms: f64,
    pub audio_clipped: u64,
    pub queue_age_ms: f64,
    pub late_video: u64,
    pub late_audio: u64,
    pub transport: String,
    pub last_error: String,
}
pub fn now_ns() -> u64 {
    use std::sync::OnceLock;
    static ORIGIN: OnceLock<Instant> = OnceLock::new();
    ORIGIN.get_or_init(Instant::now).elapsed().as_nanos() as u64
}
type PendingConfig = (
    String,
    tokio::sync::oneshot::Sender<Result<StreamConfig, String>>,
);
pub struct Context {
    pub created: Instant,
    pub id: [u8; 16],
    pub token: [u8; 32],
    pub config: Mutex<StreamConfig>,
    pub command: mpsc::Sender<Control>,
    pub video_units: mpsc::Sender<Unit>,
    pub audio_units: mpsc::Sender<Unit>,
    pub controls: Arc<Mutex<Controls>>,
    pub capabilities: Mutex<serde_json::Value>,
    pub pending_config: Mutex<Option<PendingConfig>>,
    pub fallback_anchor: Mutex<Option<(u64, u64)>>,
    pub queued_bytes: AtomicUsize,
    pub bound: AtomicBool,
    pub live: AtomicBool,
    pub cancelled: AtomicBool,
    pub stats: Mutex<Stats>,
    pub host_epoch: Mutex<Option<u64>>,
    pub clock: Mutex<crate::clock::ClockMapping>,
    pub last_media: Mutex<Instant>,
}
impl Context {
    pub fn request(&self, kind: &str, body: serde_json::Value) {
        let _ = self
            .command
            .try_send(Control::new(kind, &hex::encode(self.id), body));
    }
    pub fn accept(&self, unit: Unit) {
        if !self.live.load(Ordering::Acquire)
            || unit.header.session != self.id
            || unit.header.epoch != 1
        {
            return;
        }
        *self.last_media.lock().unwrap() = Instant::now();
        let size = unit.data.len();
        let is_video = unit.header.kind == 1;
        if self
            .queued_bytes
            .fetch_update(Ordering::AcqRel, Ordering::Acquire, |n| {
                n.checked_add(size).filter(|next| *next <= 16 * 1024 * 1024)
            })
            .is_err()
        {
            self.stats.lock().unwrap().dropped += 1;
            if is_video {
                self.request(
                    "RequestIDR",
                    serde_json::json!({"reason":"ingress_byte_limit"}),
                );
            }
            return;
        }
        let queue = if unit.header.kind == 1 {
            &self.video_units
        } else {
            &self.audio_units
        };
        if queue.try_send(unit).is_err() {
            self.queued_bytes.fetch_sub(size, Ordering::AcqRel);
            self.stats.lock().unwrap().dropped += 1;
            if is_video {
                self.request(
                    "RequestIDR",
                    serde_json::json!({"reason":"bounded_ingress_overrun"}),
                );
            }
        }
    }
    pub fn presentation_target(&self, unit: &Unit, config: &StreamConfig, now: u64) -> u64 {
        if let Some(host) = *self.host_epoch.lock().unwrap()
            && let Some(mapped) = self
                .clock
                .lock()
                .unwrap()
                .map(host.saturating_add(unit.header.pts))
        {
            mapped.saturating_add(config.playout_ms as u64 * 1_000_000)
        } else {
            let mut anchor = self.fallback_anchor.lock().unwrap();
            let (p, r) = *anchor.get_or_insert((unit.header.pts, now));
            r.saturating_add(unit.header.pts.saturating_sub(p))
                .saturating_add(config.playout_ms as u64 * 1_000_000)
        }
    }
    pub fn update_clock(&self, body: &serde_json::Value) {
        let parse = |k: &str| body[k].as_str().and_then(|s| s.parse::<f64>().ok());
        if let (Some(r1), Some(s2), Some(s3)) = (parse("r1"), parse("s2"), parse("s3")) {
            let mut clock = self.clock.lock().unwrap();
            clock.update(r1, s2, s3, now_ns() as f64);
            let mut stats = self.stats.lock().unwrap();
            stats.clock_rtt_ms = clock.rtt / 1e6;
            stats.clock_offset_ns = clock.offset();
        }
    }
}
pub fn create(
    config: StreamConfig,
    options: OutputOptions,
    output_permit: tokio::sync::OwnedSemaphorePermit,
    controls: Arc<Mutex<Controls>>,
) -> (Arc<Context>, mpsc::Receiver<Control>) {
    let (commands, rx) = mpsc::channel(32);
    let (video_units, video_input) = mpsc::channel(16);
    let (audio_units, audio_input) = mpsc::channel(32);
    let ctx = Arc::new(Context {
        created: Instant::now(),
        id: titan_transport::random(),
        token: titan_transport::random(),
        config: Mutex::new(config),
        command: commands,
        video_units,
        audio_units,
        controls,
        capabilities: Mutex::new(serde_json::json!({})),
        pending_config: Mutex::new(None),
        fallback_anchor: Mutex::new(None),
        queued_bytes: AtomicUsize::new(0),
        bound: AtomicBool::new(false),
        live: AtomicBool::new(false),
        cancelled: AtomicBool::new(false),
        stats: Mutex::new(Stats::default()),
        host_epoch: Mutex::new(None),
        clock: Mutex::new(crate::clock::ClockMapping::default()),
        last_media: Mutex::new(Instant::now()),
    });
    let c = ctx.clone();
    tokio::spawn(async move {
        // Release only after GStreamer and PipeWire resources have been dropped.
        let _output_permit = output_permit;
        if let Err(e) = tokio::try_join!(
            video_worker(c.clone(), video_input, options.clone()),
            audio_worker(c.clone(), audio_input),
        ) {
            tracing::error!("media stopped: {e}");
            c.live.store(false, Ordering::Release);
            c.cancelled.store(true, Ordering::Release);
            c.stats.lock().unwrap().last_error = e.to_string();
            c.request(
                "Error",
                serde_json::json!({"reason":"media_output_failure"}),
            );
        }
    });
    (ctx, rx)
}
async fn video_worker(
    ctx: Arc<Context>,
    mut input: mpsc::Receiver<Unit>,
    options: OutputOptions,
) -> Result<()> {
    let mut video: Option<Video> = None;
    let mut waiting_idr = true;
    let mut video_sequence = None;
    let mut video_anchor: Option<(u64, u64)> = None;
    let mut active_config = None;
    let mut active_format = None;
    let mut decoded_base = 0;
    let mut idr_at = Instant::now() - Duration::from_secs(1);
    loop {
        let unit = tokio::select! {
            unit=input.recv()=>match unit{Some(u)=>u,None=>break},
            _=tokio::time::sleep(Duration::from_millis(100))=>{if ctx.cancelled.load(Ordering::Acquire){break}continue}
        };
        ctx.queued_bytes
            .fetch_sub(unit.data.len(), Ordering::AcqRel);
        if ctx.cancelled.load(Ordering::Acquire) {
            break;
        }
        if !ctx.live.load(Ordering::Acquire) {
            continue;
        }
        let config = ctx.config.lock().unwrap().clone();
        ensure!(config.validate(), "invalid effective configuration");
        if unit.header.config != config.config_id || unit.header.session != ctx.id {
            continue;
        }
        if active_config != Some(config.config_id) {
            let format = (
                config.codec.clone(),
                config.width,
                config.height,
                config.fps,
            );
            if active_format.as_ref() != Some(&format) {
                if let Some(v) = &video {
                    decoded_base += v.decoded();
                }
                video = None;
            }
            active_format = Some(format);
            video_sequence = None;
            video_anchor = None;
            waiting_idr = true;
            active_config = Some(config.config_id);
        }
        let now = now_ns();
        let target = ctx.presentation_target(&unit, &config, now);
        if now > target.saturating_add(120_000_000) {
            let mut stats = ctx.stats.lock().unwrap();
            stats.dropped += 1;
            if unit.header.kind == 1 {
                stats.late_video += 1;
            } else {
                stats.late_audio += 1;
            }
            drop(stats);
            if unit.header.kind == 1 {
                waiting_idr = true;
            }
            continue;
        }
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
        let decoder_created = video.is_none();
        if decoder_created {
            let v = Video::new(
                &config.codec,
                options.preview,
                options.webcam.as_deref(),
                options.software,
            )
            .or_else(|error| {
                tracing::warn!("initial decoder failed, trying CPU: {error}");
                Video::new(
                    &config.codec,
                    options.preview,
                    options.webcam.as_deref(),
                    true,
                )
            })?;
            ctx.stats.lock().unwrap().decoder = v.decoder.clone();
            video = Some(v);
            video_anchor = None;
        }
        let v = video.as_ref().unwrap();
        v.set_transform(&ctx.controls.lock().unwrap());
        if let Some(e) = v.poll_error() {
            tracing::warn!("decoder error: {e}");
            decoded_base += v.decoded();
            video = Some(Video::new(
                &config.codec,
                options.preview,
                options.webcam.as_deref(),
                true,
            )?);
            ctx.stats.lock().unwrap().decoder = video.as_ref().unwrap().decoder.clone();
            video_anchor = None;
            waiting_idr = true;
            ctx.request("RequestIDR", serde_json::json!({"reason":"decoder_reset"}));
            continue;
        }
        if waiting_idr {
            if !decoder_created {
                v.reset()?;
            }
            video_anchor = None;
            ctx.stats.lock().unwrap().recoveries += 1;
            waiting_idr = false;
        }
        let (receiver_start, pipeline_start) = *video_anchor.get_or_insert((now, v.running_time()));
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
        stats.decoded_video = decoded_base + v.decoded();
        stats.queue_age_ms = now.saturating_sub(target) as f64 / 1e6;
        stats.profile = config.profile.clone();
    }
    Ok(())
}
async fn audio_worker(ctx: Arc<Context>, mut input: mpsc::Receiver<Unit>) -> Result<()> {
    let mut mic: Option<Microphone> = None;
    let mut opus: Option<AudioDecoder> = None;
    let mut audio_sequence = None;
    let mut active_config = None;
    let mut active_channels = 0;
    let mut resampler = DriftResampler::new(1);
    let mut gain = Gain::default();
    loop {
        let unit = tokio::select! {
            unit=input.recv()=>match unit{Some(u)=>u,None=>break},
            _=tokio::time::sleep(Duration::from_millis(100))=>{if ctx.cancelled.load(Ordering::Acquire){break}continue}
        };
        ctx.queued_bytes
            .fetch_sub(unit.data.len(), Ordering::AcqRel);
        if ctx.cancelled.load(Ordering::Acquire) {
            break;
        }
        if !ctx.live.load(Ordering::Acquire) {
            continue;
        }
        let config = ctx.config.lock().unwrap().clone();
        ensure!(config.validate(), "invalid effective configuration");
        if unit.header.config != config.config_id || unit.header.session != ctx.id {
            continue;
        }
        if active_config != Some(config.config_id) {
            if active_channels != config.audio_channels {
                mic = None;
            }
            active_channels = config.audio_channels;
            opus = None;
            audio_sequence = None;
            resampler = DriftResampler::new(active_channels as usize);
            active_config = Some(config.config_id);
        }
        let now = now_ns();
        let target = ctx.presentation_target(&unit, &config, now);
        if now > target.saturating_add(120_000_000) {
            let mut stats = ctx.stats.lock().unwrap();
            stats.dropped += 1;
            stats.late_audio += 1;
            continue;
        }
        if audio_sequence.is_some_and(|n| unit.header.sequence <= n) {
            continue;
        }
        if mic.is_none() {
            mic = Some(Microphone::new(config.audio_channels)?);
        }
        if opus.is_none() {
            opus = Some(AudioDecoder::new(config.audio_channels)?);
        }
        let frames = (unit.header.duration as u64 * 48_000 / 1_000_000_000) as u32;
        let mut pcm = if unit.header.kind == 3 {
            if let Some(last) = audio_sequence {
                let missing = unit.header.sequence.saturating_sub(last + 1).min(3);
                for _ in 0..missing {
                    let concealed = opus.as_mut().unwrap().decode(None, frames)?;
                    let mut concealed = resampler.process(concealed, 1.0);
                    gain.process(
                        &mut concealed,
                        config.audio_channels as usize,
                        &ctx.controls.lock().unwrap(),
                    );
                    mic.as_mut().unwrap().push(&concealed);
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
        pcm = resampler.process(&pcm, 1.0 + correction);
        let controls = ctx.controls.lock().unwrap().clone();
        gain.process(&mut pcm, config.audio_channels as usize, &controls);
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
        stats.gain_db = controls.gain_db;
        stats.muted = controls.mute;
        stats.audio_peak = gain.peak;
        stats.audio_rms = gain.rms;
        stats.audio_clipped = gain.clipped;
        stats.microphone_underruns = m.underruns();
        stats.audio_queue_frames = m.queued();
    }
    Ok(())
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
