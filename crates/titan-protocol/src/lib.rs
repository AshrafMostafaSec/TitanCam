//! Versioned, allocation-bounded TitanCam wire contract.
use serde::{Deserialize, Serialize};
use std::{
    collections::BTreeMap,
    time::{Duration, Instant},
};
use thiserror::Error;
pub const HEADER: usize = 64;
pub const CONTROL_LIMIT: usize = 65_536;
pub const VIDEO_LIMIT: usize = 8 * 1024 * 1024;
pub const AUDIO_LIMIT: usize = 65_536;
pub const REASSEMBLY_LIMIT: usize = 32 * 1024 * 1024;
#[derive(Debug, Error)]
pub enum WireError {
    #[error("invalid or truncated media header")]
    Header,
    #[error("invalid size, kind, flags, or fragment range")]
    Bounds,
    #[error("stale session, epoch, or configuration")]
    Stale,
    #[error("conflicting fragment metadata")]
    Conflict,
    #[error("reassembly memory or unit limit exceeded")]
    Capacity,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MediaHeader {
    pub kind: u8,
    pub flags: u16,
    pub session: [u8; 16],
    pub epoch: u32,
    pub config: u32,
    pub sequence: u64,
    pub pts: u64,
    pub duration: u32,
    pub unit_len: u32,
    pub index: u16,
    pub count: u16,
    pub offset: u32,
}
impl MediaHeader {
    pub fn parse(b: &[u8]) -> Result<Self, WireError> {
        Self::parse_with_payload(b, b.len().saturating_sub(HEADER))
    }
    pub fn parse_with_payload(b: &[u8], payload: usize) -> Result<Self, WireError> {
        if b.len() < HEADER || &b[..4] != b"TCAM" || b[4] != 1 {
            return Err(WireError::Header);
        }
        let u16at = |i| u16::from_be_bytes([b[i], b[i + 1]]);
        let u32at = |i| u32::from_be_bytes(b[i..i + 4].try_into().expect("header bounds"));
        let u64at = |i| u64::from_be_bytes(b[i..i + 8].try_into().expect("header bounds"));
        let h = Self {
            kind: b[5],
            flags: u16at(6),
            session: b[8..24].try_into().expect("header bounds"),
            epoch: u32at(24),
            config: u32at(28),
            sequence: u64at(32),
            pts: u64at(40),
            duration: u32at(48),
            unit_len: u32at(52),
            index: u16at(56),
            count: u16at(58),
            offset: u32at(60),
        };
        h.validate(payload)?;
        Ok(h)
    }
    pub fn validate(&self, payload: usize) -> Result<(), WireError> {
        let max = match self.kind {
            1 => VIDEO_LIMIT,
            2 | 3 => AUDIO_LIMIT,
            _ => return Err(WireError::Bounds),
        };
        if self.flags & !3 != 0
            || self.unit_len == 0
            || self.unit_len as usize > max
            || self.count == 0
            || self.count > 16384
            || self.index >= self.count
            || payload == 0
            || (self.offset as usize)
                .checked_add(payload)
                .is_none_or(|n| n > self.unit_len as usize)
            || self.duration == 0
        {
            return Err(WireError::Bounds);
        }
        Ok(())
    }
    pub fn encode(&self) -> [u8; HEADER] {
        let mut b = [0; HEADER];
        b[..4].copy_from_slice(b"TCAM");
        b[4] = 1;
        b[5] = self.kind;
        b[6..8].copy_from_slice(&self.flags.to_be_bytes());
        b[8..24].copy_from_slice(&self.session);
        for (i, n) in [
            (24, self.epoch),
            (28, self.config),
            (48, self.duration),
            (52, self.unit_len),
            (60, self.offset),
        ] {
            b[i..i + 4].copy_from_slice(&n.to_be_bytes());
        }
        for (i, n) in [(32, self.sequence), (40, self.pts)] {
            b[i..i + 8].copy_from_slice(&n.to_be_bytes());
        }
        b[56..58].copy_from_slice(&self.index.to_be_bytes());
        b[58..60].copy_from_slice(&self.count.to_be_bytes());
        b
    }
    pub fn independent(&self) -> bool {
        self.kind == 1 && self.flags & 1 != 0
    }
}
#[derive(Debug, Clone)]
pub struct Unit {
    pub header: MediaHeader,
    pub data: Vec<u8>,
}
struct PartialUnit {
    header: MediaHeader,
    parts: BTreeMap<u16, (u32, Vec<u8>)>,
    created: Instant,
}
pub struct Reassembler {
    session: [u8; 16],
    epoch: u32,
    config: u32,
    units: BTreeMap<(u8, u64), PartialUnit>,
    reserved: usize,
    ttl: Duration,
    pub expired: u64,
}
impl Reassembler {
    pub fn new(session: [u8; 16], epoch: u32, config: u32, ttl: Duration) -> Self {
        Self {
            session,
            epoch,
            config,
            units: BTreeMap::new(),
            reserved: 0,
            ttl,
            expired: 0,
        }
    }
    pub fn expire(&mut self, now: Instant) -> bool {
        let mut video = false;
        self.units.retain(|(kind, _), u| {
            let keep = now.saturating_duration_since(u.created) < self.ttl;
            if !keep {
                self.reserved -= u.header.unit_len as usize;
                self.expired += 1;
                video |= *kind == 1;
            }
            keep
        });
        video
    }
    /// At most one bounded repair request is emitted per partial unit per call.
    pub fn missing(&self, now: Instant) -> Vec<(MediaHeader, Vec<u16>, u32)> {
        self.units
            .values()
            .filter_map(|unit| {
                let age = now.saturating_duration_since(unit.created);
                if unit.header.kind != 1
                    || age < Duration::from_millis(5)
                    || age + Duration::from_millis(15) >= self.ttl
                {
                    return None;
                }
                let indices: Vec<u16> = (0..unit.header.count)
                    .filter(|i| !unit.parts.contains_key(i))
                    .take(64)
                    .collect();
                if indices.is_empty() {
                    None
                } else {
                    Some((unit.header, indices, (self.ttl - age).as_millis() as u32))
                }
            })
            .take(4)
            .collect()
    }
    pub fn push(&mut self, b: &[u8], now: Instant) -> Result<Option<Unit>, WireError> {
        let h = MediaHeader::parse(b)?;
        if h.session != self.session || h.epoch != self.epoch || h.config != self.config {
            return Err(WireError::Stale);
        }
        let key = (h.kind, h.sequence);
        if !self.units.contains_key(&key) {
            if self.units.len() >= 4 || self.reserved + h.unit_len as usize > REASSEMBLY_LIMIT {
                return Err(WireError::Capacity);
            }
            self.reserved += h.unit_len as usize;
            self.units.insert(
                key,
                PartialUnit {
                    header: h,
                    parts: BTreeMap::new(),
                    created: now,
                },
            );
        }
        let u = self.units.get_mut(&key).expect("inserted");
        let refh = u.header;
        if h.kind != refh.kind
            || h.flags != refh.flags
            || h.pts != refh.pts
            || h.duration != refh.duration
            || h.unit_len != refh.unit_len
            || h.count != refh.count
        {
            return Err(WireError::Conflict);
        }
        let payload = &b[HEADER..];
        if let Some((offset, data)) = u.parts.get(&h.index) {
            if *offset != h.offset || data.as_slice() != payload {
                return Err(WireError::Conflict);
            }
            return Ok(None);
        }
        let end = h.offset as usize + payload.len();
        for (offset, data) in u.parts.values() {
            if (h.offset as usize) < *offset as usize + data.len() && (*offset as usize) < end {
                return Err(WireError::Conflict);
            }
        }
        u.parts.insert(h.index, (h.offset, payload.to_vec()));
        if u.parts.len() != h.count as usize {
            return Ok(None);
        }
        let u = self.units.remove(&key).expect("complete");
        self.reserved -= h.unit_len as usize;
        let mut parts: Vec<_> = u.parts.into_values().collect();
        parts.sort_by_key(|p| p.0);
        let mut data = Vec::with_capacity(h.unit_len as usize);
        for (offset, part) in parts {
            if offset as usize != data.len() {
                return Err(WireError::Conflict);
            }
            data.extend(part)
        }
        if data.len() != h.unit_len as usize {
            return Err(WireError::Conflict);
        }
        Ok(Some(Unit {
            header: u.header,
            data,
        }))
    }
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Control {
    pub version: u8,
    #[serde(rename = "type")]
    pub kind: String,
    pub request_id: String,
    pub session_id: String,
    pub transport_epoch: u32,
    pub body: serde_json::Value,
}
impl Control {
    pub fn new(kind: &str, session: &str, body: serde_json::Value) -> Self {
        Self {
            version: 1,
            kind: kind.into(),
            request_id: "0".into(),
            session_id: session.into(),
            transport_epoch: 1,
            body,
        }
    }
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StreamConfig {
    pub config_id: u32,
    pub profile: String,
    pub width: u32,
    pub height: u32,
    pub fps: u32,
    pub bitrate: u32,
    pub codec: String,
    pub audio_codec: String,
    pub audio_channels: u32,
    pub audio_packet_ms: u32,
    pub playout_ms: u32,
    #[serde(default)]
    pub camera_id: Option<String>,
    #[serde(default)]
    pub audio_input_id: Option<String>,
    #[serde(default)]
    pub audio_data_source: Option<u32>,
    #[serde(default)]
    pub fallback_reason: Option<String>,
}
impl StreamConfig {
    pub fn profile(name: &str, usb: bool) -> Self {
        let (w, h, f, b, a, p) = match name {
            "saver" => (1280, 720, 30, 4_000_000, 20, 70),
            "maximum" => (3840, 2160, 60, 65_000_000, 10, 65),
            _ => (1920, 1080, 60, 14_000_000, 10, 60),
        };
        Self {
            config_id: 1,
            profile: name.into(),
            width: w,
            height: h,
            fps: f,
            bitrate: b,
            codec: "h264".into(),
            audio_codec: if usb { "pcm" } else { "opus" }.into(),
            audio_channels: 1,
            audio_packet_ms: if usb { 5 } else { a },
            playout_ms: if usb { 35 } else { p },
            camera_id: None,
            audio_input_id: None,
            audio_data_source: None,
            fallback_reason: None,
        }
    }
    pub fn validate(&self) -> bool {
        self.config_id > 0
            && ["saver", "balanced", "maximum"].contains(&self.profile.as_str())
            && self.width > 0
            && self.width <= 3840
            && self.height > 0
            && self.height <= 2160
            && (1..=60).contains(&self.fps)
            && (100_000..=100_000_000).contains(&self.bitrate)
            && ["h264", "hevc"].contains(&self.codec.as_str())
            && ["pcm", "opus"].contains(&self.audio_codec.as_str())
            && (1..=2).contains(&self.audio_channels)
            && [5, 10, 20].contains(&self.audio_packet_ms)
            && self.playout_ms <= 100
            && self
                .camera_id
                .as_ref()
                .is_none_or(|s| !s.is_empty() && s.len() <= 256)
            && self
                .audio_input_id
                .as_ref()
                .is_none_or(|s| !s.is_empty() && s.len() <= 256)
            && self.fallback_reason.as_ref().is_none_or(|s| s.len() <= 512)
    }
}
pub fn media_binding(session: [u8; 16], epoch: u32, token: [u8; 32]) -> Vec<u8> {
    let mut b = b"TCMB\x01".to_vec();
    b.extend(session);
    b.extend(epoch.to_be_bytes());
    b.extend(token);
    b
}
pub fn check_binding(b: &[u8], session: [u8; 16], epoch: u32, token: [u8; 32]) -> bool {
    b == media_binding(session, epoch, token)
}
#[cfg(test)]
mod tests {
    #[test]
    fn repair_requests_are_bounded_by_age_and_missing_fragment_count() {
        let header = MediaHeader {
            kind: 1,
            flags: 1,
            session: [1; 16],
            epoch: 1,
            config: 1,
            sequence: 0,
            pts: 0,
            duration: 16666666,
            unit_len: 200,
            index: 0,
            count: 200,
            offset: 0,
        };
        let mut packet = header.encode().to_vec();
        packet.push(1);
        let now = Instant::now();
        let mut reassembler = Reassembler::new([1; 16], 1, 1, Duration::from_millis(60));
        reassembler.push(&packet, now).unwrap();
        assert!(reassembler.missing(now).is_empty());
        let missing = reassembler.missing(now + Duration::from_millis(6));
        assert_eq!(missing[0].1.len(), 64);
        assert_eq!(missing[0].1[0], 1);
        assert!(
            reassembler
                .missing(now + Duration::from_millis(50))
                .is_empty()
        );
        assert!(reassembler.expire(now + Duration::from_millis(61)));
        assert!(
            reassembler
                .missing(now + Duration::from_millis(61))
                .is_empty()
        );
    }

    use super::*;
    fn packet(sequence: u64, index: u16, offset: u32, data: &[u8]) -> Vec<u8> {
        let h = MediaHeader {
            kind: 1,
            flags: 1,
            session: [7; 16],
            epoch: 1,
            config: 1,
            sequence,
            pts: 123,
            duration: 16_666_667,
            unit_len: 6,
            index,
            count: 2,
            offset,
        };
        let mut b = h.encode().to_vec();
        b.extend(data);
        b
    }
    #[test]
    fn out_of_order_and_duplicate() {
        let mut r = Reassembler::new([7; 16], 1, 1, Duration::from_millis(20));
        let now = Instant::now();
        assert!(r.push(&packet(1, 1, 3, b"def"), now).unwrap().is_none());
        assert!(r.push(&packet(1, 1, 3, b"def"), now).unwrap().is_none());
        assert_eq!(
            r.push(&packet(1, 0, 0, b"abc"), now).unwrap().unwrap().data,
            b"abcdef"
        );
    }
    #[test]
    fn overlap_stale_and_expiry() {
        let mut r = Reassembler::new([7; 16], 1, 1, Duration::from_millis(20));
        let now = Instant::now();
        r.push(&packet(1, 0, 0, b"abc"), now).unwrap();
        assert!(matches!(
            r.push(&packet(1, 1, 2, b"def"), now),
            Err(WireError::Conflict)
        ));
        assert!(r.expire(now + Duration::from_millis(21)));
        let mut b = packet(2, 0, 0, b"abc");
        b[8] = 9;
        assert!(matches!(r.push(&b, now), Err(WireError::Stale)));
    }
    #[test]
    fn hostile_lengths_and_short_inputs() {
        for n in 0..64 {
            assert!(MediaHeader::parse(&vec![0; n]).is_err())
        }
        let mut b = packet(1, 0, 0, b"abc");
        b[52..56].copy_from_slice(&u32::MAX.to_be_bytes());
        assert!(matches!(MediaHeader::parse(&b), Err(WireError::Bounds)));
    }
    #[test]
    fn header_golden_fixture() {
        let fixture = include_bytes!("../../../protocol/media-header-v1.bin");
        let mut b = fixture.to_vec();
        b.extend(b"abc");
        let h = MediaHeader::parse(&b).unwrap();
        assert_eq!(h.encode().as_slice(), fixture);
        assert_eq!(h.pts, 123);
    }
}
