//! Small completion reorder window; it never hides a missing reference indefinitely.
use std::{
    collections::BTreeMap,
    time::{Duration, Instant},
};
use titan_protocol::Unit;
pub struct Reorder {
    pending: BTreeMap<u64, (Instant, Unit)>,
    next: Option<u64>,
    bytes: usize,
}
impl Reorder {
    pub fn new() -> Self {
        Self {
            pending: BTreeMap::new(),
            next: None,
            bytes: 0,
        }
    }
    pub fn push(&mut self, unit: Unit, now: Instant) -> Vec<Unit> {
        if self.next.is_some_and(|next| unit.header.sequence < next)
            || self.pending.contains_key(&unit.header.sequence)
        {
            return Vec::new();
        }
        if self.next.is_none() {
            if !unit.header.independent() {
                return vec![unit];
            }
            self.next = Some(unit.header.sequence);
        }
        self.bytes += unit.data.len();
        self.pending.insert(unit.header.sequence, (now, unit));
        self.drain(now)
    }
    pub fn drain(&mut self, now: Instant) -> Vec<Unit> {
        let mut output = Vec::new();
        while let Some((&first, &(created, _))) = self.pending.first_key_value() {
            let ready = Some(first) == self.next
                || now.saturating_duration_since(created) >= Duration::from_millis(12)
                || self.pending.len() >= 4
                || self.bytes > 16 * 1024 * 1024;
            if !ready {
                break;
            }
            let (_, unit) = self.pending.remove(&first).unwrap();
            self.bytes -= unit.data.len();
            self.next = first.checked_add(1);
            output.push(unit);
        }
        output
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    use titan_protocol::MediaHeader;
    fn unit(sequence: u64, independent: bool) -> Unit {
        Unit {
            header: MediaHeader {
                kind: 1,
                flags: u16::from(independent),
                session: [1; 16],
                epoch: 1,
                config: 1,
                sequence,
                pts: sequence * 16666666,
                duration: 16666666,
                unit_len: 1,
                index: 0,
                count: 1,
                offset: 0,
            },
            data: vec![1],
        }
    }
    #[test]
    fn completion_order_is_restored_but_missing_reference_expires() {
        let mut reorder = Reorder::new();
        let now = Instant::now();
        assert_eq!(reorder.push(unit(0, true), now).len(), 1);
        assert!(reorder.push(unit(2, false), now).is_empty());
        assert_eq!(
            reorder
                .push(unit(1, false), now)
                .iter()
                .map(|u| u.header.sequence)
                .collect::<Vec<_>>(),
            vec![1, 2]
        );
        assert!(reorder.push(unit(4, false), now).is_empty());
        assert_eq!(
            reorder.drain(now + Duration::from_millis(13))[0]
                .header
                .sequence,
            4
        );
        assert!(reorder.push(unit(3, false), now).is_empty());
    }
}
