//! [`Rng`]: xoshiro256++, seeded through SplitMix64.
//!
//! Everything here is plain `u64` arithmetic, so a given seed produces the
//! same sequence — and the same ranges, floats and shuffles — on every
//! target, including 32-bit wasm. Derived operations draw only through
//! [`Rng::next_u64`] and depend only on the requested span, never on the
//! platform's `usize` width.

use std::ops::{Range, RangeInclusive};

/// A fast, seedable pseudo-random generator (xoshiro256++).
///
/// **Not for secrets.** Its output is predictable from earlier output; use
/// [`crate::secure_bytes`] / [`crate::secure_u64`] for tokens, ids and keys.
///
/// ```
/// let mut a = random::Rng::seeded(7);
/// let mut b = random::Rng::seeded(7);
/// assert_eq!(a.range(1..=6), b.range(1..=6)); // same seed, same rolls — everywhere
/// ```
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Rng {
    s: [u64; 4],
}

/// One SplitMix64 step: the standard way to expand a single `u64` seed into
/// xoshiro state (it never yields an all-zero state from distinct outputs
/// in practice, and decorrelates nearby seeds).
fn splitmix64(state: &mut u64) -> u64 {
    *state = state.wrapping_add(0x9E37_79B9_7F4A_7C15);
    let mut z = *state;
    z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
    z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
    z ^ (z >> 31)
}

impl Rng {
    /// A generator whose sequence is fully determined by `seed` — the same
    /// on every platform. Use for tests, replays and reproducible content.
    pub fn seeded(seed: u64) -> Self {
        let mut sm = seed;
        let s = [splitmix64(&mut sm), splitmix64(&mut sm), splitmix64(&mut sm), splitmix64(&mut sm)];
        Self::from_state(s)
    }

    /// A generator seeded from the OS entropy source.
    pub fn from_entropy() -> Self {
        loop {
            let mut bytes = [0u8; 32];
            crate::secure_bytes(&mut bytes);
            let mut s = [0u64; 4];
            for (word, chunk) in s.iter_mut().zip(bytes.chunks_exact(8)) {
                *word = u64::from_le_bytes(chunk.try_into().unwrap());
            }
            // xoshiro's one forbidden state (it would emit zeros forever);
            // a 2^-256 draw, but the loop costs nothing.
            if s != [0; 4] {
                return Self { s };
            }
        }
    }

    /// Build from raw xoshiro256++ state. Panics on the all-zero state,
    /// which would emit zeros forever.
    pub fn from_state(s: [u64; 4]) -> Self {
        assert!(s != [0; 4], "random::Rng: the all-zero state is invalid for xoshiro256++");
        Self { s }
    }

    /// The next 64 random bits.
    pub fn next_u64(&mut self) -> u64 {
        let s = &mut self.s;
        let result = s[0].wrapping_add(s[3]).rotate_left(23).wrapping_add(s[0]);
        let t = s[1] << 17;
        s[2] ^= s[0];
        s[3] ^= s[1];
        s[1] ^= s[2];
        s[0] ^= s[3];
        s[2] ^= t;
        s[3] = s[3].rotate_left(45);
        result
    }

    /// The next 32 random bits (the high half of [`Rng::next_u64`], which
    /// are xoshiro's strongest bits).
    pub fn next_u32(&mut self) -> u32 {
        (self.next_u64() >> 32) as u32
    }

    /// Fill `buf` with random bytes (little-endian words of
    /// [`Rng::next_u64`]).
    pub fn fill_bytes(&mut self, buf: &mut [u8]) {
        let mut chunks = buf.chunks_exact_mut(8);
        for chunk in &mut chunks {
            chunk.copy_from_slice(&self.next_u64().to_le_bytes());
        }
        let rest = chunks.into_remainder();
        if !rest.is_empty() {
            let word = self.next_u64().to_le_bytes();
            rest.copy_from_slice(&word[..rest.len()]);
        }
    }

    /// A uniform `f64` in `[0, 1)`, with 53 bits of precision.
    pub fn float(&mut self) -> f64 {
        (self.next_u64() >> 11) as f64 * (1.0 / (1u64 << 53) as f64)
    }

    /// A fair coin flip.
    pub fn bool(&mut self) -> bool {
        self.next_u64() >> 63 == 1
    }

    /// `true` with probability `p` (clamped to `[0, 1]`).
    pub fn chance(&mut self, p: f64) -> bool {
        self.float() < p
    }

    /// A uniform value in `range` — `a..b` or `a..=b`, for any integer type
    /// or `f32`/`f64` (half-open only for floats). Integer ranges are
    /// unbiased. Panics on an empty range.
    pub fn range<T, R: SampleRange<T>>(&mut self, range: R) -> T {
        range.sample(self)
    }

    /// Shuffle `items` in place (Fisher–Yates); every permutation is
    /// equally likely.
    pub fn shuffle<T>(&mut self, items: &mut [T]) {
        for i in (1..items.len()).rev() {
            let j = self.below(i as u64 + 1) as usize;
            items.swap(i, j);
        }
    }

    /// A uniformly chosen element, or `None` for an empty slice.
    pub fn choose<'a, T>(&mut self, items: &'a [T]) -> Option<&'a T> {
        if items.is_empty() {
            None
        } else {
            Some(&items[self.below(items.len() as u64) as usize])
        }
    }

    /// Uniform in `[0, n)` without modulo bias (Lemire's multiply-shift
    /// with rejection). `n` must be non-zero.
    pub(crate) fn below(&mut self, n: u64) -> u64 {
        debug_assert!(n != 0);
        let mut m = self.next_u64() as u128 * n as u128;
        if (m as u64) < n {
            // Values below `2^64 mod n` would be over-represented; redraw.
            let threshold = n.wrapping_neg() % n;
            while (m as u64) < threshold {
                m = self.next_u64() as u128 * n as u128;
            }
        }
        (m >> 64) as u64
    }
}

/// A range [`Rng::range`] can sample from. Implemented for `Range` and
/// `RangeInclusive` of every primitive integer type up to 64 bits, and for
/// `Range<f32>` / `Range<f64>`.
pub trait SampleRange<T> {
    /// Draw one uniform value from the range. Panics if it is empty.
    fn sample(self, rng: &mut Rng) -> T;
}

macro_rules! int_ranges {
    ($($t:ty => $u:ty),* $(,)?) => {$(
        impl SampleRange<$t> for Range<$t> {
            fn sample(self, rng: &mut Rng) -> $t {
                assert!(self.start < self.end, "random: empty range {:?}", self);
                // The span as the same-width unsigned type; wrapping makes
                // signed spans (e.g. -128..127) come out right.
                let span = (self.end as $u).wrapping_sub(self.start as $u) as u64;
                (self.start as $u).wrapping_add(rng.below(span) as $u) as $t
            }
        }
        impl SampleRange<$t> for RangeInclusive<$t> {
            fn sample(self, rng: &mut Rng) -> $t {
                let (start, end) = self.into_inner();
                assert!(start <= end, "random: empty range {start:?}..={end:?}");
                let span = (end as $u).wrapping_sub(start as $u);
                if span == <$u>::MAX {
                    // The whole type: every bit pattern is valid.
                    return rng.next_u64() as $u as $t;
                }
                (start as $u).wrapping_add(rng.below(span as u64 + 1) as $u) as $t
            }
        }
    )*};
}

int_ranges!(
    u8 => u8, u16 => u16, u32 => u32, u64 => u64, usize => usize,
    i8 => u8, i16 => u16, i32 => u32, i64 => u64, isize => usize,
);

impl SampleRange<f64> for Range<f64> {
    fn sample(self, rng: &mut Rng) -> f64 {
        assert!(
            self.start < self.end && self.start.is_finite() && self.end.is_finite(),
            "random: empty or non-finite range {:?}",
            self
        );
        loop {
            let v = self.start + (self.end - self.start) * rng.float();
            // Rounding can land exactly on `end`; redraw to keep it half-open.
            if v < self.end {
                return v;
            }
        }
    }
}

impl SampleRange<f32> for Range<f32> {
    fn sample(self, rng: &mut Rng) -> f32 {
        assert!(
            self.start < self.end && self.start.is_finite() && self.end.is_finite(),
            "random: empty or non-finite range {:?}",
            self
        );
        loop {
            let unit = (rng.next_u32() >> 8) as f32 * (1.0 / (1u32 << 24) as f32);
            let v = self.start + (self.end - self.start) * unit;
            if v < self.end {
                return v;
            }
        }
    }
}
