//! `#[host_fn]`'s records: an app function a bundle calls by name.
//!
//! In the app a `#[host_fn]` is the real function plus a [`HostFnDef`] the
//! app lists in its allowlist (`stream_host::remote::install_with`). In a
//! bundle it's a stub calling ONE wasm import, under [`HOST_FN_MODULE`],
//! named [`import_name`]`(path, schema)` — so the bundle's import section is
//! its list of host functions, and the app refuses a bundle at load that
//! calls one it doesn't export, or exports with a different signature.
//!
//! These are the records the bridged loader (`stream_host::kernel`) checks a
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
// know ends. The app decodes them as the `RemoteValue`s below; a bundle's
// stub, which has the real types, writes and reads them with these
// functions — so the two agree byte for byte.

use super::RemoteValue;
use crate::host_types::{Key, KeyBytes, OpaqueBytes};

fn frame(bytes: &[u8], out: &mut Vec<u8>) {
    super::__send_value(&(bytes.len() as u64), out);
    out.extend_from_slice(bytes);
}

fn unframe<'a>(input: &mut &'a [u8]) -> Result<&'a [u8], String> {
    let n = super::__try_receive_value::<u64>(input)?;
    let n = usize::try_from(n).ok().filter(|&n| n <= input.len()).ok_or_else(|| format!("a value claims {n} bytes with {} left", input.len()))?;
    let (head, tail) = input.split_at(n);
    *input = tail;
    Ok(head)
}

impl RemoteValue for KeyBytes {
    fn encode(&self, out: &mut Vec<u8>) {
        frame(self.as_bytes(), out)
    }
    fn decode(input: &mut &[u8]) -> Result<Self, String> {
        Ok(KeyBytes::from_raw(unframe(input)?.to_vec()))
    }
}

impl RemoteValue for OpaqueBytes {
    fn encode(&self, out: &mut Vec<u8>) {
        frame(self.as_raw(), out)
    }
    fn decode(input: &mut &[u8]) -> Result<Self, String> {
        Ok(OpaqueBytes::from_raw(unframe(input)?.to_vec()))
    }
}

/// A bundle's `K` as the app's `KeyBytes` reads it.
#[doc(hidden)]
pub fn encode_key<K: Key>(k: &K, out: &mut Vec<u8>) {
    let mut bytes = Vec::new();
    k.encode_key(&mut bytes);
    frame(&bytes, out)
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
    let mut bytes = Vec::new();
    v.encode(&mut bytes);
    frame(&bytes, out)
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
        let keys = vec![(3u16, "b".to_string()), (1, "a\0".to_string()), (3, "a".to_string())];
        let mut wire = Vec::new();
        super::super::__send_value(&(keys.len() as u64), &mut wire);
        for k in &keys {
            encode_key(k, &mut wire);
        }
        let mut app: Vec<KeyBytes> = RemoteValue::decode(&mut &wire[..]).unwrap();
        assert_eq!(app, keys.iter().map(KeyBytes::of).collect::<Vec<_>>());
        app.sort();
        let mut reply = Vec::new();
        app.encode(&mut reply);
        let mut input = &reply[..];
        let n = super::super::__try_receive_value::<u64>(&mut input).unwrap();
        let back: Vec<(u16, String)> = (0..n).map(|_| decode_key(&mut input).unwrap()).collect();
        let mut want = keys.clone();
        want.sort();
        assert_eq!(back, want, "sorting the bytes sorted the keys");
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

    /// A key frame that doesn't hold exactly one key is refused.
    #[test]
    fn a_malformed_key_frame_is_refused() {
        let mut wire = Vec::new();
        encode_key(&7u32, &mut wire);
        assert!(decode_key::<u16>(&mut &wire[..]).is_err(), "bytes left in the frame");
        assert!(decode_key::<u64>(&mut &wire[..]).is_err(), "the frame ends early");
        assert!(decode_key::<u32>(&mut &[9u8, 1][..]).is_err(), "a frame longer than the input");
    }
}
