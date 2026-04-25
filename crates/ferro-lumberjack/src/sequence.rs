// SPDX-License-Identifier: Apache-2.0
//! Wrapping `u32` sequence-number arithmetic for Lumberjack ACKs.
//!
//! Lumberjack sequence numbers are `u32` and wrap modulo `2^32`. A naive
//! signed comparison (`acked >= expected`) is **wrong** for long-running
//! connections that emit more than `2^32` events between reconnects: the
//! ACK seq wraps around and a correct ACK gets rejected as "stale".
//!
//! [RFC 1982] describes the standard solution — "serial number arithmetic"
//! — and that is what this module implements.
//!
//! [RFC 1982]: https://www.rfc-editor.org/rfc/rfc1982

/// A monotonic sequence number with wrapping `u32` arithmetic.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct Sequence(u32);

impl Sequence {
    /// Construct a sequence number from a raw `u32`.
    #[must_use]
    pub const fn new(value: u32) -> Self {
        Self(value)
    }

    /// Underlying `u32` value.
    #[must_use]
    pub const fn value(self) -> u32 {
        self.0
    }

    /// Advance the sequence number by `n`, wrapping at `u32::MAX`.
    #[must_use]
    pub const fn advance(self, n: u32) -> Self {
        Self(self.0.wrapping_add(n))
    }

    /// Returns true if `acked` is exactly equal to this sequence under
    /// wrapping arithmetic. Equivalent to `acked == self.value()`, but
    /// the explicit form documents intent.
    #[must_use]
    pub const fn is_exactly_acked_by(self, acked: u32) -> bool {
        acked.wrapping_sub(self.0) == 0
    }

    /// Returns true if `acked` is *at least* this sequence — i.e. the
    /// receiver has acknowledged this sequence or any newer one — under
    /// wrapping arithmetic per [RFC 1982].
    ///
    /// "At least" is interpreted on the half-circle: `acked` is at least
    /// `self` iff `(acked - self) mod 2^32 < 2^31`. This means the
    /// comparison is well-defined as long as the two values are within
    /// `2^31` of each other on the wire — far larger than any plausible
    /// in-flight window.
    ///
    /// [RFC 1982]: https://www.rfc-editor.org/rfc/rfc1982
    #[must_use]
    pub const fn is_at_least_acked_by(self, acked: u32) -> bool {
        // If acked - self underflows past 2^31, acked is "behind" us.
        acked.wrapping_sub(self.0) < 0x8000_0000
    }
}

impl From<u32> for Sequence {
    fn from(value: u32) -> Self {
        Self::new(value)
    }
}

impl From<Sequence> for u32 {
    fn from(seq: Sequence) -> Self {
        seq.value()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn exact_match() {
        let s = Sequence::new(100);
        assert!(s.is_exactly_acked_by(100));
        assert!(!s.is_exactly_acked_by(99));
        assert!(!s.is_exactly_acked_by(101));
    }

    #[test]
    fn exact_match_wraps_around() {
        let s = Sequence::new(u32::MAX);
        assert!(s.is_exactly_acked_by(u32::MAX));
        // Not acked by 0 — that would be the *next* sequence.
        assert!(!s.is_exactly_acked_by(0));
    }

    #[test]
    fn advance_wraps() {
        let s = Sequence::new(u32::MAX);
        assert_eq!(s.advance(1).value(), 0);
        assert_eq!(s.advance(2).value(), 1);
    }

    #[test]
    fn at_least_basic() {
        let s = Sequence::new(100);
        assert!(s.is_at_least_acked_by(100));
        assert!(s.is_at_least_acked_by(101));
        assert!(s.is_at_least_acked_by(200));
        assert!(!s.is_at_least_acked_by(99));
        assert!(!s.is_at_least_acked_by(0));
    }

    #[test]
    fn at_least_across_wrap() {
        // Sender sent seq u32::MAX; receiver acks with seq 5 (after wrap).
        // The ACK is "ahead" by 6 → at least.
        let s = Sequence::new(u32::MAX);
        assert!(s.is_at_least_acked_by(0));
        assert!(s.is_at_least_acked_by(5));
        // 2^31 - 1 ahead is still "ahead".
        assert!(s.is_at_least_acked_by(0x7FFF_FFFE));
        // 2^31 ahead is the equidistant midpoint — by the RFC 1982 rule,
        // values exactly 2^31 away are unordered, and we treat them as
        // "behind" (strict <).
        assert!(!s.is_at_least_acked_by(0x7FFF_FFFF));
    }

    #[test]
    fn round_trip_u32() {
        let s = Sequence::from(42_u32);
        let v: u32 = s.into();
        assert_eq!(v, 42);
    }
}
