//! iOS / macOS / tvOS local notifications and responses via
//! `UNUserNotificationCenter` (objc2).
//!
//! **Compile-checked only ⚠️** — this messages real UserNotifications APIs
//! through the Obj-C runtime; it has not been exercised on a device from
//! this crate. Selectors and constants are the iOS 26 SDK's
//! `UserNotifications` headers.
//!
//! ## Posting
//!
//! `notify` / `schedule` build a `UNMutableNotificationContent` — title,
//! subtitle, body, `userInfo` (the payload), `threadIdentifier` (group),
//! the default sound unless [`Sound::Silent`], `interruptionLevel` (iOS 15
//! / macOS 12, from [`Priority`]), an image attachment, and the category
//! that carries the actions — run the app's [`Notification::apple`] hook on
//! it, and add a `UNNotificationRequest` keyed by the resolved id (a
//! request under a delivered id replaces it, and alerts again). Immediate
//! posts use a `nil` trigger; delayed posts a
//! `UNTimeIntervalNotificationTrigger`.
//!
//! [`Notification::alert_once`] has no direct counterpart: when
//! `getDeliveredNotificationsWithCompletionHandler:` lists the id, the
//! replacement goes out without a sound at the passive interruption level
//! (added to the list without lighting the screen), and `willPresent`
//! shows it in the list only.
//!
//! ## Actions are categories
//!
//! UserNotifications attaches buttons through a *category* registered on
//! the center ahead of the post, not on the notification. Each distinct
//! action list becomes a category whose id is a hash of the list, every
//! category seen is kept, and the full set is re-registered when a new one
//! appears (`setNotificationCategories:` replaces the set). Registration
//! is asynchronous inside the OS: a request posted straight after it can
//! show without its buttons, so the post first waits on
//! `getNotificationCategoriesWithCompletionHandler:`, which answers only
//! after the set has been applied. Every category — including the one for
//! a notification with no actions — carries `customDismissAction`, so a
//! dismissal is reported like on the other platforms.
//!
//! ## Responses
//!
//! [`init`] installs this crate's `UNUserNotificationCenterDelegate` on the
//! center. It must be installed before the app finishes launching: the
//! response that launched the app is delivered to whatever delegate is set
//! by then, and to nobody otherwise. The delegate:
//! - `didReceiveNotificationResponse:` → a [`NotificationResponse`] (the
//!   default action is [`ResponseAction::Open`], the dismiss action
//!   [`ResponseAction::Dismiss`], any other an action id; a
//!   `UNTextInputNotificationResponse` carries the reply text);
//! - `willPresentNotification:` → show the banner, list entry and sound
//!   while the app is in the foreground. Without a delegate iOS shows
//!   nothing for a foreground app, which no other platform does.
//!
//! The center holds its delegate weakly; [`DELEGATE`] keeps ours alive for
//! the life of the process.
//!
//! ## No app bundle
//!
//! `currentNotificationCenter` raises an Objective-C exception (an abort)
//! in a process without a bundle identifier — a `cargo test` binary, a CLI
//! tool. Every entry point checks for one first and reports
//! [`NotifyError::Backend`] (or does nothing) instead.
//!
//! ## APNs token = host seam
//!
//! [`push_token`] needs `registerForRemoteNotifications` and the token
//! delivered to the app delegate's
//! `didRegisterForRemoteNotificationsWithDeviceToken:`, plus the
//! `aps-environment` entitlement. Until the host installs that bridge, it
//! reports `NotSupported`.

use std::cell::OnceCell;
use std::collections::{BTreeMap, HashMap};
use std::ffi::CStr;
use std::sync::Mutex;

use block2::{Block, RcBlock};
use objc2::rc::Retained;
use objc2::runtime::{AnyClass, AnyObject, Bool, NSObjectProtocol};
use objc2::{class, declare_class, msg_send, msg_send_id, mutability, sel, ClassType, DeclaredClass};
use objc2_foundation::{NSObject, NSString};

use crate::{
    deliver_response, resolve_id, Action, Notification, NotificationId, NotifyError, Priority, PushToken,
    ResponseAction, Sound,
};

#[link(name = "UserNotifications", kind = "framework")]
extern "C" {}

// UNNotificationActionOptions.
const ACTION_AUTH_REQUIRED: usize = 1 << 0;
const ACTION_DESTRUCTIVE: usize = 1 << 1;
const ACTION_FOREGROUND: usize = 1 << 2;
// UNNotificationCategoryOptions.customDismissAction.
const CATEGORY_CUSTOM_DISMISS: usize = 1 << 0;
// UNNotificationPresentationOptions: badge | sound | list | banner.
const PRESENT_IN_FOREGROUND: usize = (1 << 0) | (1 << 1) | (1 << 3) | (1 << 4);
// UNNotificationPresentationOptions: list only — a quiet replacement.
const PRESENT_QUIETLY: usize = 1 << 3;
/// The `userInfo` key marking a quiet [`Notification::alert_once`]
/// replacement, so `willPresent` holds back the banner and sound. Reserved
/// (`idealyst.`), so responses never report it.
const QUIET_KEY: &str = "idealyst.quiet";
// UNNotificationInterruptionLevel.
const LEVEL_PASSIVE: usize = 0;
const LEVEL_ACTIVE: usize = 1;
const LEVEL_TIME_SENSITIVE: usize = 2;

const DEFAULT_ACTION: &str = "com.apple.UNNotificationDefaultActionIdentifier";
const DISMISS_ACTION: &str = "com.apple.UNNotificationDismissActionIdentifier";
/// The category of a notification without actions: present so its
/// dismissal is reported.
const PLAIN_CATEGORY: &str = "idealyst.plain";
/// UserNotifications shows at most four actions on a notification.
const MAX_ACTIONS: usize = 4;

// ---------------------------------------------------------------------------
// Delegate.
// ---------------------------------------------------------------------------

declare_class!(
    struct CenterDelegate;

    unsafe impl ClassType for CenterDelegate {
        type Super = NSObject;
        // The center calls its delegate on a queue of its choosing; the
        // delegate holds no state, and `deliver_response` is thread-safe.
        type Mutability = mutability::InteriorMutable;
        const NAME: &'static str = "IdealystNotificationCenterDelegate";
    }

    impl DeclaredClass for CenterDelegate {}

    unsafe impl NSObjectProtocol for CenterDelegate {}

    unsafe impl CenterDelegate {
        #[method(userNotificationCenter:willPresentNotification:withCompletionHandler:)]
        fn will_present(&self, _center: *mut AnyObject, notification: *mut AnyObject, handler: &Block<dyn Fn(usize)>) {
            guard_callback("willPresentNotification", || {
                let options = if unsafe { is_quiet(notification) } { PRESENT_QUIETLY } else { PRESENT_IN_FOREGROUND };
                handler.call((options,))
            });
        }

        #[method(userNotificationCenter:didReceiveNotificationResponse:withCompletionHandler:)]
        fn did_receive(&self, _center: *mut AnyObject, response: *mut AnyObject, handler: &Block<dyn Fn()>) {
            guard_callback("didReceiveNotificationResponse", || {
                if !response.is_null() {
                    unsafe { deliver_platform_response(response) };
                }
                handler.call(());
            });
        }
    }
);

impl CenterDelegate {
    fn new() -> Retained<Self> {
        let this = Self::alloc().set_ivars(());
        unsafe { msg_send_id![super(this), init] }
    }
}

thread_local! {
    /// Our delegate, kept alive: the center references its delegate weakly.
    static DELEGATE: OnceCell<Retained<CenterDelegate>> = const { OnceCell::new() };
}

/// A delegate body must not unwind across the Obj-C frame.
fn guard_callback(name: &str, f: impl FnOnce()) {
    if std::panic::catch_unwind(std::panic::AssertUnwindSafe(f)).is_err() {
        eprintln!("notifications: panic in {name}; aborting");
        std::process::abort();
    }
}

/// Mark the delegate class as adopting `UNUserNotificationCenterDelegate`,
/// so a `conformsToProtocol:` check sees what the methods implement.
fn adopt_delegate_protocol() {
    unsafe {
        let proto = objc2::ffi::objc_getProtocol(c"UNUserNotificationCenterDelegate".as_ptr());
        if !proto.is_null() {
            let cls = CenterDelegate::class() as *const AnyClass;
            objc2::ffi::class_addProtocol(cls as *mut objc2::ffi::objc_class, proto);
        }
    }
}

pub(super) fn init() {
    if !has_bundle() {
        return;
    }
    adopt_delegate_protocol();
    DELEGATE.with(|cell| {
        let delegate = cell.get_or_init(CenterDelegate::new);
        unsafe {
            let _: () = msg_send![center(), setDelegate: &**delegate];
        }
    });
}

/// Turn a `UNNotificationResponse` into a [`crate::NotificationResponse`]
/// and hand it to the app's handlers.
unsafe fn deliver_platform_response(response: *mut AnyObject) {
    let notification: *mut AnyObject = msg_send![response, notification];
    let request: *mut AnyObject = msg_send![notification, request];
    let id = ns_to_string(msg_send![request, identifier]).unwrap_or_default();
    let content: *mut AnyObject = msg_send![request, content];
    let data = string_dictionary(msg_send![content, userInfo]);

    let action_id = ns_to_string(msg_send![response, actionIdentifier]).unwrap_or_default();
    let action = match action_id.as_str() {
        DEFAULT_ACTION => ResponseAction::Open,
        DISMISS_ACTION => ResponseAction::Dismiss,
        _ => ResponseAction::Action(action_id),
    };
    let text_class = AnyClass::get("UNTextInputNotificationResponse");
    let is_text: Bool = match text_class {
        Some(cls) => msg_send![response, isKindOfClass: cls],
        None => Bool::NO,
    };
    let text = if is_text.as_bool() { ns_to_string(msg_send![response, userText]) } else { None };

    deliver_response(NotificationId(id), action, text, data);
}

// ---------------------------------------------------------------------------
// Posting.
// ---------------------------------------------------------------------------

/// Whether this process has a bundle identifier — without one,
/// `currentNotificationCenter` raises (see the module docs).
fn has_bundle() -> bool {
    unsafe {
        let bundle: *mut AnyObject = msg_send![class!(NSBundle), mainBundle];
        if bundle.is_null() {
            return false;
        }
        let ident: *mut AnyObject = msg_send![bundle, bundleIdentifier];
        !ident.is_null()
    }
}

fn require_bundle() -> Result<(), NotifyError> {
    if has_bundle() {
        Ok(())
    } else {
        Err(NotifyError::Backend("UserNotifications needs an app bundle (no bundle identifier)".into()))
    }
}

/// `[UNUserNotificationCenter currentNotificationCenter]` — a process-wide
/// singleton; the raw pointer is valid for the program's lifetime without
/// retaining.
unsafe fn center() -> *mut AnyObject {
    msg_send![class!(UNUserNotificationCenter), currentNotificationCenter]
}

/// Whether `notification` (a `UNNotification`) is a quiet replacement.
unsafe fn is_quiet(notification: *mut AnyObject) -> bool {
    if notification.is_null() {
        return false;
    }
    let request: *mut AnyObject = msg_send![notification, request];
    let content: *mut AnyObject = msg_send![request, content];
    let info: *mut AnyObject = msg_send![content, userInfo];
    if info.is_null() {
        return false;
    }
    let value: *mut AnyObject = msg_send![info, objectForKey: &*NSString::from_str(QUIET_KEY)];
    !value.is_null()
}

/// Whether a notification posted under `id` is still delivered (on screen
/// or in Notification Center).
async unsafe fn is_delivered(id: &NotificationId) -> bool {
    let want = id.as_str().to_string();
    let (tx, rx) = async_channel::bounded::<bool>(1);
    let done = RcBlock::new(move |list: *mut AnyObject| {
        let found = unsafe {
            let count: usize = if list.is_null() { 0 } else { msg_send![list, count] };
            (0..count).any(|i| {
                let n: *mut AnyObject = msg_send![list, objectAtIndex: i];
                let request: *mut AnyObject = msg_send![n, request];
                ns_to_string(msg_send![request, identifier]).as_deref() == Some(want.as_str())
            })
        };
        let _ = tx.try_send(found);
    });
    let _: () = msg_send![center(), getDeliveredNotificationsWithCompletionHandler: &*done];
    rx.recv().await.unwrap_or(false)
}

/// Build the `UNMutableNotificationContent` for `n`, owned by the caller.
/// `quiet`: an [`Notification::alert_once`] replacement — no sound, the
/// passive level, and marked for `willPresent`.
unsafe fn make_content(n: &Notification, category: &str, quiet: bool) -> Result<Retained<AnyObject>, NotifyError> {
    let content: Retained<AnyObject> = msg_send_id![class!(UNMutableNotificationContent), new];
    let _: () = msg_send![&*content, setTitle: &*NSString::from_str(&n.title)];
    let _: () = msg_send![&*content, setBody: &*NSString::from_str(&n.body)];
    if let Some(sub) = &n.subtitle {
        let _: () = msg_send![&*content, setSubtitle: &*NSString::from_str(sub)];
    }
    if !n.data.is_empty() || quiet {
        // NSMutableDictionary<NSString*, NSString*> by raw messaging — the
        // typed `NSDictionary` helpers require `NSString: IsRetainable`,
        // which this objc2-foundation version doesn't provide.
        let dict: *mut AnyObject = msg_send![class!(NSMutableDictionary), dictionary];
        for (k, v) in &n.data {
            let key = NSString::from_str(k);
            let val = NSString::from_str(v);
            let _: () = msg_send![dict, setObject: &*val, forKey: &*key];
        }
        if quiet {
            let one = NSString::from_str("1");
            let _: () = msg_send![dict, setObject: &*one, forKey: &*NSString::from_str(QUIET_KEY)];
        }
        let _: () = msg_send![&*content, setUserInfo: dict];
    }
    if let Some(group) = &n.group {
        let _: () = msg_send![&*content, setThreadIdentifier: &*NSString::from_str(group)];
    }
    if n.sound == Sound::Default && !quiet {
        let sound: *mut AnyObject = msg_send![class!(UNNotificationSound), defaultSound];
        let _: () = msg_send![&*content, setSound: sound];
    }
    let responds: Bool = msg_send![&*content, respondsToSelector: sel!(setInterruptionLevel:)];
    if responds.as_bool() {
        let level = match n.priority {
            _ if quiet => LEVEL_PASSIVE,
            Priority::Low => LEVEL_PASSIVE,
            Priority::Default | Priority::High => LEVEL_ACTIVE,
            Priority::Urgent => LEVEL_TIME_SENSITIVE,
        };
        let _: () = msg_send![&*content, setInterruptionLevel: level];
    }
    let _: () = msg_send![&*content, setCategoryIdentifier: &*NSString::from_str(category)];
    if let Some(path) = &n.image {
        let attachment = image_attachment(path)?;
        let list: *mut AnyObject = msg_send![class!(NSArray), arrayWithObject: attachment];
        let _: () = msg_send![&*content, setAttachments: list];
    }
    if let Some(hook) = &n.hooks.apple {
        hook(&content);
    }
    Ok(content)
}

/// A `UNNotificationAttachment` for the image file at `path`. The OS moves
/// a file inside the app's container into its own attachment store.
unsafe fn image_attachment(path: &str) -> Result<*mut AnyObject, NotifyError> {
    let url: *mut AnyObject = msg_send![class!(NSURL), fileURLWithPath: &*NSString::from_str(path)];
    let mut error: *mut AnyObject = std::ptr::null_mut();
    let attachment: *mut AnyObject = msg_send![
        class!(UNNotificationAttachment),
        attachmentWithIdentifier: &*NSString::from_str("image"),
        URL: url,
        options: std::ptr::null_mut::<AnyObject>(),
        error: &mut error,
    ];
    if attachment.is_null() {
        let reason = if error.is_null() { None } else { ns_to_string(msg_send![error, localizedDescription]) };
        return Err(NotifyError::Backend(format!(
            "image attachment {path}: {}",
            reason.unwrap_or_else(|| "rejected".into())
        )));
    }
    Ok(attachment)
}

/// Post `content` under `id` with an optional `trigger` (nil = immediate),
/// resolving when the center accepted or rejected the request.
async unsafe fn add_request(
    id: &NotificationId,
    content: &AnyObject,
    trigger: *mut AnyObject,
) -> Result<(), NotifyError> {
    let ident = NSString::from_str(id.as_str());
    let request: *mut AnyObject = msg_send![
        class!(UNNotificationRequest),
        requestWithIdentifier: &*ident,
        content: content,
        trigger: trigger,
    ];
    let (tx, rx) = async_channel::bounded::<Result<(), NotifyError>>(1);
    let done = RcBlock::new(move |error: *mut AnyObject| {
        let outcome = if error.is_null() {
            Ok(())
        } else {
            let reason = unsafe { ns_to_string(msg_send![error, localizedDescription]) };
            Err(NotifyError::Backend(reason.unwrap_or_else(|| "request rejected".into())))
        };
        let _ = tx.try_send(outcome);
    });
    let _: () = msg_send![center(), addNotificationRequest: request, withCompletionHandler: &*done];
    rx.recv().await.unwrap_or(Err(NotifyError::Backend("request completion dropped".into())))
}

pub(super) async fn notify(n: Notification) -> Result<NotificationId, NotifyError> {
    require_bundle()?;
    let id = resolve_id(&n);
    let category = ensure_category(&n.actions).await;
    unsafe {
        let quiet = n.alert_once && is_delivered(&id).await;
        let content = make_content(&n, &category, quiet)?;
        // nil trigger = deliver immediately.
        add_request(&id, &content, std::ptr::null_mut()).await?;
    }
    Ok(id)
}

pub(super) async fn schedule(n: Notification, after: std::time::Duration) -> Result<NotificationId, NotifyError> {
    require_bundle()?;
    let id = resolve_id(&n);
    let category = ensure_category(&n.actions).await;
    // UNTimeIntervalNotificationTrigger requires a strictly-positive
    // interval; clamp a zero/sub-millisecond delay up so the OS accepts it.
    let secs = after.as_secs_f64().max(0.001);
    unsafe {
        // `alert_once` is decided when posting; a scheduled one always alerts.
        let content = make_content(&n, &category, false)?;
        let trigger: *mut AnyObject = msg_send![
            class!(UNTimeIntervalNotificationTrigger),
            triggerWithTimeInterval: secs,
            repeats: false,
        ];
        add_request(&id, &content, trigger).await?;
    }
    Ok(id)
}

pub(super) async fn cancel(id: &NotificationId) {
    if !has_bundle() {
        return;
    }
    unsafe {
        let ident = NSString::from_str(id.as_str());
        // `[NSArray arrayWithObject:]` — raw messaging (the typed
        // `NSArray::from_slice` requires `NSString: IsRetainable`, absent
        // in this objc2-foundation version).
        let arr: *mut AnyObject = msg_send![class!(NSArray), arrayWithObject: &*ident];
        let c = center();
        let _: () = msg_send![c, removePendingNotificationRequestsWithIdentifiers: arr];
        let _: () = msg_send![c, removeDeliveredNotificationsWithIdentifiers: arr];
    }
}

pub(super) async fn cancel_all() {
    if !has_bundle() {
        return;
    }
    unsafe {
        let c = center();
        let _: () = msg_send![c, removeAllPendingNotificationRequests];
        let _: () = msg_send![c, removeAllDeliveredNotifications];
    }
}

pub(super) async fn push_token() -> Result<PushToken, NotifyError> {
    // APNs token comes from `registerForRemoteNotifications` + the app
    // delegate callback — a host seam (see module docs).
    Err(NotifyError::NotSupported)
}

// ---------------------------------------------------------------------------
// Categories.
// ---------------------------------------------------------------------------

/// Every category registered so far, by id. `setNotificationCategories:`
/// replaces the whole set, so the set is rebuilt from this.
static CATEGORIES: Mutex<BTreeMap<String, Vec<Action>>> = Mutex::new(BTreeMap::new());

/// The category id for an action list: [`PLAIN_CATEGORY`] for none, else
/// a stable hash of everything that shapes the buttons.
pub(crate) fn category_id(actions: &[Action]) -> String {
    if actions.is_empty() {
        return PLAIN_CATEGORY.to_string();
    }
    // FNV-1a: stable across runs and processes (the category must keep its
    // id when the app relaunches to handle a response).
    let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
    let mut eat = |bytes: &[u8]| {
        for b in bytes {
            hash ^= *b as u64;
            hash = hash.wrapping_mul(0x0100_0000_01b3);
        }
        hash ^= 0xff;
        hash = hash.wrapping_mul(0x0100_0000_01b3);
    };
    for a in actions {
        eat(a.id.as_bytes());
        eat(a.title.as_bytes());
        if let Some(input) = &a.input {
            eat(input.button_title.as_bytes());
            eat(input.placeholder.as_bytes());
        }
        eat(&[a.opens_app as u8, a.destructive as u8, a.requires_unlock as u8, a.input.is_some() as u8]);
    }
    format!("idealyst.{hash:016x}")
}

/// Register the category for `actions` if it is new, and wait until the OS
/// has applied it. Returns its id.
async fn ensure_category(actions: &[Action]) -> String {
    let actions = &actions[..actions.len().min(MAX_ACTIONS)];
    let id = category_id(actions);
    let all = {
        let mut cats = CATEGORIES.lock().unwrap_or_else(|e| e.into_inner());
        if cats.contains_key(&id) {
            return id;
        }
        cats.insert(id.clone(), actions.to_vec());
        cats.clone()
    };
    unsafe {
        let set: *mut AnyObject = msg_send![class!(NSMutableSet), set];
        for (cat_id, cat_actions) in &all {
            let category = make_category(cat_id, cat_actions);
            let _: () = msg_send![set, addObject: &*category];
        }
        let _: () = msg_send![center(), setNotificationCategories: set];

        // Answered only once the set above is applied (module docs).
        let (tx, rx) = async_channel::bounded::<()>(1);
        let done = RcBlock::new(move |_set: *mut AnyObject| {
            let _ = tx.try_send(());
        });
        let _: () = msg_send![center(), getNotificationCategoriesWithCompletionHandler: &*done];
        let _ = rx.recv().await;
    }
    id
}

unsafe fn make_category(id: &str, actions: &[Action]) -> Retained<AnyObject> {
    let list: *mut AnyObject = msg_send![class!(NSMutableArray), array];
    for a in actions {
        let action = make_action(a);
        let _: () = msg_send![list, addObject: &*action];
    }
    let intents: *mut AnyObject = msg_send![class!(NSArray), array];
    msg_send_id![
        class!(UNNotificationCategory),
        categoryWithIdentifier: &*NSString::from_str(id),
        actions: list,
        intentIdentifiers: intents,
        options: CATEGORY_CUSTOM_DISMISS,
    ]
}

unsafe fn make_action(a: &Action) -> Retained<AnyObject> {
    let mut options = 0usize;
    if a.opens_app {
        options |= ACTION_FOREGROUND;
    }
    if a.destructive {
        options |= ACTION_DESTRUCTIVE;
    }
    if a.requires_unlock {
        options |= ACTION_AUTH_REQUIRED;
    }
    let id = NSString::from_str(&a.id);
    let title = NSString::from_str(&a.title);
    match &a.input {
        Some(input) => msg_send_id![
            class!(UNTextInputNotificationAction),
            actionWithIdentifier: &*id,
            title: &*title,
            options: options,
            textInputButtonTitle: &*NSString::from_str(&input.button_title),
            textInputPlaceholder: &*NSString::from_str(&input.placeholder),
        ],
        None => msg_send_id![
            class!(UNNotificationAction),
            actionWithIdentifier: &*id,
            title: &*title,
            options: options,
        ],
    }
}

// ---------------------------------------------------------------------------
// Foundation helpers.
// ---------------------------------------------------------------------------

/// An `NSString*` as a Rust string (`None` for nil).
unsafe fn ns_to_string(s: *mut AnyObject) -> Option<String> {
    if s.is_null() {
        return None;
    }
    let ptr: *const std::os::raw::c_char = msg_send![s, UTF8String];
    if ptr.is_null() {
        return None;
    }
    Some(CStr::from_ptr(ptr).to_string_lossy().into_owned())
}

/// The string entries of an `NSDictionary` (`userInfo`): string values as
/// they are, numbers by `stringValue`, everything else (a push payload's
/// nested `aps` dictionary) skipped.
unsafe fn string_dictionary(dict: *mut AnyObject) -> HashMap<String, String> {
    let mut out = HashMap::new();
    if dict.is_null() {
        return out;
    }
    let keys: *mut AnyObject = msg_send![dict, allKeys];
    let count: usize = msg_send![keys, count];
    let string_class = class!(NSString);
    let number_class = class!(NSNumber);
    for i in 0..count {
        let key: *mut AnyObject = msg_send![keys, objectAtIndex: i];
        let key_is_string: Bool = msg_send![key, isKindOfClass: string_class];
        if !key_is_string.as_bool() {
            continue;
        }
        let value: *mut AnyObject = msg_send![dict, objectForKey: key];
        let is_string: Bool = msg_send![value, isKindOfClass: string_class];
        let is_number: Bool = msg_send![value, isKindOfClass: number_class];
        let text = if is_string.as_bool() {
            ns_to_string(value)
        } else if is_number.as_bool() {
            ns_to_string(msg_send![value, stringValue])
        } else {
            None
        };
        if let (Some(k), Some(v)) = (ns_to_string(key), text) {
            out.insert(k, v);
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn plain_notifications_share_one_category() {
        assert_eq!(category_id(&[]), PLAIN_CATEGORY);
    }

    #[test]
    fn category_id_is_stable_and_shaped_by_every_action_field() {
        let reply = Action::new("reply", "Reply").text_input("Send", "Message");
        let id = category_id(std::slice::from_ref(&reply));
        assert_eq!(id, category_id(&[reply.clone()]), "same actions, same category");
        assert!(id.starts_with("idealyst.") && id != PLAIN_CATEGORY);
        for changed in [
            Action::new("reply", "Answer").text_input("Send", "Message"),
            Action::new("reply", "Reply").text_input("Go", "Message"),
            Action::new("reply", "Reply"),
            reply.clone().opens_app(false),
            reply.clone().destructive(),
            reply.clone().requires_unlock(),
        ] {
            assert_ne!(category_id(&[changed.clone()]), id, "{changed:?}");
        }
        // Order matters: the buttons show in list order.
        let mute = Action::new("mute", "Mute");
        assert_ne!(category_id(&[reply.clone(), mute.clone()]), category_id(&[mute, reply]));
    }

    /// The bug: in a process without a bundle identifier (this test binary)
    /// `currentNotificationCenter` raises and aborts the process. Every
    /// entry point must check first.
    #[test]
    fn regression_no_bundle_reports_instead_of_aborting() {
        assert!(!has_bundle(), "test binaries have no bundle identifier");
        init();
        let out = pollster::block_on(notify(Notification::new("a", "b")));
        assert!(matches!(out, Err(NotifyError::Backend(_))), "{out:?}");
        pollster::block_on(cancel(&NotificationId::from("x")));
        pollster::block_on(cancel_all());
    }
}
