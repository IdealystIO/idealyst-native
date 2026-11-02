//! Compile-checked usage **recipes** for the `nfc` SDK.
//!
//! Each `recipe!(target, fn ...)` is a real, type-checked example, compiled
//! against this crate's live API — so an API change that isn't reflected
//! here is a compile error (whenever the `catalog` feature is built). With
//! `catalog` off (every production build) `recipe!` expands to nothing.
//!
//! Recipes are self-contained — every `use` lives inside the fn — so the
//! captured source reads as a complete, copy-pasteable example.

use runtime_core::recipe;

recipe!(
    read_ndef,
    /// Scan one tag and show the first text or URI record on it. The press
    /// handler runs the async session with `spawn_then`, which writes the
    /// result into the signal only while this view is still mounted;
    /// `next_tag` waits for a tap, and the session closes when it drops at
    /// the end of the task.
    pub fn nfc_scan_button() -> ::runtime_core::Element {
        use ::runtime_core::{signal, spawn_then, text, ui};
        use crate::{NfcSession, SessionOptions};

        let last = signal(String::from("No tag yet"));

        let on_click = move || {
            spawn_then(
                async {
                    let session = NfcSession::begin(SessionOptions::prompt("Hold a tag near the phone"))?;
                    let tag = session.next_tag().await?;
                    let message = tag.read_ndef().await?;
                    Ok::<_, crate::NfcError>(
                        message
                            .iter()
                            .flat_map(|m| &m.records)
                            .find_map(|r| r.as_text().map(|t| t.text).or_else(|| r.as_uri()))
                            .unwrap_or_else(|| format!("Tag {:02x?} has no text", tag.id())),
                    )
                },
                move |result| {
                    last.set(match result {
                        Ok(s) => s,
                        Err(e) => e.to_string(),
                    })
                },
            );
        };

        ui! {
            view {
                button(label = "Scan tag".to_string(), on_click = on_click)
                text(move || last.get())
            }
        }
    }
);

recipe!(
    write_ndef,
    /// Write a URL to the next tag tapped. `write_ndef` replaces whatever
    /// NDEF message the tag held.
    pub fn nfc_write_url() -> ::runtime_core::Element {
        use ::runtime_core::{signal, spawn_then, text, ui};
        use crate::{NdefMessage, NdefRecord, NfcSession, SessionOptions};

        let status = signal(String::new());

        let on_click = move || {
            spawn_then(
                async {
                    let session = NfcSession::begin(SessionOptions::prompt("Hold a tag to write"))?;
                    let tag = session.next_tag().await?;
                    let message = NdefMessage::new(vec![NdefRecord::uri("https://example.com")]);
                    tag.write_ndef(&message).await?;
                    session.close();
                    Ok::<_, crate::NfcError>(())
                },
                move |result| {
                    status.set(match result {
                        Ok(()) => "Written".to_string(),
                        Err(e) => e.to_string(),
                    })
                },
            );
        };

        ui! {
            view {
                button(label = "Write URL".to_string(), on_click = on_click)
                text(move || status.get())
            }
        }
    }
);
