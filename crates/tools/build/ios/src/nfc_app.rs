//! App-level NFC configuration — the iOS Info.plist lists Core NFC reads.
//!
//! Core NFC only reports an ISO 7816 (smart card) tag when it answers one
//! of the application ids the app lists, and a FeliCa tag when it carries
//! one of the listed system codes. Both lists are app policy, so they come
//! from the app's manifest; the `nfc` SDK's capability supplies the rest
//! (usage string, entitlement, Android permission).
//!
//! ```toml
//! [package.metadata.idealyst.app.nfc]
//! iso7816_aids = ["D2760000850101"]
//! felica_system_codes = ["12FC"]
//! ```
//!
//! Validation happens at manifest parse, so a malformed id fails the build
//! instead of producing an app that silently never sees the tag.

use anyhow::{bail, Result};
use serde::Deserialize;

/// Info.plist key listing ISO 7816 application ids for Core NFC.
pub const ISO7816_AIDS_KEY: &str = "com.apple.developer.nfc.readersession.iso7816.select-identifiers";
/// Info.plist key listing FeliCa system codes for Core NFC.
pub const FELICA_SYSTEM_CODES_KEY: &str = "com.apple.developer.nfc.readersession.felica.systemcodes";

/// `[package.metadata.idealyst.app.nfc]`, validated. Default = no lists.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct NfcAppMetadata {
    /// ISO 7816 application ids, uppercase hex (5–16 bytes each).
    pub iso7816_aids: Vec<String>,
    /// FeliCa system codes, uppercase hex (2 bytes each).
    pub felica_system_codes: Vec<String>,
}

#[derive(Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct RawNfcAppMetadata {
    #[serde(default)]
    iso7816_aids: Vec<String>,
    #[serde(default)]
    felica_system_codes: Vec<String>,
}

fn hex_bytes(value: &str, what: &str, bytes: std::ops::RangeInclusive<usize>) -> Result<String> {
    let v = value.to_ascii_uppercase();
    if v.len() % 2 != 0 || !v.chars().all(|c| c.is_ascii_hexdigit()) {
        bail!("[package.metadata.idealyst.app.nfc] {what} `{value}` is not hex");
    }
    if !bytes.contains(&(v.len() / 2)) {
        bail!(
            "[package.metadata.idealyst.app.nfc] {what} `{value}` must be {}–{} bytes",
            bytes.start(),
            bytes.end()
        );
    }
    Ok(v)
}

impl NfcAppMetadata {
    pub(crate) fn from_raw(raw: RawNfcAppMetadata) -> Result<Self> {
        Ok(Self {
            // ISO 7816-4 limits an AID to 16 bytes; RID alone is 5.
            iso7816_aids: raw
                .iso7816_aids
                .iter()
                .map(|a| hex_bytes(a, "iso7816_aids entry", 5..=16))
                .collect::<Result<_>>()?,
            felica_system_codes: raw
                .felica_system_codes
                .iter()
                .map(|c| hex_bytes(c, "felica_system_codes entry", 2..=2))
                .collect::<Result<_>>()?,
        })
    }

    /// The Info.plist entries (`<key>…</key><array>…</array>`), empty when
    /// neither list is set. Values are validated hex, so need no escaping.
    pub fn plist_entries(&self) -> String {
        [(ISO7816_AIDS_KEY, &self.iso7816_aids), (FELICA_SYSTEM_CODES_KEY, &self.felica_system_codes)]
            .iter()
            .filter(|(_, values)| !values.is_empty())
            .map(|(key, values)| {
                let items: String =
                    values.iter().map(|v| format!("\n        <string>{v}</string>")).collect();
                format!("<key>{key}</key>\n    <array>{items}\n    </array>")
            })
            .collect::<Vec<_>>()
            .join("\n    ")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn raw(aids: &[&str], codes: &[&str]) -> RawNfcAppMetadata {
        RawNfcAppMetadata {
            iso7816_aids: aids.iter().map(|s| s.to_string()).collect(),
            felica_system_codes: codes.iter().map(|s| s.to_string()).collect(),
        }
    }

    #[test]
    fn lists_are_uppercased_and_rendered_as_plist_arrays() {
        let m = NfcAppMetadata::from_raw(raw(&["d2760000850101"], &["12fc"])).unwrap();
        assert_eq!(m.iso7816_aids, vec!["D2760000850101"]);
        let plist = m.plist_entries();
        assert!(plist.contains(&format!("<key>{ISO7816_AIDS_KEY}</key>")));
        assert!(plist.contains("<string>D2760000850101</string>"));
        assert!(plist.contains(&format!("<key>{FELICA_SYSTEM_CODES_KEY}</key>")));
        assert!(plist.contains("<string>12FC</string>"));
    }

    #[test]
    fn empty_lists_render_nothing() {
        assert_eq!(NfcAppMetadata::default().plist_entries(), "");
        let only_aid = NfcAppMetadata::from_raw(raw(&["A000000003"], &[])).unwrap();
        assert!(!only_aid.plist_entries().contains(FELICA_SYSTEM_CODES_KEY));
    }

    #[test]
    fn malformed_ids_fail_the_parse() {
        assert!(NfcAppMetadata::from_raw(raw(&["XYZ"], &[])).is_err());
        assert!(NfcAppMetadata::from_raw(raw(&["A0000000"], &[])).is_err(), "4 bytes is too short");
        assert!(NfcAppMetadata::from_raw(raw(&["A00"], &[])).is_err(), "odd length");
        assert!(NfcAppMetadata::from_raw(raw(&[], &["12FC00"])).is_err());
    }
}
