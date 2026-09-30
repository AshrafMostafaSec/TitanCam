//! Local synthetic microphone test: captures its own generated tone, never the user's microphone.
use anyhow::{Result, ensure};
use std::{
    process::{Command, Stdio},
    time::{Duration, Instant},
};
fn main() -> Result<()> {
    let path = std::env::args()
        .nth(1)
        .unwrap_or_else(|| "work/microphone-tone.pcm".into());
    let mut microphone = titan_media::Microphone::new(1)?;
    std::thread::sleep(Duration::from_millis(400));
    let mut capture = Command::new("pw-record")
        .args([
            "--target",
            "titancam_mic",
            "--rate",
            "48000",
            "--channels",
            "1",
            "--format",
            "s16",
            "--raw",
            &path,
        ])
        .stdout(Stdio::null())
        .spawn()?;
    std::thread::sleep(Duration::from_millis(500));
    let origin = Instant::now();
    for packet in 0..300u64 {
        let pcm: Vec<_> = (0..480u64)
            .map(|i| {
                ((2.0 * std::f64::consts::PI * 440.0 * ((packet * 480 + i) as f64) / 48000.0).sin()
                    * 12000.0) as i16
            })
            .collect();
        let _ = microphone.push(&pcm);
        let target = Duration::from_millis((packet + 1) * 10);
        if let Some(delay) = target.checked_sub(origin.elapsed()) {
            std::thread::sleep(delay);
        }
    }
    capture.kill()?;
    capture.wait()?;
    let bytes = std::fs::read(&path)?;
    let pcm: Vec<_> = bytes
        .as_chunks::<2>()
        .0
        .iter()
        .map(|b| i16::from_le_bytes(*b))
        .collect();
    let energetic = pcm.iter().filter(|x| x.unsigned_abs() > 3000).count();
    ensure!(
        energetic > 48000,
        "PipeWire consumer did not receive the generated tone"
    );
    println!(
        "Synthetic native PipeWire source → pw-record consumer: {} samples, {} energetic samples; queue {}, underruns {}",
        pcm.len(),
        energetic,
        microphone.queued(),
        microphone.underruns()
    );
    Ok(())
}
