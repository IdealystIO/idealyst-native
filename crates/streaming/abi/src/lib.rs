//! `Wire`: the little-endian, length-prefixed codec the hand-written kernel
//! bridge code uses for host-owned values.
//!
//! The bridged loader (`stream_host::kernel`) exports app signals and
//! context to a bundle as raw bytes; `export_signal` / `export_read_signal`
//! encode their values with [`Wire`], and the hand-written test bundles
//! (`spike/kernelguest`, `spike/remoteguest`) decode them with the same
//! impls, so the encoding cannot drift between the two sides. Code written
//! with `#[component(remote)]` never sees this crate: its props, context and
//! `#[host_fn]` values cross as the framework's `RemoteValue`
//! (`#[derive(runtime_core::Remote)]`).
//!
//! Zero dependencies on purpose: a bundle links it, and every byte it pulls
//! in ships inside every bundle.

#![forbid(unsafe_code)]

/// Little-endian length-prefixed encoding shared by both sides.
pub trait Wire: Sized {
    fn encode(&self, out: &mut Vec<u8>);
    /// Consume one value from the front of `input`. `None` on truncation
    /// or a malformed value.
    fn decode(input: &mut &[u8]) -> Option<Self>;

    fn to_bytes(&self) -> Vec<u8> {
        let mut out = Vec::new();
        self.encode(&mut out);
        out
    }

    /// Decode a value that must occupy all of `bytes`.
    fn from_bytes(mut bytes: &[u8]) -> Option<Self> {
        let v = Self::decode(&mut bytes)?;
        bytes.is_empty().then_some(v)
    }
}

fn take<'a>(input: &mut &'a [u8], n: usize) -> Option<&'a [u8]> {
    if input.len() < n {
        return None;
    }
    let (head, tail) = input.split_at(n);
    *input = tail;
    Some(head)
}

macro_rules! wire_le {
    ($($t:ty),*) => {$(
        impl Wire for $t {
            fn encode(&self, out: &mut Vec<u8>) {
                out.extend_from_slice(&self.to_le_bytes());
            }
            fn decode(input: &mut &[u8]) -> Option<Self> {
                let b = take(input, core::mem::size_of::<$t>())?;
                Some(<$t>::from_le_bytes(b.try_into().ok()?))
            }
        }
    )*};
}
wire_le!(u8, u32, i32, u64, i64, f64);

impl Wire for bool {
    fn encode(&self, out: &mut Vec<u8>) {
        out.push(*self as u8);
    }
    fn decode(input: &mut &[u8]) -> Option<Self> {
        match u8::decode(input)? {
            0 => Some(false),
            1 => Some(true),
            _ => None,
        }
    }
}

impl Wire for String {
    fn encode(&self, out: &mut Vec<u8>) {
        (self.len() as u32).encode(out);
        out.extend_from_slice(self.as_bytes());
    }
    fn decode(input: &mut &[u8]) -> Option<Self> {
        let n = u32::decode(input)? as usize;
        String::from_utf8(take(input, n)?.to_vec()).ok()
    }
}

impl<T: Wire> Wire for Vec<T> {
    fn encode(&self, out: &mut Vec<u8>) {
        (self.len() as u32).encode(out);
        for v in self {
            v.encode(out);
        }
    }
    fn decode(input: &mut &[u8]) -> Option<Self> {
        let n = u32::decode(input)? as usize;
        // Cap the pre-allocation by what the input could possibly hold, so
        // a corrupt length cannot make the host allocate gigabytes.
        let mut v = Vec::with_capacity(n.min(input.len()));
        for _ in 0..n {
            v.push(T::decode(input)?);
        }
        Some(v)
    }
}

impl<T: Wire> Wire for Option<T> {
    fn encode(&self, out: &mut Vec<u8>) {
        match self {
            None => out.push(0),
            Some(v) => {
                out.push(1);
                v.encode(out);
            }
        }
    }
    fn decode(input: &mut &[u8]) -> Option<Self> {
        match u8::decode(input)? {
            0 => Some(None),
            1 => Some(Some(T::decode(input)?)),
            _ => None,
        }
    }
}

impl<T: Wire, E: Wire> Wire for Result<T, E> {
    fn encode(&self, out: &mut Vec<u8>) {
        match self {
            Ok(v) => {
                out.push(0);
                v.encode(out);
            }
            Err(e) => {
                out.push(1);
                e.encode(out);
            }
        }
    }
    fn decode(input: &mut &[u8]) -> Option<Self> {
        match u8::decode(input)? {
            0 => Some(Ok(T::decode(input)?)),
            1 => Some(Err(E::decode(input)?)),
            _ => None,
        }
    }
}

impl Wire for () {
    fn encode(&self, _: &mut Vec<u8>) {}
    fn decode(_: &mut &[u8]) -> Option<Self> {
        Some(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn values_round_trip() {
        assert_eq!(i64::from_bytes(&(-7i64).to_bytes()), Some(-7));
        assert_eq!(f64::from_bytes(&1.5f64.to_bytes()), Some(1.5));
        assert_eq!(bool::from_bytes(&true.to_bytes()), Some(true));
        assert_eq!(String::from_bytes(&"ada".to_string().to_bytes()), Some("ada".to_string()));
        assert_eq!(<()>::from_bytes(&().to_bytes()), Some(()));
    }

    #[test]
    fn truncated_input_is_rejected_not_misread() {
        let bytes = "hello".to_string().to_bytes();
        assert_eq!(String::from_bytes(&bytes[..bytes.len() - 1]), None);
        assert_eq!(String::from_bytes(&[bytes.as_slice(), &[0]].concat()), None);
    }

    #[test]
    fn corrupt_vec_length_does_not_preallocate_unbounded() {
        // u32::MAX elements claimed, nothing behind it: must fail, not OOM.
        let bytes = u32::MAX.to_bytes();
        assert_eq!(Vec::<u64>::from_bytes(&bytes), None);
    }

    #[test]
    fn option_and_result_round_trip() {
        let v: Vec<Result<Option<String>, u32>> = vec![Ok(Some("a".into())), Ok(None), Err(7)];
        assert_eq!(Vec::from_bytes(&v.to_bytes()), Some(v));
    }

    #[test]
    fn a_bool_other_than_0_or_1_is_rejected() {
        assert_eq!(bool::from_bytes(&[2]), None);
    }
}
