//! Lists of numbers cross as one byte run.
//!
//! A `Vec<T>` crosses as its length, then its elements. Encoded one by one
//! (postcard, an allocation per value), a list costs the bundle more to
//! send than most work done with it: measured on 200k `u32`s sent to a host
//! function that sorts them, per-element encoding took 339 ms where sorting
//! in the bundle's own interpreted wasm took 182 ms, and one byte run took
//! 17.8 ms (`showcase/app/src/bench.rs`, `sort_host*`). So a list of a
//! [`Bulk`] type is its elements' little-endian bytes, back to back — a
//! memcpy on both sides (wasm is little-endian, and so is every host the
//! framework ships on; a big-endian one swaps per element).
//!
//! Every codec trait (`RemoteValue`, `RemoteProp`, `ImportArg`) routes a
//! list through its element type's `*_many` hook, which the [`Bulk`] types
//! override with [`encode`] / [`decode`]; any other type keeps the
//! per-element default. Changing this format is a change to
//! [`CODEC_VERSION`](super::CODEC_VERSION).

use std::mem::size_of;

/// A number type whose list crosses as raw little-endian bytes.
///
/// # Safety
///
/// The type must have no padding and accept every bit pattern of its size
/// (a `bool` or `char` doesn't: not every byte is one), since [`decode`]
/// fills a `Vec<Self>` straight from the bundle's bytes. `usize`/`isize`
/// aren't `Bulk` either: their width differs between the wasm32 bundle and
/// a 64-bit app.
pub unsafe trait Bulk: Copy + 'static {
    /// `self` converted between native and little-endian byte order (its
    /// own inverse).
    fn swap_le(self) -> Self;
}

macro_rules! bulk_int {
    ($($t:ty),*) => {$(
        // SAFETY: a primitive integer: no padding, every bit pattern valid.
        unsafe impl Bulk for $t {
            fn swap_le(self) -> Self {
                self.to_le()
            }
        }
    )*};
}
bulk_int!(u8, i8, u16, i16, u32, i32, u64, i64);

// SAFETY: IEEE floats: no padding, every bit pattern is some f32/f64 (NaN
// payloads included, carried bit for bit).
unsafe impl Bulk for f32 {
    fn swap_le(self) -> Self {
        f32::from_bits(self.to_bits().to_le())
    }
}
// SAFETY: as for f32.
unsafe impl Bulk for f64 {
    fn swap_le(self) -> Self {
        f64::from_bits(self.to_bits().to_le())
    }
}

/// Append `items` as little-endian bytes.
pub fn encode<T: Bulk>(items: &[T], out: &mut Vec<u8>) {
    #[cfg(target_endian = "little")]
    {
        // SAFETY: `T: Bulk` has no padding, so all `size_of_val(items)`
        // bytes are initialized; u8 has no alignment requirement.
        let bytes = unsafe { std::slice::from_raw_parts(items.as_ptr() as *const u8, std::mem::size_of_val(items)) };
        out.extend_from_slice(bytes);
    }
    #[cfg(not(target_endian = "little"))]
    {
        out.reserve(std::mem::size_of_val(items));
        for x in items {
            let x = x.swap_le();
            // SAFETY: as above, for one element.
            out.extend_from_slice(unsafe { std::slice::from_raw_parts(&x as *const T as *const u8, size_of::<T>()) });
        }
    }
}

/// Take `n` elements from the front of `input`. `Err` when fewer than `n`
/// elements' bytes are left (a malformed count is refused before anything
/// is allocated).
pub fn decode<T: Bulk>(n: usize, input: &mut &[u8]) -> Result<Vec<T>, String> {
    let bytes = n
        .checked_mul(size_of::<T>())
        .filter(|&b| b <= input.len())
        .ok_or_else(|| format!("remote codec: a list claims {n} numbers with {} bytes left", input.len()))?;
    let mut v: Vec<T> = Vec::with_capacity(n);
    // SAFETY: `input` holds at least `bytes` bytes and `v` has room for
    // exactly `bytes`; they don't overlap (`v` is fresh). The copy is
    // bytewise, so the source needs no alignment, and `T: Bulk` accepts
    // any bit pattern, so all `n` elements are initialized values.
    unsafe {
        std::ptr::copy_nonoverlapping(input.as_ptr(), v.as_mut_ptr() as *mut u8, bytes);
        v.set_len(n);
    }
    #[cfg(not(target_endian = "little"))]
    for x in &mut v {
        *x = x.swap_le();
    }
    *input = &input[bytes..];
    Ok(v)
}

#[cfg(test)]
mod tests {
    use super::super::RemoteValue;

    fn round<T: RemoteValue>(v: &T) -> (Vec<u8>, T) {
        let mut out = Vec::new();
        v.encode(&mut out);
        let mut input = &out[..];
        let back = T::decode(&mut input).expect("decodes");
        assert!(input.is_empty(), "consumed exactly");
        (out, back)
    }

    /// The format itself: a list of numbers is its count (postcard varint),
    /// then each element's little-endian bytes, nothing between. The
    /// per-element encoding this replaces wrote varints (`[2, 1, 132, 134,
    /// 136, 8]` here), so a test that only round-trips would pass either way.
    #[test]
    fn a_number_list_crosses_as_its_little_endian_bytes() {
        let (bytes, back) = round(&vec![1u32, 0x0102_0304]);
        assert_eq!(bytes, [2, 1, 0, 0, 0, 4, 3, 2, 1]);
        assert_eq!(back, [1, 0x0102_0304]);
        let (bytes, _) = round(&[0x0102u16, 0x0304]);
        assert_eq!(bytes, [2, 1, 4, 3], "an array has no count");
    }

    #[test]
    fn every_number_type_round_trips() {
        assert_eq!(round(&vec![0u8, 255, 7]).1, [0, 255, 7]);
        assert_eq!(round(&vec![i8::MIN, -1, i8::MAX]).1, [i8::MIN, -1, i8::MAX]);
        assert_eq!(round(&vec![u16::MAX, 0]).1, [u16::MAX, 0]);
        assert_eq!(round(&vec![i16::MIN, 3]).1, [i16::MIN, 3]);
        assert_eq!(round(&vec![i32::MIN, -5, i32::MAX]).1, [i32::MIN, -5, i32::MAX]);
        assert_eq!(round(&vec![u64::MAX, 1 << 40]).1, [u64::MAX, 1 << 40]);
        assert_eq!(round(&vec![i64::MIN, -1]).1, [i64::MIN, -1]);
        assert_eq!(round(&vec![1.5f32, -0.0, f32::INFINITY]).1, [1.5, -0.0, f32::INFINITY]);
        assert_eq!(round(&[std::f64::consts::PI, -2.0, 0.1]).1, [std::f64::consts::PI, -2.0, 0.1]);
        assert_eq!(round(&Vec::<u32>::new()).1, Vec::<u32>::new());
        // Nested: each inner list is a byte run of its own.
        assert_eq!(round(&vec![vec![1u16, 2], vec![], vec![3]]).1, [vec![1, 2], vec![], vec![3]]);
        assert_eq!(round(&(vec![9u8], 4u32, vec![-1i64])).1, (vec![9], 4, vec![-1]));
    }

    /// Bit for bit: a NaN's payload and the sign of zero survive.
    #[test]
    fn floats_cross_bit_for_bit() {
        let nan = f64::from_bits(0x7ff8_0000_dead_beef);
        let back = round(&vec![nan, -0.0f64]).1;
        assert_eq!(back[0].to_bits(), nan.to_bits());
        assert_eq!(back[1].to_bits(), (-0.0f64).to_bits());
    }

    /// The bundle's bytes are untrusted: a count the input can't hold is an
    /// `Err`, before anything is allocated for it (a count of 2^60 `u64`s
    /// would overflow, or abort the app on allocation).
    #[test]
    fn a_short_or_overlong_list_is_refused() {
        let mut bytes = Vec::new();
        vec![1u32, 2, 3].encode(&mut bytes);
        let mut short = &bytes[..bytes.len() - 1];
        assert!(Vec::<u32>::decode(&mut short).is_err());

        let mut huge = Vec::new();
        super::super::__send_value(&(1u64 << 60), &mut huge);
        huge.extend_from_slice(&[0; 64]);
        assert!(Vec::<u64>::decode(&mut &huge[..]).is_err());
        assert!(super::decode::<u64>(usize::MAX, &mut &huge[..]).is_err(), "n * size overflows: refused, not wrapped");
        assert!(<[u32; 4]>::decode(&mut &[0u8; 15][..]).is_err());
    }

    /// Only the number types are byte runs: `usize` (32 bits in the wasm32
    /// bundle, 64 in the app) and `bool`/`char` (not every byte is one)
    /// keep the per-element encoding.
    #[test]
    fn other_lists_keep_the_per_element_encoding() {
        assert_eq!(round(&vec![300usize]).0, [1, 172, 2]);
        assert_eq!(round(&vec![true, false]).0, [2, 1, 0]);
        assert_eq!(round(&vec!["a".to_string()]).1, ["a"]);
    }

    /// A prop list (`RemoteProp`, app → bundle) and an app component's prop
    /// list (`ImportArg`, bundle → app) use the same byte run.
    #[cfg(feature = "remote-loopback")]
    #[test]
    fn props_cross_number_lists_as_the_same_byte_run() {
        use super::super::{ImportArg, RemoteProp};
        let v = vec![1u32, 0x0102_0304];
        let mut prop = Vec::new();
        RemoteProp::send(&v, &mut prop, &mut Vec::new());
        assert_eq!(prop, [2, 1, 0, 0, 0, 4, 3, 2, 1]);
        assert_eq!(<Vec<u32> as RemoteProp>::receive(&mut &prop[..]), v);
        let mut import = Vec::new();
        ImportArg::send(v.clone(), &mut import);
        assert_eq!(import, prop);
    }
}
