//! `#[host_fn]`'s records: an app function a bundle calls by name.
//!
//! In the app a `#[host_fn]` is the real function plus a [`HostFnDef`] the
//! app lists in its allowlist (`remote_host::remote::install_with`). In a
//! bundle it's a stub calling ONE wasm import, under [`HOST_FN_MODULE`],
//! named [`import_name`]`(path, schema)` — so the bundle's import section is
//! its list of host functions, and the app refuses a bundle at load that
//! calls one it doesn't export, or exports with a different signature.
//!
//! These are the records the bridged loader (`remote_host::kernel`) checks a
//! bundle's host-function imports against. They live in the vocabulary so a
//! library's `#[host_fn]` (idea-ui) can emit them: the vocabulary is
//! published, the loader crate isn't.

use std::future::Future;
use std::pin::Pin;

/// The wasm import module every host function lives under.
pub const HOST_FN_MODULE: &str = "idealyst_host_fn";

/// An async host function's body: encoded args in, encoded result out.
/// `Err` when the arguments don't decode (the bundle is stopped).
pub type AsyncCall = fn(Vec<u8>) -> Result<Pin<Box<dyn Future<Output = Vec<u8>>>>, String>;

#[derive(Clone, Copy)]
pub enum HostFnKind {
    /// Import `(args_ptr, args_len) -> reply_len`.
    /// `Err` when the arguments don't decode (the bundle is stopped).
    Sync(fn(&[u8]) -> Result<Vec<u8>, String>),
    /// Import `(args_ptr, args_len, then_callback)`: returns at once; the
    /// app runs the future and calls `then_callback` with the result.
    Async(AsyncCall),
    /// A generic host function over an app type (`S: Area`): one
    /// instantiation per type the app lists ([`host_fn_instances!`]), picked
    /// by the type's [`RemoteName`], which the call starts with. `lookup`
    /// maps that name to the instantiation (a `Sync` or `Async`, as
    /// `asynchronous` says); a name it doesn't know is an `Err` naming it.
    ///
    /// [`host_fn_instances!`]: crate::host_fn_instances
    Listed { asynchronous: bool, lookup: fn(&str) -> Option<HostFnKind> },
}

/// What an app exports for one host function (`<fn>::export()`).
#[derive(Clone, Copy)]
pub struct HostFnDef {
    /// `module_path!()::fn_name` — the same on both sides.
    pub path: &'static str,
    /// Fingerprint of the signature (argument and return types, asyncness).
    pub schema: u64,
    pub kind: HostFnKind,
}

/// A type's name across the boundary, `module_path::Name`, the same in the
/// app and the bundle: how a call to a listed generic host function says
/// which instantiation it wants. `#[derive(Remote)]` gives it; the
/// primitives have their own names.
pub trait RemoteName {
    const NAME: &'static str;
}

macro_rules! remote_name {
    ($($t:ty),*) => {$(
        impl RemoteName for $t {
            const NAME: &'static str = stringify!($t);
        }
    )*};
}
remote_name!(u8, i8, u16, i16, u32, i32, u64, i64, u128, i128, usize, isize, f32, f64, bool, char, String, ());

/// The instance name a listed call starts with, and the rest of its
/// arguments.
#[doc(hidden)]
pub fn take_instance_name(args: &[u8]) -> Result<(String, &[u8]), String> {
    let mut input = args;
    let name = super::__try_receive_value::<String>(&mut input).map_err(|e| format!("the call names no type ({e})"))?;
    Ok((name, input))
}

/// The allowlist entry for a generic host function over an app type: the
/// types bundles may call it with.
///
/// ```ignore
/// #[host_fn]
/// pub fn total_area<S: Area>(shapes: Vec<S>) -> f64 { … }
///
/// // In the app's allowlist:
/// host_fn_instances!(tools::total_area: Circle, Rect)
/// ```
///
/// Each type must cross as a value and have a stable name: derive
/// `Remote` on it. A bundle calling the function with a type not listed
/// is stopped, with an error naming the instance.
#[macro_export]
macro_rules! host_fn_instances {
    ($($f:ident)::+ : $($t:ty),+ $(,)?) => {{
        // The function's name is also its record module (`instance`,
        // `export_listed`): one alias reaches both.
        use $($f)::+ as __host_fn;
        fn __lookup(name: &str) -> ::core::option::Option<$crate::remote::host_fn::HostFnKind> {
            $(
                if name == <$t as $crate::remote::host_fn::RemoteName>::NAME {
                    return ::core::option::Option::Some(__host_fn::instance::<$t>());
                }
            )+
            ::core::option::Option::None
        }
        __host_fn::export_listed(__lookup)
    }};
}

/// The import name a bundle's stub links against.
pub fn import_name(path: &str, schema: u64) -> String {
    format!("{path}#{schema:016x}")
}

/// Split an import name back into `(path, schema)`.
pub fn parse_import_name(name: &str) -> Option<(&str, u64)> {
    let (path, hash) = name.rsplit_once('#')?;
    Some((path, u64::from_str_radix(hash, 16).ok()?))
}

// ---------------------------------------------------------------------------
// Generic host functions on the wire
// ---------------------------------------------------------------------------
//
// A generic `#[host_fn]`'s `K: Key` parameter is `KeyBytes` in the app and
// `V: Opaque` is `OpaqueBytes`; each crosses FRAMED (its length, then its
// bytes), because the app can't tell where a value of a type it doesn't
// know ends. The length is a fixed 4 bytes (little-endian) so the bundle
// writes the value straight into the call's buffer and patches the length
// after: framing through a temporary buffer and a varint cost several
// allocations per key, interpreted — 373 ms to send 200k two-field keys,
// against 51 ms for the same bytes built by hand. The app decodes them as the `RemoteValue`s below; a bundle's
// stub, which has the real types, writes and reads them with these
// functions — so the two agree byte for byte.

use super::RemoteValue;
use crate::host_types::{Key, KeyBytes, OpaqueBytes};

/// Write a frame whose body `write` appends.
fn frame(out: &mut Vec<u8>, write: impl FnOnce(&mut Vec<u8>)) {
    let at = out.len();
    out.extend_from_slice(&[0; 4]);
    write(out);
    let n = u32::try_from(out.len() - at - 4).expect("a value over 4 GB");
    out[at..at + 4].copy_from_slice(&n.to_le_bytes());
}

fn unframe<'a>(input: &mut &'a [u8]) -> Result<&'a [u8], String> {
    let len: [u8; 4] = input.get(..4).ok_or("a value's length is cut off")?.try_into().expect("4 bytes");
    let n = u32::from_le_bytes(len) as usize;
    let body = input.get(4..4 + n).ok_or_else(|| format!("a value claims {n} bytes with {} left", input.len() - 4))?;
    *input = &input[4 + n..];
    Ok(body)
}

/// A list of keys starts with its layout: [`STRIDE`] then one length for
/// all (keys of a fixed width, `Key::WIDTH`, or a reply whose keys happen
/// to be one length), or [`FRAMED`], a length per key.
const FRAMED: u8 = 0;
const STRIDE: u8 = 1;

impl RemoteValue for KeyBytes {
    fn encode(&self, out: &mut Vec<u8>) {
        frame(out, |out| out.extend_from_slice(self.as_bytes()))
    }
    fn decode(input: &mut &[u8]) -> Result<Self, String> {
        Ok(KeyBytes::from_raw(unframe(input)?.to_vec()))
    }
    fn encode_many(items: &[Self], out: &mut Vec<u8>) {
        match items.first() {
            Some(first) if items.iter().all(|k| k.as_bytes().len() == first.as_bytes().len()) => {
                out.push(STRIDE);
                out.extend_from_slice(&(first.as_bytes().len() as u32).to_le_bytes());
                for k in items {
                    out.extend_from_slice(k.as_bytes());
                }
            }
            _ => {
                out.push(FRAMED);
                for k in items {
                    k.encode(out);
                }
            }
        }
    }
    fn decode_many(n: usize, input: &mut &[u8]) -> Result<Vec<Self>, String> {
        match take_layout(input)? {
            None => (0..n).map(|_| <KeyBytes as RemoteValue>::decode(input)).collect(),
            Some(stride) => {
                let run = take_run(n, stride, input)?;
                Ok(if stride == 0 { vec![KeyBytes::from_raw(Vec::new()); n] } else { run.chunks(stride).map(|c| KeyBytes::from_raw(c.to_vec())).collect() })
            }
        }
    }
}

/// A list's layout byte: `Some(stride)` or `None` (framed).
fn take_layout(input: &mut &[u8]) -> Result<Option<usize>, String> {
    let (&mode, rest) = input.split_first().ok_or("a key list's layout is cut off")?;
    *input = rest;
    match mode {
        FRAMED => Ok(None),
        STRIDE => {
            let len: [u8; 4] = input.get(..4).ok_or("a key list's stride is cut off")?.try_into().expect("4 bytes");
            *input = &input[4..];
            Ok(Some(u32::from_le_bytes(len) as usize))
        }
        m => Err(format!("a key list has layout {m}")),
    }
}

/// `n` keys of `stride` bytes, checked against what's left before anything
/// is allocated.
fn take_run<'a>(n: usize, stride: usize, input: &mut &'a [u8]) -> Result<&'a [u8], String> {
    let total = n
        .checked_mul(stride)
        .filter(|&t| t <= input.len())
        .ok_or_else(|| format!("{n} keys of {stride} bytes with {} left", input.len()))?;
    let (run, rest) = input.split_at(total);
    *input = rest;
    Ok(run)
}

/// A bundle's `Vec<K>` as the app's `Vec<KeyBytes>` reads it: one run
/// when `K` has a fixed width, a frame per key otherwise.
#[doc(hidden)]
pub fn encode_keys<K: Key>(keys: &[K], out: &mut Vec<u8>) {
    super::__send_value(&(keys.len() as u64), out);
    match K::WIDTH {
        Some(w) => {
            out.push(STRIDE);
            out.extend_from_slice(&(w as u32).to_le_bytes());
            out.reserve(w * keys.len());
            for k in keys {
                k.encode_key(out);
            }
        }
        None => {
            out.push(FRAMED);
            for k in keys {
                encode_key(k, out);
            }
        }
    }
}

/// The app's `Vec<KeyBytes>` reply as the bundle's `Vec<K>`.
#[doc(hidden)]
pub fn decode_keys<K: Key>(input: &mut &[u8]) -> Result<Vec<K>, String> {
    let n = super::__try_receive_value::<u64>(input)?;
    let n = usize::try_from(n).map_err(|_| "a key list too long".to_string())?;
    match take_layout(input)? {
        None => {
            let mut keys = Vec::new();
            for _ in 0..n {
                keys.push(decode_key(input)?);
            }
            Ok(keys)
        }
        Some(stride) => {
            let mut run = take_run(n, stride, input)?;
            let mut keys = Vec::with_capacity(n);
            for _ in 0..n {
                let (mut one, rest) = run.split_at(stride);
                keys.push(K::decode_key(&mut one)?);
                if !one.is_empty() {
                    return Err(format!("{} bytes after a key", one.len()));
                }
                run = rest;
            }
            Ok(keys)
        }
    }
}

impl RemoteValue for OpaqueBytes {
    fn encode(&self, out: &mut Vec<u8>) {
        frame(out, |out| out.extend_from_slice(self.as_raw()))
    }
    fn decode(input: &mut &[u8]) -> Result<Self, String> {
        Ok(OpaqueBytes::from_raw(unframe(input)?.to_vec()))
    }
}

/// A bundle's `K` as the app's `KeyBytes` reads it.
#[doc(hidden)]
pub fn encode_key<K: Key>(k: &K, out: &mut Vec<u8>) {
    frame(out, |out| k.encode_key(out))
}

/// A `KeyBytes` the app sent back, as the bundle's `K`. `Err` when the
/// bytes aren't one whole `K`.
#[doc(hidden)]
pub fn decode_key<K: Key>(input: &mut &[u8]) -> Result<K, String> {
    let mut bytes = unframe(input)?;
    let k = K::decode_key(&mut bytes)?;
    if bytes.is_empty() { Ok(k) } else { Err(format!("{} bytes after a key", bytes.len())) }
}

/// A bundle's `V` as the app's `OpaqueBytes` reads it.
#[doc(hidden)]
pub fn encode_opaque<V: RemoteValue>(v: &V, out: &mut Vec<u8>) {
    frame(out, |out| v.encode(out))
}

/// An `OpaqueBytes` the app sent back, as the bundle's `V`.
#[doc(hidden)]
pub fn decode_opaque<V: RemoteValue>(input: &mut &[u8]) -> Result<V, String> {
    let mut bytes = unframe(input)?;
    let v = V::decode(&mut bytes)?;
    if bytes.is_empty() { Ok(v) } else { Err(format!("{} bytes after a value", bytes.len())) }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// What a bundle's stub writes for `Vec<K>` (one framed key each) is
    /// what the app's `Vec<KeyBytes>` reads, and the app's reply reads back
    /// as the bundle's keys — byte for byte, so the two sides agree.
    #[test]
    fn a_bundles_keys_read_as_the_apps_key_bytes_and_back() {
        fn round<K: Key + std::fmt::Debug>(keys: Vec<K>) {
            let mut wire = Vec::new();
            encode_keys(&keys, &mut wire);
            let mut app: Vec<KeyBytes> = RemoteValue::decode(&mut &wire[..]).unwrap();
            assert_eq!(app, keys.iter().map(KeyBytes::of).collect::<Vec<_>>());
            app.sort();
            let mut reply = Vec::new();
            app.encode(&mut reply);
            let back: Vec<K> = decode_keys(&mut &reply[..]).unwrap();
            let mut want = keys;
            want.sort();
            assert_eq!(back, want, "sorting the bytes sorted the keys");
        }
        // Framed (a String has no fixed width), then one run.
        round(vec![(3u16, "b".to_string()), (1, "a\0".to_string()), (3, "a".to_string())]);
        round(vec![(3u16, -1i32), (1, 7), (3, -9)]);
        round(Vec::<u32>::new());
        round(vec![(), ()]);
    }

    #[test]
    fn opaque_values_cross_unread() {
        let v = (7u32, vec![1.5f64], "x".to_string());
        let mut wire = Vec::new();
        encode_opaque(&v, &mut wire);
        let held = OpaqueBytes::decode(&mut &wire[..]).unwrap();
        let mut back = Vec::new();
        held.encode(&mut back);
        assert_eq!(decode_opaque::<(u32, Vec<f64>, String)>(&mut &back[..]).unwrap(), v);
    }

    /// A fixed-width list really is one run: the count, the layout, the
    /// stride, then the keys back to back.
    #[test]
    fn fixed_width_keys_cross_as_one_run() {
        let mut wire = Vec::new();
        encode_keys(&[1u16, 0x0203], &mut wire);
        assert_eq!(wire, [2, STRIDE, 2, 0, 0, 0, 0, 1, 2, 3]);
    }

    #[test]
    fn a_malformed_key_list_is_refused() {
        // 3 keys of 4 bytes claimed, 8 bytes there.
        let bad = [3u8, STRIDE, 4, 0, 0, 0, 1, 2, 3, 4, 5, 6, 7, 8];
        assert!(decode_keys::<u32>(&mut &bad[..]).is_err());
        assert!(<Vec<KeyBytes>>::decode(&mut &bad[..]).is_err());
        // A stride the type doesn't fill exactly.
        let odd = [1u8, STRIDE, 5, 0, 0, 0, 1, 2, 3, 4, 5];
        assert!(decode_keys::<u32>(&mut &odd[..]).is_err());
        assert!(decode_keys::<u32>(&mut &[1u8, 9][..]).is_err(), "an unknown layout");
        // A stride times a count that overflows is refused, not wrapped.
        let mut huge = Vec::new();
        super::super::__send_value(&(u32::MAX as u64), &mut huge);
        huge.extend_from_slice(&[STRIDE, 0xff, 0xff, 0xff, 0xff]);
        huge.extend(std::iter::repeat_n(0u8, 64));
        assert!(decode_keys::<u32>(&mut &huge[..]).is_err());
    }

    /// A key frame that doesn't hold exactly one key is refused.
    #[test]
    fn a_malformed_key_frame_is_refused() {
        let mut wire = Vec::new();
        encode_key(&7u32, &mut wire);
        assert!(decode_key::<u16>(&mut &wire[..]).is_err(), "bytes left in the frame");
        assert!(decode_key::<u64>(&mut &wire[..]).is_err(), "the frame ends early");
        assert!(decode_key::<u32>(&mut &[9u8, 0, 0, 0, 1][..]).is_err(), "a frame longer than the input");
        assert!(decode_key::<u32>(&mut &[4u8, 0][..]).is_err(), "a length cut off");
    }
}
