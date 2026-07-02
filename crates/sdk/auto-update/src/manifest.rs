//! The signed release manifest — the portable, platform-agnostic core of the
//! updater. Everything here is pure Rust with no platform dependency, so it is
//! fully unit-tested below.
//!
//! A manifest is JSON published by `idealyst publish` (see the crate-level
//! docs) and fetched by the running app. It lists, per channel, the available
//! [`Release`]s across platforms. The app:
//!
//! 1. [`select`](ReleaseManifest::select)s the entry matching its own
//!    [`Platform`] / [`Arch`],
//! 2. checks the entry's Ed25519 [`verify`](Release::verify) signature against
//!    the public key baked into the app at build time,
//! 3. asks [`is_newer_than`](Release::is_newer_than) whether it beats what's
//!    currently running.
//!
//! Only if all three pass is an update offered. TLS is *not* the trust anchor
//! here — a compromised CDN still can't ship a release the app will install,
//! because it can't forge the signature.

use serde::{Deserialize, Serialize};

/// The desktop platform a [`Release`] targets. Matches
/// `runtime_core::Platform`'s desktop variants; kept local so the manifest
/// format doesn't couple to the framework enum's wire layout.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Platform {
    /// macOS (Developer ID / direct distribution).
    MacOs,
    /// Windows (MSIX / installer).
    Windows,
    /// Linux (AppImage).
    Linux,
}

impl Platform {
    /// The platform this build is running on, or `None` on targets that never
    /// self-update (iOS, Android, web). Used to pick the right [`Release`].
    pub fn current() -> Option<Platform> {
        #[cfg(all(target_os = "macos", not(target_arch = "wasm32")))]
        {
            Some(Platform::MacOs)
        }
        #[cfg(all(target_os = "windows", not(target_arch = "wasm32")))]
        {
            Some(Platform::Windows)
        }
        #[cfg(all(target_os = "linux", not(target_arch = "wasm32")))]
        {
            Some(Platform::Linux)
        }
        #[cfg(not(any(
            all(target_os = "macos", not(target_arch = "wasm32")),
            all(target_os = "windows", not(target_arch = "wasm32")),
            all(target_os = "linux", not(target_arch = "wasm32")),
        )))]
        {
            None
        }
    }
}

/// CPU architecture of a [`Release`] artifact. `None` on an entry means the
/// artifact is architecture-neutral (e.g. a macOS universal binary), so it
/// matches any [`current`](Arch::current).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Arch {
    /// 64-bit ARM (Apple Silicon, aarch64).
    Arm64,
    /// 64-bit x86 (Intel/AMD, x86_64).
    X64,
}

impl Arch {
    /// The architecture this build was compiled for.
    pub fn current() -> Option<Arch> {
        #[cfg(target_arch = "aarch64")]
        {
            Some(Arch::Arm64)
        }
        #[cfg(target_arch = "x86_64")]
        {
            Some(Arch::X64)
        }
        #[cfg(not(any(target_arch = "aarch64", target_arch = "x86_64")))]
        {
            None
        }
    }
}

/// One installable release for one platform (and optionally one architecture).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Release {
    /// Which platform this artifact is for.
    pub platform: Platform,
    /// Which architecture, or `None` for an arch-neutral artifact (matches any).
    #[serde(default)]
    pub arch: Option<Arch>,
    /// User-visible semantic version (`CFBundleShortVersionString` on macOS).
    pub version: String,
    /// Monotonic build number (`CFBundleVersion`). The authoritative "is this
    /// newer?" tiebreaker when two builds share a `version`.
    pub build: u64,
    /// Absolute URL of the artifact (`.dmg` / `.msix` / `.AppImage`).
    pub url: String,
    /// Lowercase hex SHA-256 of the artifact. The apply step re-hashes the
    /// download and refuses to install on mismatch.
    pub sha256: String,
    /// Hex-encoded Ed25519 signature over this entry's canonical message
    /// (see [`Release::signing_message`]). 64 bytes → 128 hex chars.
    pub signature: String,
    /// Minimum OS version required to install (e.g. `"11.0"`). Entries the
    /// running OS is too old for are filtered out during [`select`].
    #[serde(default)]
    pub min_os: Option<String>,
    /// URL of human-readable release notes to surface in the update prompt.
    #[serde(default)]
    pub notes_url: Option<String>,
    /// When true, the app should refuse to keep running the old version
    /// (forced update). Author policy decides how hard to enforce.
    #[serde(default)]
    pub mandatory: bool,
    /// Staged-rollout fraction in `0.0..=1.0`. `None` means 100%. A client
    /// self-selects into the rollout via a stable per-install hash (see
    /// [`Release::in_rollout`]) so a bad release can be limited to a slice of
    /// users before going wide.
    #[serde(default)]
    pub rollout: Option<f32>,
}

impl Release {
    /// The exact byte string the [`signature`](Release::signature) is computed
    /// over. Deterministic and canonical: any change to version, build, url,
    /// or digest invalidates the signature. `publish` signs this same string.
    pub fn signing_message(&self) -> Vec<u8> {
        // Newline-delimited so no field can bleed into the next; the digest
        // pins the artifact bytes, the url pins where they come from.
        format!("{}\n{}\n{}\n{}", self.version, self.build, self.url, self.sha256).into_bytes()
    }

    /// Verify this entry's signature against the app's embedded public key.
    /// A failure here means the manifest was tampered with (or signed by the
    /// wrong key) — the release must not be installed.
    pub fn verify(&self, public_key: &[u8; 32]) -> Result<(), SignatureError> {
        use ed25519_dalek::{Signature, Verifier, VerifyingKey};

        let key = VerifyingKey::from_bytes(public_key).map_err(|_| SignatureError::BadKey)?;
        let sig_bytes = hex_decode(&self.signature).ok_or(SignatureError::Malformed)?;
        let sig_arr: [u8; 64] = sig_bytes.as_slice().try_into().map_err(|_| SignatureError::Malformed)?;
        let sig = Signature::from_bytes(&sig_arr);
        key.verify(&self.signing_message(), &sig)
            .map_err(|_| SignatureError::Mismatch)
    }

    /// Whether this release is strictly newer than the running `(version,
    /// build)`. Compares semver first; on equal versions the higher `build`
    /// wins (a re-spin of the same version). A malformed version string sorts
    /// as *not newer* — we never offer an update we can't reason about.
    pub fn is_newer_than(&self, current_version: &str, current_build: u64) -> bool {
        use semver::Version;
        match (Version::parse(&self.version), Version::parse(current_version)) {
            (Ok(theirs), Ok(mine)) => theirs > mine || (theirs == mine && self.build > current_build),
            _ => false,
        }
    }

    /// Whether this install falls inside the staged-rollout slice. `bucket` is
    /// a stable per-install value in `0.0..=1.0` (e.g. derived by hashing a
    /// persistent install id) so a given machine's membership doesn't flip
    /// between checks. `None` rollout ⇒ always in.
    pub fn in_rollout(&self, bucket: f32) -> bool {
        match self.rollout {
            None => true,
            Some(frac) => bucket <= frac.clamp(0.0, 1.0),
        }
    }
}

/// The full manifest for one release channel.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ReleaseManifest {
    /// Schema version of this document, so the client can reject a format it
    /// doesn't understand rather than mis-parse it.
    #[serde(default = "default_schema")]
    pub schema: u32,
    /// The channel these releases belong to (`"stable"`, `"beta"`, …). The
    /// client fetches the manifest for the channel it's subscribed to.
    pub channel: String,
    /// Every available release across platforms/arches for this channel.
    pub releases: Vec<Release>,
}

fn default_schema() -> u32 {
    1
}

/// The schema version this build knows how to read.
pub const SCHEMA_VERSION: u32 = 1;

impl ReleaseManifest {
    /// Parse a manifest from its JSON bytes.
    pub fn parse(bytes: &[u8]) -> Result<ReleaseManifest, ManifestError> {
        let m: ReleaseManifest = serde_json::from_slice(bytes).map_err(|e| ManifestError::Parse(e.to_string()))?;
        if m.schema > SCHEMA_VERSION {
            return Err(ManifestError::UnsupportedSchema(m.schema));
        }
        Ok(m)
    }

    /// The best [`Release`] for the given platform/arch, or `None` if this
    /// manifest has nothing for it. Prefers an arch-specific artifact over an
    /// arch-neutral one, then the highest `(version, build)`.
    pub fn select(&self, platform: Platform, arch: Option<Arch>) -> Option<&Release> {
        self.releases
            .iter()
            .filter(|r| r.platform == platform)
            .filter(|r| match (r.arch, arch) {
                // Arch-neutral artifact matches anything; a specific artifact
                // must match the running arch (or the running arch is unknown).
                (None, _) => true,
                (Some(_), None) => true,
                (Some(a), Some(b)) => a == b,
            })
            .max_by(|a, b| {
                // Arch-specific beats arch-neutral, then newer beats older.
                let arch_rank = |r: &Release| u8::from(r.arch.is_some());
                arch_rank(a)
                    .cmp(&arch_rank(b))
                    .then_with(|| version_key(a).cmp(&version_key(b)))
            })
    }
}

/// Total-order sort key `(semver-or-min, build)` for ranking releases.
fn version_key(r: &Release) -> (semver::Version, u64) {
    let v = semver::Version::parse(&r.version).unwrap_or_else(|_| semver::Version::new(0, 0, 0));
    (v, r.build)
}

/// Decode a lowercase/uppercase hex string to bytes; `None` on any non-hex
/// char or odd length. Kept dependency-free — the only hex we touch is short.
fn hex_decode(s: &str) -> Option<Vec<u8>> {
    if s.len() % 2 != 0 {
        return None;
    }
    fn nibble(c: u8) -> Option<u8> {
        match c {
            b'0'..=b'9' => Some(c - b'0'),
            b'a'..=b'f' => Some(c - b'a' + 10),
            b'A'..=b'F' => Some(c - b'A' + 10),
            _ => None,
        }
    }
    let bytes = s.as_bytes();
    let mut out = Vec::with_capacity(bytes.len() / 2);
    for pair in bytes.chunks_exact(2) {
        out.push((nibble(pair[0])? << 4) | nibble(pair[1])?);
    }
    Some(out)
}

/// Lowercase-hex SHA-256 of `data`. Used by the apply step to confirm a
/// downloaded artifact matches the signed [`Release::sha256`].
pub fn sha256_hex(data: &[u8]) -> String {
    use sha2::{Digest, Sha256};
    let digest = Sha256::digest(data);
    let mut s = String::with_capacity(64);
    for b in digest {
        s.push(char::from_digit((b >> 4) as u32, 16).unwrap());
        s.push(char::from_digit((b & 0xf) as u32, 16).unwrap());
    }
    s
}

/// Why a manifest couldn't be understood.
#[derive(Debug, thiserror::Error, PartialEq)]
pub enum ManifestError {
    /// The JSON didn't parse into the expected shape.
    #[error("manifest parse error: {0}")]
    Parse(String),
    /// The manifest declares a schema newer than this build can read.
    #[error("unsupported manifest schema version {0}")]
    UnsupportedSchema(u32),
}

/// Why a release signature check failed. Every variant means "do not install".
#[derive(Debug, thiserror::Error, PartialEq)]
pub enum SignatureError {
    /// The embedded public key wasn't a valid Ed25519 key.
    #[error("invalid public key")]
    BadKey,
    /// The signature field wasn't valid hex / wasn't 64 bytes.
    #[error("malformed signature")]
    Malformed,
    /// The signature didn't match the entry — tampered or wrong signing key.
    #[error("signature does not verify")]
    Mismatch,
}

#[cfg(test)]
mod tests {
    use super::*;
    use ed25519_dalek::{Signer, SigningKey};

    fn hex_encode(bytes: &[u8]) -> String {
        let mut s = String::with_capacity(bytes.len() * 2);
        for b in bytes {
            s.push(char::from_digit((b >> 4) as u32, 16).unwrap());
            s.push(char::from_digit((b & 0xf) as u32, 16).unwrap());
        }
        s
    }

    fn signing_key() -> SigningKey {
        // Deterministic key so the test needs no RNG.
        SigningKey::from_bytes(&[7u8; 32])
    }

    fn signed_release(version: &str, build: u64, platform: Platform) -> (Release, [u8; 32]) {
        let key = signing_key();
        let pubkey = key.verifying_key().to_bytes();
        let mut r = Release {
            platform,
            arch: None,
            version: version.into(),
            build,
            url: format!("https://cdn.example.com/app-{version}.dmg"),
            sha256: "ab".repeat(32),
            signature: String::new(),
            min_os: None,
            notes_url: None,
            mandatory: false,
            rollout: None,
        };
        let sig = key.sign(&r.signing_message());
        r.signature = hex_encode(&sig.to_bytes());
        (r, pubkey)
    }

    #[test]
    fn signature_round_trips() {
        let (r, pubkey) = signed_release("1.2.0", 42, Platform::MacOs);
        assert_eq!(r.verify(&pubkey), Ok(()));
    }

    #[test]
    fn tampered_url_fails_verification() {
        let (mut r, pubkey) = signed_release("1.2.0", 42, Platform::MacOs);
        r.url = "https://evil.example.com/pwned.dmg".into();
        assert_eq!(r.verify(&pubkey), Err(SignatureError::Mismatch));
    }

    #[test]
    fn tampered_digest_fails_verification() {
        let (mut r, pubkey) = signed_release("1.2.0", 42, Platform::MacOs);
        r.sha256 = "cd".repeat(32);
        assert_eq!(r.verify(&pubkey), Err(SignatureError::Mismatch));
    }

    #[test]
    fn malformed_signature_is_rejected_not_panicked() {
        let (mut r, pubkey) = signed_release("1.2.0", 42, Platform::MacOs);
        r.signature = "not-hex".into();
        assert_eq!(r.verify(&pubkey), Err(SignatureError::Malformed));
    }

    #[test]
    fn newer_by_semver_and_by_build() {
        let (r, _) = signed_release("1.2.0", 10, Platform::MacOs);
        assert!(r.is_newer_than("1.1.9", 999)); // higher version wins regardless of build
        assert!(r.is_newer_than("1.2.0", 9)); // same version, higher build wins
        assert!(!r.is_newer_than("1.2.0", 10)); // identical: not newer
        assert!(!r.is_newer_than("1.2.0", 11)); // older build of same version
        assert!(!r.is_newer_than("2.0.0", 0)); // downgrade is never offered
    }

    #[test]
    fn unparseable_current_version_is_never_newer() {
        let (r, _) = signed_release("1.2.0", 10, Platform::MacOs);
        assert!(!r.is_newer_than("garbage", 0));
    }

    #[test]
    fn select_prefers_arch_specific_then_newest() {
        let (neutral, _) = signed_release("1.0.0", 1, Platform::MacOs);
        let mut arm = neutral.clone();
        arm.arch = Some(Arch::Arm64);
        arm.version = "1.0.0".into();
        let manifest = ReleaseManifest {
            schema: 1,
            channel: "stable".into(),
            releases: vec![neutral.clone(), arm.clone()],
        };
        // With a matching arch, the arch-specific artifact wins the tie.
        let chosen = manifest.select(Platform::MacOs, Some(Arch::Arm64)).unwrap();
        assert_eq!(chosen.arch, Some(Arch::Arm64));
        // A different platform selects nothing.
        assert!(manifest.select(Platform::Windows, None).is_none());
    }

    #[test]
    fn rollout_gating() {
        let (mut r, _) = signed_release("1.2.0", 42, Platform::MacOs);
        r.rollout = Some(0.25);
        assert!(r.in_rollout(0.10)); // inside the 25% slice
        assert!(!r.in_rollout(0.80)); // outside it
        r.rollout = None;
        assert!(r.in_rollout(0.99)); // no rollout ⇒ everyone
    }

    #[test]
    fn parse_rejects_future_schema() {
        let json = br#"{"schema":999,"channel":"stable","releases":[]}"#;
        assert_eq!(
            ReleaseManifest::parse(json),
            Err(ManifestError::UnsupportedSchema(999))
        );
    }

    #[test]
    fn parse_roundtrips_real_manifest() {
        let (r, _) = signed_release("1.2.0", 42, Platform::MacOs);
        let manifest = ReleaseManifest {
            schema: 1,
            channel: "stable".into(),
            releases: vec![r],
        };
        let json = serde_json::to_vec(&manifest).unwrap();
        assert_eq!(ReleaseManifest::parse(&json).unwrap(), manifest);
    }

    #[test]
    fn sha256_hex_matches_known_vector() {
        // SHA-256 of the empty string.
        assert_eq!(
            sha256_hex(b""),
            "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855"
        );
    }
}
