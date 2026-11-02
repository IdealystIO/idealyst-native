//! Desktop backend: a USB contactless reader through PC/SC (`PCSC.framework`
//! on macOS, `winscard.dll` on Windows, `libpcsclite` on Linux).
//!
//! A session runs a poll thread that waits on `SCardGetStatusChange` across
//! every connected reader (and the PnP pseudo-reader, so a reader plugged in
//! mid-session is picked up). When a card appears, the thread connects,
//! reads the UID with the PC/SC GET DATA command (`FF CA 00 00 00`) and the
//! technology from the ATR, and hands the tag to the session's mailbox.
//!
//! PC/SC exposes only a command channel, so NDEF goes through
//! [`crate::tagproto`]: the Type 4 mapping for ISO-DEP tags and the Type 2
//! mapping (over the PC/SC storage-card READ/UPDATE BINARY pseudo-APDUs) for
//! MIFARE Ultralight / NTAG. MIFARE Classic NDEF needs sector keys and is
//! not offered. Card I/O blocks, so every operation runs on its own thread
//! and resolves a oneshot; the UI thread never waits on the reader.
//!
//! VERIFICATION: the NDEF mappings are unit-tested against simulated Type 2
//! and Type 4 tags (`tagproto` tests). The PC/SC plumbing is exercised on
//! macOS up to "no reader connected" (`availability()` → `Disabled`); a run
//! against a physical reader has not been done in this repo.

use std::ffi::CStr;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::Duration;

use futures_channel::oneshot;
use pcsc::{Attribute, Card, Context, Protocols, ReaderState, Scope, ShareMode, State};

use crate::mailbox::Mailbox;
use crate::tagproto::{self, Transport};
use crate::{Availability, NdefMessage, NdefRecord, NfcError, SessionOptions, Tag, Technology};

/// One session at a time, matching the mobile platforms.
static ACTIVE: AtomicBool = AtomicBool::new(false);

/// How long the poll thread blocks in one status-change wait before it
/// re-checks for a stop request. `Context::cancel` interrupts the wait
/// sooner; this bounds the case where cancel races the wait's start.
const POLL_TIMEOUT: Duration = Duration::from_millis(500);

fn pcsc_err(e: pcsc::Error) -> NfcError {
    match e {
        pcsc::Error::RemovedCard | pcsc::Error::ResetCard | pcsc::Error::NoSmartcard => {
            NfcError::TagLost
        }
        pcsc::Error::NoReadersAvailable | pcsc::Error::ReaderUnavailable => NfcError::Disabled,
        pcsc::Error::NoService | pcsc::Error::ServiceStopped => {
            NfcError::Unsupported("the PC/SC service is not running".into())
        }
        other => NfcError::Backend(format!("PC/SC: {other}")),
    }
}

pub(crate) fn availability() -> Availability {
    let Ok(ctx) = Context::establish(Scope::User) else {
        return Availability::Unsupported;
    };
    match ctx.list_readers_len() {
        // The length includes the list's terminating NUL; an empty list is
        // one or two bytes depending on the platform.
        Ok(len) if len > 2 => Availability::Available,
        Ok(_) | Err(pcsc::Error::NoReadersAvailable) => Availability::Disabled,
        Err(_) => Availability::Unsupported,
    }
}

pub(crate) struct Session {
    ctx: Context,
    mailbox: Mailbox<Tag>,
    stop: Arc<AtomicBool>,
    closed: AtomicBool,
}

impl Session {
    pub(crate) fn begin(_options: &SessionOptions) -> Result<Session, NfcError> {
        let ctx = Context::establish(Scope::User).map_err(pcsc_err)?;
        if ACTIVE.swap(true, Ordering::SeqCst) {
            return Err(NfcError::Busy);
        }
        let mailbox = Mailbox::new();
        let stop = Arc::new(AtomicBool::new(false));
        let thread_ctx = ctx.clone();
        let thread_mailbox = mailbox.clone();
        let thread_stop = Arc::clone(&stop);
        let spawned = thread::Builder::new()
            .name("nfc-pcsc-poll".into())
            .spawn(move || poll_loop(thread_ctx, thread_mailbox, thread_stop));
        if let Err(e) = spawned {
            ACTIVE.store(false, Ordering::SeqCst);
            return Err(NfcError::Backend(format!("spawn PC/SC poll thread: {e}")));
        }
        Ok(Session { ctx, mailbox, stop, closed: AtomicBool::new(false) })
    }

    pub(crate) async fn next_tag(&self) -> Result<Tag, NfcError> {
        self.mailbox.recv().await
    }

    pub(crate) fn set_prompt(&self, _text: &str) {}

    pub(crate) fn close(&self, _error_message: Option<&str>) {
        if self.closed.swap(true, Ordering::SeqCst) {
            return;
        }
        self.stop.store(true, Ordering::SeqCst);
        let _ = self.ctx.cancel();
        self.mailbox.close(NfcError::Cancelled);
        ACTIVE.store(false, Ordering::SeqCst);
    }
}

/// Wait for cards on every reader until stopped, pushing each new one.
fn poll_loop(ctx: Context, mailbox: Mailbox<Tag>, stop: Arc<AtomicBool>) {
    let mut states = vec![ReaderState::new(pcsc::PNP_NOTIFICATION(), State::UNAWARE)];
    while !stop.load(Ordering::SeqCst) {
        // Drop readers that went away, add new ones.
        states.retain(|rs| {
            !rs.event_state().intersects(State::UNKNOWN | State::IGNORE)
                || is_pnp(rs.name())
        });
        match ctx.list_readers_owned() {
            Ok(names) => {
                for name in names {
                    if !states.iter().any(|rs| rs.name() == name.as_c_str()) {
                        states.push(ReaderState::new(name, State::UNAWARE));
                    }
                }
            }
            Err(pcsc::Error::NoReadersAvailable) => {}
            Err(e) => {
                mailbox.close(pcsc_err(e));
                return;
            }
        }
        for rs in &mut states {
            rs.sync_current_state();
        }
        match ctx.get_status_change(POLL_TIMEOUT, &mut states) {
            Ok(()) => {}
            Err(pcsc::Error::Timeout) => continue,
            Err(pcsc::Error::Cancelled) => return,
            Err(e) => {
                mailbox.close(pcsc_err(e));
                return;
            }
        }
        for rs in &states {
            if is_pnp(rs.name()) {
                continue;
            }
            let arrived = rs.event_state().contains(State::CHANGED | State::PRESENT)
                && !rs.current_state().contains(State::PRESENT);
            if arrived {
                match connect_tag(&ctx, rs.name()) {
                    Ok(tag) => mailbox.push(tag),
                    // A card pulled away mid-connect: wait for the next one.
                    Err(NfcError::TagLost) => {}
                    Err(e) => {
                        mailbox.close(e);
                        return;
                    }
                }
            }
        }
    }
}

fn is_pnp(name: &CStr) -> bool {
    name == pcsc::PNP_NOTIFICATION()
}

/// What a tag is, read from its ATR. PC/SC Part 3 gives contactless
/// storage cards a synthetic ATR carrying the PC/SC RID (A0 00 00 03 06),
/// a standard byte and a two-byte card name; anything else is an ISO-DEP
/// (ISO 14443-4) card answering with its own ATR.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Kind {
    IsoDep,
    Type2,
    MifareClassic,
    FeliCa,
    Iso15693,
    OtherStorage,
}

const PCSC_RID: [u8; 5] = [0xA0, 0x00, 0x00, 0x03, 0x06];

fn classify_atr(atr: &[u8]) -> Kind {
    let Some(at) = atr.windows(PCSC_RID.len()).position(|w| w == PCSC_RID) else {
        return Kind::IsoDep;
    };
    let standard = atr.get(at + 5).copied();
    let name = atr.get(at + 6..at + 8).map(|n| u16::from_be_bytes([n[0], n[1]]));
    match (standard, name) {
        // Card names from the PC/SC Part 3 supplement.
        (_, Some(0x0001 | 0x0002 | 0x0026)) => Kind::MifareClassic,
        (_, Some(0x0003 | 0x003A | 0x003C..=0x003E)) => Kind::Type2,
        (_, Some(0x003B)) | (Some(0x11), _) => Kind::FeliCa,
        (Some(0x0B | 0x0C), _) => Kind::Iso15693,
        // An ISO 14443A storage card the reader doesn't name: NTAG parts
        // on older reader firmware. Type 2 is the only NDEF mapping that
        // fits, and `type2_open` checks the CC magic before trusting it.
        (Some(0x03), _) => Kind::Type2,
        _ => Kind::OtherStorage,
    }
}

fn technologies(kind: Kind) -> Vec<Technology> {
    match kind {
        Kind::IsoDep => vec![Technology::IsoDep],
        Kind::Type2 => vec![Technology::NfcA, Technology::MifareUltralight],
        Kind::MifareClassic => vec![Technology::NfcA, Technology::MifareClassic],
        Kind::FeliCa => vec![Technology::NfcF],
        Kind::Iso15693 => vec![Technology::NfcV],
        Kind::OtherStorage => vec![],
    }
}

fn connect_tag(ctx: &Context, reader: &CStr) -> Result<Tag, NfcError> {
    let card = ctx.connect(reader, ShareMode::Shared, Protocols::ANY).map_err(pcsc_err)?;
    let atr = card.get_attribute_owned(Attribute::AtrString).map_err(pcsc_err)?;
    let kind = classify_atr(&atr);
    let mut card = PcscCard(card);
    // GET DATA (UID). Readers that lack it answer an error status; the tag
    // is still usable, just without an id.
    let id = card
        .transmit(&[0xFF, 0xCA, 0x00, 0x00, 0x00])
        .ok()
        .filter(|r| r.ends_with(&[0x90, 0x00]))
        .map(|r| r[..r.len() - 2].to_vec())
        .unwrap_or_default();
    let handle = TagHandle { card: Arc::new(Mutex::new(card)), kind };
    Ok(Tag::new(id, technologies(kind), handle))
}

struct PcscCard(Card);

impl Transport for PcscCard {
    fn transmit(&mut self, apdu: &[u8]) -> Result<Vec<u8>, NfcError> {
        let mut buf = [0u8; pcsc::MAX_BUFFER_SIZE_EXTENDED];
        let resp = self.0.transmit(apdu, &mut buf).map_err(pcsc_err)?;
        Ok(resp.to_vec())
    }
}

pub(crate) struct TagHandle {
    card: Arc<Mutex<PcscCard>>,
    kind: Kind,
}

impl TagHandle {
    /// Run `op` against the card on a worker thread.
    async fn run<T: Send + 'static>(
        &self,
        op: impl FnOnce(&mut PcscCard, Kind) -> Result<T, NfcError> + Send + 'static,
    ) -> Result<T, NfcError> {
        let (tx, rx) = oneshot::channel();
        let card = Arc::clone(&self.card);
        let kind = self.kind;
        thread::Builder::new()
            .name("nfc-pcsc-op".into())
            .spawn(move || {
                let result = op(&mut card.lock().unwrap(), kind);
                let _ = tx.send(result);
            })
            .map_err(|e| NfcError::Backend(format!("spawn PC/SC worker: {e}")))?;
        rx.await.map_err(|_| NfcError::Backend("PC/SC worker exited without a result".into()))?
    }

    pub(crate) async fn read_ndef(&self) -> Result<Option<NdefMessage>, NfcError> {
        self.run(|card, kind| match kind {
            Kind::IsoDep => tagproto::type4_read(card),
            Kind::Type2 => tagproto::type2_read(card),
            Kind::MifareClassic => {
                Err(NfcError::NotAvailable("NDEF on MIFARE Classic through a PC/SC reader"))
            }
            Kind::FeliCa | Kind::Iso15693 | Kind::OtherStorage => Ok(None),
        })
        .await
    }

    pub(crate) async fn write_ndef(&self, message: &NdefMessage) -> Result<(), NfcError> {
        let bytes = if message.records.is_empty() {
            NdefMessage::new(vec![NdefRecord::empty()]).to_bytes()?
        } else {
            message.to_bytes()?
        };
        self.run(move |card, kind| {
            let written = match kind {
                Kind::IsoDep => tagproto::type4_write(card, &bytes)?,
                Kind::Type2 => tagproto::type2_write(card, &bytes)?,
                Kind::MifareClassic => {
                    return Err(NfcError::NotAvailable(
                        "NDEF on MIFARE Classic through a PC/SC reader",
                    ))
                }
                Kind::FeliCa | Kind::Iso15693 | Kind::OtherStorage => false,
            };
            if written { Ok(()) } else { Err(NfcError::NotNdef) }
        })
        .await
    }

    pub(crate) async fn transceive(&self, command: &[u8]) -> Result<Vec<u8>, NfcError> {
        let command = command.to_vec();
        self.run(move |card, _| card.transmit(&command)).await
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn classify_atr_reads_pcsc_part3_card_names() {
        // ACR122U ATRs: 3B 8F 80 01 80 4F 0C A0 00 00 03 06 SS NN NN 00 00 00 00 xx
        let atr = |ss: u8, nn: [u8; 2]| {
            let mut a = vec![0x3B, 0x8F, 0x80, 0x01, 0x80, 0x4F, 0x0C];
            a.extend_from_slice(&PCSC_RID);
            a.extend_from_slice(&[ss, nn[0], nn[1], 0, 0, 0, 0, 0x6A]);
            a
        };
        assert_eq!(classify_atr(&atr(0x03, [0x00, 0x01])), Kind::MifareClassic);
        assert_eq!(classify_atr(&atr(0x03, [0x00, 0x03])), Kind::Type2);
        assert_eq!(classify_atr(&atr(0x03, [0x00, 0x00])), Kind::Type2);
        assert_eq!(classify_atr(&atr(0x11, [0x00, 0x3B])), Kind::FeliCa);
        assert_eq!(classify_atr(&atr(0x0B, [0x00, 0x00])), Kind::Iso15693);
        // A DESFire / phone HCE answers with its own ATR: ISO-DEP.
        assert_eq!(classify_atr(&[0x3B, 0x81, 0x80, 0x01, 0x80, 0x80]), Kind::IsoDep);
    }
}
