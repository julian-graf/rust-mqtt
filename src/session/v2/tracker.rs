use crate::{
    fmt::{assert, const_assert, debug_assert},
    types::PacketIdentifier,
};

use alloc::vec::Vec;
use core::cmp::min;
use heapless::LinearMap;
#[cfg(test)]
use std::collections::HashMap;

pub trait FlightTracker<T> {
    fn add_entry(&mut self, pid: PacketIdentifier, t: T);
    fn remove_entry(&mut self, pid: PacketIdentifier) -> Option<T>;
    fn clear(&mut self);

    fn get_state(&self, pid: PacketIdentifier) -> Option<T>;
    fn len(&self) -> u16;
    fn has_capacity_available(&self) -> bool {
        self.len() < u16::MAX
    }
}

#[cfg(test)]
impl<T: Copy> FlightTracker<T> for HashMap<PacketIdentifier, T> {
    fn add_entry(&mut self, pid: PacketIdentifier, t: T) {
        debug_assert!(self.get(&pid).is_none());
        debug_assert!(self.len() < usize::from(u16::MAX));

        let e = self.insert(pid, t);

        assert!(e.is_none());
    }
    fn remove_entry(&mut self, pid: PacketIdentifier) -> Option<T> {
        self.remove(&pid)
    }
    fn clear(&mut self) {
        self.clear();
    }

    fn get_state(&self, pid: PacketIdentifier) -> Option<T> {
        self.get(&pid).copied()
    }
    fn len(&self) -> u16 {
        debug_assert!(u16::try_from(self.len()).is_ok());

        self.len() as u16
    }
}

impl<T: Copy> FlightTracker<T> for Vec<(PacketIdentifier, T)> {
    fn add_entry(&mut self, pid: PacketIdentifier, t: T) {
        debug_assert!(self.iter().find(|f| f.0 == pid).is_none());
        debug_assert!(self.len() < usize::from(u16::MAX));

        self.push((pid, t));
    }
    fn clear(&mut self) {
        self.clear();
    }
    fn remove_entry(&mut self, pid: PacketIdentifier) -> Option<T> {
        self.iter()
            .enumerate()
            .find(|&(_, f)| f.0 == pid)
            .map(|(i, _)| i)
            .map(|i| self.swap_remove(i).1)
    }

    fn get_state(&self, pid: PacketIdentifier) -> Option<T> {
        self.iter().find_map(|f| (f.0 == pid).then_some(f.1))
    }
    fn len(&self) -> u16 {
        debug_assert!(u16::try_from(self.len()).is_ok());

        self.len() as u16
    }
}
impl<T: Copy, const N: usize> FlightTracker<T> for LinearMap<PacketIdentifier, T, N> {
    fn add_entry(&mut self, pid: PacketIdentifier, t: T) {
        let r = self.insert(pid, t);

        debug_assert!(r.is_ok_and(|o| o.is_none()));
    }
    fn remove_entry(&mut self, pid: PacketIdentifier) -> Option<T> {
        self.remove(&pid)
    }
    fn clear(&mut self) {
        self.clear();
    }

    fn get_state(&self, pid: PacketIdentifier) -> Option<T> {
        self.get(&pid).map(|f| *f)
    }
    fn len(&self) -> u16 {
        debug_assert!(u16::try_from(self.len()).is_ok());

        self.len() as u16
    }
    fn has_capacity_available(&self) -> bool {
        let capacity = min(self.capacity(), usize::from(u16::MAX));

        capacity > self.len()
    }
}

#[cfg(test)]
mod unit {
    use crate::session::tracker::FlightTracker;
    use crate::types::PacketIdentifier;
    use alloc::vec::Vec;
    use core::fmt::Debug;
    use core::num::NonZero;
    use heapless::LinearMap;

    fn axiom_empty_state<F: FlightTracker<T>, T: Copy + Debug>(tr: &F) {
        assert_eq!(tr.len(), 0);
        assert!(tr.has_capacity_available());
    }

    fn axiom_add_and_get<F: FlightTracker<T>, T: Copy + PartialEq + Debug>(
        tr: &mut F,
        pid: PacketIdentifier,
        value: T,
    ) {
        tr.add_entry(pid, value);
        assert_eq!(tr.len(), 1);
        assert_eq!(tr.get_state(pid), Some(value));
    }

    fn axiom_add_two_and_remove_one<F: FlightTracker<T>, T: Copy + PartialEq + Debug>(
        tr: &mut F,
        pid1: PacketIdentifier,
        v1: T,
        pid2: PacketIdentifier,
        v2: T,
    ) {
        tr.add_entry(pid1, v1);
        tr.add_entry(pid2, v2);
        assert_eq!(tr.len(), 2);
        assert_eq!(tr.get_state(pid1), Some(v1));
        assert_eq!(tr.get_state(pid2), Some(v2));

        let removed = tr.remove_entry(pid1);
        assert_eq!(removed, Some(v1));
        assert_eq!(tr.get_state(pid1), None);
        assert_eq!(tr.get_state(pid2), Some(v2));
        assert_eq!(tr.len(), 1);
    }

    fn axiom_remove_nonexistent<F: FlightTracker<T>, T: Copy + PartialEq + Debug>(
        tr: &mut F,
        pid_existing: PacketIdentifier,
        val: T,
        pid_missing: PacketIdentifier,
    ) {
        tr.add_entry(pid_existing, val);
        let before = tr.len();
        assert_eq!(tr.remove_entry(pid_missing), None);
        assert_eq!(tr.len(), before);
    }

    fn axiom_clear<F: FlightTracker<T>, T: Copy + PartialEq + Debug>(
        tr: &mut F,
        entries: &[(PacketIdentifier, T)],
    ) {
        for &(pid, v) in entries {
            tr.add_entry(pid, v);
        }
        assert_eq!(tr.len() as usize, entries.len());
        tr.clear();
        assert_eq!(tr.len(), 0);
        for &(pid, _) in entries {
            assert_eq!(tr.get_state(pid), None);
        }
    }

    fn axiom_get_non_mutating<F: FlightTracker<T>, T: Copy + PartialEq + Debug>(
        tr: &mut F,
        pid: PacketIdentifier,
        val: T,
    ) {
        tr.add_entry(pid, val);
        let len_before = tr.len();
        assert_eq!(tr.get_state(pid), Some(val));
        assert_eq!(tr.get_state(pid), Some(val));
        assert_eq!(tr.len(), len_before);
    }

    fn axiom_len_consistency<F: FlightTracker<T>, T: Copy + Debug>(
        tr: &mut F,
        entries: &[(PacketIdentifier, T)],
    ) {
        for (i, &(pid, v)) in entries.iter().enumerate() {
            tr.add_entry(pid, v);
            assert_eq!(tr.len(), (i + 1) as u16);
        }
    }

    fn axiom_capacity_behavior<F: FlightTracker<T>, T: Copy + Debug>(
        tr: &mut F,
        known_capacity: usize,
        fill_values: &[(PacketIdentifier, T)],
    ) {
        for (i, &(pid, v)) in fill_values.iter().enumerate() {
            tr.add_entry(pid, v);
            if i + 1 < known_capacity {
                assert!(tr.has_capacity_available());
            } else if i + 1 == known_capacity {
                assert!(!tr.has_capacity_available() || tr.has_capacity_available() == false);
            }
        }
    }

    #[test]
    fn test_linear_map_flight_tracker() {
        let mut vec_tracker = LinearMap::<PacketIdentifier, u8, 1>::new();
        let p1 = PacketIdentifier::ONE;
        let p2 = PacketIdentifier::new(NonZero::new(2).unwrap());

        axiom_empty_state(&vec_tracker);
        axiom_add_and_get(&mut vec_tracker, p1, 10u8);

        let mut v2 = LinearMap::<PacketIdentifier, u8, 1>::new();
        axiom_add_two_and_remove_one(&mut v2, p1, 10u8, p2, 20u8);
    }
}
