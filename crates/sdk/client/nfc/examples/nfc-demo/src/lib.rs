//! `nfc-demo` — reads and writes NFC tags with the `nfc` SDK.
//!
//! **Scan** opens a reader session and lists every tag tapped until **Stop**:
//! its id, technologies and NDEF records. **Write text** writes the typed
//! text to the next tag as an NDEF Text record, then reads it back.
//!
//! A phone emulating a tag (host card emulation) appears as an ISO-DEP tag;
//! when the emulating app implements the NFC Forum Type 4 NDEF application,
//! its message reads like any sticker's.

use std::cell::RefCell;
use std::rc::Rc;

use idea_ui::{install_idea_theme, light_theme, Stack, StackGap, StackPadding, Typography};
use nfc::{NdefMessage, NdefRecord, NfcError, NfcSession, SessionOptions, Tag, Tnf};
use runtime_core::{signal, spawn_then, text, ui, Element, Signal};

/// SDK-handler registration seam. `nfc` is a capability crate, not a
/// rendered primitive, so there is nothing to register.
pub fn register_scene_extensions<H: runtime_scene::Host>(
    _registry: &mut runtime_scene::Registry<H>,
) {
}

/// Recorder-side seam for the runtime-server sidecar.
#[cfg(feature = "sidecar")]
pub fn register_scene_extensions_recorder(_registry: &mut dev_server::newcore::SceneRegistry) {}

/// Android entry: the generated wrapper mounts `scene_app()`.
pub fn scene_app() -> Element {
    app()
}

/// The open session, shared with the Stop button so it can end a scan that
/// is still waiting for a tap.
type ActiveSession = Rc<RefCell<Option<Rc<NfcSession>>>>;

/// One line per record: the decoded text / URI, else the type and payload.
fn describe_record(r: &NdefRecord) -> String {
    if let Some(t) = r.as_text() {
        return format!("text [{}] {}", t.language, t.text);
    }
    if let Some(uri) = r.as_uri() {
        return format!("uri {uri}");
    }
    let kind = match r.tnf {
        Tnf::Empty => return "empty record".to_string(),
        Tnf::Media => "mime",
        Tnf::External => "external",
        Tnf::WellKnown => "well-known",
        Tnf::AbsoluteUri => "absolute-uri",
        Tnf::Unknown => "unknown",
        Tnf::Reserved => "reserved",
    };
    format!(
        "{kind} {} ({} bytes: {})",
        String::from_utf8_lossy(&r.record_type),
        r.payload.len(),
        hex(&r.payload[..r.payload.len().min(24)]),
    )
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02X}")).collect::<Vec<_>>().join(":")
}

async fn describe_tag(tag: &Tag) -> String {
    let mut lines = vec![format!("Tag {} {:?}", hex(tag.id()), tag.technologies())];
    match tag.read_ndef().await {
        Ok(None) => lines.push("  not NDEF formatted".to_string()),
        Ok(Some(m)) if m.records.is_empty() => lines.push("  NDEF, empty".to_string()),
        Ok(Some(m)) => lines.extend(m.records.iter().map(|r| format!("  {}", describe_record(r)))),
        Err(e) => lines.push(format!("  read failed: {e}")),
    }
    lines.join("\n")
}

/// Open a session and remember it for Stop. `None` (with the status set)
/// when one is already running or NFC can't start.
fn open_session(active: &ActiveSession, prompt: &str, status: Signal<String>) -> Option<Rc<NfcSession>> {
    if active.borrow().is_some() {
        status.set("A session is already running — Stop it first".to_string());
        return None;
    }
    match NfcSession::begin(SessionOptions::prompt(prompt)) {
        Ok(s) => {
            let s = Rc::new(s);
            *active.borrow_mut() = Some(Rc::clone(&s));
            Some(s)
        }
        Err(e) => {
            status.set(format!("Could not start: {e}"));
            None
        }
    }
}

pub fn app() -> Element {
    install_idea_theme(light_theme());

    let status: Signal<String> = signal(format!("NFC availability: {:?}", nfc::availability()));
    let log: Signal<String> = signal(String::new());
    let draft: Signal<String> = signal("Hello from Idealyst".to_string());
    let active: ActiveSession = Rc::default();

    let on_scan = {
        let active = Rc::clone(&active);
        move || {
            let Some(session) = open_session(&active, "Hold a tag near the device", status) else {
                return;
            };
            status.set("Scanning — tap tags, then Stop".to_string());
            let active = Rc::clone(&active);
            spawn_then(
                async move {
                    loop {
                        match session.next_tag().await {
                            Ok(tag) => {
                                let line = describe_tag(&tag).await;
                                log.set(format!("{line}\n{}", log.get()));
                            }
                            Err(e) => return e,
                        }
                    }
                },
                move |end: NfcError| {
                    active.borrow_mut().take();
                    status.set(format!("Session ended: {end}"));
                },
            );
        }
    };

    let on_write = {
        let active = Rc::clone(&active);
        move || {
            let Some(session) = open_session(&active, "Hold a tag to write", status) else {
                return;
            };
            let body = draft.get();
            status.set(format!("Tap a tag to write \"{body}\""));
            let active = Rc::clone(&active);
            spawn_then(
                async move {
                    let tag = session.next_tag().await?;
                    let message = NdefMessage::new(vec![NdefRecord::text("en", &body)]);
                    tag.write_ndef(&message).await?;
                    let back = describe_tag(&tag).await;
                    session.close();
                    Ok::<_, NfcError>(back)
                },
                move |result| {
                    active.borrow_mut().take();
                    match result {
                        Ok(back) => {
                            status.set("Written".to_string());
                            log.set(format!("Wrote, read back:\n{back}\n{}", log.get()));
                        }
                        Err(e) => status.set(format!("Write failed: {e}")),
                    }
                },
            );
        }
    };

    let on_stop = move || {
        if let Some(session) = active.borrow().as_ref() {
            session.close();
        }
    };

    ui! {
        Stack(gap = StackGap::Md, padding = StackPadding::Lg) {
            Typography(content = "NFC".to_string(), kind = idea_ui::typography_kind::H1)
            text(move || status.get())
            button(label = "Scan".to_string(), on_click = on_scan)
            text_input(value = draft, on_change = move |s| draft.set(s), placeholder = "Text to write")
            button(label = "Write text".to_string(), on_click = on_write)
            button(label = "Stop".to_string(), on_click = on_stop)
            text(move || log.get())
        }
    }
}
