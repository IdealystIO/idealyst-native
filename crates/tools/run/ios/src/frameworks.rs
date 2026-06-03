//! Apple-framework discovery for the iOS build, derived from the
//! dependency graph instead of being hardcoded.
//!
//! # Why this exists
//!
//! The objc2 Rust bindings resolve Objective-C *classes* lazily at runtime
//! via `objc_getClass` — so importing `RPScreenRecorder` from the
//! `screen-recorder` SDK compiles and links fine even when ReplayKit isn't
//! passed to the linker. But the class is only *loaded* into the runtime if
//! its framework is linked into the binary. With ReplayKit absent,
//! `class!(RPScreenRecorder)` panics at first use ("class not found") and the
//! app crashes the moment it tries to record. The C-symbol frameworks
//! (CoreMedia/CoreVideo) fail even earlier — at link time — because their
//! functions are direct undefined symbols.
//!
//! Previously the framework list was hardcoded to the camera demo's set
//! (AVFoundation/CoreMedia/CoreVideo) in both the device pbxproj template and
//! the simulator `swiftc` invocation, so a screen-capture app silently
//! shipped without ReplayKit. The fix mirrors how Android already derives its
//! per-SDK Kotlin/Java sources: each SDK declares the frameworks it needs in
//! its own `Cargo.toml`,
//!
//! ```toml
//! [package.metadata.idealyst.ios]
//! frameworks = ["ReplayKit", "CoreMedia", "CoreVideo"]
//! ```
//!
//! and [`collect_ios_frameworks`] walks the wrapper's `cargo metadata` (whose
//! dependency closure is exactly what the app links) and unions those into a
//! fixed base set.
//!
//! # Weak vs strong
//!
//! UIKit/Foundation are **weak-linked** (the objc2 back-deployment fix — see
//! the `device` module docs / `pbxproj` `ATTRIBUTES = (Weak, )`): objc2-ui-kit
//! / objc2-foundation reference framework symbols introduced after the
//! deployment floor, and Rust/objc2 has no `@available` gating, so weak-linking
//! makes the absent symbols resolve to NULL on older OSes. SDK-declared
//! frameworks (ReplayKit/AVFoundation/CoreMedia/CoreVideo/…) all exist on the
//! iOS-16 deployment floor and aren't objc2 newer-symbol cases, so they
//! strong-link.

use std::path::Path;
use std::process::Command;

use anyhow::{Context, Result};
use serde_json::Value;

/// One Apple framework to link, plus whether it must be weak-linked.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Framework {
    /// Bare framework name, e.g. `UIKit`, `ReplayKit` (no `.framework`).
    pub name: String,
    /// Weak-link this framework (`-weak_framework` / pbxproj
    /// `ATTRIBUTES = (Weak, )`) — the objc2 back-deployment fix.
    pub weak: bool,
}

/// The frameworks ALWAYS linked, regardless of which SDKs are present. The
/// Rust staticlib pulls in objc2/objc2-foundation/objc2-ui-kit which need
/// these at link time:
///
/// - **UIKit / Foundation** — weak (objc2 back-deploy fix, see module docs).
/// - **CoreGraphics / QuartzCore** — strong (old/stable; CGRect/CGFloat at the
///   FFI boundary + backend-ios' CALayer code).
///
/// Order here is the order they appear in the generated pbxproj / swiftc line,
/// so it's fixed for reproducibility. BASE wins on a name conflict with an SDK
/// declaration (an SDK can't downgrade UIKit's weak, nor upgrade CoreGraphics).
const BASE: &[(&str, bool)] = &[
    ("UIKit", true),
    ("Foundation", true),
    ("CoreGraphics", false),
    ("QuartzCore", false),
];

/// Compute the full framework set for an iOS build: the [`BASE`] set unioned
/// with every `package.metadata.idealyst.ios.frameworks` entry declared across
/// the wrapper's `cargo metadata` dependency closure.
///
/// `wrapper_manifest` is the standalone build wrapper's `Cargo.toml`, so the
/// package set is exactly the app's dependency closure — the same scope the
/// Kotlin-runtime and capability discovery walk. A missing wrapper (caller
/// hasn't generated one) yields just the base set; the framework's own runtime
/// links fine without third-party SDKs.
///
/// SDK frameworks are strong-linked and sorted; the base set leads in fixed
/// order. The result is deterministic so the generated pbxproj is byte-stable
/// across reruns.
pub fn collect_ios_frameworks(wrapper_manifest: &Path) -> Result<Vec<Framework>> {
    let sdk_frameworks = if wrapper_manifest.is_file() {
        discover_sdk_frameworks(wrapper_manifest)?
    } else {
        Vec::new()
    };
    Ok(merge_frameworks(&sdk_frameworks))
}

/// Test-only: build the merged framework list from an explicit set of
/// SDK-declared framework names, skipping `cargo metadata`. Used by callers'
/// render tests so they don't need a real wrapper on disk.
#[cfg(test)]
pub(crate) fn collect_ios_frameworks_for_test(sdk_frameworks: &[String]) -> Vec<Framework> {
    merge_frameworks(sdk_frameworks)
}

/// Run `cargo metadata` on the wrapper and return every framework name
/// declared under `[package.metadata.idealyst.ios].frameworks` across the
/// dependency closure (unsorted, possibly with duplicates — [`merge_frameworks`]
/// dedups + orders).
fn discover_sdk_frameworks(wrapper_manifest: &Path) -> Result<Vec<String>> {
    let output = Command::new("cargo")
        .args(["metadata", "--format-version", "1"])
        .arg("--manifest-path")
        .arg(wrapper_manifest)
        .output()
        .with_context(|| {
            format!(
                "spawn cargo metadata --manifest-path {}",
                wrapper_manifest.display()
            )
        })?;
    if !output.status.success() {
        anyhow::bail!(
            "cargo metadata failed for {}: {}",
            wrapper_manifest.display(),
            String::from_utf8_lossy(&output.stderr),
        );
    }
    let json: Value =
        serde_json::from_slice(&output.stdout).with_context(|| "parse cargo metadata JSON")?;
    Ok(collect_sdk_frameworks(&json))
}

/// The pure core of [`discover_sdk_frameworks`], split out so it's unit-testable
/// against a synthetic metadata document without invoking cargo.
fn collect_sdk_frameworks(metadata: &Value) -> Vec<String> {
    let mut out = Vec::new();
    let Some(packages) = metadata.get("packages").and_then(|v| v.as_array()) else {
        return out;
    };
    for pkg in packages {
        let Some(frameworks) = pkg
            .pointer("/metadata/idealyst/ios/frameworks")
            .and_then(|v| v.as_array())
        else {
            continue;
        };
        for fw in frameworks {
            if let Some(name) = fw.as_str() {
                out.push(name.to_string());
            }
        }
    }
    out
}

/// Union the [`BASE`] set with the SDK-declared framework names into a
/// deterministic, deduped [`Framework`] list. BASE leads in its fixed order;
/// SDK frameworks follow, strong-linked and sorted. A name that BASE already
/// covers is dropped from the SDK side — BASE wins on conflict so the weak/strong
/// decision on UIKit/Foundation/CoreGraphics/QuartzCore can't be overridden.
fn merge_frameworks(sdk_frameworks: &[String]) -> Vec<Framework> {
    let mut out: Vec<Framework> = BASE
        .iter()
        .map(|(name, weak)| Framework {
            name: (*name).to_string(),
            weak: *weak,
        })
        .collect();

    let base_names: std::collections::HashSet<&str> = BASE.iter().map(|(n, _)| *n).collect();
    let mut sdk: Vec<String> = sdk_frameworks
        .iter()
        .filter(|n| !base_names.contains(n.as_str()))
        .cloned()
        .collect();
    sdk.sort();
    sdk.dedup();
    for name in sdk {
        out.push(Framework { name, weak: false });
    }
    out
}

/// Deterministically derive the two 24-hex-char object IDs (PBXBuildFile +
/// PBXFileReference) a framework needs in the pbxproj. The IDs are a stable
/// function of the framework name so reruns are byte-identical, and the two
/// kinds use distinct hash-input prefixes so a framework's build-file ID never
/// collides with its own file-ref ID. The `ios-fw-` namespace prefix keeps
/// these clear of the template's other (fixed) object IDs.
pub fn pbx_ids(name: &str) -> (String, String) {
    let build_file = hex24(&format!("ios-fw-buildfile:{name}"));
    let file_ref = hex24(&format!("ios-fw-fileref:{name}"));
    (build_file, file_ref)
}

/// Hash `input` to a 24-uppercase-hex-char string (96 bits) using a stable
/// FNV-1a-derived expansion. We roll our own rather than lean on
/// `DefaultHasher` (whose output isn't guaranteed stable across std versions)
/// because the pbxproj must stay byte-identical across toolchains/reruns.
fn hex24(input: &str) -> String {
    // Three independent 32-bit FNV-1a hashes (different seeds) → 96 bits.
    // FNV-1a is fast, deterministic, and well-distributed for short keys;
    // these IDs only need to be collision-free within a handful of framework
    // names, which three distinct seeds comfortably give.
    let h0 = fnv1a32(input, 0x811c9dc5);
    let h1 = fnv1a32(input, 0x01000193);
    let h2 = fnv1a32(input, 0xdeadbeef);
    format!("{h0:08X}{h1:08X}{h2:08X}")
}

/// 32-bit FNV-1a with an explicit offset basis (used as the per-slot seed).
fn fnv1a32(input: &str, seed: u32) -> u32 {
    const PRIME: u32 = 0x0100_0193;
    let mut hash = seed;
    for b in input.as_bytes() {
        hash ^= *b as u32;
        hash = hash.wrapping_mul(PRIME);
    }
    hash
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    /// With no SDK frameworks, the result is exactly the base set in fixed
    /// order with the documented weak/strong split.
    #[test]
    fn base_only_when_no_sdk_frameworks() {
        let fws = merge_frameworks(&[]);
        let names: Vec<&str> = fws.iter().map(|f| f.name.as_str()).collect();
        assert_eq!(names, ["UIKit", "Foundation", "CoreGraphics", "QuartzCore"]);
        assert!(fws[0].weak && fws[1].weak, "UIKit/Foundation weak");
        assert!(!fws[2].weak && !fws[3].weak, "CoreGraphics/QuartzCore strong");
    }

    /// BASE + SDK frameworks merge, dedup, and SDK entries are strong-linked
    /// and sorted after the (fixed-order) base set.
    #[test]
    fn merges_base_and_sdk_frameworks_dedup_and_sorted() {
        let sdk = vec![
            "ReplayKit".to_string(),
            "CoreMedia".to_string(),
            "CoreVideo".to_string(),
            "ReplayKit".to_string(), // duplicate → dropped
        ];
        let fws = merge_frameworks(&sdk);
        let names: Vec<&str> = fws.iter().map(|f| f.name.as_str()).collect();
        assert_eq!(
            names,
            [
                "UIKit",
                "Foundation",
                "CoreGraphics",
                "QuartzCore",
                "CoreMedia",
                "CoreVideo",
                "ReplayKit",
            ]
        );
        // Every SDK framework is strong.
        for f in &fws[4..] {
            assert!(!f.weak, "{} (SDK-declared) must be strong-linked", f.name);
        }
    }

    /// An SDK redeclaring a base framework can't change its weak/strong
    /// decision — BASE wins on conflict and the name isn't duplicated.
    #[test]
    fn sdk_cannot_override_base_weakness() {
        // An SDK declaring CoreGraphics (strong in BASE) and UIKit (weak in
        // BASE) must not flip either, nor add a second copy.
        let sdk = vec!["CoreGraphics".to_string(), "UIKit".to_string()];
        let fws = merge_frameworks(&sdk);
        assert_eq!(fws.len(), 4, "no duplicates added: {fws:?}");
        let cg = fws.iter().find(|f| f.name == "CoreGraphics").unwrap();
        assert!(!cg.weak, "CoreGraphics stays strong");
        let uikit = fws.iter().find(|f| f.name == "UIKit").unwrap();
        assert!(uikit.weak, "UIKit stays weak");
    }

    /// ReplayKit (the regression) renders strong while UIKit/Foundation stay
    /// weak and CoreGraphics/QuartzCore strong — the exact link disposition the
    /// screen-capture app needs.
    #[test]
    fn replaykit_is_strong_and_present() {
        let fws = merge_frameworks(&["ReplayKit".to_string()]);
        let rk = fws
            .iter()
            .find(|f| f.name == "ReplayKit")
            .expect("ReplayKit present");
        assert!(!rk.weak, "ReplayKit must strong-link (class load at runtime)");
    }

    #[test]
    fn collect_reads_ios_frameworks_from_metadata() {
        let metadata = json!({
            "packages": [
                { "name": "camera", "metadata": { "idealyst": { "ios": { "frameworks": ["AVFoundation", "CoreMedia", "CoreVideo"] } } } },
                { "name": "screen-recorder", "metadata": { "idealyst": { "ios": { "frameworks": ["ReplayKit", "CoreMedia", "CoreVideo"] } } } },
                { "name": "unrelated", "metadata": { "idealyst": { "android": {} } } },
                { "name": "no-metadata" },
            ]
        });
        let collected = collect_sdk_frameworks(&metadata);
        assert!(collected.contains(&"AVFoundation".to_string()));
        assert!(collected.contains(&"ReplayKit".to_string()));
        // collect doesn't dedup; merge_frameworks does.
        let merged = merge_frameworks(&collected);
        let names: Vec<&str> = merged.iter().map(|f| f.name.as_str()).collect();
        assert_eq!(
            names,
            [
                "UIKit",
                "Foundation",
                "CoreGraphics",
                "QuartzCore",
                "AVFoundation",
                "CoreMedia",
                "CoreVideo",
                "ReplayKit",
            ]
        );
    }

    #[test]
    fn collect_empty_when_no_packages() {
        assert!(collect_sdk_frameworks(&json!({})).is_empty());
    }

    /// ID generation is deterministic (same name → same IDs) and the two kinds
    /// differ for a given framework (no buildfile/fileref collision).
    #[test]
    fn pbx_ids_are_deterministic_and_24_hex() {
        let (bf1, fr1) = pbx_ids("ReplayKit");
        let (bf2, fr2) = pbx_ids("ReplayKit");
        assert_eq!(bf1, bf2, "build-file ID stable across calls");
        assert_eq!(fr1, fr2, "file-ref ID stable across calls");
        assert_ne!(bf1, fr1, "build-file and file-ref IDs must differ");
        for id in [&bf1, &fr1] {
            assert_eq!(id.len(), 24, "ID must be 24 chars: {id}");
            assert!(
                id.chars().all(|c| c.is_ascii_hexdigit() && !c.is_lowercase()),
                "ID must be uppercase hex: {id}"
            );
        }
    }

    /// Different framework names produce different IDs (no cross-framework
    /// collisions for the set we ship).
    #[test]
    fn pbx_ids_distinct_across_frameworks() {
        let mut seen = std::collections::HashSet::new();
        for fw in [
            "UIKit",
            "Foundation",
            "CoreGraphics",
            "QuartzCore",
            "AVFoundation",
            "CoreMedia",
            "CoreVideo",
            "ReplayKit",
        ] {
            let (bf, fr) = pbx_ids(fw);
            assert!(seen.insert(bf.clone()), "build-file ID collision: {bf}");
            assert!(seen.insert(fr.clone()), "file-ref ID collision: {fr}");
        }
    }
}
