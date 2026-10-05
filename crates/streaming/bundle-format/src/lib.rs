//! The release format of a remote-component bundle.
//!
//! A bundle is one `.wasm` file. Release builds (`idealyst build --remote`)
//! add two custom sections, which a wasm engine ignores:
//!
//! - [`METADATA_SECTION`] — [`Metadata`] as JSON: the bundle's name, the
//!   crate and version it was built from, and the codec version it encodes
//!   values with.
//! - [`SIGNATURE_SECTION`] — optional, and always the LAST section: an
//!   Ed25519 signature over everything before it (the module, metadata
//!   included), with the signing key's id.
//!
//! So a signature covers exactly the bytes an app will run, plus what the
//! bundle says it is, and nothing can be appended after it unnoticed.
//!
//! An app decides what it accepts with [`Trust`]: the public keys it trusts,
//! and whether a bundle must be signed by one of them. See
//! `remote_host::remote::install_with_options`.
//!
//! Keys are 32 bytes, written as hex (64 characters) wherever they are text
//! — the convention the `auto-update` SDK uses for its release signatures.

use std::fmt;

use ed25519_dalek::{Signer, Verifier};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

/// The custom section holding a bundle's [`Metadata`].
pub const METADATA_SECTION: &str = "idealyst.bundle";
/// The custom section holding a bundle's signature.
pub const SIGNATURE_SECTION: &str = "idealyst.signature";
/// Prefixed to what is signed, so a bundle signature can't be replayed as a
/// signature over anything else made with the same key.
const DOMAIN: &[u8] = b"idealyst remote bundle signature v1\0";
/// The signature section's layout version.
const SIGNATURE_V1: u8 = 1;

// ---------------------------------------------------------------------------
// Errors
// ---------------------------------------------------------------------------

/// The bytes aren't a wasm module this format can read.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FormatError(pub String);

impl fmt::Display for FormatError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "not a readable bundle: {}", self.0)
    }
}

impl std::error::Error for FormatError {}

/// Why an app refused a bundle ([`Trust::check`]).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TrustError {
    /// The app requires a signature and the bundle has none.
    Unsigned,
    /// Signed with a key the app doesn't trust.
    UnknownKey(KeyId),
    /// Signed by a trusted key, but the bytes changed after signing (or the
    /// signature is forged).
    Mismatch(KeyId),
    /// The signature section is malformed, or isn't the last section.
    Malformed(String),
}

impl fmt::Display for TrustError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            TrustError::Unsigned => write!(f, "the bundle is not signed, and this app requires a signature"),
            TrustError::UnknownKey(k) => write!(f, "the bundle is signed with key {k}, which this app doesn't trust"),
            TrustError::Mismatch(k) => {
                write!(f, "the bundle's signature (key {k}) doesn't match its contents: it was changed after signing")
            }
            TrustError::Malformed(m) => write!(f, "the bundle's signature is malformed: {m}"),
        }
    }
}

impl std::error::Error for TrustError {}

// ---------------------------------------------------------------------------
// Sections
// ---------------------------------------------------------------------------

/// One section of a module: where it is in the file, and its custom name
/// (`None` for the standard sections).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Section {
    /// The byte range of the whole section (id, size and contents).
    pub range: std::ops::Range<usize>,
    pub custom_name: Option<String>,
    /// The custom section's payload (after its name); empty otherwise.
    pub payload: std::ops::Range<usize>,
}

fn leb_u32(bytes: &[u8], at: &mut usize) -> Result<u32, FormatError> {
    let mut value: u32 = 0;
    for shift in (0..35).step_by(7) {
        let b = *bytes.get(*at).ok_or_else(|| FormatError("a length runs past the end".into()))?;
        *at += 1;
        value |= u32::from(b & 0x7f).checked_shl(shift).ok_or_else(|| FormatError("a length overflows".into()))?;
        if b & 0x80 == 0 {
            return Ok(value);
        }
    }
    Err(FormatError("a length is longer than 5 bytes".into()))
}

fn push_leb_u32(out: &mut Vec<u8>, mut v: u32) {
    loop {
        let b = (v & 0x7f) as u8;
        v >>= 7;
        if v == 0 {
            out.push(b);
            return;
        }
        out.push(b | 0x80);
    }
}

/// Every section of `wasm`, in order. Checks the header and that each
/// section lies inside the file; it doesn't validate the module (the
/// engine does, at load).
pub fn sections(wasm: &[u8]) -> Result<Vec<Section>, FormatError> {
    if wasm.len() < 8 || &wasm[..4] != b"\0asm" {
        return Err(FormatError("no wasm header".into()));
    }
    if wasm[4..8] != [1, 0, 0, 0] {
        return Err(FormatError("not wasm version 1".into()));
    }
    let mut out = Vec::new();
    let mut at = 8;
    while at < wasm.len() {
        let start = at;
        let id = wasm[at];
        at += 1;
        let size = leb_u32(wasm, &mut at)? as usize;
        let end = at.checked_add(size).filter(|&e| e <= wasm.len()).ok_or_else(|| FormatError("a section runs past the end".into()))?;
        let (custom_name, payload) = if id == 0 {
            let mut p = at;
            let len = leb_u32(wasm, &mut p)? as usize;
            let name_end = p.checked_add(len).filter(|&e| e <= end).ok_or_else(|| FormatError("a custom section's name runs past it".into()))?;
            let name = std::str::from_utf8(&wasm[p..name_end]).map_err(|_| FormatError("a custom section's name isn't UTF-8".into()))?;
            (Some(name.to_string()), name_end..end)
        } else {
            (None, end..end)
        };
        out.push(Section { range: start..end, custom_name, payload });
        at = end;
    }
    Ok(out)
}

fn custom_section(name: &str, payload: &[u8]) -> Vec<u8> {
    let mut body = Vec::new();
    push_leb_u32(&mut body, name.len() as u32);
    body.extend_from_slice(name.as_bytes());
    body.extend_from_slice(payload);
    let mut out = vec![0u8];
    push_leb_u32(&mut out, body.len() as u32);
    out.extend_from_slice(&body);
    out
}

/// `wasm` without its custom sections named in `names`.
fn without(wasm: &[u8], names: &[&str]) -> Result<Vec<u8>, FormatError> {
    let mut out = wasm[..8].to_vec();
    for s in sections(wasm)? {
        if s.custom_name.as_deref().is_some_and(|n| names.contains(&n)) {
            continue;
        }
        out.extend_from_slice(&wasm[s.range]);
    }
    Ok(out)
}

// ---------------------------------------------------------------------------
// Metadata
// ---------------------------------------------------------------------------

/// What a release bundle says it is.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Metadata {
    /// The bundle's name in the app's configuration (`shop`).
    pub name: String,
    /// The crate it was built from, and that crate's version.
    pub package: String,
    pub version: String,
    /// The value codec it was built with
    /// (`runtime_vocabulary::remote::CODEC_VERSION`); the loader checks the
    /// bundle's own export, this is for people and tools reading the file.
    pub codec: u32,
}

/// `wasm` with `metadata` as its metadata section, replacing any earlier
/// one. Any signature is dropped: it no longer covers the bytes.
pub fn with_metadata(wasm: &[u8], metadata: &Metadata) -> Result<Vec<u8>, FormatError> {
    let mut out = without(wasm, &[METADATA_SECTION, SIGNATURE_SECTION])?;
    let json = serde_json::to_vec(metadata).expect("metadata serializes");
    out.extend_from_slice(&custom_section(METADATA_SECTION, &json));
    Ok(out)
}

/// The bundle's metadata section, if it has one.
pub fn metadata(wasm: &[u8]) -> Result<Option<Metadata>, FormatError> {
    for s in sections(wasm)? {
        if s.custom_name.as_deref() == Some(METADATA_SECTION) {
            return serde_json::from_slice(&wasm[s.payload]).map(Some).map_err(|e| FormatError(format!("bad metadata: {e}")));
        }
    }
    Ok(None)
}

/// The SHA-256 of the whole file, hex: what a server or cache keys a bundle
/// by.
pub fn content_hash(wasm: &[u8]) -> String {
    hex(&Sha256::digest(wasm))
}

// ---------------------------------------------------------------------------
// Keys
// ---------------------------------------------------------------------------

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

fn unhex<const N: usize>(text: &str) -> Option<[u8; N]> {
    let text = text.trim();
    if text.len() != N * 2 || !text.is_ascii() {
        return None;
    }
    let mut out = [0u8; N];
    for (i, b) in out.iter_mut().enumerate() {
        *b = u8::from_str_radix(&text[i * 2..i * 2 + 2], 16).ok()?;
    }
    Some(out)
}

/// A key's short id: the first 8 bytes of the SHA-256 of its public key,
/// hex. Named in errors, and stored in a signature so the app knows which
/// of its keys to check.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct KeyId(pub [u8; 8]);

impl fmt::Display for KeyId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&hex(&self.0))
    }
}

/// A public key an app trusts.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PublicKey([u8; 32]);

impl PublicKey {
    /// From its 32 bytes. `Err` if they aren't a valid Ed25519 point.
    pub fn from_bytes(bytes: [u8; 32]) -> Result<PublicKey, String> {
        ed25519_dalek::VerifyingKey::from_bytes(&bytes).map_err(|_| "not an Ed25519 public key".to_string())?;
        Ok(PublicKey(bytes))
    }

    /// From its 64-character hex form (what `idealyst remote keygen`
    /// prints).
    pub fn from_hex(text: &str) -> Result<PublicKey, String> {
        PublicKey::from_bytes(unhex::<32>(text).ok_or("a public key is 64 hex characters")?)
    }

    pub fn to_hex(&self) -> String {
        hex(&self.0)
    }

    pub fn bytes(&self) -> [u8; 32] {
        self.0
    }

    pub fn id(&self) -> KeyId {
        KeyId(Sha256::digest(self.0)[..8].try_into().expect("8 bytes"))
    }
}

/// A private signing key. Keep it out of the app and out of version
/// control: whoever has it can sign bundles the app will run.
pub struct SigningKey(ed25519_dalek::SigningKey);

impl SigningKey {
    /// A new random key, from the OS's random source.
    pub fn generate() -> Result<SigningKey, String> {
        let mut seed = [0u8; 32];
        getrandom::getrandom(&mut seed).map_err(|e| format!("no random source: {e}"))?;
        Ok(SigningKey(ed25519_dalek::SigningKey::from_bytes(&seed)))
    }

    /// From its 64-character hex form (a key file's contents).
    pub fn from_hex(text: &str) -> Result<SigningKey, String> {
        Ok(SigningKey(ed25519_dalek::SigningKey::from_bytes(&unhex::<32>(text).ok_or("a signing key is 64 hex characters")?)))
    }

    pub fn to_hex(&self) -> String {
        hex(self.0.as_bytes())
    }

    pub fn public(&self) -> PublicKey {
        PublicKey(self.0.verifying_key().to_bytes())
    }
}

impl fmt::Debug for SigningKey {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "SigningKey({})", self.public().id())
    }
}

// ---------------------------------------------------------------------------
// Signing and verifying
// ---------------------------------------------------------------------------

fn message(unsigned: &[u8]) -> Vec<u8> {
    let mut m = Vec::with_capacity(DOMAIN.len() + unsigned.len());
    m.extend_from_slice(DOMAIN);
    m.extend_from_slice(unsigned);
    m
}

/// `wasm` signed with `key`: any earlier signature is replaced, and the new
/// one covers every other byte.
pub fn sign(wasm: &[u8], key: &SigningKey) -> Result<Vec<u8>, FormatError> {
    let mut out = without(wasm, &[SIGNATURE_SECTION])?;
    let signature = key.0.sign(&message(&out));
    let mut payload = vec![SIGNATURE_V1];
    payload.extend_from_slice(&key.public().id().0);
    payload.extend_from_slice(&signature.to_bytes());
    out.extend_from_slice(&custom_section(SIGNATURE_SECTION, &payload));
    Ok(out)
}

/// A bundle's signature, as read from the file.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Signature {
    pub key: KeyId,
    bytes: [u8; 64],
    /// Where the signed bytes end (the signature section's start).
    signed_len: usize,
}

/// The bundle's signature, if it has one. `Err` when a signature section is
/// there but malformed, or isn't the last section.
pub fn signature(wasm: &[u8]) -> Result<Option<Signature>, TrustError> {
    let all = sections(wasm).map_err(|e| TrustError::Malformed(e.0))?;
    let Some(i) = all.iter().position(|s| s.custom_name.as_deref() == Some(SIGNATURE_SECTION)) else {
        return Ok(None);
    };
    if i != all.len() - 1 {
        return Err(TrustError::Malformed("sections follow the signature".into()));
    }
    let s = &all[i];
    let p = &wasm[s.payload.clone()];
    if p.len() != 1 + 8 + 64 || p[0] != SIGNATURE_V1 {
        return Err(TrustError::Malformed(format!("{} bytes, layout {}", p.len(), p.first().copied().unwrap_or(0))));
    }
    Ok(Some(Signature {
        key: KeyId(p[1..9].try_into().expect("8 bytes")),
        bytes: p[9..].try_into().expect("64 bytes"),
        signed_len: s.range.start,
    }))
}

/// Check `wasm`'s signature against `keys`: `Ok` with the key that signed
/// it.
pub fn verify(wasm: &[u8], keys: &[PublicKey]) -> Result<KeyId, TrustError> {
    let sig = signature(wasm)?.ok_or(TrustError::Unsigned)?;
    let key = keys.iter().find(|k| k.id() == sig.key).ok_or(TrustError::UnknownKey(sig.key))?;
    let verifying = ed25519_dalek::VerifyingKey::from_bytes(&key.0).expect("checked at construction");
    let signature = ed25519_dalek::Signature::from_bytes(&sig.bytes);
    verifying.verify(&message(&wasm[..sig.signed_len]), &signature).map_err(|_| TrustError::Mismatch(sig.key))?;
    Ok(sig.key)
}

/// What an app accepts. The default trusts nothing and requires nothing:
/// every bundle loads, signed or not (development, and apps that ship their
/// bundles inside the app).
#[derive(Debug, Clone, Default)]
pub struct Trust {
    keys: Vec<PublicKey>,
    require: bool,
}

impl Trust {
    /// Accept bundles signed by `key` (call again for more: a key being
    /// rotated out and its replacement).
    pub fn key(mut self, key: PublicKey) -> Trust {
        self.keys.push(key);
        self
    }

    /// Refuse a bundle that isn't signed by one of the trusted keys.
    pub fn require_signature(mut self) -> Trust {
        self.require = true;
        self
    }

    /// Whether `wasm` may load: `Ok(Some(key))` when a trusted key signed
    /// it, `Ok(None)` when it may load unsigned (nothing is required).
    ///
    /// - Required, unsigned, or signed by an unknown key: refused.
    /// - Not required, but the app has keys and the bundle is signed: its
    ///   signature must still check out. A bundle that claims a trusted
    ///   key and doesn't match it was tampered with, whatever the policy.
    /// - Not required and no keys: anything loads.
    pub fn check(&self, wasm: &[u8]) -> Result<Option<KeyId>, TrustError> {
        if self.require {
            return verify(wasm, &self.keys).map(Some);
        }
        match signature(wasm)? {
            Some(sig) if self.keys.iter().any(|k| k.id() == sig.key) => verify(wasm, &self.keys).map(Some),
            _ => Ok(None),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A minimal valid module with a type and a function section, and a
    /// custom section in the middle.
    fn module() -> Vec<u8> {
        let mut m = b"\0asm\x01\0\0\0".to_vec();
        m.extend_from_slice(&[1, 4, 1, 0x60, 0, 0]); // type: () -> ()
        m.extend_from_slice(&custom_section("name", b"\x00\x01x"));
        m.extend_from_slice(&[3, 2, 1, 0]); // function 0 has type 0
        m.extend_from_slice(&[10, 4, 1, 2, 0, 0x0b]); // code: empty body
        m
    }

    fn meta() -> Metadata {
        Metadata { name: "shop".into(), package: "shop-screens".into(), version: "1.2.0".into(), codec: 2 }
    }

    #[test]
    fn sections_are_read_in_order() {
        let s = sections(&module()).unwrap();
        assert_eq!(s.len(), 4);
        assert_eq!(s[1].custom_name.as_deref(), Some("name"));
        assert!(s.iter().all(|s| s.range.end <= module().len()));
        assert!(sections(b"\0asm\x01\0\0\0\x01\x09").is_err(), "a section past the end");
        assert!(sections(b"nope").is_err());
        assert!(sections(b"\0asm\x02\0\0\0").is_err());
    }

    #[test]
    fn metadata_round_trips_and_replaces() {
        let m = with_metadata(&module(), &meta()).unwrap();
        assert_eq!(metadata(&m).unwrap(), Some(meta()));
        let again = with_metadata(&m, &Metadata { version: "1.3.0".into(), ..meta() }).unwrap();
        assert_eq!(metadata(&again).unwrap().unwrap().version, "1.3.0");
        assert_eq!(sections(&again).unwrap().iter().filter(|s| s.custom_name.as_deref() == Some(METADATA_SECTION)).count(), 1);
        assert_eq!(metadata(&module()).unwrap(), None);
    }

    #[test]
    fn a_signed_bundle_verifies_with_its_key() {
        let key = SigningKey::generate().unwrap();
        let signed = sign(&with_metadata(&module(), &meta()).unwrap(), &key).unwrap();
        assert_eq!(verify(&signed, &[key.public()]), Ok(key.public().id()));
        assert_eq!(metadata(&signed).unwrap(), Some(meta()), "signing keeps the metadata");
        // The signature is the last section.
        let last = sections(&signed).unwrap().pop().unwrap();
        assert_eq!(last.custom_name.as_deref(), Some(SIGNATURE_SECTION));
    }

    /// Every byte before the signature is covered: code, a custom section,
    /// the metadata.
    #[test]
    fn any_change_after_signing_is_a_mismatch() {
        let key = SigningKey::generate().unwrap();
        let signed = sign(&with_metadata(&module(), &meta()).unwrap(), &key).unwrap();
        let sig_start = sections(&signed).unwrap().last().unwrap().range.start;
        for at in 8..sig_start {
            let mut t = signed.clone();
            t[at] ^= 0x01;
            // A flipped length byte can make the file unreadable instead:
            // refused either way.
            assert!(verify(&t, &[key.public()]).is_err(), "byte {at} changed and still verified");
        }
        // A different metadata section, re-attached under the old
        // signature.
        let other = with_metadata(&signed, &Metadata { name: "evil".into(), ..meta() }).unwrap();
        let sig = &signed[sig_start..];
        let mut forged = other.clone();
        forged.extend_from_slice(sig);
        assert_eq!(verify(&forged, &[key.public()]), Err(TrustError::Mismatch(key.public().id())));
    }

    #[test]
    fn an_unknown_key_or_no_signature_is_refused() {
        let (a, b) = (SigningKey::generate().unwrap(), SigningKey::generate().unwrap());
        let signed = sign(&module(), &a).unwrap();
        assert_eq!(verify(&signed, &[b.public()]), Err(TrustError::UnknownKey(a.public().id())));
        assert_eq!(verify(&module(), &[a.public()]), Err(TrustError::Unsigned));
        // Rotation: either of two trusted keys.
        assert!(verify(&signed, &[b.public(), a.public()]).is_ok());
    }

    #[test]
    fn signing_again_replaces_the_signature() {
        let (a, b) = (SigningKey::generate().unwrap(), SigningKey::generate().unwrap());
        let twice = sign(&sign(&module(), &a).unwrap(), &b).unwrap();
        assert_eq!(verify(&twice, &[a.public(), b.public()]), Ok(b.public().id()));
        assert_eq!(
            sections(&twice).unwrap().iter().filter(|s| s.custom_name.as_deref() == Some(SIGNATURE_SECTION)).count(),
            1
        );
    }

    /// Nothing may follow the signature: appended bytes would be outside
    /// what was signed.
    #[test]
    fn a_section_after_the_signature_is_refused() {
        let key = SigningKey::generate().unwrap();
        let mut signed = sign(&module(), &key).unwrap();
        signed.extend_from_slice(&custom_section("extra", b"x"));
        assert!(matches!(verify(&signed, &[key.public()]), Err(TrustError::Malformed(_))));
    }

    #[test]
    fn trust_policies() {
        let (a, b) = (SigningKey::generate().unwrap(), SigningKey::generate().unwrap());
        let signed_a = sign(&module(), &a).unwrap();
        let signed_b = sign(&module(), &b).unwrap();
        let unsigned = module();
        let mut tampered = signed_a.clone();
        tampered[10] ^= 1;

        // Default: anything loads.
        let open = Trust::default();
        assert_eq!(open.check(&unsigned), Ok(None));
        assert_eq!(open.check(&signed_a), Ok(None));

        // Required: only a valid signature by a trusted key.
        let strict = Trust::default().key(a.public()).require_signature();
        assert_eq!(strict.check(&signed_a), Ok(Some(a.public().id())));
        assert_eq!(strict.check(&unsigned), Err(TrustError::Unsigned));
        assert_eq!(strict.check(&signed_b), Err(TrustError::UnknownKey(b.public().id())));
        assert!(strict.check(&tampered).is_err());

        // Keys but not required: unsigned and unknown-key bundles load, a
        // bundle claiming a trusted key must match it.
        let lenient = Trust::default().key(a.public());
        assert_eq!(lenient.check(&unsigned), Ok(None));
        assert_eq!(lenient.check(&signed_b), Ok(None));
        assert_eq!(lenient.check(&signed_a), Ok(Some(a.public().id())));
        assert!(lenient.check(&tampered).is_err());
    }

    #[test]
    fn keys_round_trip_through_hex() {
        let key = SigningKey::generate().unwrap();
        let again = SigningKey::from_hex(&key.to_hex()).unwrap();
        assert_eq!(again.public(), key.public());
        assert_eq!(PublicKey::from_hex(&key.public().to_hex()).unwrap(), key.public());
        assert!(PublicKey::from_hex("abc").is_err());
        assert!(SigningKey::from_hex(&"zz".repeat(32)).is_err());
        assert_eq!(key.public().id().to_string().len(), 16);
        assert!(!format!("{key:?}").contains(&key.to_hex()), "Debug never prints the private key");
    }

    #[test]
    fn content_hash_is_sha256_hex() {
        assert_eq!(content_hash(b""), "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855");
    }
}
