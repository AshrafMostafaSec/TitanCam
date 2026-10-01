//! Low-RTT affine clock mapping with bounded drift and no wall-clock dependency.
use std::collections::VecDeque;
#[derive(Default)]
pub struct ClockMapping {
    samples: VecDeque<(f64, f64, f64)>,
    anchor: Option<(f64, f64)>,
    rate: f64,
    pub rtt: f64,
}
impl ClockMapping {
    pub fn update(&mut self, r1: f64, s2: f64, s3: f64, r4: f64) {
        if [r1, s2, s3, r4].iter().any(|v| !v.is_finite() || *v < 0.0) || s3 < s2 || r4 < r1 {
            return;
        }
        let rtt = (r4 - r1) - (s3 - s2);
        if !(0.0..=500_000_000.0).contains(&rtt) {
            return;
        }
        let sender = (s2 + s3) / 2.0;
        let receiver = (r1 + r4) / 2.0;
        self.samples.push_back((sender, receiver, rtt));
        while self.samples.len() > 32 {
            self.samples.pop_front();
        }
        let best = self
            .samples
            .iter()
            .map(|s| s.2)
            .fold(f64::INFINITY, f64::min);
        if rtt > best * 1.5 + 1_000_000.0 {
            return;
        }
        let selected: Vec<_> = self
            .samples
            .iter()
            .filter(|s| s.2 <= best * 1.5 + 1_000_000.0)
            .collect();
        let base = selected[0];
        let mean_x = selected.iter().map(|s| s.0 - base.0).sum::<f64>() / selected.len() as f64;
        let mean_y = selected.iter().map(|s| s.1 - base.1).sum::<f64>() / selected.len() as f64;
        let covariance = selected
            .iter()
            .map(|s| ((s.0 - base.0) - mean_x) * ((s.1 - base.1) - mean_y))
            .sum::<f64>();
        let variance = selected
            .iter()
            .map(|s| ((s.0 - base.0) - mean_x).powi(2))
            .sum::<f64>();
        let desired_rate =
            if selected.len() >= 6 && sender - base.0 > 5_000_000_000.0 && variance > 0.0 {
                (covariance / variance).clamp(0.9995, 1.0005)
            } else {
                1.0
            };
        let desired_receiver = base.1 + mean_y + (sender - base.0 - mean_x) * desired_rate;
        if let Some((old_sender, old_receiver)) = self.anchor {
            let predicted = old_receiver + (sender - old_sender) * self.rate;
            self.anchor = Some((
                sender,
                predicted + (desired_receiver - predicted).clamp(-500_000.0, 500_000.0),
            ));
            self.rate += (desired_rate - self.rate) * 0.2;
        } else {
            self.anchor = Some((sender, desired_receiver));
            self.rate = 1.0;
        }
        self.rtt = rtt;
    }
    pub fn map(&self, sender: u64) -> Option<u64> {
        self.anchor
            .map(|(s, r)| (r + (sender as f64 - s) * self.rate).max(0.0) as u64)
    }
    pub fn offset(&self) -> f64 {
        self.anchor.map_or(0.0, |(s, r)| s - r)
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn long_clock_drift_and_high_rtt_outliers_do_not_break_sync() {
        for rate in [0.9997, 1.0003] {
            let mut clock = ClockMapping::default();
            for second in 0..7200 {
                let r = second as f64 * 1e9;
                let s = 8e12 + r * rate;
                clock.update(r, s + 1e6 * rate, s + 1e6 * rate, r + 2e6);
                if second > 60 {
                    let mapped = clock.map((s + 1e6 * rate) as u64).unwrap();
                    assert!((mapped as f64 - (r + 1e6)).abs() < 1e6);
                }
            }
            let before = clock.map(8_000_000_000_000).unwrap();
            clock.update(f64::NAN, 0.0, 0.0, 0.0);
            assert_eq!(before, clock.map(8_000_000_000_000).unwrap());
        }
    }
}
