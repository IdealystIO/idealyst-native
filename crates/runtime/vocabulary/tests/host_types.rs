//! `Key`: a key's bytes order as the key does, and decode back to it —
//! for every impl and for `#[derive(Key)]`. This is the contract generic
//! host functions rest on: the app sorts a bundle's keys as bytes
//! (`KeyBytes`), and the answer must be the one the bundle's own `Ord`
//! gives.
//!
//! Property tests over a deterministic generator. Values are drawn from
//! small domains (short strings over an alphabet with `\0` and a multibyte
//! char, small numbers around zero and the extremes) so pairs often share
//! prefixes, tie, or differ only past an escape — where a wrong encoding
//! shows.

use std::cmp::{Ordering, Reverse};
use std::collections::hash_map::DefaultHasher;
use std::fmt::Debug;
use std::hash::{Hash, Hasher};

use runtime_macros::Key;
use runtime_vocabulary::host_types::{Key, KeyBytes, Numeric};

struct Rng(u64);
impl Rng {
    fn next(&mut self) -> u64 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        self.0
    }
    fn below(&mut self, n: u64) -> u64 {
        self.next() % n
    }
}

trait Gen: Sized {
    fn gen(r: &mut Rng) -> Self;
}

macro_rules! gen_int {
    ($($t:ty),*) => {$(
        impl Gen for $t {
            fn gen(r: &mut Rng) -> Self {
                match r.below(4) {
                    0 => <$t>::MIN,
                    1 => <$t>::MAX,
                    2 => (r.below(5) as i64 - 2) as $t,
                    _ => r.next() as $t,
                }
            }
        }
    )*};
}
gen_int!(u8, u16, u32, u64, u128, i8, i16, i32, i64, i128, usize, isize);

impl Gen for () {
    fn gen(_: &mut Rng) -> Self {}
}
impl Gen for bool {
    fn gen(r: &mut Rng) -> Self {
        r.below(2) == 1
    }
}
impl Gen for char {
    fn gen(r: &mut Rng) -> Self {
        ['\0', 'a', 'b', 'é', '\u{10FFFF}'][r.below(5) as usize]
    }
}
impl Gen for String {
    fn gen(r: &mut Rng) -> Self {
        (0..r.below(4)).map(|_| char::gen(r)).collect()
    }
}
impl Gen for f64 {
    fn gen(r: &mut Rng) -> Self {
        [0.0, -0.0, 1.5, -1.5, f64::INFINITY, f64::NEG_INFINITY, f64::NAN, -f64::NAN, f64::MIN_POSITIVE, 2.0]
            [r.below(10) as usize]
    }
}
impl Gen for f32 {
    fn gen(r: &mut Rng) -> Self {
        f64::gen(r) as f32
    }
}
impl<T: Gen> Gen for Vec<T> {
    fn gen(r: &mut Rng) -> Self {
        (0..r.below(4)).map(|_| T::gen(r)).collect()
    }
}
impl<T: Gen> Gen for Option<T> {
    fn gen(r: &mut Rng) -> Self {
        (r.below(3) != 0).then(|| T::gen(r))
    }
}
impl<T: Gen> Gen for Box<T> {
    fn gen(r: &mut Rng) -> Self {
        Box::new(T::gen(r))
    }
}
impl<T: Gen> Gen for Reverse<T> {
    fn gen(r: &mut Rng) -> Self {
        Reverse(T::gen(r))
    }
}
impl<T: Gen, const N: usize> Gen for [T; N] {
    fn gen(r: &mut Rng) -> Self {
        std::array::from_fn(|_| T::gen(r))
    }
}
impl<A: Gen, B: Gen> Gen for (A, B) {
    fn gen(r: &mut Rng) -> Self {
        (A::gen(r), B::gen(r))
    }
}
impl<A: Gen, B: Gen, C: Gen> Gen for (A, B, C) {
    fn gen(r: &mut Rng) -> Self {
        (A::gen(r), B::gen(r), C::gen(r))
    }
}

fn bytes<K: Key>(k: &K) -> Vec<u8> {
    KeyBytes::of(k).as_bytes().to_vec()
}

fn hash<T: Hash>(t: &T) -> u64 {
    let mut h = DefaultHasher::new();
    t.hash(&mut h);
    h.finish()
}

/// For `n` random pairs: byte order == `Ord`, equal bytes == `Eq`, equal
/// values hash alike, and each value decodes back from its bytes, using
/// them all.
fn check<K: Key + Gen + Debug>(seed: u64) {
    let mut r = Rng(seed);
    for _ in 0..4000 {
        let (a, b) = (K::gen(&mut r), K::gen(&mut r));
        let (ea, eb) = (bytes(&a), bytes(&b));
        assert_eq!(a.cmp(&b), ea.cmp(&eb), "{a:?} vs {b:?}: bytes {ea:02x?} vs {eb:02x?}");
        assert_eq!(a == b, ea == eb, "{a:?} vs {b:?}");
        if a == b {
            assert_eq!(hash(&a), hash(&b), "{a:?} == {b:?} but hash differently");
        }
        let back: K = KeyBytes::of(&a).decode().unwrap_or_else(|| panic!("{a:?} doesn't decode from {ea:02x?}"));
        assert_eq!(back.cmp(&a), Ordering::Equal, "{a:?} decoded as {back:?}");
    }
}

#[test]
fn integers_order_as_their_bytes() {
    check::<u8>(1);
    check::<u16>(2);
    check::<u32>(3);
    check::<u64>(4);
    check::<u128>(5);
    check::<i8>(6);
    check::<i16>(7);
    check::<i32>(8);
    check::<i64>(9);
    check::<i128>(10);
    check::<usize>(11);
    check::<isize>(12);
}

#[test]
fn bool_char_and_strings_order_as_their_bytes() {
    check::<bool>(20);
    check::<char>(21);
    check::<String>(22);
    // A string then another field: the terminator keeps "a" + "b" apart
    // from "ab" + "".
    check::<(String, String)>(23);
}

#[test]
fn containers_order_as_their_bytes() {
    check::<Vec<u8>>(30);
    check::<Vec<String>>(31);
    check::<Vec<Vec<i16>>>(32);
    check::<(Vec<u8>, u8)>(33);
    check::<Option<i32>>(34);
    check::<Option<Option<String>>>(35);
    check::<Box<i64>>(36);
    check::<[i8; 3]>(37);
    check::<(bool, i32, String)>(38);
    check::<()>(39);
}

#[test]
fn reverse_sorts_descending() {
    check::<Reverse<u32>>(40);
    check::<Reverse<String>>(41);
    check::<(Reverse<String>, u8)>(42);
    check::<Vec<Reverse<Vec<u8>>>>(43);
    assert!(bytes(&Reverse(2u8)) < bytes(&Reverse(1u8)));
}

// --- #[derive(Key)] ---------------------------------------------------------

#[derive(Clone, Debug, Key)]
struct Employee {
    dept: u16,
    name: String,
    salary: f64,
    tags: Vec<i8>,
    manager: Option<u32>,
}

impl Gen for Employee {
    fn gen(r: &mut Rng) -> Self {
        Employee {
            dept: r.below(3) as u16,
            name: String::gen(r),
            salary: f64::gen(r),
            tags: Vec::gen(r),
            manager: Option::gen(r),
        }
    }
}

#[derive(Clone, Debug, Key)]
enum Shape {
    Dot,
    Line(i32, i32),
    Named { name: String, size: Reverse<u64>, ratio: f32 },
}

impl Gen for Shape {
    fn gen(r: &mut Rng) -> Self {
        match r.below(3) {
            0 => Shape::Dot,
            1 => Shape::Line(r.below(3) as i32 - 1, i32::gen(r)),
            _ => Shape::Named { name: String::gen(r), size: Reverse::gen(r), ratio: f32::gen(r) },
        }
    }
}

#[derive(Clone, Debug, Key)]
struct Pair<A, B>(A, B);

impl<A: Gen, B: Gen> Gen for Pair<A, B> {
    fn gen(r: &mut Rng) -> Self {
        Pair(A::gen(r), B::gen(r))
    }
}

#[derive(Clone, Debug, Key)]
struct Unit;

impl Gen for Unit {
    fn gen(_: &mut Rng) -> Self {
        Unit
    }
}

#[test]
fn derived_keys_order_as_their_bytes() {
    check::<Employee>(50);
    check::<Shape>(51);
    check::<Pair<String, Shape>>(52);
    check::<Vec<Employee>>(53);
    check::<Unit>(54);
}

/// The derived order is `#[derive(Ord)]`'s: fields in declaration order,
/// floats by `total_cmp`, an enum's variants in declaration order first.
#[test]
fn the_derived_order_is_field_by_field() {
    let mut r = Rng(60);
    for _ in 0..4000 {
        let (a, b) = (Employee::gen(&mut r), Employee::gen(&mut r));
        let mirror = |e: &Employee| (e.dept, e.name.clone(), e.tags.clone(), e.manager);
        let want = (a.dept, &a.name)
            .cmp(&(b.dept, &b.name))
            .then(a.salary.total_cmp(&b.salary))
            .then(mirror(&a).2.cmp(&mirror(&b).2))
            .then(a.manager.cmp(&b.manager));
        assert_eq!(a.cmp(&b), want, "{a:?} vs {b:?}");
    }
    assert!(Shape::Dot < Shape::Line(i32::MIN, 0));
    assert!(Shape::Line(5, 0) < Shape::Named { name: String::new(), size: Reverse(0), ratio: 0.0 });
    assert!(Pair(1u8, 9u8) < Pair(2u8, 0u8));
}

/// A float field: `-0.0 < 0.0` and NaN sorts past infinity, as `total_cmp`
/// has it — and equal floats are equal keys (a `PartialEq` on `f64` would
/// say NaN != NaN, and the key would disagree with itself).
#[test]
fn float_fields_use_total_cmp() {
    let e = |salary: f64| Employee { dept: 0, name: String::new(), salary, tags: vec![], manager: None };
    assert!(e(-0.0) < e(0.0));
    assert!(e(f64::INFINITY) < e(f64::NAN));
    assert_eq!(e(f64::NAN), e(f64::NAN));
    assert!(bytes(&e(-1.5)) < bytes(&e(-0.0)));
}

/// Malformed bytes are an `Err`, never a panic: these come from the other
/// side of the boundary.
#[test]
fn malformed_keys_are_refused() {
    for bad in [&[][..], &[0u8][..], &[0, 0][..], &[b'a', 0, 7][..]] {
        assert!(String::decode_key(&mut &bad[..]).is_err(), "{bad:?}");
    }
    assert!(bool::decode_key(&mut &[2u8][..]).is_err());
    assert!(char::decode_key(&mut &[0, 0x11, 0, 0][..]).is_err());
    assert!(Vec::<u8>::decode_key(&mut &[1u8, 5, 2][..]).is_err());
    assert!(Shape::decode_key(&mut &[9u8][..]).is_err());
    assert!(KeyBytes::of(&7u32).decode::<u16>().is_none(), "bytes left over");
}

#[test]
fn numeric_covers_every_number_type_with_distinct_tags() {
    fn tag<T: Numeric>() -> u8 {
        T::TAG
    }
    let mut tags = vec![
        tag::<u8>(), tag::<i8>(), tag::<u16>(), tag::<i16>(), tag::<u32>(),
        tag::<i32>(), tag::<u64>(), tag::<i64>(), tag::<f32>(), tag::<f64>(),
    ];
    tags.sort();
    tags.dedup();
    assert_eq!(tags.len(), runtime_vocabulary::host_types::NUMERIC_TAGS as usize);
    fn sorted<T: Numeric>(mut v: Vec<T>) -> Vec<T> {
        v.sort_unstable_by(T::total_cmp);
        v
    }
    assert_eq!(sorted(vec![3i8, -1, 2]), [-1, 2, 3]);
    let f = sorted(vec![1.0f64, f64::NAN, -0.0, 0.0]);
    assert_eq!(f[..3], [-0.0, 0.0, 1.0]);
    assert!(f[3].is_nan());
    assert_eq!(<f32 as Numeric>::from_f64(2.5), 2.5);
    assert_eq!(<u8 as Numeric>::from_f64(300.0), 255);
}
