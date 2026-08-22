//! A bit-for-bit reimplementation of `java.util.Random`, plus the reversal
//! primitives the cracking modes are built on.
//!
//! Minecraft Java's worldgen is deterministic because it leans on this
//! generator: a 48-bit linear congruential generator
//!
//! ```text
//! state_{n+1} = (state_n * 0x5DEECE66D + 0xB) mod 2^48
//! ```
//!
//! The multiplier is odd, so it is invertible mod 2^48 and the sequence can be
//! walked backwards as cheaply as forwards. That is the whole basis of
//! [`JavaRandom::previous`], which modes 4, 9 and 10 depend on.
//!
//! Everything here is checked against a third-party implementation of the same
//! LCG (the `java_random` crate) in the test module, plus a handful of literal
//! values produced by a real JVM.

/// LCG multiplier used by `java.util.Random`.
pub const MULTIPLIER: u64 = 0x5DEECE66D;
/// LCG addend used by `java.util.Random`.
pub const ADDEND: u64 = 0xB;
/// The generator only ever holds 48 bits of state.
pub const MASK: u64 = (1 << 48) - 1;

/// Multiplicative inverse of `a` modulo `modulus`, by the extended Euclidean
/// algorithm.
///
/// Only valid when `gcd(a, modulus) == 1`; for our fixed inputs (an odd
/// multiplier and a power of two) that always holds.
const fn mod_inverse_egcd(a: u64, modulus: u128) -> u64 {
    let (mut old_r, mut r) = (a as i128, modulus as i128);
    let (mut old_s, mut s) = (1i128, 0i128);

    while r != 0 {
        let q = old_r / r;
        let next_r = old_r - q * r;
        old_r = r;
        r = next_r;
        let next_s = old_s - q * s;
        old_s = s;
        s = next_s;
    }

    // old_r is gcd(a, modulus), which is 1 for every input we use.
    let m = modulus as i128;
    (((old_s % m) + m) % m) as u64
}

/// `MULTIPLIER^-1 mod 2^48` — steps the LCG backwards.
pub const INV_MULTIPLIER: u64 = mod_inverse_egcd(MULTIPLIER, 1 << 48);

/// A `java.util.Random` clone.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct JavaRandom {
    /// The *scrambled* internal state, i.e. what Java stores in its `seed`
    /// field after `setSeed` has XORed the multiplier in.
    state: u64,
}

impl JavaRandom {
    /// Equivalent to `new Random(seed)`.
    #[inline]
    pub const fn new(seed: i64) -> Self {
        Self {
            state: ((seed as u64) ^ MULTIPLIER) & MASK,
        }
    }

    /// Wraps a raw scrambled state (what [`JavaRandom::state`] returns).
    #[inline]
    pub const fn from_state(state: u64) -> Self {
        Self { state: state & MASK }
    }

    /// Equivalent to `setSeed(seed)`.
    #[inline]
    pub fn set_seed(&mut self, seed: i64) {
        self.state = ((seed as u64) ^ MULTIPLIER) & MASK;
    }

    /// The raw scrambled internal state.
    #[inline]
    pub const fn state(&self) -> u64 {
        self.state
    }

    /// The unscrambled seed — the value you would have passed to
    /// `new Random(..)` to reach this state. Inverse of [`JavaRandom::new`].
    #[inline]
    pub const fn seed(&self) -> u64 {
        (self.state ^ MULTIPLIER) & MASK
    }

    /// Advances the state one step and returns it.
    #[inline]
    pub fn advance(&mut self) -> u64 {
        self.state = self
            .state
            .wrapping_mul(MULTIPLIER)
            .wrapping_add(ADDEND)
            & MASK;
        self.state
    }

    /// Rewinds the state one step and returns it. Exact inverse of
    /// [`JavaRandom::advance`].
    #[inline]
    pub fn previous(&mut self) -> u64 {
        self.state = self
            .state
            .wrapping_sub(ADDEND)
            .wrapping_mul(INV_MULTIPLIER)
            & MASK;
        self.state
    }

    /// Skips `n` steps forward (negative `n` rewinds).
    pub fn skip(&mut self, n: i64) {
        if n >= 0 {
            for _ in 0..n {
                self.advance();
            }
        } else {
            for _ in 0..(-n) {
                self.previous();
            }
        }
    }

    /// `protected int next(int bits)`.
    #[inline]
    pub fn next(&mut self, bits: u32) -> i32 {
        debug_assert!(bits > 0 && bits <= 32);
        (self.advance() >> (48 - bits)) as u32 as i32
    }

    /// `nextInt()`.
    #[inline]
    pub fn next_int(&mut self) -> i32 {
        self.next(32)
    }

    /// `nextInt(bound)`, including Java's power-of-two fast path and its
    /// rejection loop for other bounds.
    #[inline]
    pub fn next_int_bound(&mut self, bound: i32) -> i32 {
        assert!(bound > 0, "bound must be positive");

        if (bound & bound.wrapping_neg()) == bound {
            // Power of two: Java takes the high bits instead of a modulo.
            return (((bound as i64).wrapping_mul(self.next(31) as i64)) >> 31) as i32;
        }

        loop {
            let bits = self.next(31);
            let val = bits % bound;
            // Java's overflow-based rejection test, reproduced with wrapping
            // i32 arithmetic so the sign check matches.
            if bits
                .wrapping_sub(val)
                .wrapping_add(bound - 1)
                >= 0
            {
                return val;
            }
        }
    }

    /// `nextLong()`.
    #[inline]
    pub fn next_long(&mut self) -> i64 {
        let hi = (self.next(32) as i64) << 32;
        hi.wrapping_add(self.next(32) as i64)
    }

    /// `nextBoolean()`.
    #[inline]
    pub fn next_boolean(&mut self) -> bool {
        self.next(1) != 0
    }

    /// `nextFloat()`.
    #[inline]
    pub fn next_float(&mut self) -> f32 {
        self.next(24) as f32 / (1u32 << 24) as f32
    }

    /// `nextDouble()`.
    #[inline]
    pub fn next_double(&mut self) -> f64 {
        let hi = (self.next(26) as i64) << 27;
        let lo = self.next(27) as i64;
        (hi.wrapping_add(lo)) as f64 * (1.0 / (1u64 << 53) as f64)
    }
}

/// Java's `Collections.shuffle(list, rnd)` — a Fisher–Yates walking downwards,
/// swapping index `i-1` with `rnd.nextInt(i)`.
///
/// The index order matters: implementing Fisher–Yates the other way round
/// produces a different permutation for the same seed, which would silently
/// break the End-pillar shortcut in mode 9.
pub fn collections_shuffle<T>(list: &mut [T], rng: &mut JavaRandom) {
    let size = list.len();
    let mut i = size;
    while i > 1 {
        let j = rng.next_int_bound(i as i32) as usize;
        list.swap(i - 1, j);
        i -= 1;
    }
}

/// Java's `String.hashCode()`, needed for the namespaced-seed hashes that
/// 1.18+ worldgen mixes in (e.g. `minecraft:bedrock_floor`).
pub fn java_string_hash(s: &str) -> i32 {
    let mut h: i32 = 0;
    for c in s.encode_utf16() {
        h = h.wrapping_mul(31).wrapping_add(c as i32);
    }
    h
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn inverse_multiplier_is_correct() {
        assert_eq!(
            (MULTIPLIER.wrapping_mul(INV_MULTIPLIER)) & MASK,
            1,
            "extended Euclid did not produce a modular inverse"
        );
        // The value the technical Minecraft community quotes for this inverse.
        assert_eq!(INV_MULTIPLIER, 0xDFE05BCB1365);
    }

    #[test]
    fn next_previous_round_trip() {
        // The property the whole reversal story rests on: stepping forward and
        // then backward must be the identity, from any state.
        for seed in [0i64, 1, -1, 42, i64::MAX, i64::MIN, 765906787396911863] {
            let mut r = JavaRandom::new(seed);
            let start = r.state();
            for _ in 0..1000 {
                r.advance();
            }
            for _ in 0..1000 {
                r.previous();
            }
            assert_eq!(r.state(), start, "round trip failed for seed {seed}");
        }
    }

    #[test]
    fn previous_undoes_a_single_draw() {
        let mut r = JavaRandom::new(12345);
        let before = r.state();
        let _ = r.next(32);
        r.previous();
        assert_eq!(r.state(), before);
    }

    #[test]
    fn seed_round_trips_through_scrambling() {
        for seed in [0i64, 12345, -99999, i64::MAX] {
            let r = JavaRandom::new(seed);
            assert_eq!(r.seed(), (seed as u64) & MASK);
        }
    }

    #[test]
    fn matches_real_jvm_output() {
        // Literal values from a real JVM; these catch a wrong multiplier,
        // addend, shift or scrambling step immediately.
        let mut r = JavaRandom::new(0);
        assert_eq!(r.next_int(), -1155484576);
        assert_eq!(r.next_int(), -723955400);

        let mut r = JavaRandom::new(0);
        assert_eq!(r.next_long(), -4962768465676381896);

        let mut r = JavaRandom::new(0);
        assert!((r.next_double() - 0.730967787376657).abs() < 1e-15);

        let mut r = JavaRandom::new(42);
        assert_eq!(r.next_int(), -1170105035);

        let mut r = JavaRandom::new(0);
        assert!(r.next_boolean());
    }

    #[test]
    fn cross_check_against_independent_implementation() {
        // `java_random` is an unrelated crate implementing the same LCG.
        // Agreeing with it across all the draw kinds we use is a much stronger
        // check than any single hand-copied constant.
        for seed in [0i64, 1, -7, 42, 123456789, -987654321, i64::MAX, i64::MIN] {
            let mut ours = JavaRandom::new(seed);
            let mut theirs = java_random::Random::with_seed(seed as u64);

            for _ in 0..64 {
                assert_eq!(ours.next_int(), theirs.next_int(), "nextInt @ {seed}");
                assert_eq!(
                    ours.next_int_bound(10),
                    theirs.next_int_n(10),
                    "nextInt(10) @ {seed}"
                );
                assert_eq!(
                    ours.next_int_bound(16),
                    theirs.next_int_n(16),
                    "nextInt(16) power-of-two path @ {seed}"
                );
                assert_eq!(ours.next_long(), theirs.next_long(), "nextLong @ {seed}");
                assert_eq!(
                    ours.next_double().to_bits(),
                    theirs.next_double().to_bits(),
                    "nextDouble @ {seed}"
                );
                assert_eq!(
                    ours.next_float().to_bits(),
                    theirs.next_float().to_bits(),
                    "nextFloat @ {seed}"
                );
                assert_eq!(
                    ours.next_boolean(),
                    theirs.next_boolean(),
                    "nextBoolean @ {seed}"
                );
            }
        }
    }

    #[test]
    fn java_string_hash_matches_known_values() {
        // Verified against the constants baked into 19MisterX98's
        // Nether_Bedrock_Cracker (ROOF_HASH / FLOOR_HASH).
        assert_eq!(java_string_hash("minecraft:bedrock_roof") as i64, 343340730);
        assert_eq!(java_string_hash("minecraft:bedrock_floor") as i64, 2042456806);
        assert_eq!(java_string_hash(""), 0);
        assert_eq!(java_string_hash("a"), 97);
    }

    #[test]
    fn shuffle_is_deterministic_and_a_permutation() {
        let mut a: Vec<i32> = (0..10).collect();
        let mut b: Vec<i32> = (0..10).collect();
        collections_shuffle(&mut a, &mut JavaRandom::new(1234));
        collections_shuffle(&mut b, &mut JavaRandom::new(1234));
        assert_eq!(a, b);

        let mut sorted = a.clone();
        sorted.sort();
        assert_eq!(sorted, (0..10).collect::<Vec<_>>());
    }
}
