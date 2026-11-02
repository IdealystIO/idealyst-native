//! **NFC** — read and write NFC tags.
//!
//! The raw capability: open a reader session, wait for a tag, then read its
//! NDEF message, write one, or exchange raw commands with it. What the data
//! means (a badge id, a URL to open, an asset tag) is the app's business.
//!
//! | Platform | Backend | Notes |
//! | --- | --- | --- |
//! | iOS | Core NFC (`NFCTagReaderSession`) | iPhone 7+; the system scan sheet shows while a session runs |
//! | Android | `NfcAdapter` reader mode | the Activity must be in the foreground |
//! | macOS / Windows | PC/SC (a USB contactless reader, e.g. ACR122U) | NDEF on Type 2 and Type 4 tags |
//! | Linux | PC/SC behind the `linux-pcsc` feature (needs `libpcsclite`) | as above |
//! | web | Web NFC (`NDEFReader`) | Chrome on Android only; NDEF only, no raw commands |
//! | other | [`Availability::Unsupported`] | |
//!
//! ```no_run
//! # async fn demo() -> Result<(), nfc::NfcError> {
//! use nfc::{NdefMessage, NdefRecord, NfcSession, SessionOptions};
//!
//! let session = NfcSession::begin(SessionOptions::prompt("Hold your badge near the phone"))?;
//! let tag = session.next_tag().await?;
//! println!("tag id {:02x?}", tag.id());
//! if let Some(message) = tag.read_ndef().await? {
//!     for record in &message.records {
//!         if let Some(uri) = record.as_uri() {
//!             println!("uri: {uri}");
//!         }
//!     }
//! }
//! tag.write_ndef(&NdefMessage::new(vec![NdefRecord::uri("https://example.com")])).await?;
//! session.close();
//! # Ok(())
//! # }
//! ```
//!
//! # Sessions
//!
//! One session runs at a time; a second [`NfcSession::begin`] while one is
//! open fails with [`NfcError::Busy`]. Dropping the session (or calling
//! [`close`](NfcSession::close), which also ends a pending `next_tag`) ends it: the iOS sheet is dismissed,
//! Android leaves reader mode, the PC/SC poll thread stops. A [`Tag`] is only
//! usable while its session is open and the tag stays in the field.
//!
//! On iOS the user can dismiss the sheet and the system times a session out
//! after 60 seconds; [`next_tag`](NfcSession::next_tag) then returns
//! [`NfcError::Cancelled`] or [`NfcError::Timeout`], and the session is over.
//!
//! # Platform setup
//!
//! This crate declares the `nfc` capability, so the CLI adds the Android
//! `NFC` permission, the iOS `NFCReaderUsageDescription` string and the
//! iOS Core NFC entitlement. Write the reason the iOS sheet shows in the app:
//!
//! ```toml
//! [package.metadata.idealyst.app.permissions]
//! nfc = "Scan equipment tags"
//! ```
//!
//! iOS only reports ISO 7816 (smart card) tags whose application ids the app
//! lists, and FeliCa tags whose system codes it lists. Both go in the app's
//! manifest:
//!
//! ```toml
//! [package.metadata.idealyst.app.nfc]
//! iso7816_aids = ["D2760000850101"]
//! felica_system_codes = ["12FC"]
//! ```

#![warn(missing_docs)]

use std::fmt;

pub mod ndef;
pub use ndef::{NdefError, NdefMessage, NdefRecord, TextRecord, Tnf};

mod mailbox;

/// Compile-checked usage recipes (docs / MCP catalog). Present only under the
/// `catalog` feature — see [`recipes`].
#[cfg(feature = "catalog")]
pub mod recipes;

/// Reading and writing NDEF over raw tag commands, for backends that only
/// get a command channel (PC/SC). Mobile and web platforms do NDEF natively.
#[cfg(any(test, target_os = "macos", target_os = "windows", all(target_os = "linux", feature = "linux-pcsc")))]
mod tagproto;

#[cfg_attr(target_os = "ios", path = "apple.rs")]
#[cfg_attr(target_os = "android", path = "android.rs")]
#[cfg_attr(target_arch = "wasm32", path = "web.rs")]
#[cfg_attr(
    any(target_os = "macos", target_os = "windows", all(target_os = "linux", feature = "linux-pcsc")),
    path = "pcsc.rs"
)]
#[cfg_attr(
    not(any(
        target_os = "ios",
        target_os = "android",
        target_arch = "wasm32",
        target_os = "macos",
        target_os = "windows",
        all(target_os = "linux", feature = "linux-pcsc"),
    )),
    path = "stub.rs"
)]
mod imp;

/// Whether this device can read NFC tags right now.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Availability {
    /// A session can be started.
    Available,
    /// The hardware exists but NFC is switched off (Android settings), or
    /// no reader is plugged in (PC/SC).
    Disabled,
    /// No NFC on this device or platform.
    Unsupported,
}

/// Check NFC availability without starting a session.
pub fn availability() -> Availability {
    imp::availability()
}

/// Options for [`NfcSession::begin`].
#[derive(Debug, Clone, Default)]
pub struct SessionOptions {
    /// Text on the iOS scan sheet ("Hold your badge near the phone").
    /// Other platforms show no system UI and ignore it.
    pub prompt: Option<String>,
}

impl SessionOptions {
    /// Options with the iOS scan-sheet text set.
    pub fn prompt(text: impl Into<String>) -> Self {
        Self { prompt: Some(text.into()) }
    }
}

/// A tag technology: what kind of commands a tag accepts.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Technology {
    /// ISO 14443-3A.
    NfcA,
    /// ISO 14443-3B.
    NfcB,
    /// JIS 6319-4 (FeliCa).
    NfcF,
    /// ISO 15693.
    NfcV,
    /// ISO 14443-4 (ISO 7816 APDUs): payment cards, passports, DESFire.
    IsoDep,
    /// MIFARE Classic.
    MifareClassic,
    /// MIFARE Ultralight and NTAG (NFC Forum Type 2).
    MifareUltralight,
    /// The tag holds, or can hold, an NDEF message.
    Ndef,
    /// The tag can be formatted for NDEF (Android only reports this).
    NdefFormatable,
}

/// Why an NFC operation failed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum NfcError {
    /// No NFC on this platform or device; the text says what is missing.
    Unsupported(String),
    /// NFC is switched off, or no PC/SC reader is connected.
    Disabled,
    /// Another session is already open.
    Busy,
    /// The user dismissed the scan sheet, or the app closed the session.
    Cancelled,
    /// The system ended the session (iOS: 60 seconds).
    Timeout,
    /// The tag left the field, or the session ended, before the operation
    /// finished.
    TagLost,
    /// The tag is not NDEF formatted and cannot be written with NDEF.
    NotNdef,
    /// The tag is write-protected.
    ReadOnly,
    /// The message does not fit on the tag.
    TooLarge {
        /// Encoded message size in bytes.
        needed: usize,
        /// Bytes the tag can hold.
        capacity: usize,
    },
    /// The tag's NDEF data is malformed.
    Ndef(NdefError),
    /// The platform cannot do this operation (raw commands on web).
    NotAvailable(&'static str),
    /// The platform API reported an error.
    Backend(String),
}

impl fmt::Display for NfcError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            NfcError::Unsupported(why) => write!(f, "NFC unsupported: {why}"),
            NfcError::Disabled => f.write_str("NFC is turned off or no reader is connected"),
            NfcError::Busy => f.write_str("an NFC session is already open"),
            NfcError::Cancelled => f.write_str("NFC session cancelled"),
            NfcError::Timeout => f.write_str("NFC session timed out"),
            NfcError::TagLost => f.write_str("NFC tag lost"),
            NfcError::NotNdef => f.write_str("tag is not NDEF formatted"),
            NfcError::ReadOnly => f.write_str("tag is read-only"),
            NfcError::TooLarge { needed, capacity } => {
                write!(f, "NDEF message of {needed} bytes does not fit the tag's {capacity}")
            }
            NfcError::Ndef(e) => write!(f, "invalid NDEF on tag: {e}"),
            NfcError::NotAvailable(what) => write!(f, "not available on this platform: {what}"),
            NfcError::Backend(msg) => write!(f, "NFC error: {msg}"),
        }
    }
}

impl std::error::Error for NfcError {}

impl From<NdefError> for NfcError {
    fn from(e: NdefError) -> Self {
        NfcError::Ndef(e)
    }
}

/// An open reader session. Drop it (or call [`close`](Self::close)) to stop
/// scanning.
pub struct NfcSession {
    inner: imp::Session,
}

impl NfcSession {
    /// Start scanning. On iOS this shows the system scan sheet.
    pub fn begin(options: SessionOptions) -> Result<Self, NfcError> {
        Ok(Self { inner: imp::Session::begin(&options)? })
    }

    /// Wait for the next tag to enter the field.
    ///
    /// The previous tag stays usable until this is called; on iOS calling
    /// it again restarts polling for a new tag.
    pub async fn next_tag(&self) -> Result<Tag, NfcError> {
        self.inner.next_tag().await
    }

    /// Change the iOS scan-sheet text, e.g. to "Writing…". A no-op
    /// elsewhere.
    pub fn set_prompt(&self, text: &str) {
        self.inner.set_prompt(text);
    }

    /// End the session. On iOS the sheet shows a success check. A pending
    /// [`next_tag`](Self::next_tag) resolves with [`NfcError::Cancelled`],
    /// so a Stop button can end a scan that is still waiting. Later calls
    /// do nothing.
    pub fn close(&self) {
        self.inner.close(None);
    }

    /// End the session showing `message` as an error on the iOS sheet.
    pub fn close_with_error(&self, message: &str) {
        self.inner.close(Some(message));
    }
}

impl Drop for NfcSession {
    fn drop(&mut self) {
        // Idempotent in every backend, so `close` followed by this is fine.
        self.inner.close(None);
    }
}

/// A tag in the field.
pub struct Tag {
    id: Vec<u8>,
    technologies: Vec<Technology>,
    handle: imp::TagHandle,
}

impl Tag {
    pub(crate) fn new(id: Vec<u8>, technologies: Vec<Technology>, handle: imp::TagHandle) -> Self {
        Self { id, technologies, handle }
    }

    /// The tag's identifier (UID). Empty on web when the browser withholds
    /// it. Many tags randomize it per read, so it is not always a stable id.
    pub fn id(&self) -> &[u8] {
        &self.id
    }

    /// What the tag supports.
    pub fn technologies(&self) -> &[Technology] {
        &self.technologies
    }

    /// Read the tag's NDEF message. `Ok(None)` when the tag is not NDEF
    /// formatted; `Ok(Some(empty))` when it is formatted but holds nothing.
    pub async fn read_ndef(&self) -> Result<Option<NdefMessage>, NfcError> {
        self.handle.read_ndef().await
    }

    /// Replace the tag's NDEF message. Formats the tag for NDEF first when
    /// the platform can (Android).
    pub async fn write_ndef(&self, message: &NdefMessage) -> Result<(), NfcError> {
        self.handle.write_ndef(message).await
    }

    /// Send a raw command and return the tag's response: an ISO 7816 APDU
    /// for [`Technology::IsoDep`] tags (the response ends with SW1 SW2), a
    /// native command frame otherwise. FeliCa frames start with their own
    /// length byte, in both directions, as Android's `NfcF` takes them.
    /// Not available on web; ISO 15693 has no raw channel on iOS.
    pub async fn transceive(&self, command: &[u8]) -> Result<Vec<u8>, NfcError> {
        self.handle.transceive(command).await
    }
}

impl fmt::Debug for Tag {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Tag").field("id", &self.id).field("technologies", &self.technologies).finish()
    }
}
