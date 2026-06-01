//! Cross-platform capability → permission expansion.
//!
//! # The hybrid model
//!
//! A platform permission is two separable facts:
//!
//! 1. **The requirement** — the manifest entry / plist key / entitlement
//!    that must exist. Mechanical, platform-specific, and the *library*
//!    knows it. The `microphone` SDK knows it needs `RECORD_AUDIO` and an
//!    `NSMicrophoneUsageDescription` key.
//! 2. **The justification** — the user-facing reason string the OS shows
//!    in its prompt. Policy, app-specific, and only the *app author* can
//!    write it well.
//!
//! So a library declares the capability it needs (auto-discovered from the
//! dependency graph, the same way SDK Kotlin sources are), and the app
//! declares the reason:
//!
//! ```toml
//! # in an SDK's Cargo.toml — the requirement
//! [package.metadata.idealyst]
//! capabilities = ["microphone"]
//!
//! # in the app's Cargo.toml — the justification
//! [package.metadata.idealyst.app.permissions]
//! microphone = "Record voice notes"
//! ```
//!
//! [`discover`] walks the build wrapper's `cargo metadata` and collects
//! every declared capability; [`resolve`] expands them into per-platform
//! artifacts, pulling each reason from the app block or falling back to a
//! generic default **with a loud warning** (a generic iOS usage string
//! risks App Store rejection — the default is a stopgap, not a
//! destination).

use std::collections::BTreeMap;
use std::path::Path;
use std::process::Command;

use anyhow::{Context, Result};
use serde_json::Value;

/// One known capability and how it maps onto each platform.
pub struct Capability {
    /// The name SDKs/apps write in TOML. Lowercase, hyphen-free.
    pub name: &'static str,
    /// iOS `Info.plist` usage-description key (its value is the reason).
    pub ios_plist_key: Option<&'static str>,
    /// macOS `Info.plist` usage-description key.
    pub macos_plist_key: Option<&'static str>,
    /// macOS hardened-runtime entitlement, consumed when the `.app` is
    /// codesigned. Carried here so a future signing step is correct by
    /// construction; unsigned dev builds rely on the plist key alone.
    pub macos_entitlement: Option<&'static str>,
    /// Android `<uses-permission>` names.
    pub android_permissions: &'static [&'static str],
    /// Reason used when the app supplied none. Deliberately generic.
    pub default_reason: &'static str,
}

/// The registry. **Adding a row is how you support a new capability** —
/// this is the single place platform knowledge lives, so an SDK author
/// declares `capabilities = ["microphone"]` and never repeats the
/// per-platform key names. Keep names lowercase + hyphen-free.
pub const REGISTRY: &[Capability] = &[
    Capability {
        name: "microphone",
        ios_plist_key: Some("NSMicrophoneUsageDescription"),
        macos_plist_key: Some("NSMicrophoneUsageDescription"),
        macos_entitlement: Some("com.apple.security.device.audio-input"),
        android_permissions: &["android.permission.RECORD_AUDIO"],
        default_reason: "This app uses the microphone to capture audio.",
    },
    Capability {
        name: "camera",
        ios_plist_key: Some("NSCameraUsageDescription"),
        macos_plist_key: Some("NSCameraUsageDescription"),
        macos_entitlement: Some("com.apple.security.device.camera"),
        android_permissions: &["android.permission.CAMERA"],
        default_reason: "This app uses the camera.",
    },
];

/// Look up a capability by the name an SDK declared.
pub fn lookup(name: &str) -> Option<&'static Capability> {
    REGISTRY.iter().find(|c| c.name == name)
}

/// A capability requested by one or more crates in the dependency graph.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DiscoveredCapability {
    /// The capability name as declared.
    pub name: String,
    /// The crate(s) whose metadata declared it — surfaced in the build
    /// report so an auto-added permission is never anonymous.
    pub requested_by: Vec<String>,
}

/// Walk `manifest_path`'s `cargo metadata` and collect every capability
/// declared under `[package.metadata.idealyst].capabilities`.
///
/// `manifest_path` should be the standalone build wrapper's `Cargo.toml`
/// so the package set is exactly the app's dependency closure (the same
/// reason the Kotlin-runtime discovery walks the wrapper, not the app).
pub fn discover(manifest_path: &Path) -> Result<Vec<DiscoveredCapability>> {
    let output = Command::new("cargo")
        .args(["metadata", "--format-version", "1"])
        .arg("--manifest-path")
        .arg(manifest_path)
        .output()
        .with_context(|| format!("run cargo metadata for {}", manifest_path.display()))?;
    if !output.status.success() {
        anyhow::bail!(
            "cargo metadata failed for {}: {}",
            manifest_path.display(),
            String::from_utf8_lossy(&output.stderr)
        );
    }
    let json: Value =
        serde_json::from_slice(&output.stdout).context("parse cargo metadata json")?;
    Ok(collect(&json))
}

/// The pure core of [`discover`], split out so it's unit-testable against
/// a synthetic metadata document without invoking cargo.
fn collect(metadata: &Value) -> Vec<DiscoveredCapability> {
    let mut by_name: BTreeMap<String, Vec<String>> = BTreeMap::new();
    if let Some(packages) = metadata.get("packages").and_then(|p| p.as_array()) {
        for pkg in packages {
            let pkg_name = pkg.get("name").and_then(|n| n.as_str()).unwrap_or("?");
            let Some(caps) = pkg
                .pointer("/metadata/idealyst/capabilities")
                .and_then(|v| v.as_array())
            else {
                continue;
            };
            for cap in caps {
                if let Some(cap) = cap.as_str() {
                    by_name
                        .entry(cap.to_string())
                        .or_default()
                        .push(pkg_name.to_string());
                }
            }
        }
    }
    by_name
        .into_iter()
        .map(|(name, mut requested_by)| {
            requested_by.sort();
            requested_by.dedup();
            DiscoveredCapability { name, requested_by }
        })
        .collect()
}

/// The platform artifacts a set of capabilities expands into, plus the
/// human-facing report + warnings the CLI prints.
#[derive(Debug, Default, PartialEq, Eq)]
pub struct Resolved {
    /// iOS `Info.plist` `(key, value)` pairs.
    pub ios_plist: Vec<(String, String)>,
    /// macOS `Info.plist` `(key, value)` pairs.
    pub macos_plist: Vec<(String, String)>,
    /// macOS boolean-`true` entitlement keys (for a future signing step).
    pub macos_entitlements: Vec<String>,
    /// Android `<uses-permission>` names.
    pub android_permissions: Vec<String>,
    /// Human lines describing what was bundled + which crate asked, so
    /// auto-added permissions are visible in the build output.
    pub report: Vec<String>,
    /// Loud warnings: generic-reason fallback, unknown capability.
    pub warnings: Vec<String>,
}

/// Expand discovered capabilities into platform artifacts, resolving each
/// reason from `app_reasons` (capability name → reason string) or the
/// registry default. Unknown capabilities are warned about and skipped —
/// the build never blocks on a permission.
pub fn resolve(
    discovered: &[DiscoveredCapability],
    app_reasons: &BTreeMap<String, String>,
) -> Resolved {
    let mut out = Resolved::default();
    for d in discovered {
        let requesters = d.requested_by.join(", ");
        let Some(cap) = lookup(&d.name) else {
            out.warnings.push(format!(
                "unknown capability `{}` (requested by {}) — no permission mapping, ignored. \
                 Known: {}",
                d.name,
                requesters,
                REGISTRY
                    .iter()
                    .map(|c| c.name)
                    .collect::<Vec<_>>()
                    .join(", ")
            ));
            continue;
        };

        // Reason: an app override wins; otherwise the registry default,
        // and a loud warning because a generic reason is a stopgap.
        let reason = match app_reasons.get(&d.name) {
            Some(r) => r.clone(),
            None => {
                out.warnings.push(format!(
                    "capability `{}` (requested by {}) has no reason string — using a generic \
                     default. Add `[package.metadata.idealyst.app.permissions]` with `{} = \"…\"`; \
                     generic iOS usage strings risk App Store rejection.",
                    d.name, requesters, d.name
                ));
                cap.default_reason.to_string()
            }
        };

        if let Some(k) = cap.ios_plist_key {
            out.ios_plist.push((k.to_string(), reason.clone()));
        }
        if let Some(k) = cap.macos_plist_key {
            out.macos_plist.push((k.to_string(), reason.clone()));
        }
        if let Some(e) = cap.macos_entitlement {
            out.macos_entitlements.push(e.to_string());
        }
        for p in cap.android_permissions {
            out.android_permissions.push((*p).to_string());
        }
        out.report
            .push(format!("{} (requested by {})", d.name, requesters));
    }
    // Two crates requesting the same capability would otherwise double the
    // Android permission / entitlement; dedup defensively.
    out.android_permissions.sort();
    out.android_permissions.dedup();
    out.macos_entitlements.sort();
    out.macos_entitlements.dedup();
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn reasons(pairs: &[(&str, &str)]) -> BTreeMap<String, String> {
        pairs
            .iter()
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect()
    }

    #[test]
    fn collect_reads_capabilities_and_merges_requesters() {
        let metadata = json!({
            "packages": [
                { "name": "microphone", "metadata": { "idealyst": { "capabilities": ["microphone"] } } },
                { "name": "some-cam-sdk", "metadata": { "idealyst": { "capabilities": ["camera", "microphone"] } } },
                { "name": "unrelated", "metadata": { "idealyst": { "android": {} } } },
                { "name": "no-metadata" },
            ]
        });
        let discovered = collect(&metadata);
        // Sorted by name (BTreeMap): camera, microphone.
        assert_eq!(discovered.len(), 2);
        assert_eq!(discovered[0].name, "camera");
        assert_eq!(discovered[0].requested_by, vec!["some-cam-sdk"]);
        assert_eq!(discovered[1].name, "microphone");
        // Both crates that asked for microphone, deduped + sorted.
        assert_eq!(
            discovered[1].requested_by,
            vec!["microphone", "some-cam-sdk"]
        );
    }

    #[test]
    fn collect_empty_when_no_packages() {
        assert!(collect(&json!({})).is_empty());
    }

    #[test]
    fn resolve_uses_app_reason_when_present() {
        let discovered = vec![DiscoveredCapability {
            name: "microphone".into(),
            requested_by: vec!["microphone".into()],
        }];
        let r = resolve(&discovered, &reasons(&[("microphone", "Record voice notes")]));
        assert_eq!(
            r.ios_plist,
            vec![(
                "NSMicrophoneUsageDescription".to_string(),
                "Record voice notes".to_string()
            )]
        );
        assert_eq!(
            r.android_permissions,
            vec!["android.permission.RECORD_AUDIO".to_string()]
        );
        assert_eq!(
            r.macos_entitlements,
            vec!["com.apple.security.device.audio-input".to_string()]
        );
        // No fallback warning when the app supplied a reason.
        assert!(r.warnings.is_empty());
        assert_eq!(r.report.len(), 1);
    }

    #[test]
    fn resolve_falls_back_to_default_reason_and_warns() {
        let discovered = vec![DiscoveredCapability {
            name: "microphone".into(),
            requested_by: vec!["microphone".into()],
        }];
        let r = resolve(&discovered, &BTreeMap::new());
        assert_eq!(r.ios_plist[0].0, "NSMicrophoneUsageDescription");
        assert_eq!(r.ios_plist[0].1, "This app uses the microphone to capture audio.");
        assert_eq!(r.warnings.len(), 1, "must warn about the generic reason");
        assert!(r.warnings[0].contains("App Store"));
    }

    #[test]
    fn resolve_warns_and_skips_unknown_capability() {
        let discovered = vec![DiscoveredCapability {
            name: "telepathy".into(),
            requested_by: vec!["psychic-sdk".into()],
        }];
        let r = resolve(&discovered, &BTreeMap::new());
        assert!(r.ios_plist.is_empty());
        assert!(r.android_permissions.is_empty());
        assert_eq!(r.warnings.len(), 1);
        assert!(r.warnings[0].contains("unknown capability"));
    }

    #[test]
    fn resolve_dedups_android_permission_across_requesters() {
        // Same capability surfaced once (already merged by `collect`),
        // but resolve must still be idempotent on its own output shape.
        let discovered = vec![DiscoveredCapability {
            name: "microphone".into(),
            requested_by: vec!["a".into(), "b".into()],
        }];
        let r = resolve(&discovered, &reasons(&[("microphone", "x")]));
        assert_eq!(r.android_permissions.len(), 1);
    }
}
