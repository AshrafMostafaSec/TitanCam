//! Stateful receiver DSP. This runs on the producer, never the PipeWire RT callback.
use anyhow::{Result, ensure};

#[derive(Clone, Debug, Default)]
pub struct Controls {
    pub gain_db: f64,
    pub mute: bool,
    pub mirror: bool,
    pub flip: bool,
}
impl Controls {
    pub fn validate(&self) -> Result<()> {
        ensure!(
            self.gain_db.is_finite() && (-60.0..=12.0).contains(&self.gain_db),
            "gain must be -60..12 dB"
        );
        Ok(())
    }
    pub fn direction(&self) -> &'static str {
        match (self.mirror, self.flip) {
            (false, false) => "identity",
            (true, false) => "horiz",
            (false, true) => "vert",
            (true, true) => "180",
        }
    }
}
pub struct Gain {
    level: f64,
    pub clipped: u64,
    pub peak: f64,
    pub rms: f64,
}
impl Default for Gain {
    fn default() -> Self {
        Self {
            level: 1.0,
            clipped: 0,
            peak: 0.0,
            rms: 0.0,
        }
    }
}
impl Gain {
    pub fn process(&mut self, pcm: &mut [i16], channels: usize, controls: &Controls) {
        let target = if controls.mute {
            0.0
        } else {
            10f64.powf(controls.gain_db / 20.0)
        };
        let mut squares = 0.0;
        self.peak = 0.0;
        for frame in pcm.chunks_exact_mut(channels) {
            // One gain per frame; stereo channels never receive different ramps.
            self.level += (target - self.level).clamp(-1.0 / 480.0, 1.0 / 480.0);
            for sample in frame {
                let value = *sample as f64 * self.level;
                if !(-32768.0..=32767.0).contains(&value) {
                    self.clipped += 1;
                }
                *sample = value.round().clamp(-32768.0, 32767.0) as i16;
                let normalized = *sample as f64 / 32768.0;
                self.peak = self.peak.max(normalized.abs());
                squares += normalized * normalized;
            }
        }
        self.rms = if pcm.is_empty() {
            0.0
        } else {
            (squares / pcm.len() as f64).sqrt()
        };
    }
}

/// Streaming linear drift corrector. Carries fractional phase and the last
/// interleaved frame across packets, rather than rounding each packet's length.
pub struct DriftResampler {
    channels: usize,
    phase: f64,
    tail: Vec<i16>,
}
impl DriftResampler {
    pub fn new(channels: usize) -> Self {
        Self {
            channels,
            phase: 0.0,
            tail: Vec::new(),
        }
    }
    pub fn process(&mut self, input: &[i16], ratio: f64) -> Vec<i16> {
        let ratio = ratio.clamp(0.9997, 1.0003);
        let mut samples = std::mem::take(&mut self.tail);
        samples.extend_from_slice(input);
        let frames = samples.len() / self.channels;
        let mut output = Vec::with_capacity(samples.len());
        while self.phase + 1.0 < frames as f64 {
            let a = self.phase.floor() as usize;
            let fraction = self.phase - a as f64;
            for c in 0..self.channels {
                let x = samples[a * self.channels + c] as f64;
                let y = samples[(a + 1) * self.channels + c] as f64;
                output.push((x + (y - x) * fraction).round() as i16);
            }
            self.phase += ratio;
        }
        if frames > 0 {
            self.phase -= (frames - 1) as f64;
            self.tail
                .extend_from_slice(&samples[(frames - 1) * self.channels..frames * self.channels]);
        }
        output
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn gain_clips_without_wrapping_and_mute_preserves_duration() {
        let mut gain = Gain::default();
        let mut pcm = vec![20000; 4800];
        gain.process(
            &mut pcm,
            1,
            &Controls {
                gain_db: 12.0,
                ..Default::default()
            },
        );
        assert!(pcm.iter().all(|s| *s > 0));
        assert!(gain.clipped > 0);
        let controls = Controls {
            mute: true,
            ..Default::default()
        };
        gain.process(&mut pcm, 1, &controls);
        assert_eq!(pcm.len(), 4800);
        assert_eq!(pcm[4799], 0);
        assert!(
            Controls {
                gain_db: f64::NAN,
                ..Default::default()
            }
            .validate()
            .is_err()
        );
    }
    #[test]
    fn drift_carries_fractional_phase_across_packets() {
        for ratio in [0.9997, 1.0, 1.0003] {
            let mut resampler = DriftResampler::new(2);
            let pcm: Vec<i16> = (0..480).flat_map(|_| [1000, -1000]).collect();
            let mut frames = 0;
            for _ in 0..1000 {
                let output = resampler.process(&pcm, ratio);
                assert!(
                    output
                        .as_chunks::<2>()
                        .0
                        .iter()
                        .all(|s| *s == [1000, -1000])
                );
                frames += output.len() / 2;
            }
            assert!((frames as f64 - 480000.0 / ratio).abs() < 3.0);
        }
    }
}
