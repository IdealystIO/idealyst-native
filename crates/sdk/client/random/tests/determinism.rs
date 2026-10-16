//! The generator and every derived operation, on the host.
#![cfg(not(target_arch = "wasm32"))]

mod golden;

use random::Rng;
use rand_core::{RngCore, SeedableRng};
use rand_xoshiro::Xoshiro256PlusPlus;

#[test]
fn matches_the_reference_xoshiro256plusplus_vectors() {
    assert_eq!(golden::first(Rng::from_state([1, 2, 3, 4])), golden::STATE_1234);
    assert_eq!(golden::first(Rng::seeded(0)), golden::SEEDED_0);
    assert_eq!(golden::first(Rng::seeded(42)), golden::SEEDED_42);
}

/// Differential: `Rng::seeded` is xoshiro256++ seeded through SplitMix64,
/// exactly what `rand_xoshiro`'s `seed_from_u64` does.
#[test]
fn matches_rand_xoshiro_stream_for_many_seeds() {
    for seed in [0, 1, 2, 42, 0xDEAD_BEEF, u64::MAX] {
        let mut ours = Rng::seeded(seed);
        let mut reference = Xoshiro256PlusPlus::seed_from_u64(seed);
        for i in 0..10_000 {
            assert_eq!(ours.next_u64(), reference.next_u64(), "seed {seed}, draw {i}");
        }
    }
}

#[test]
fn fill_bytes_is_little_endian_words_including_the_remainder() {
    let mut a = Rng::seeded(7);
    let mut b = Rng::seeded(7);
    let mut buf = [0u8; 13];
    a.fill_bytes(&mut buf);
    let w0 = b.next_u64().to_le_bytes();
    let w1 = b.next_u64().to_le_bytes();
    assert_eq!(&buf[..8], &w0);
    assert_eq!(&buf[8..], &w1[..5]);
}

#[test]
#[should_panic(expected = "all-zero state")]
fn the_all_zero_state_is_rejected() {
    Rng::from_state([0; 4]);
}

#[test]
fn integer_ranges_stay_in_bounds_and_hit_both_ends() {
    let mut rng = Rng::seeded(1);
    let (mut lo, mut hi) = (false, false);
    for _ in 0..10_000 {
        let v: i8 = rng.range(-128..=127);
        lo |= v == -128;
        hi |= v == 127;
        let d = rng.range(1..=6u32);
        assert!((1..=6).contains(&d));
        let h: i64 = rng.range(-5..5);
        assert!((-5..5).contains(&h));
    }
    assert!(lo && hi, "an inclusive range must reach both ends");
    assert_eq!(rng.range(9..=9u8), 9);
    // Whole-type ranges take the full-width path.
    let _: u64 = rng.range(0..=u64::MAX);
    let _: i32 = rng.range(i32::MIN..=i32::MAX);
}

#[test]
#[should_panic(expected = "empty range")]
fn an_empty_range_panics() {
    let _: u32 = Rng::seeded(0).range(5..5);
}

/// A non-power-of-two span comes out uniform: chi-square over 3 buckets
/// (2 degrees of freedom) stays below the 99.9% critical value, 13.8.
#[test]
fn small_ranges_are_uniform() {
    let mut rng = Rng::seeded(99);
    let n = 300_000;
    let mut counts = [0u64; 3];
    for _ in 0..n {
        counts[rng.range(0..3usize)] += 1;
    }
    let expected = n as f64 / 3.0;
    let chi2: f64 = counts.iter().map(|&c| (c as f64 - expected).powi(2) / expected).sum();
    assert!(chi2 < 13.8, "chi-square {chi2} for {counts:?}");
}

#[test]
fn floats_are_half_open() {
    let mut rng = Rng::seeded(3);
    for _ in 0..100_000 {
        let f = rng.float();
        assert!((0.0..1.0).contains(&f));
        let g = rng.range(-2.5..2.5f64);
        assert!((-2.5..2.5).contains(&g));
        let h = rng.range(0.0..1.0f32);
        assert!((0.0..1.0).contains(&h));
    }
}

#[test]
fn chance_respects_its_extremes() {
    let mut rng = Rng::seeded(4);
    assert!((0..1000).all(|_| !rng.chance(0.0)));
    assert!((0..1000).all(|_| rng.chance(1.0)));
}

#[test]
fn shuffle_permutes_and_choose_handles_empty() {
    let mut rng = Rng::seeded(5);
    let mut v: Vec<u32> = (0..100).collect();
    rng.shuffle(&mut v);
    assert_ne!(v, (0..100).collect::<Vec<_>>(), "a 100-element shuffle staying sorted is ~1/100!");
    let mut sorted = v.clone();
    sorted.sort();
    assert_eq!(sorted, (0..100).collect::<Vec<_>>());
    assert_eq!(rng.choose::<u8>(&[]), None);
    assert!([1, 2, 3].contains(rng.choose(&[1, 2, 3]).unwrap()));
}

/// `random::seed` makes the free functions reproducible.
#[test]
fn seed_makes_the_thread_generator_deterministic() {
    random::seed(42);
    let a = [random::u64(), random::u64(), random::u64(), random::u64(), random::u64()];
    assert_eq!(a, golden::SEEDED_42);
    random::seed(42);
    let rolls: Vec<u8> = (0..20).map(|_| random::range(1..=6)).collect();
    random::seed(42);
    let again: Vec<u8> = (0..20).map(|_| random::range(1..=6)).collect();
    assert_eq!(rolls, again);
}

#[test]
fn secure_bytes_are_filled_and_differ_between_calls() {
    let mut a = [0u8; 64];
    let mut b = [0u8; 64];
    random::secure_bytes(&mut a);
    random::secure_bytes(&mut b);
    assert_ne!(a, [0u8; 64]);
    assert_ne!(a, b);
    assert_ne!(random::secure_u64(), random::secure_u64());
    // Two entropy-seeded generators diverge.
    assert_ne!(Rng::from_entropy().next_u64(), Rng::from_entropy().next_u64());
}
