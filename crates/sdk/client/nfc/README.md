# `nfc`

Read and write NFC tags. Open a reader session, wait for a tag, then read its
NDEF message, write one, or send it raw commands. What the data means is the
app's business.

```rust
use nfc::{NdefMessage, NdefRecord, NfcSession, SessionOptions};

let session = NfcSession::begin(SessionOptions::prompt("Hold your badge near the phone"))?;
let tag = session.next_tag().await?;
if let Some(message) = tag.read_ndef().await? {
    for record in &message.records {
        if let Some(text) = record.as_text() {
            println!("{}", text.text);
        }
    }
}
tag.write_ndef(&NdefMessage::new(vec![NdefRecord::uri("https://example.com")])).await?;
session.close();
```

## API

- `nfc::availability()` → `Available`, `Disabled` (NFC off / no reader plugged
  in) or `Unsupported`.
- `NfcSession::begin(options)` starts scanning; one session at a time (a
  second is `NfcError::Busy`). `close()` / drop ends it, and a pending
  `next_tag()` then resolves with `NfcError::Cancelled`, so a Stop button
  works. `set_prompt` changes the iOS sheet text.
- `Tag`: `id()`, `technologies()`, `read_ndef()` (`Ok(None)` when the tag is
  not NDEF formatted), `write_ndef(&message)`, `transceive(&command)` (an ISO
  7816 APDU for `IsoDep` tags; FeliCa frames carry their length byte).
- `ndef`: `NdefMessage::to_bytes` / `from_bytes` (chunked records are
  reassembled), `NdefRecord::text` / `uri` / `mime` / `external` / `empty`,
  and `as_text` / `as_uri` / `media_type` to read them. Pure Rust, so it also
  works on a server.

## Platforms

| Platform | Backend | Notes |
| --- | --- | --- |
| iOS | Core NFC `NFCTagReaderSession` | iPhone 7+; system scan sheet; 60 s session timeout (`NfcError::Timeout`) |
| Android | `NfcAdapter` reader mode (Kotlin shim `RustNfc`) | the Activity must be in the foreground |
| macOS / Windows | PC/SC with a USB contactless reader (e.g. ACR122U) | NDEF on Type 2 (NTAG / Ultralight) and Type 4 tags; not on MIFARE Classic |
| Linux | PC/SC behind the `linux-pcsc` feature | needs `libpcsclite` and `pcscd` |
| web | Web NFC `NDEFReader` | Chrome on Android over HTTPS; NDEF only, no `transceive`; `write_ndef` writes to whichever tag is tapped next |

## App setup

Depending on this crate declares the `nfc` capability, so the CLI adds the
Android `NFC` permission, the iOS `NFCReaderUsageDescription` and Core NFC
entitlement, and the macOS smartcard entitlement. The app writes the reason
the iOS sheet shows:

```toml
[package.metadata.idealyst.app.permissions]
nfc = "Scan equipment tags"
```

iOS only reports ISO 7816 tags whose application id the app lists, and
FeliCa tags whose system code it lists (FeliCa polling is turned on only
when that list exists):

```toml
[package.metadata.idealyst.app.nfc]
iso7816_aids = ["D2760000850101"]   # the NFC Forum Type 4 NDEF application
felica_system_codes = ["12FC"]
```

The iOS entitlement needs a provisioning profile with NFC Tag Reading; the
CLI's automatic signing adds it.

## Example

`examples/nfc-demo`: Scan lists every tag tapped (id, technologies,
records) until Stop; Write text writes an NDEF Text record and reads it back.

```sh
idealyst build --android   # then: adb install dist/android/nfc-demo-0.0.1.apk
```

A phone emulating a tag (host card emulation) reads as an `IsoDep` tag; if
the emulating app implements the Type 4 NDEF application, its message reads
like a sticker's.

## Verification

- Unit tests: the NDEF codec, the Type 2 / Type 4 NDEF mappings against
  simulated tags, the tag mailbox, ATR classification.
- `tests/pcsc_session.rs`: the session lifecycle against the system PC/SC
  service on macOS / Windows.
- `tests/web_nfc.rs`: the Web NFC record mapping in a browser, with a fake
  `NDEFReader` (`cargo test -p nfc --target wasm32-unknown-unknown`).
- iOS and Android compile and the Android APK ships the permission and the
  shim; neither has read a real tag yet.
