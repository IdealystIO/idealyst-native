//! The bounds a generic `#[host_fn]` is written against: [`Key`],
//! [`Opaque`] and [`Numeric`].
//!
//! A generic function exists in the app only as the instantiations the app
//! compiled, and a remote bundle may call one with a type the app has never
//! seen (one defined in the bundle). So each type parameter is classified
//! by what its bound lets the body do with it:
//!
//! - **[`Key`]**: compare, hash, clone. The app compiles the body once,
//!   with the parameter as [`KeyBytes`]: a key's bytes, which order as the
//!   key does. Any `Key` type works, including a bundle-only one.
//! - **[`Opaque`]**: move (and clone). The app compiles the body once, with
//!   the parameter as [`OpaqueBytes`], the value's encoding it never reads.
//! - **[`Numeric`]**: arithmetic on the number types. The app compiles the
//!   body for each of them, and a call says which.
//!
//! Native callers in the app call the generic function with their real
//! types, as any Rust function; only a bundle's call goes through the
//! stand-ins. These traits exist in every build (web included), so a
//! generic `#[host_fn]` compiles everywhere; crossing needs the `remote`
//! feature.

use std::cmp::{Ordering, Reverse};
use std::fmt::{Debug, Display};
use std::hash::Hash;
use std::iter::{Product, Sum};
use std::ops::{Add, AddAssign, Div, DivAssign, Mul, MulAssign, Sub, SubAssign};

// ---------------------------------------------------------------------------
// Key
// ---------------------------------------------------------------------------

/// A value a host function can compare, hash and clone without knowing its
/// type. `#[derive(Key)]` gives a struct or enum this, together with
/// `PartialEq`, `Eq`, `PartialOrd`, `Ord` and `Hash` — all from the same
/// fields in the same order, so a type can't carry an `Ord` its key bytes
/// disagree with (deriving or writing those yourself as well is a
/// conflicting-impl error).
///
/// The encoding is order-preserving: comparing two keys' bytes gives the
/// same answer as comparing the keys. Numbers are big-endian (signed ones
/// with the sign bit flipped); strings and lists are escaped and
/// terminated, so a shorter one sorts first and fields can follow it;
/// `Option`, tuples, arrays and derived fields are their parts in order;
/// [`Reverse`] inverts its value's bytes.
///
/// `f32`/`f64` aren't `Key` (they aren't `Ord`); as a field of a derived
/// `Key` they compare with `total_cmp`. A list of floats on its own goes
/// through [`Numeric`].
pub trait Key: Ord + Hash + Clone + 'static {
    /// Append this key's order-preserving bytes.
    #[doc(hidden)]
    fn encode_key(&self, out: &mut Vec<u8>);
    /// Take one key from the front of `input`.
    #[doc(hidden)]
    fn decode_key(input: &mut &[u8]) -> Result<Self, String>;
}

fn take<'a>(input: &mut &'a [u8], n: usize) -> Result<&'a [u8], String> {
    if input.len() < n {
        return Err(format!("key: {n} bytes wanted, {} left", input.len()));
    }
    let (head, tail) = input.split_at(n);
    *input = tail;
    Ok(head)
}

macro_rules! key_unsigned {
    ($($t:ty),*) => {$(
        impl Key for $t {
            fn encode_key(&self, out: &mut Vec<u8>) {
                out.extend_from_slice(&self.to_be_bytes());
            }
            fn decode_key(input: &mut &[u8]) -> Result<Self, String> {
                Ok(<$t>::from_be_bytes(take(input, size_of::<$t>())?.try_into().expect("sized")))
            }
        }
    )*};
}
key_unsigned!(u8, u16, u32, u64, u128);

// Two's complement orders negatives above positives as unsigned bytes;
// flipping the sign bit puts them below, in order.
macro_rules! key_signed {
    ($($t:ty => $u:ty),*) => {$(
        impl Key for $t {
            fn encode_key(&self, out: &mut Vec<u8>) {
                out.extend_from_slice(&((*self as $u) ^ (1 << (<$u>::BITS - 1))).to_be_bytes());
            }
            fn decode_key(input: &mut &[u8]) -> Result<Self, String> {
                let u = <$u>::from_be_bytes(take(input, size_of::<$u>())?.try_into().expect("sized"));
                Ok((u ^ (1 << (<$u>::BITS - 1))) as $t)
            }
        }
    )*};
}
key_signed!(i8 => u8, i16 => u16, i32 => u32, i64 => u64, i128 => u128);

// The bundle is wasm32 and the app usually 64-bit: `usize`/`isize` cross as
// 64 bits, and a bundle refuses one too big for it.
impl Key for usize {
    fn encode_key(&self, out: &mut Vec<u8>) {
        (*self as u64).encode_key(out)
    }
    fn decode_key(input: &mut &[u8]) -> Result<Self, String> {
        usize::try_from(u64::decode_key(input)?).map_err(|_| "key: usize out of range".to_string())
    }
}
impl Key for isize {
    fn encode_key(&self, out: &mut Vec<u8>) {
        (*self as i64).encode_key(out)
    }
    fn decode_key(input: &mut &[u8]) -> Result<Self, String> {
        isize::try_from(i64::decode_key(input)?).map_err(|_| "key: isize out of range".to_string())
    }
}

impl Key for bool {
    fn encode_key(&self, out: &mut Vec<u8>) {
        out.push(*self as u8);
    }
    fn decode_key(input: &mut &[u8]) -> Result<Self, String> {
        match take(input, 1)?[0] {
            0 => Ok(false),
            1 => Ok(true),
            b => Err(format!("key: {b} is not a bool")),
        }
    }
}

impl Key for char {
    fn encode_key(&self, out: &mut Vec<u8>) {
        (*self as u32).encode_key(out)
    }
    fn decode_key(input: &mut &[u8]) -> Result<Self, String> {
        let u = u32::decode_key(input)?;
        char::from_u32(u).ok_or_else(|| format!("key: {u:#x} is not a char"))
    }
}

impl Key for () {
    fn encode_key(&self, _out: &mut Vec<u8>) {}
    fn decode_key(_input: &mut &[u8]) -> Result<Self, String> {
        Ok(())
    }
}

/// A byte string's key: each 0x00 becomes 0x00 0xFF, then 0x00 0x01 ends
/// it. A string that is a prefix of another ends with 0x00 0x01 where the
/// other continues with a byte ≥ 0x01 (or 0x00 0xFF), so it sorts first,
/// and the terminator lets another field follow.
fn encode_escaped(bytes: &[u8], out: &mut Vec<u8>) {
    for &b in bytes {
        out.push(b);
        if b == 0 {
            out.push(0xFF);
        }
    }
    out.extend_from_slice(&[0, 1]);
}

fn decode_escaped(input: &mut &[u8]) -> Result<Vec<u8>, String> {
    let mut out = Vec::new();
    let mut i = 0;
    loop {
        match input.get(i) {
            None => return Err("key: unterminated string".into()),
            Some(0) => match input.get(i + 1) {
                Some(0xFF) => {
                    out.push(0);
                    i += 2;
                }
                Some(1) => {
                    *input = &input[i + 2..];
                    return Ok(out);
                }
                _ => return Err("key: bad string escape".into()),
            },
            Some(&b) => {
                out.push(b);
                i += 1;
            }
        }
    }
}

impl Key for String {
    fn encode_key(&self, out: &mut Vec<u8>) {
        encode_escaped(self.as_bytes(), out)
    }
    fn decode_key(input: &mut &[u8]) -> Result<Self, String> {
        String::from_utf8(decode_escaped(input)?).map_err(|_| "key: string is not UTF-8".to_string())
    }
}

/// Each element after a 0x01, then a 0x00: a list that is a prefix of
/// another hits 0x00 where the other has 0x01, so it sorts first.
impl<T: Key> Key for Vec<T> {
    fn encode_key(&self, out: &mut Vec<u8>) {
        for v in self {
            out.push(1);
            v.encode_key(out);
        }
        out.push(0);
    }
    fn decode_key(input: &mut &[u8]) -> Result<Self, String> {
        let mut v = Vec::new();
        loop {
            match take(input, 1)?[0] {
                0 => return Ok(v),
                1 => v.push(T::decode_key(input)?),
                b => return Err(format!("key: bad list marker {b}")),
            }
        }
    }
}

impl<T: Key> Key for Option<T> {
    fn encode_key(&self, out: &mut Vec<u8>) {
        match self {
            None => out.push(0),
            Some(v) => {
                out.push(1);
                v.encode_key(out);
            }
        }
    }
    fn decode_key(input: &mut &[u8]) -> Result<Self, String> {
        match take(input, 1)?[0] {
            0 => Ok(None),
            1 => Ok(Some(T::decode_key(input)?)),
            b => Err(format!("key: bad option marker {b}")),
        }
    }
}

impl<T: Key> Key for Box<T> {
    fn encode_key(&self, out: &mut Vec<u8>) {
        (**self).encode_key(out)
    }
    fn decode_key(input: &mut &[u8]) -> Result<Self, String> {
        T::decode_key(input).map(Box::new)
    }
}

/// Descending: the value's bytes, each inverted. Every encoding here is
/// prefix-free (two keys first differ at a byte where neither has ended),
/// so inverting the bytes inverts the order.
impl<T: Key> Key for Reverse<T> {
    fn encode_key(&self, out: &mut Vec<u8>) {
        let start = out.len();
        self.0.encode_key(out);
        for b in &mut out[start..] {
            *b = !*b;
        }
    }
    fn decode_key(input: &mut &[u8]) -> Result<Self, String> {
        // Un-invert what's left to find where the value ends. A key is
        // decoded from its own frame, so "what's left" is one key's bytes.
        let plain: Vec<u8> = input.iter().map(|b| !b).collect();
        let mut rest = &plain[..];
        let v = T::decode_key(&mut rest)?;
        *input = &input[plain.len() - rest.len()..];
        Ok(Reverse(v))
    }
}

impl<T: Key, const N: usize> Key for [T; N] {
    fn encode_key(&self, out: &mut Vec<u8>) {
        for v in self {
            v.encode_key(out);
        }
    }
    fn decode_key(input: &mut &[u8]) -> Result<Self, String> {
        let items = (0..N).map(|_| T::decode_key(input)).collect::<Result<Vec<T>, String>>()?;
        items.try_into().map_err(|_| "key: array length".to_string())
    }
}

macro_rules! key_tuple {
    ($($n:ident),+) => {
        impl<$($n: Key),+> Key for ($($n,)+) {
            #[allow(non_snake_case)]
            fn encode_key(&self, out: &mut Vec<u8>) {
                let ($($n,)+) = self;
                $($n.encode_key(out);)+
            }
            fn decode_key(input: &mut &[u8]) -> Result<Self, String> {
                Ok(($($n::decode_key(input)?,)+))
            }
        }
    };
}
key_tuple!(A);
key_tuple!(A, B);
key_tuple!(A, B, C);
key_tuple!(A, B, C, D);
key_tuple!(A, B, C, D, E);
key_tuple!(A, B, C, D, E, F);
key_tuple!(A, B, C, D, E, F, G);
key_tuple!(A, B, C, D, E, F, G, H);

/// A float field of a derived `Key` (`#[derive(Key)]` calls these): the
/// bits reordered so unsigned comparison is `total_cmp`'s order —
/// negatives (sign set) inverted, positives with the sign bit set.
#[doc(hidden)]
pub mod float {
    use std::cmp::Ordering;

    pub fn encode_f64(v: f64, out: &mut Vec<u8>) {
        let b = v.to_bits();
        let k = if b >> 63 == 1 { !b } else { b ^ (1 << 63) };
        out.extend_from_slice(&k.to_be_bytes());
    }
    pub fn decode_f64(input: &mut &[u8]) -> Result<f64, String> {
        let k = <u64 as super::Key>::decode_key(input)?;
        Ok(f64::from_bits(if k >> 63 == 1 { k ^ (1 << 63) } else { !k }))
    }
    pub fn encode_f32(v: f32, out: &mut Vec<u8>) {
        let b = v.to_bits();
        let k = if b >> 31 == 1 { !b } else { b ^ (1 << 31) };
        out.extend_from_slice(&k.to_be_bytes());
    }
    pub fn decode_f32(input: &mut &[u8]) -> Result<f32, String> {
        let k = <u32 as super::Key>::decode_key(input)?;
        Ok(f32::from_bits(if k >> 31 == 1 { k ^ (1 << 31) } else { !k }))
    }
    pub fn cmp_f64(a: &f64, b: &f64) -> Ordering {
        a.total_cmp(b)
    }
    pub fn cmp_f32(a: &f32, b: &f32) -> Ordering {
        a.total_cmp(b)
    }
    /// Equal under `total_cmp` exactly when the bits are equal, so hashing
    /// the bits agrees with that `Eq`.
    pub fn hash_f64<H: std::hash::Hasher>(v: &f64, h: &mut H) {
        std::hash::Hash::hash(&v.to_bits(), h)
    }
    pub fn hash_f32<H: std::hash::Hasher>(v: &f32, h: &mut H) {
        std::hash::Hash::hash(&v.to_bits(), h)
    }
}

/// Any [`Key`], as its bytes: what a generic host function's `K: Key`
/// parameter is in the app when a bundle calls it. Its order, equality and
/// hash are those of the bytes, which are the key's.
#[derive(Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct KeyBytes(Vec<u8>);

impl KeyBytes {
    /// The key `k`'s bytes.
    pub fn of<K: Key>(k: &K) -> KeyBytes {
        let mut out = Vec::new();
        k.encode_key(&mut out);
        KeyBytes(out)
    }
    /// Decode the key these bytes are (`None` if they aren't a `K`).
    pub fn decode<K: Key>(&self) -> Option<K> {
        let mut input = &self.0[..];
        let k = K::decode_key(&mut input).ok()?;
        input.is_empty().then_some(k)
    }
    pub fn as_bytes(&self) -> &[u8] {
        &self.0
    }
    #[doc(hidden)]
    pub fn from_raw(bytes: Vec<u8>) -> KeyBytes {
        KeyBytes(bytes)
    }
    #[doc(hidden)]
    pub fn into_raw(self) -> Vec<u8> {
        self.0
    }
}

impl Debug for KeyBytes {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "KeyBytes({:02x?})", self.0)
    }
}

/// Its bytes are already a key: nested (a tuple of `KeyBytes` built by the
/// body), they're written as they are. Decoding needs the original type to
/// know where a key ends, so a `KeyBytes` decodes only from a framed run
/// (the host-function wire does that).
impl Key for KeyBytes {
    fn encode_key(&self, out: &mut Vec<u8>) {
        out.extend_from_slice(&self.0);
    }
    fn decode_key(_input: &mut &[u8]) -> Result<Self, String> {
        Err("key: KeyBytes can't be read back out of a larger key".into())
    }
}

// ---------------------------------------------------------------------------
// Opaque
// ---------------------------------------------------------------------------

/// A value a host function only moves (and clones, with `+ Clone`): it
/// carries it along unread. Every type is `Opaque`; a bundle's value
/// crosses with its own codec (`#[derive(Remote)]`), and the app holds it
/// as [`OpaqueBytes`].
pub trait Opaque: 'static {}
impl<T: 'static> Opaque for T {}

/// Any [`Opaque`] value, as its encoding: what a generic host function's
/// `V: Opaque` parameter is in the app when a bundle calls it.
#[derive(Clone, Debug)]
pub struct OpaqueBytes(Vec<u8>);

impl OpaqueBytes {
    #[doc(hidden)]
    pub fn from_raw(bytes: Vec<u8>) -> OpaqueBytes {
        OpaqueBytes(bytes)
    }
    #[doc(hidden)]
    pub fn as_raw(&self) -> &[u8] {
        &self.0
    }
}

// ---------------------------------------------------------------------------
// Numeric
// ---------------------------------------------------------------------------

mod sealed {
    pub trait Sealed {}
}

/// The number types (`u8`…`u64`, `i8`…`i64`, `f32`, `f64`) for a generic
/// host function: the app compiles the body for each, and a bundle's call
/// says which. A list of them crosses as one byte run.
///
/// `total_cmp` orders every value, floats included (`NaN`s last), so
/// `v.sort_unstable_by(T::total_cmp)` works for all of them.
pub trait Numeric:
    Copy
    + Default
    + PartialEq
    + PartialOrd
    + Debug
    + Display
    + Add<Output = Self>
    + Sub<Output = Self>
    + Mul<Output = Self>
    + Div<Output = Self>
    + AddAssign
    + SubAssign
    + MulAssign
    + DivAssign
    + Sum
    + Product
    + 'static
    + sealed::Sealed
{
    /// Which number type a bundle's call carries.
    #[doc(hidden)]
    const TAG: u8;
    const ZERO: Self;
    const ONE: Self;
    const MIN: Self;
    const MAX: Self;
    /// A total order: `Ord::cmp` for the integers, `total_cmp` for floats.
    fn total_cmp(&self, other: &Self) -> Ordering;
    /// As an `f64` (exact for every integer up to 2^53).
    fn to_f64(self) -> f64;
    /// From an `f64`, as `as` converts (saturating; `NaN` is 0 for
    /// integers).
    fn from_f64(v: f64) -> Self;
}

macro_rules! numeric_int {
    ($($t:ty => $tag:expr),*) => {$(
        impl sealed::Sealed for $t {}
        impl Numeric for $t {
            const TAG: u8 = $tag;
            const ZERO: Self = 0;
            const ONE: Self = 1;
            const MIN: Self = <$t>::MIN;
            const MAX: Self = <$t>::MAX;
            fn total_cmp(&self, other: &Self) -> Ordering {
                Ord::cmp(self, other)
            }
            fn to_f64(self) -> f64 {
                self as f64
            }
            fn from_f64(v: f64) -> Self {
                v as $t
            }
        }
    )*};
}
numeric_int!(u8 => 0, i8 => 1, u16 => 2, i16 => 3, u32 => 4, i32 => 5, u64 => 6, i64 => 7);

macro_rules! numeric_float {
    ($($t:ty => $tag:expr),*) => {$(
        impl sealed::Sealed for $t {}
        impl Numeric for $t {
            const TAG: u8 = $tag;
            const ZERO: Self = 0.0;
            const ONE: Self = 1.0;
            const MIN: Self = <$t>::MIN;
            const MAX: Self = <$t>::MAX;
            fn total_cmp(&self, other: &Self) -> Ordering {
                <$t>::total_cmp(self, other)
            }
            fn to_f64(self) -> f64 {
                self as f64
            }
            fn from_f64(v: f64) -> Self {
                v as $t
            }
        }
    )*};
}
numeric_float!(f32 => 8, f64 => 9);

/// Every `Numeric` type, by tag: what the app's dispatch matches on
/// (`#[host_fn]` emits one arm per entry).
#[doc(hidden)]
pub const NUMERIC_TAGS: u8 = 10;
