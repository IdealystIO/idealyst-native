//! iOS backend: Core NFC's `NFCTagReaderSession`.
//!
//! [`Session::begin`] creates the session with a declared delegate class
//! ([`NfcTagDelegate`]) and the **main** dispatch queue, so every delegate
//! callback and every completion handler runs on the main thread — where the
//! `!Send` Obj-C objects and the futures awaiting them live. The delegate
//! pushes each detected tag into the session's [`Mailbox`]; an invalidation
//! (user cancel, the 60-second timeout, our own `close`) closes it.
//!
//! Every tag operation first connects to the tag (`connectToTag:`), then
//! issues one Core NFC call whose completion block resolves a oneshot.
//!
//! - NDEF goes through the `NFCNDEFTag` protocol every Core NFC tag adopts.
//!   Messages are written with `+[NFCNDEFMessage ndefMessageWithData:]` from
//!   the crate's own encoding, so the bytes on the tag are exactly
//!   [`NdefMessage::to_bytes`].
//! - Raw commands per tag type: ISO 7816 and MIFARE Plus / DESFire take an
//!   APDU and return the response data followed by SW1 SW2 (the raw R-APDU,
//!   as Android's `IsoDep.transceive` does); MIFARE Ultralight takes native
//!   command bytes; FeliCa takes a packet that **starts with its length
//!   byte** (Android's `NfcF.transceive` form) — Core NFC adds the length
//!   itself, so it is stripped going out and prepended coming back. ISO 15693
//!   has no raw-frame API in Core NFC.
//!
//! Requirements, added by the CLI from the `nfc` capability: the
//! `NFCReaderUsageDescription` Info.plist string and the
//! `com.apple.developer.nfc.readersession.formats` entitlement (`TAG`).
//! Without the entitlement the session invalidates with a security
//! violation, reported as a [`NfcError::Backend`] naming it. FeliCa polling
//! is turned on only when the app's Info.plist lists FeliCa system codes,
//! since Core NFC refuses a FeliCa session without them.
//!
//! VERIFICATION: compile-checked for `aarch64-apple-ios` and
//! `aarch64-apple-ios-sim` only. No NFC hardware was available, and the
//! simulator reports `readingAvailable == NO`, so the session, delegate and
//! completion-handler paths have not run. Method signatures and enum values
//! were taken from the iOS 26.5 SDK's CoreNFC headers.

use std::cell::Cell;
use std::ffi::CStr;
use std::rc::Rc;

use block2::RcBlock;
use futures_channel::oneshot;
use objc2::rc::{Allocated, Retained};
use objc2::runtime::{AnyObject, Bool, MessageReceiver, NSObjectProtocol, Sel};
use objc2::{class, declare_class, msg_send, msg_send_id, mutability, ClassType, DeclaredClass};
use objc2_foundation::{MainThreadMarker, NSData, NSObject, NSString};

use crate::mailbox::Mailbox;
use crate::{Availability, NdefMessage, NdefRecord, NfcError, SessionOptions, Tag, Technology, Tnf};

// Link CoreNFC so `NFCTagReaderSession` / `NFCNDEFMessage` are registered for
// `class!()` lookup. Used only through the Obj-C runtime.
#[link(name = "CoreNFC", kind = "framework")]
extern "C" {}

extern "C" {
    /// The main dispatch queue (`dispatch_get_main_queue()` is an inline
    /// function returning this object's address).
    static _dispatch_main_q: u8;
}

// NFCPollingOption.
const POLL_ISO14443: isize = 0x1;
const POLL_ISO15693: isize = 0x2;
const POLL_ISO18092: isize = 0x4;

// NFCTagType.
const TAG_ISO15693: usize = 1;
const TAG_FELICA: usize = 2;
const TAG_ISO7816: usize = 3;
const TAG_MIFARE: usize = 4;

// NFCMiFareFamily.
const MIFARE_ULTRALIGHT: usize = 2;
const MIFARE_PLUS: usize = 3;
const MIFARE_DESFIRE: usize = 4;

// NFCNDEFStatus.
const NDEF_NOT_SUPPORTED: usize = 1;
const NDEF_READ_ONLY: usize = 3;

// NFCReaderError codes (NFCError.h).
const ERR_UNSUPPORTED_FEATURE: isize = 1;
const ERR_SECURITY_VIOLATION: isize = 2;
const ERR_RADIO_DISABLED: isize = 6;
const ERR_TAG_CONNECTION_LOST: isize = 100;
const ERR_SESSION_INVALIDATED: isize = 103;
const ERR_TAG_NOT_CONNECTED: isize = 104;
const ERR_USER_CANCELED: isize = 200;
const ERR_SESSION_TIMEOUT: isize = 201;
const ERR_SYSTEM_BUSY: isize = 203;
const ERR_TAG_NOT_WRITABLE: isize = 400;
const ERR_ZERO_LENGTH_MESSAGE: isize = 403;

const FELICA_PLIST_KEY: &str = "com.apple.developer.nfc.readersession.felica.systemcodes";

thread_local! {
    /// Id of the session that has begun and not yet been invalidated or
    /// closed; 0 when none. Core NFC runs one reader session at a time.
    static ACTIVE: Cell<u64> = const { Cell::new(0) };
    static NEXT_ID: Cell<u64> = const { Cell::new(1) };
}

fn release_active(id: u64) {
    ACTIVE.with(|a| {
        if a.get() == id {
            a.set(0);
        }
    });
}

pub(crate) fn availability() -> Availability {
    if reading_available() {
        Availability::Available
    } else {
        Availability::Unsupported
    }
}

fn reading_available() -> bool {
    let available: Bool = unsafe { msg_send![class!(NFCTagReaderSession), readingAvailable] };
    available.as_bool()
}

// ===========================================================================
// The delegate.
// ===========================================================================

/// State shared between a session and its delegate.
struct Flags {
    id: u64,
    /// Set by `close`: the invalidation that follows is ours, not an error.
    closed_by_app: Cell<bool>,
}

struct DelegateIvars {
    mailbox: Mailbox<Retained<AnyObject>>,
    flags: Rc<Flags>,
}

declare_class!(
    struct NfcTagDelegate;

    unsafe impl ClassType for NfcTagDelegate {
        type Super = NSObject;
        type Mutability = mutability::MainThreadOnly;
        const NAME: &'static str = "IdealystNfcTagReaderDelegate";
    }

    impl DeclaredClass for NfcTagDelegate {
        type Ivars = DelegateIvars;
    }

    unsafe impl NSObjectProtocol for NfcTagDelegate {}

    unsafe impl NfcTagDelegate {
        #[method(tagReaderSessionDidBecomeActive:)]
        fn did_become_active(&self, _session: *mut AnyObject) {}

        #[method(tagReaderSession:didInvalidateWithError:)]
        fn did_invalidate(&self, _session: *mut AnyObject, error: *mut AnyObject) {
            guard_callback("didInvalidateWithError", || {
                let ivars = self.ivars();
                let reason = if ivars.flags.closed_by_app.get() {
                    NfcError::Cancelled
                } else {
                    unsafe { map_error(error) }
                };
                ivars.mailbox.close(reason);
                release_active(ivars.flags.id);
            });
        }

        #[method(tagReaderSession:didDetectTags:)]
        fn did_detect_tags(&self, _session: *mut AnyObject, tags: *mut AnyObject) {
            guard_callback("didDetectTags", || {
                if tags.is_null() {
                    return;
                }
                // Several tags in the field at once: the first is used, the
                // way a single-tag reader behaves; `next_tag` restarts
                // polling for the rest.
                let first: Option<Retained<AnyObject>> = unsafe { msg_send_id![tags, firstObject] };
                if let Some(tag) = first {
                    self.ivars().mailbox.push(tag);
                }
            });
        }
    }
);

impl NfcTagDelegate {
    fn new(mtm: MainThreadMarker, ivars: DelegateIvars) -> Retained<Self> {
        let this = mtm.alloc::<Self>().set_ivars(ivars);
        unsafe { msg_send_id![super(this), init] }
    }
}

/// Run a delegate body; a panic must not unwind across the Obj-C frame.
fn guard_callback(name: &str, f: impl FnOnce()) {
    if std::panic::catch_unwind(std::panic::AssertUnwindSafe(f)).is_err() {
        eprintln!("nfc: panic in {name}; aborting");
        std::process::abort();
    }
}

/// Mark the delegate class as adopting `NFCTagReaderSessionDelegate`, so a
/// `conformsToProtocol:` check inside Core NFC sees what the methods
/// already implement.
fn adopt_delegate_protocol() {
    use std::sync::Once;
    static ADOPT: Once = Once::new();
    ADOPT.call_once(|| unsafe {
        let proto = objc2::ffi::objc_getProtocol(c"NFCTagReaderSessionDelegate".as_ptr());
        if !proto.is_null() {
            let cls = NfcTagDelegate::class() as *const objc2::runtime::AnyClass;
            objc2::ffi::class_addProtocol(cls as *mut objc2::ffi::objc_class, proto);
        }
    });
}

// ===========================================================================
// Session.
// ===========================================================================

struct SessionState {
    session: Retained<AnyObject>,
    /// The session holds its delegate weakly; this keeps it alive.
    _delegate: Retained<NfcTagDelegate>,
    mailbox: Mailbox<Retained<AnyObject>>,
    flags: Rc<Flags>,
    /// A tag has been handed out, so the next `next_tag` restarts polling.
    delivered: Cell<bool>,
}

impl SessionState {
    fn close(&self, error_message: Option<&str>) {
        if self.flags.closed_by_app.replace(true) {
            return;
        }
        release_active(self.flags.id);
        // Already invalidated by the system (cancel / timeout): nothing to
        // dismiss, and messaging it again only logs a Core NFC warning.
        if !self.mailbox.is_closed() {
            unsafe {
                match error_message {
                    Some(m) => {
                        let m = NSString::from_str(m);
                        let _: () = msg_send![&*self.session, invalidateSessionWithErrorMessage: &*m];
                    }
                    None => {
                        let _: () = msg_send![&*self.session, invalidateSession];
                    }
                }
            }
        }
        self.mailbox.close(NfcError::Cancelled);
    }
}

pub(crate) struct Session {
    state: Rc<SessionState>,
}

impl Session {
    pub(crate) fn begin(options: &SessionOptions) -> Result<Session, NfcError> {
        let Some(mtm) = MainThreadMarker::new() else {
            return Err(NfcError::Backend("an NFC session must be started on the main thread".into()));
        };
        if !reading_available() {
            return Err(NfcError::Unsupported(
                "this device has no NFC reader (iPhone 7 or later; the simulator has none)".into(),
            ));
        }
        if ACTIVE.with(Cell::get) != 0 {
            return Err(NfcError::Busy);
        }

        let id = NEXT_ID.with(|n| {
            let id = n.get();
            n.set(id + 1);
            id
        });
        let flags = Rc::new(Flags { id, closed_by_app: Cell::new(false) });
        let mailbox = Mailbox::new();

        adopt_delegate_protocol();
        let delegate =
            NfcTagDelegate::new(mtm, DelegateIvars { mailbox: mailbox.clone(), flags: Rc::clone(&flags) });

        let mut polling = POLL_ISO14443 | POLL_ISO15693;
        if felica_configured() {
            polling |= POLL_ISO18092;
        }

        let session: Option<Retained<AnyObject>> = unsafe {
            let main_queue = std::ptr::addr_of!(_dispatch_main_q) as *mut AnyObject;
            let alloc: Allocated<AnyObject> = msg_send_id![class!(NFCTagReaderSession), alloc];
            msg_send_id![
                alloc,
                initWithPollingOption: polling,
                delegate: &*delegate,
                queue: main_queue,
            ]
        };
        let Some(session) = session else {
            return Err(NfcError::Backend("NFCTagReaderSession init returned nil".into()));
        };

        unsafe {
            if let Some(prompt) = options.prompt.as_deref() {
                let prompt = NSString::from_str(prompt);
                let _: () = msg_send![&*session, setAlertMessage: &*prompt];
            }
            let _: () = msg_send![&*session, beginSession];
        }
        ACTIVE.with(|a| a.set(id));

        Ok(Session {
            state: Rc::new(SessionState {
                session,
                _delegate: delegate,
                mailbox,
                flags,
                delivered: Cell::new(false),
            }),
        })
    }

    pub(crate) async fn next_tag(&self) -> Result<Tag, NfcError> {
        let state = Rc::clone(&self.state);
        // Core NFC stops polling once it reports a tag; ask for the next one.
        if state.delivered.get() && !state.mailbox.is_closed() {
            unsafe {
                let _: () = msg_send![&*state.session, restartPolling];
            }
        }
        let tag = state.mailbox.recv().await?;
        state.delivered.set(true);
        let (id, technologies, kind) = unsafe { describe(&tag) };
        Ok(Tag::new(id, technologies, TagHandle { session: state, tag, kind, connected: Cell::new(false) }))
    }

    pub(crate) fn set_prompt(&self, text: &str) {
        if self.state.mailbox.is_closed() {
            return;
        }
        let text = NSString::from_str(text);
        unsafe {
            let _: () = msg_send![&*self.state.session, setAlertMessage: &*text];
        }
    }

    pub(crate) fn close(&self, error_message: Option<&str>) {
        self.state.close(error_message);
    }
}

/// Whether the app's Info.plist lists FeliCa system codes. Without them
/// Core NFC rejects a session that polls ISO 18092.
fn felica_configured() -> bool {
    unsafe {
        let bundle: *mut AnyObject = msg_send![class!(NSBundle), mainBundle];
        if bundle.is_null() {
            return false;
        }
        let key = NSString::from_str(FELICA_PLIST_KEY);
        let value: *mut AnyObject = msg_send![bundle, objectForInfoDictionaryKey: &*key];
        if value.is_null() {
            return false;
        }
        let is_array: Bool = msg_send![value, isKindOfClass: class!(NSArray)];
        if !is_array.as_bool() {
            return false;
        }
        let count: usize = msg_send![value, count];
        count > 0
    }
}

// ===========================================================================
// Tags.
// ===========================================================================

/// Which Core NFC protocol a tag speaks, decided once at detection.
#[derive(Clone, Copy)]
enum Kind {
    Iso15693,
    FeliCa,
    Iso7816,
    MiFare { apdu: bool },
    Other,
}

/// Send a zero-argument message whose selector is a Rust keyword (`type`),
/// which `msg_send!` cannot spell.
unsafe fn send_type<R: objc2::encode::EncodeReturn>(obj: *mut AnyObject) -> R {
    MessageReceiver::send_message(obj, Sel::register("type"), ())
}

/// The tag's id, technologies and protocol.
///
/// # Safety
/// `tag` must be a Core NFC tag object.
unsafe fn describe(tag: &AnyObject) -> (Vec<u8>, Vec<Technology>, Kind) {
    let ptr = tag as *const AnyObject as *mut AnyObject;
    let tag_type: usize = send_type(ptr);
    let (id_selector_owner, id_sel, mut techs, kind): (*mut AnyObject, &str, Vec<Technology>, Kind) =
        match tag_type {
            TAG_MIFARE => {
                let family: usize = msg_send![ptr, mifareFamily];
                let mut techs = vec![Technology::NfcA];
                let apdu = matches!(family, MIFARE_PLUS | MIFARE_DESFIRE);
                if family == MIFARE_ULTRALIGHT {
                    techs.push(Technology::MifareUltralight);
                }
                if apdu {
                    techs.push(Technology::IsoDep);
                }
                (ptr, "identifier", techs, Kind::MiFare { apdu })
            }
            TAG_ISO7816 => (ptr, "identifier", vec![Technology::IsoDep], Kind::Iso7816),
            TAG_FELICA => (ptr, "currentIDm", vec![Technology::NfcF], Kind::FeliCa),
            TAG_ISO15693 => (ptr, "identifier", vec![Technology::NfcV], Kind::Iso15693),
            _ => (ptr, "", Vec::new(), Kind::Other),
        };
    // Every Core NFC tag adopts NFCNDEFTag.
    techs.push(Technology::Ndef);
    let id = if id_sel.is_empty() {
        Vec::new()
    } else {
        let data: *mut AnyObject =
            MessageReceiver::send_message(id_selector_owner, Sel::register(id_sel), ());
        data_to_vec(data)
    };
    (id, techs, kind)
}

pub(crate) struct TagHandle {
    session: Rc<SessionState>,
    tag: Retained<AnyObject>,
    kind: Kind,
    connected: Cell<bool>,
}

impl TagHandle {
    fn tag_ptr(&self) -> *mut AnyObject {
        Retained::as_ptr(&self.tag) as *mut AnyObject
    }

    /// Connect to this tag unless it already is the session's connected
    /// tag. Core NFC needs a connection before any command.
    async fn connect(&self) -> Result<(), NfcError> {
        if self.session.mailbox.is_closed() {
            return Err(NfcError::TagLost);
        }
        if self.connected.get() {
            let current: *mut AnyObject = unsafe { msg_send![&*self.session.session, connectedTag] };
            if current == self.tag_ptr() {
                return Ok(());
            }
        }
        let (tx, rx) = oneshot::channel::<Result<(), NfcError>>();
        let tx = Cell::new(Some(tx));
        let block = RcBlock::new(move |error: *mut AnyObject| {
            let result = if error.is_null() { Ok(()) } else { Err(unsafe { map_error(error) }) };
            if let Some(tx) = tx.take() {
                let _ = tx.send(result);
            }
        });
        unsafe {
            let _: () = msg_send![
                &*self.session.session,
                connectToTag: self.tag_ptr(),
                completionHandler: &*block,
            ];
        }
        await_result(rx).await?;
        self.connected.set(true);
        Ok(())
    }

    /// `queryNDEFStatusWithCompletionHandler:` → (status, capacity).
    async fn ndef_status(&self) -> Result<(usize, usize), NfcError> {
        let (tx, rx) = oneshot::channel::<Result<(usize, usize), NfcError>>();
        let tx = Cell::new(Some(tx));
        let block = RcBlock::new(move |status: usize, capacity: usize, error: *mut AnyObject| {
            let result =
                if error.is_null() { Ok((status, capacity)) } else { Err(unsafe { map_error(error) }) };
            if let Some(tx) = tx.take() {
                let _ = tx.send(result);
            }
        });
        unsafe {
            let _: () = msg_send![self.tag_ptr(), queryNDEFStatusWithCompletionHandler: &*block];
        }
        await_result(rx).await
    }

    pub(crate) async fn read_ndef(&self) -> Result<Option<NdefMessage>, NfcError> {
        self.connect().await?;
        let (status, _) = self.ndef_status().await?;
        if status == NDEF_NOT_SUPPORTED {
            return Ok(None);
        }

        let (tx, rx) = oneshot::channel::<Result<Option<NdefMessage>, NfcError>>();
        let tx = Cell::new(Some(tx));
        let block = RcBlock::new(move |message: *mut AnyObject, error: *mut AnyObject| {
            let result = if !error.is_null() {
                // A formatted tag holding a zero-length message.
                if unsafe { nfc_error_code(error) } == Some(ERR_ZERO_LENGTH_MESSAGE) {
                    Ok(Some(NdefMessage::default()))
                } else {
                    Err(unsafe { map_error(error) })
                }
            } else {
                Ok(Some(unsafe { message_from_objc(message) }))
            };
            if let Some(tx) = tx.take() {
                let _ = tx.send(result);
            }
        });
        unsafe {
            let _: () = msg_send![self.tag_ptr(), readNDEFWithCompletionHandler: &*block];
        }
        await_result(rx).await
    }

    pub(crate) async fn write_ndef(&self, message: &NdefMessage) -> Result<(), NfcError> {
        // Core NFC has no zero-record message; the Empty record is the
        // standard way to store "nothing" on a formatted tag.
        let bytes = if message.records.is_empty() {
            NdefMessage::new(vec![NdefRecord::empty()]).to_bytes()?
        } else {
            message.to_bytes()?
        };

        self.connect().await?;
        let (status, capacity) = self.ndef_status().await?;
        match status {
            NDEF_NOT_SUPPORTED => return Err(NfcError::NotNdef),
            NDEF_READ_ONLY => return Err(NfcError::ReadOnly),
            _ => {}
        }
        if bytes.len() > capacity {
            return Err(NfcError::TooLarge { needed: bytes.len(), capacity });
        }

        let data = NSData::with_bytes(&bytes);
        let objc_message: *mut AnyObject =
            unsafe { msg_send![class!(NFCNDEFMessage), ndefMessageWithData: &*data] };
        if objc_message.is_null() {
            return Err(NfcError::Backend("Core NFC rejected the encoded NDEF message".into()));
        }

        let (tx, rx) = oneshot::channel::<Result<(), NfcError>>();
        let tx = Cell::new(Some(tx));
        let block = RcBlock::new(move |error: *mut AnyObject| {
            let result = if error.is_null() { Ok(()) } else { Err(unsafe { map_error(error) }) };
            if let Some(tx) = tx.take() {
                let _ = tx.send(result);
            }
        });
        unsafe {
            let _: () = msg_send![
                self.tag_ptr(),
                writeNDEF: objc_message,
                completionHandler: &*block,
            ];
        }
        await_result(rx).await
    }

    pub(crate) async fn transceive(&self, command: &[u8]) -> Result<Vec<u8>, NfcError> {
        match self.kind {
            Kind::Iso7816 | Kind::MiFare { apdu: true } => {
                self.connect().await?;
                self.send_apdu(command).await
            }
            Kind::MiFare { apdu: false } => {
                self.connect().await?;
                self.send_data(command, "sendMiFareCommand:completionHandler:").await
            }
            Kind::FeliCa => {
                // Android form: [LEN, packet…] with LEN counting itself.
                let Some((&len, packet)) = command.split_first() else {
                    return Err(NfcError::Backend("empty FeliCa command".into()));
                };
                if len as usize != command.len() {
                    return Err(NfcError::Backend(format!(
                        "FeliCa command length byte is {len}, but the command is {} bytes",
                        command.len()
                    )));
                }
                self.connect().await?;
                let response =
                    self.send_data(packet, "sendFeliCaCommandPacket:completionHandler:").await?;
                let mut out = Vec::with_capacity(response.len() + 1);
                out.push((response.len() + 1) as u8);
                out.extend_from_slice(&response);
                Ok(out)
            }
            Kind::Iso15693 => Err(NfcError::NotAvailable(
                "raw ISO 15693 frames on iOS (Core NFC exposes only typed ISO 15693 commands)",
            )),
            Kind::Other => Err(NfcError::NotAvailable("raw commands to this tag type on iOS")),
        }
    }

    /// An `(NSData, NSError)`-completion command (`sendMiFareCommand:` /
    /// `sendFeliCaCommandPacket:`).
    async fn send_data(&self, command: &[u8], selector: &str) -> Result<Vec<u8>, NfcError> {
        let (tx, rx) = oneshot::channel::<Result<Vec<u8>, NfcError>>();
        let tx = Cell::new(Some(tx));
        let block = RcBlock::new(move |response: *mut AnyObject, error: *mut AnyObject| {
            let result =
                if error.is_null() { Ok(unsafe { data_to_vec(response) }) } else { Err(unsafe { map_error(error) }) };
            if let Some(tx) = tx.take() {
                let _ = tx.send(result);
            }
        });
        let data = NSData::with_bytes(command);
        unsafe {
            let _: () = MessageReceiver::send_message(
                self.tag_ptr(),
                Sel::register(selector),
                (Retained::as_ptr(&data) as *mut AnyObject, &*block),
            );
        }
        await_result(rx).await
    }

    /// Send an ISO 7816 APDU; the result is the response data then SW1 SW2.
    async fn send_apdu(&self, command: &[u8]) -> Result<Vec<u8>, NfcError> {
        let data = NSData::with_bytes(command);
        let apdu: Option<Retained<AnyObject>> = unsafe {
            let alloc: Allocated<AnyObject> = msg_send_id![class!(NFCISO7816APDU), alloc];
            msg_send_id![alloc, initWithData: &*data]
        };
        let Some(apdu) = apdu else {
            return Err(NfcError::Backend("not a valid ISO 7816 command APDU".into()));
        };

        let (tx, rx) = oneshot::channel::<Result<Vec<u8>, NfcError>>();
        let tx = Cell::new(Some(tx));
        let block = RcBlock::new(move |response: *mut AnyObject, sw1: u8, sw2: u8, error: *mut AnyObject| {
            let result = if error.is_null() {
                let mut out = unsafe { data_to_vec(response) };
                out.push(sw1);
                out.push(sw2);
                Ok(out)
            } else {
                Err(unsafe { map_error(error) })
            };
            if let Some(tx) = tx.take() {
                let _ = tx.send(result);
            }
        });
        let selector = match self.kind {
            Kind::MiFare { .. } => "sendMiFareISO7816Command:completionHandler:",
            _ => "sendCommandAPDU:completionHandler:",
        };
        unsafe {
            let _: () = MessageReceiver::send_message(
                self.tag_ptr(),
                Sel::register(selector),
                (Retained::as_ptr(&apdu) as *mut AnyObject, &*block),
            );
        }
        await_result(rx).await
    }
}

/// Await a completion handler's result. Core NFC calls every handler
/// exactly once; a dropped sender means the session went away first.
async fn await_result<T>(rx: oneshot::Receiver<Result<T, NfcError>>) -> Result<T, NfcError> {
    rx.await.unwrap_or(Err(NfcError::TagLost))
}

// ===========================================================================
// Conversions.
// ===========================================================================

/// `NSData` → bytes; empty for nil.
///
/// # Safety
/// `data` must be an `NSData*` or null.
unsafe fn data_to_vec(data: *mut AnyObject) -> Vec<u8> {
    if data.is_null() {
        return Vec::new();
    }
    let len: usize = msg_send![data, length];
    if len == 0 {
        return Vec::new();
    }
    let bytes: *const u8 = msg_send![data, bytes];
    std::slice::from_raw_parts(bytes, len).to_vec()
}

/// An `NFCNDEFMessage` → [`NdefMessage`].
///
/// # Safety
/// `message` must be an `NFCNDEFMessage*` or null.
unsafe fn message_from_objc(message: *mut AnyObject) -> NdefMessage {
    if message.is_null() {
        return NdefMessage::default();
    }
    let records: *mut AnyObject = msg_send![message, records];
    if records.is_null() {
        return NdefMessage::default();
    }
    let count: usize = msg_send![records, count];
    let mut out = Vec::with_capacity(count);
    for i in 0..count {
        let payload: *mut AnyObject = msg_send![records, objectAtIndex: i];
        let tnf: u8 = msg_send![payload, typeNameFormat];
        let record_type: *mut AnyObject = send_type(payload);
        let id: *mut AnyObject = msg_send![payload, identifier];
        let body: *mut AnyObject = msg_send![payload, payload];
        out.push(NdefRecord::new(
            // Core NFC reassembles chunks, so Unchanged never reaches here;
            // map it to Unknown rather than dropping the record.
            Tnf::from_bits(tnf).unwrap_or(Tnf::Unknown),
            data_to_vec(record_type),
            data_to_vec(id),
            data_to_vec(body),
        ));
    }
    NdefMessage::new(out)
}

/// The `NFCReaderError` code of an `NSError` in `NFCErrorDomain`.
///
/// # Safety
/// `error` must be an `NSError*`.
unsafe fn nfc_error_code(error: *mut AnyObject) -> Option<isize> {
    let domain: *mut AnyObject = msg_send![error, domain];
    if domain.is_null() || ns_string(domain) != "NFCError" {
        return None;
    }
    Some(msg_send![error, code])
}

/// Map a Core NFC `NSError` to an [`NfcError`].
///
/// # Safety
/// `error` must be an `NSError*` or null.
unsafe fn map_error(error: *mut AnyObject) -> NfcError {
    if error.is_null() {
        return NfcError::Backend("Core NFC reported an unspecified error".into());
    }
    let description = || {
        let d: *mut AnyObject = msg_send![error, localizedDescription];
        if d.is_null() {
            String::from("unknown error")
        } else {
            ns_string(d)
        }
    };
    match nfc_error_code(error) {
        Some(ERR_USER_CANCELED) => NfcError::Cancelled,
        Some(ERR_SESSION_TIMEOUT) => NfcError::Timeout,
        Some(ERR_SYSTEM_BUSY) => NfcError::Busy,
        Some(ERR_RADIO_DISABLED) => NfcError::Disabled,
        Some(ERR_UNSUPPORTED_FEATURE) => NfcError::Unsupported(description()),
        Some(ERR_TAG_CONNECTION_LOST | ERR_SESSION_INVALIDATED | ERR_TAG_NOT_CONNECTED) => NfcError::TagLost,
        Some(ERR_TAG_NOT_WRITABLE) => NfcError::ReadOnly,
        Some(ERR_SECURITY_VIOLATION) => NfcError::Backend(format!(
            "{} — the app is missing the com.apple.developer.nfc.readersession.formats \
             entitlement, or selected an ISO 7816 AID not listed in its Info.plist",
            description()
        )),
        Some(code) => NfcError::Backend(format!("NFCReaderError {code}: {}", description())),
        None => NfcError::Backend(description()),
    }
}

/// `NSString` → `String`.
///
/// # Safety
/// `s` must be a non-null `NSString*`.
unsafe fn ns_string(s: *mut AnyObject) -> String {
    let cstr: *const std::os::raw::c_char = msg_send![s, UTF8String];
    if cstr.is_null() {
        return String::new();
    }
    CStr::from_ptr(cstr).to_string_lossy().into_owned()
}
