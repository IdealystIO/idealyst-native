//! Inbound links — the build-time half.
//!
//! An OS only hands a URL to an app that DECLARES it, so deep links need
//! per-platform build configuration the runtime cannot provide. This module
//! is the single source for all of it, from one manifest block:
//!
//! ```toml
//! [package.metadata.idealyst.app.links]
//! # Custom URL schemes — `myapp://items/42`.
//! schemes = ["myapp"]
//! # Domains whose https links open the app (universal links on Apple,
//! # verified App Links on Android). A leading `*.` covers subdomains.
//! domains = ["example.com"]
//! # Apple Developer Team ID — goes into apple-app-site-association.
//! apple_team_id = "ABCDE12345"
//! # SHA-256 fingerprints of the certificates that sign the Android app —
//! # go into assetlinks.json.
//! android_cert_fingerprints = ["AB:CD:…"]
//! ```
//!
//! What each platform gets:
//!
//! | | custom `schemes` | `domains` |
//! |---|---|---|
//! | iOS / macOS | `CFBundleURLTypes` in Info.plist | `com.apple.developer.associated-domains` entitlement (`applinks:`) |
//! | Android | a `VIEW`/`BROWSABLE` `<intent-filter>` | an `autoVerify` https `<intent-filter>` per domain |
//! | web build | — | `/.well-known/apple-app-site-association` + `/.well-known/assetlinks.json` |
//!
//! The two `.well-known` files are what the OS fetches from each domain to
//! confirm the app may open its links, so they must be served from every
//! listed domain. The web build emits them; host the web build there (or
//! copy them to whatever serves the domain).
//!
//! Validation happens at manifest parse, so a typo fails the build instead
//! of producing an app the OS silently never sends links to.

use anyhow::{bail, Result};
use serde::Deserialize;

/// `[package.metadata.idealyst.app.links]`, validated. Default = no links.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct LinksMetadata {
    /// Custom URL schemes, lowercase (`myapp`).
    pub schemes: Vec<String>,
    /// Hosts whose https links open the app (`example.com`,
    /// `*.example.com`), lowercase.
    pub domains: Vec<String>,
    /// Apple Developer Team ID (10 characters) for the
    /// apple-app-site-association `appIDs`.
    pub apple_team_id: Option<String>,
    /// Android signing-certificate SHA-256 fingerprints, normalized to
    /// uppercase colon-separated form, for assetlinks.json.
    pub android_cert_fingerprints: Vec<String>,
}

#[derive(Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct RawLinksMetadata {
    #[serde(default)]
    schemes: Vec<String>,
    #[serde(default)]
    domains: Vec<String>,
    #[serde(default)]
    apple_team_id: Option<String>,
    #[serde(default)]
    android_cert_fingerprints: Vec<String>,
}

/// Schemes an app may not claim: the OS (or the browser) owns them, and a
/// web link belongs in `domains`.
const RESERVED_SCHEMES: &[&str] = &[
    "http", "https", "file", "ftp", "mailto", "tel", "sms", "data", "javascript", "about",
];

impl LinksMetadata {
    pub(crate) fn from_raw(raw: RawLinksMetadata) -> Result<Self> {
        let mut schemes = Vec::new();
        for s in raw.schemes {
            let s = s.trim().to_ascii_lowercase();
            // RFC 3986: ALPHA *( ALPHA / DIGIT / "+" / "-" / "." )
            let valid = s.chars().next().is_some_and(|c| c.is_ascii_alphabetic())
                && s.chars().all(|c| c.is_ascii_alphanumeric() || matches!(c, '+' | '-' | '.'));
            if !valid {
                bail!(
                    "[package.metadata.idealyst.app.links] scheme `{s}` is not a valid URL scheme \
                     (a letter, then letters, digits, `+`, `-` or `.`; no `://`)"
                );
            }
            if RESERVED_SCHEMES.contains(&s.as_str()) {
                bail!(
                    "[package.metadata.idealyst.app.links] scheme `{s}` is reserved; \
                     list web links under `domains` instead"
                );
            }
            if !schemes.contains(&s) {
                schemes.push(s);
            }
        }

        let mut domains = Vec::new();
        for d in raw.domains {
            let d = d.trim().to_ascii_lowercase();
            let host = d.strip_prefix("*.").unwrap_or(&d);
            let valid = !host.is_empty()
                && host.contains('.')
                && host
                    .split('.')
                    .all(|label| {
                        !label.is_empty()
                            && label.chars().all(|c| c.is_ascii_alphanumeric() || c == '-')
                    });
            if !valid {
                bail!(
                    "[package.metadata.idealyst.app.links] domain `{d}` must be a bare host name \
                     like `example.com` or `*.example.com` (no scheme, port or path)"
                );
            }
            if !domains.contains(&d) {
                domains.push(d);
            }
        }

        let apple_team_id = match raw.apple_team_id.map(|t| t.trim().to_string()) {
            Some(t) if t.len() == 10 && t.chars().all(|c| c.is_ascii_alphanumeric()) => {
                Some(t.to_ascii_uppercase())
            }
            Some(t) => bail!(
                "[package.metadata.idealyst.app.links] apple_team_id `{t}` must be the \
                 10-character Team ID from developer.apple.com"
            ),
            None => None,
        };

        let mut android_cert_fingerprints = Vec::new();
        for f in raw.android_cert_fingerprints {
            let hex: String = f.chars().filter(|c| *c != ':').collect();
            if hex.len() != 64 || !hex.chars().all(|c| c.is_ascii_hexdigit()) {
                bail!(
                    "[package.metadata.idealyst.app.links] android_cert_fingerprints entry `{f}` \
                     must be a SHA-256 fingerprint (32 hex bytes, e.g. from \
                     `keytool -list -v -keystore <keystore>`)"
                );
            }
            let hex = hex.to_ascii_uppercase();
            let colon = hex
                .as_bytes()
                .chunks(2)
                .map(|pair| std::str::from_utf8(pair).expect("ascii hex"))
                .collect::<Vec<_>>()
                .join(":");
            if !android_cert_fingerprints.contains(&colon) {
                android_cert_fingerprints.push(colon);
            }
        }

        Ok(LinksMetadata { schemes, domains, apple_team_id, android_cert_fingerprints })
    }

    /// Info.plist entries (iOS and macOS): `CFBundleURLTypes` declaring the
    /// custom schemes. Empty when there are none. Indented for the
    /// templates' one-level `<dict>`.
    pub fn plist_url_types(&self, bundle_id: &str) -> String {
        if self.schemes.is_empty() {
            return String::new();
        }
        let schemes = self
            .schemes
            .iter()
            .map(|s| format!("                <string>{}</string>", xml_escape(s)))
            .collect::<Vec<_>>()
            .join("\n");
        format!(
            "<key>CFBundleURLTypes</key>\n    <array>\n        <dict>\n            \
             <key>CFBundleURLName</key>\n            <string>{}</string>\n            \
             <key>CFBundleURLSchemes</key>\n            <array>\n{schemes}\n            </array>\n        \
             </dict>\n    </array>",
            xml_escape(bundle_id)
        )
    }

    /// The `com.apple.developer.associated-domains` values (`applinks:…`),
    /// one per domain.
    pub fn associated_domains(&self) -> Vec<String> {
        self.domains.iter().map(|d| format!("applinks:{d}")).collect()
    }

    /// A complete entitlements plist carrying the associated-domains
    /// entitlement, for code signing — `None` when there are no domains
    /// (a build without universal links needs no entitlements file, and
    /// declaring an empty one would still demand the capability on the
    /// provisioning profile).
    pub fn entitlements_plist(&self) -> Option<String> {
        if self.domains.is_empty() {
            return None;
        }
        let values = self
            .associated_domains()
            .iter()
            .map(|v| format!("        <string>{}</string>", xml_escape(v)))
            .collect::<Vec<_>>()
            .join("\n");
        Some(format!(
            "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n\
             <!DOCTYPE plist PUBLIC \"-//Apple//DTD PLIST 1.0//EN\" \
             \"http://www.apple.com/DTDs/PropertyList-1.0.dtd\">\n\
             <plist version=\"1.0\">\n<dict>\n    \
             <key>com.apple.developer.associated-domains</key>\n    <array>\n{values}\n    </array>\n\
             </dict>\n</plist>\n"
        ))
    }

    /// AndroidManifest `<intent-filter>`s for the launch Activity: one
    /// `VIEW`/`BROWSABLE` filter for the custom schemes, and one
    /// `autoVerify` https filter per domain (separate filters, because
    /// Android verifies — and lists in system settings — each filter's
    /// hosts as a unit). Empty when there are no links.
    pub fn android_intent_filters(&self) -> String {
        let mut filters = Vec::new();
        if !self.schemes.is_empty() {
            let data = self
                .schemes
                .iter()
                .map(|s| format!("                <data android:scheme=\"{}\" />", xml_escape(s)))
                .collect::<Vec<_>>()
                .join("\n");
            filters.push(format!(
                "<intent-filter>\n{VIEW_BROWSABLE}\n{data}\n            </intent-filter>"
            ));
        }
        for d in &self.domains {
            filters.push(format!(
                "<intent-filter android:autoVerify=\"true\">\n{VIEW_BROWSABLE}\n                \
                 <data android:scheme=\"https\" android:host=\"{}\" />\n            </intent-filter>",
                xml_escape(d)
            ));
        }
        filters.join("\n            ")
    }

    /// `/.well-known/apple-app-site-association` — lets iOS / macOS open
    /// every path on the listed domains in the app `team.bundle_id`.
    /// `None` without domains or without `apple_team_id` (the file can't
    /// name the app then).
    pub fn apple_app_site_association(&self, bundle_id: &str) -> Option<String> {
        if self.domains.is_empty() {
            return None;
        }
        let team = self.apple_team_id.as_ref()?;
        let doc = serde_json::json!({
            "applinks": {
                "details": [{
                    "appIDs": [format!("{team}.{bundle_id}")],
                    "components": [{ "/": "*" }],
                }]
            }
        });
        Some(serde_json::to_string_pretty(&doc).expect("static JSON shape"))
    }

    /// `/.well-known/assetlinks.json` (Digital Asset Links) — lets Android
    /// verify the App Link filters for `package`. `None` without domains
    /// or without fingerprints.
    pub fn asset_links(&self, package: &str) -> Option<String> {
        if self.domains.is_empty() || self.android_cert_fingerprints.is_empty() {
            return None;
        }
        let doc = serde_json::json!([{
            "relation": ["delegate_permission/common.handle_all_urls"],
            "target": {
                "namespace": "android_app",
                "package_name": package,
                "sha256_cert_fingerprints": self.android_cert_fingerprints,
            }
        }]);
        Some(serde_json::to_string_pretty(&doc).expect("static JSON shape"))
    }
}

const VIEW_BROWSABLE: &str = "                <action android:name=\"android.intent.action.VIEW\" />\n                \
     <category android:name=\"android.intent.category.DEFAULT\" />\n                \
     <category android:name=\"android.intent.category.BROWSABLE\" />";

fn xml_escape(s: &str) -> String {
    s.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;").replace('"', "&quot;")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn raw(schemes: &[&str], domains: &[&str]) -> RawLinksMetadata {
        RawLinksMetadata {
            schemes: schemes.iter().map(|s| s.to_string()).collect(),
            domains: domains.iter().map(|s| s.to_string()).collect(),
            apple_team_id: None,
            android_cert_fingerprints: Vec::new(),
        }
    }

    const FP: &str = "ab:cd:ef:01:23:45:67:89:ab:cd:ef:01:23:45:67:89:ab:cd:ef:01:23:45:67:89:ab:cd:ef:01:23:45:67:89";

    #[test]
    fn normalizes_and_dedups() {
        let l = LinksMetadata::from_raw(raw(&["MyApp", "myapp"], &["Example.com", "*.example.com"]))
            .unwrap();
        assert_eq!(l.schemes, vec!["myapp"]);
        assert_eq!(l.domains, vec!["example.com", "*.example.com"]);
    }

    #[test]
    fn rejects_bad_schemes_and_domains() {
        for bad in ["my app", "myapp://", "1app", "", "https"] {
            assert!(LinksMetadata::from_raw(raw(&[bad], &[])).is_err(), "scheme {bad:?}");
        }
        for bad in ["https://example.com", "example.com/path", "example.com:443", "localhost", ""] {
            assert!(LinksMetadata::from_raw(raw(&[], &[bad])).is_err(), "domain {bad:?}");
        }
    }

    #[test]
    fn validates_team_id_and_fingerprints() {
        let mut r = raw(&[], &["example.com"]);
        r.apple_team_id = Some("short".into());
        assert!(LinksMetadata::from_raw(r).is_err());

        let mut r = raw(&[], &["example.com"]);
        r.android_cert_fingerprints = vec!["nothex".into()];
        assert!(LinksMetadata::from_raw(r).is_err());

        let mut r = raw(&[], &["example.com"]);
        r.apple_team_id = Some("abcde12345".into());
        r.android_cert_fingerprints = vec![FP.replace(':', "")];
        let l = LinksMetadata::from_raw(r).unwrap();
        assert_eq!(l.apple_team_id.as_deref(), Some("ABCDE12345"));
        assert_eq!(l.android_cert_fingerprints, vec![FP.to_ascii_uppercase()]);
    }

    #[test]
    fn plist_declares_url_types_only_for_schemes() {
        let none = LinksMetadata::from_raw(raw(&[], &["example.com"])).unwrap();
        assert_eq!(none.plist_url_types("com.acme.app"), "");
        let l = LinksMetadata::from_raw(raw(&["myapp", "acme"], &[])).unwrap();
        let p = l.plist_url_types("com.acme.app");
        assert!(p.contains("<key>CFBundleURLTypes</key>"));
        assert!(p.contains("<string>com.acme.app</string>"));
        assert!(p.contains("<string>myapp</string>") && p.contains("<string>acme</string>"));
    }

    #[test]
    fn entitlements_only_with_domains() {
        assert!(LinksMetadata::from_raw(raw(&["myapp"], &[])).unwrap().entitlements_plist().is_none());
        let e = LinksMetadata::from_raw(raw(&[], &["example.com"]))
            .unwrap()
            .entitlements_plist()
            .unwrap();
        assert!(e.contains("<key>com.apple.developer.associated-domains</key>"));
        assert!(e.contains("<string>applinks:example.com</string>"));
    }

    #[test]
    fn android_filters_cover_schemes_and_verified_domains() {
        assert_eq!(LinksMetadata::default().android_intent_filters(), "");
        let f = LinksMetadata::from_raw(raw(&["myapp"], &["example.com", "*.acme.io"]))
            .unwrap()
            .android_intent_filters();
        assert_eq!(f.matches("<intent-filter").count(), 3);
        assert_eq!(f.matches("android:autoVerify=\"true\"").count(), 2);
        assert!(f.contains("<data android:scheme=\"myapp\" />"));
        assert!(f.contains("android:host=\"*.acme.io\""));
        assert_eq!(f.matches("android.intent.category.BROWSABLE").count(), 3);
    }

    #[test]
    fn well_known_files_need_their_identifiers() {
        let mut r = raw(&[], &["example.com"]);
        let bare = LinksMetadata::from_raw(raw(&[], &["example.com"])).unwrap();
        assert!(bare.apple_app_site_association("com.acme.app").is_none());
        assert!(bare.asset_links("com.acme.app").is_none());

        r.apple_team_id = Some("ABCDE12345".into());
        r.android_cert_fingerprints = vec![FP.into()];
        let l = LinksMetadata::from_raw(r).unwrap();
        let aasa: serde_json::Value =
            serde_json::from_str(&l.apple_app_site_association("com.acme.app").unwrap()).unwrap();
        assert_eq!(aasa["applinks"]["details"][0]["appIDs"][0], "ABCDE12345.com.acme.app");
        assert_eq!(aasa["applinks"]["details"][0]["components"][0]["/"], "*");
        let al: serde_json::Value =
            serde_json::from_str(&l.asset_links("com.acme.app").unwrap()).unwrap();
        assert_eq!(al[0]["target"]["package_name"], "com.acme.app");
        assert_eq!(al[0]["target"]["sha256_cert_fingerprints"][0], FP.to_ascii_uppercase());
        assert_eq!(al[0]["relation"][0], "delegate_permission/common.handle_all_urls");

        // No domains ⇒ nothing to verify, even with the identifiers.
        let mut r = raw(&["myapp"], &[]);
        r.apple_team_id = Some("ABCDE12345".into());
        assert!(LinksMetadata::from_raw(r).unwrap().apple_app_site_association("x").is_none());
    }
}
