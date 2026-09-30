//! GStreamer video and allocation-free native PipeWire source adapter.
use anyhow::{Result, ensure};
use gstreamer::{self as gst, prelude::*};
use gstreamer_app as app;
use std::{ffi::c_void, ptr::NonNull, time::Duration};
unsafe extern "C" {
    fn tc_mic_create(channels: u32) -> *mut c_void;
    fn tc_mic_push(ptr: *mut c_void, samples: *const i16, frames: u32) -> u32;
    fn tc_mic_queued(ptr: *mut c_void) -> u32;
    fn tc_mic_underruns(ptr: *mut c_void) -> u64;
    fn tc_mic_destroy(ptr: *mut c_void);
    fn opus_decoder_create(rate: i32, channels: i32, error: *mut i32) -> *mut c_void;
    fn opus_decode(
        dec: *mut c_void,
        input: *const u8,
        len: i32,
        pcm: *mut i16,
        frames: i32,
        fec: i32,
    ) -> i32;
    fn opus_decoder_destroy(dec: *mut c_void);
}
pub struct Microphone {
    ptr: NonNull<c_void>,
    channels: u32,
}
// SAFETY: ownership moves to one media worker; RT C callback only accesses atomic SPSC state.
unsafe impl Send for Microphone {}
impl Microphone {
    pub fn new(channels: u32) -> Result<Self> {
        // SAFETY: C validates channel count and owns all stream resources.
        let ptr = NonNull::new(unsafe { tc_mic_create(channels) })
            .ok_or_else(|| anyhow::anyhow!("cannot publish PipeWire microphone"))?;
        Ok(Self { ptr, channels })
    }
    pub fn push(&mut self, pcm: &[i16]) -> usize {
        // SAFETY: a single producer and immutable slice; C checks ring capacity.
        (unsafe {
            tc_mic_push(
                self.ptr.as_ptr(),
                pcm.as_ptr(),
                (pcm.len() / self.channels as usize) as u32,
            )
        }) as usize
    }
    pub fn queued(&self) -> u32 {
        // SAFETY: allocated atomic state remains alive until Drop.
        unsafe { tc_mic_queued(self.ptr.as_ptr()) }
    }
    pub fn underruns(&self) -> u64 {
        // SAFETY: same lifetime and atomic read contract as queued.
        unsafe { tc_mic_underruns(self.ptr.as_ptr()) }
    }
}
impl Drop for Microphone {
    fn drop(&mut self) {
        // SAFETY: joins RT thread before freeing, no producer survives ownership.
        unsafe { tc_mic_destroy(self.ptr.as_ptr()) }
    }
}
pub struct AudioDecoder {
    ptr: NonNull<c_void>,
    channels: u32,
    pcm: Vec<i16>,
}
// SAFETY: one decoder worker exclusively owns this decoder and its output buffer.
unsafe impl Send for AudioDecoder {}
impl AudioDecoder {
    pub fn new(channels: u32) -> Result<Self> {
        ensure!((1..=2).contains(&channels), "invalid audio channels");
        let mut error = 0; // SAFETY: valid output error pointer and supported parameters.
        let ptr = NonNull::new(unsafe { opus_decoder_create(48000, channels as i32, &mut error) })
            .ok_or_else(|| anyhow::anyhow!("Opus decoder error {error}"))?;
        Ok(Self {
            ptr,
            channels,
            pcm: vec![0; 5760 * channels as usize],
        })
    }
    pub fn decode(&mut self, data: Option<&[u8]>, samples: u32) -> Result<&[i16]> {
        ensure!(samples <= 5760, "invalid Opus duration");
        let (ptr, len) = data.map_or((std::ptr::null(), 0), |b| (b.as_ptr(), b.len() as i32)); // SAFETY: decoder exclusively owned, payload bounded, PCM buffer supports maximum output.
        let n = unsafe {
            opus_decode(
                self.ptr.as_ptr(),
                ptr,
                len,
                self.pcm.as_mut_ptr(),
                samples as i32,
                0,
            )
        };
        ensure!(n >= 0, "Opus decode failed {n}");
        Ok(&self.pcm[..n as usize * self.channels as usize])
    }
}
impl Drop for AudioDecoder {
    fn drop(&mut self) {
        // SAFETY: unique pointer from opus_decoder_create.
        unsafe { opus_decoder_destroy(self.ptr.as_ptr()) }
    }
}
pub struct Video {
    pipeline: gst::Pipeline,
    source: app::AppSrc,
    pub decoder: String,
}
impl Video {
    pub fn new(codec: &str, preview: bool, webcam: Option<&str>, software: bool) -> Result<Self> {
        gst::init()?;
        ensure!(["h264", "hevc"].contains(&codec), "invalid codec");
        let (parser, hardware, cpu, caps) = if codec == "hevc" {
            ("h265parse", "nvh265dec", "avdec_h265", "video/x-h265")
        } else {
            ("h264parse", "nvh264dec", "avdec_h264", "video/x-h264")
        };
        let decoder = if !software && gst::ElementFactory::find(hardware).is_some() {
            hardware
        } else {
            cpu
        };
        let settings = if decoder == hardware {
            " max-display-delay=0 num-output-surfaces=0 discard-corrupted-frames=true"
        } else {
            ""
        };
        let mut branches = String::new();
        if preview {
            let sink = if decoder == hardware && gst::ElementFactory::find("glimagesink").is_some()
            {
                "glimagesink"
            } else {
                "autovideosink"
            };
            let conversion = if sink == "glimagesink" {
                ""
            } else {
                "videoconvert ! "
            };
            branches.push_str(&format!(" t. ! queue max-size-buffers=2 max-size-bytes=0 max-size-time=0 leaky=downstream ! {conversion}{sink} sync=true qos=true max-lateness=20000000 "));
        }
        if let Some(device) = webcam {
            ensure!(
                device.starts_with("/dev/video")
                    && device[10..].chars().all(|c| c.is_ascii_digit())
                    && device.len() > 10,
                "invalid V4L2 output device"
            );
            branches.push_str(&format!(" t. ! queue max-size-buffers=2 max-size-bytes=0 max-size-time=0 leaky=downstream ! videoconvert ! video/x-raw,format=NV12 ! v4l2sink device={device} sync=true "));
        }
        if branches.is_empty() {
            branches.push_str(" t. ! queue max-size-buffers=2 max-size-bytes=0 max-size-time=0 leaky=downstream ! fakesink sync=true ");
        }
        let launch = format!(
            "appsrc name=video is-live=true format=time block=false max-buffers=2 max-bytes=16777216 ! {parser} ! {decoder}{settings} ! tee name=t {branches}"
        );
        let pipeline = gst::parse::launch(&launch)?
            .downcast::<gst::Pipeline>()
            .map_err(|_| anyhow::anyhow!("invalid pipeline"))?;
        let source = pipeline
            .by_name("video")
            .unwrap()
            .downcast::<app::AppSrc>()
            .unwrap();
        source.set_caps(Some(
            &gst::Caps::builder(caps)
                .field("stream-format", "byte-stream")
                .field("alignment", "au")
                .build(),
        ));
        pipeline.set_state(gst::State::Playing)?;
        Ok(Self {
            pipeline,
            source,
            decoder: decoder.into(),
        })
    }
    pub fn push(&self, data: Vec<u8>, pts: u64, duration: u32) -> Result<()> {
        ensure!(
            self.source.current_level_buffers() < 2,
            "video ingress overrun; reference recovery required"
        );
        let mut b = gst::Buffer::from_mut_slice(data);
        {
            let b = b.get_mut().unwrap();
            b.set_pts(gst::ClockTime::from_nseconds(pts));
            b.set_dts(gst::ClockTime::from_nseconds(pts));
            b.set_duration(gst::ClockTime::from_nseconds(duration as u64));
        }
        self.source.push_buffer(b)?;
        Ok(())
    }
    pub fn running_time(&self) -> u64 {
        self.pipeline
            .current_running_time()
            .map_or(0, |t| t.nseconds())
    }
    pub fn poll_error(&self) -> Option<String> {
        let bus = self.pipeline.bus()?;
        for m in bus.iter_filtered(&[gst::MessageType::Error]) {
            if let gst::MessageView::Error(e) = m.view() {
                return Some(e.error().to_string());
            }
        }
        None
    }
    pub fn reset(&self) -> Result<()> {
        ensure!(
            self.pipeline.send_event(gst::event::FlushStart::new()),
            "video flush start rejected"
        );
        ensure!(
            self.pipeline.send_event(gst::event::FlushStop::new(true)),
            "video flush stop rejected"
        );
        Ok(())
    }
}
impl Drop for Video {
    fn drop(&mut self) {
        let _ = self.pipeline.set_state(gst::State::Null);
    }
}
pub fn decode_probe() -> Result<String> {
    gst::init()?;
    let encoder = if gst::ElementFactory::find("x264enc").is_some() {
        "x264enc tune=zerolatency speed-preset=ultrafast"
    } else {
        "openh264enc"
    };
    let decoder = if gst::ElementFactory::find("nvh264dec").is_some() {
        "nvh264dec"
    } else {
        "avdec_h264"
    };
    let p=gst::parse::launch(&format!("videotestsrc num-buffers=30 ! video/x-raw,width=1280,height=720,framerate=30/1 ! {encoder} ! h264parse ! {decoder} ! fakesink"))?.downcast::<gst::Pipeline>().map_err(|_|anyhow::anyhow!("probe pipeline"))?;
    p.set_state(gst::State::Playing)?;
    let outcome = p.bus().unwrap().timed_pop_filtered(
        gst::ClockTime::from_seconds(15),
        &[gst::MessageType::Eos, gst::MessageType::Error],
    );
    p.set_state(gst::State::Null)?;
    match outcome {
        Some(m) if matches!(m.view(), gst::MessageView::Eos(..)) => {
            Ok(format!("30 generated H.264 frames decoded by {decoder}"))
        }
        Some(m) => bail_message(m),
        None => Err(anyhow::anyhow!("decode probe timed out")),
    }
}
fn bail_message(m: gst::Message) -> Result<String> {
    if let gst::MessageView::Error(e) = m.view() {
        Err(anyhow::anyhow!("decoder probe: {}", e.error()))
    } else {
        Err(anyhow::anyhow!("unexpected probe message"))
    }
}
pub fn microphone_probe() -> Result<String> {
    let mut mic = Microphone::new(1)?;
    for _ in 0..30 {
        let _ = mic.push(&[0; 480]);
        std::thread::sleep(Duration::from_millis(10));
    }
    Ok(format!(
        "PipeWire source published; queued={}, underruns={}",
        mic.queued(),
        mic.underruns()
    ))
}
