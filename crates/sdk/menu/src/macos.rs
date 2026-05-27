//! macOS implementation of the menu-bar SDK.
//!
//! Builds an `NSMenu` tree from the supplied [`MenuBarSpec`], wires
//! per-item target/action through a Rust-owned `MenuActionTarget`
//! delegate, and installs the result via
//! `NSApplication.setMainMenu:`. The bar appears at the top of the
//! screen, next to the Apple logo — the system menu bar, not an
//! in-window control.
//!
//! # Lifetime
//!
//! NSMenu retains its items strongly; NSApplication retains its
//! `mainMenu` strongly. **NSMenuItem retains its `target` weakly**
//! (per Apple docs — same shape as NSControl), so we have to keep
//! the `MenuActionTarget` alive on the Rust side. A thread-local
//! `Box`-leaked anchor does that — the delegate is process-lifetime
//! anyway, the leak is bounded.
//!
//! # Re-installation
//!
//! Calling [`install`] a second time replaces the previous menu bar.
//! The previous `MenuActionTarget` is dropped (its keep-alive in
//! `LAST_TARGET` is overwritten); since NSApplication's old mainMenu
//! ref is replaced atomically, the system never sees a stale menu
//! with a freed target.

use crate::{Menu, MenuBarSpec, MenuCommand, MenuItem, Modifiers, Shortcut};
use backend_macos::MacosBackend;
use objc2::rc::Retained;
use objc2::runtime::{NSObject as NSObjectRuntime, NSObjectProtocol};
use objc2::{
    class, declare_class, msg_send, msg_send_id, mutability, sel, ClassType, DeclaredClass,
};
use objc2_foundation::{MainThreadMarker, NSObject, NSString};
use std::cell::RefCell;
use std::rc::Rc;

/// Install (or replace) the application's main menu bar. Idempotent
/// per the docs — call it once at bootstrap, or again any time you
/// want to swap the entire bar.
pub fn install(backend: &mut MacosBackend, spec: MenuBarSpec) {
    let mtm = backend.mtm();

    // Build the delegate first; we register every command's callback
    // onto it as we walk the spec.
    let delegate = MenuActionTarget::new(mtm);

    // Build the top-level NSMenu (the menu bar itself — yes, the
    // menu bar is just an NSMenu in AppKit, despite its appearance).
    let main_menu = make_nsmenu(mtm, "", &delegate);

    for menu in &spec.menus {
        // macOS displays the **NSMenuItem.title** for top-level menu
        // bar entries, NOT the submenu's title. (The submenu's title
        // is internal — used in NSMenu's window header when a menu
        // is torn off, and as the accessibility label.) Set both
        // so accessibility tools see the same name visible users do.
        //
        // Construct via plain alloc/init + setTitle: rather than
        // make_nsmenuitem, because the latter wires an `itemClicked:`
        // action whose default tag (0) would clash with the first
        // registered callback if the action ever fired. Top-level
        // bar entries don't take actions anyway — clicking one just
        // opens its submenu, handled internally by AppKit.
        let title_ns = NSString::from_str(&menu.title);
        let item_class = class!(NSMenuItem);
        let top_item: Retained<NSObject> = unsafe {
            let allocated: *mut objc2::runtime::AnyObject = msg_send![item_class, alloc];
            let inited: *mut objc2::runtime::AnyObject = msg_send![allocated, init];
            Retained::from_raw(inited.cast::<NSObject>())
                .expect("NSMenuItem (top-level) init returned nil")
        };
        let _: () = unsafe { msg_send![&*top_item, setTitle: &*title_ns] };
        let submenu = make_nsmenu(mtm, &menu.title, &delegate);
        populate_menu(&submenu, &menu.items, &delegate, mtm);
        unsafe {
            let _: () = msg_send![&*top_item, setSubmenu: &*submenu];
            let _: () = msg_send![&*main_menu, addItem: &*top_item];
        }
    }

    // Hand the assembled bar to NSApplication.
    let nsapp_class = class!(NSApplication);
    let nsapp: *mut NSObject = unsafe { msg_send![nsapp_class, sharedApplication] };
    if !nsapp.is_null() {
        let _: () = unsafe { msg_send![nsapp, setMainMenu: &*main_menu] };
    }

    // Anchor the delegate so its NSMenuItem weak-target refs stay
    // valid. Any previous install's delegate is dropped here; its
    // NSMenu has already been replaced by setMainMenu: above, so no
    // stale menu can reach the freed target.
    LAST_TARGET.with(|slot| {
        *slot.borrow_mut() = Some(delegate);
    });
}

thread_local! {
    /// Process-lifetime anchor for the most recently installed
    /// `MenuActionTarget`. Holds a `Retained<MenuActionTarget>` so
    /// the per-NSMenuItem weak `target` refs survive. A new
    /// `install` call overwrites this slot, dropping the prior
    /// target only after the new mainMenu has been swapped in.
    static LAST_TARGET: RefCell<Option<Retained<MenuActionTarget>>> =
        const { RefCell::new(None) };
}

// =========================================================================
// MenuActionTarget — NSObject subclass that receives every menu-item
// click and dispatches to the stored Rust closure indexed by the
// item's `tag`.
// =========================================================================

pub(crate) struct MenuActionTargetIvars {
    /// Per-item callbacks. The NSMenuItem's `tag` is the index into
    /// this vec; on `itemClicked:` we read the sender's tag, look up
    /// the closure, and invoke it.
    ///
    /// `Vec` rather than `HashMap` because tags are dense (we hand
    /// them out sequentially as we walk the spec), and the read path
    /// is hot (every menu click), so the cache-friendly indexed
    /// access is preferred.
    callbacks: RefCell<Vec<Option<Rc<dyn Fn()>>>>,
}

declare_class!(
    pub(crate) struct MenuActionTarget;

    unsafe impl ClassType for MenuActionTarget {
        type Super = NSObject;
        type Mutability = mutability::MainThreadOnly;
        const NAME: &'static str = "IdealystMenuActionTarget";
    }

    impl DeclaredClass for MenuActionTarget {
        type Ivars = MenuActionTargetIvars;
    }

    unsafe impl NSObjectProtocol for MenuActionTarget {}

    unsafe impl MenuActionTarget {
        /// Action selector wired to every NSMenuItem we vend. The
        /// sender is the NSMenuItem itself; its `tag` indexes into
        /// the `callbacks` table.
        #[method(itemClicked:)]
        fn item_clicked(&self, sender: &NSObjectRuntime) {
            let tag: isize = unsafe { msg_send![sender, tag] };
            if tag < 0 {
                return;
            }
            // Clone the Rc out before firing so the callback is free
            // to mutate menu state (re-install, enable/disable other
            // items via tag lookup) without re-entrant borrow panic.
            let cb = {
                let callbacks = self.ivars().callbacks.borrow();
                callbacks.get(tag as usize).and_then(|slot| slot.clone())
            };
            if let Some(cb) = cb {
                cb();
            }
        }
    }
);

impl MenuActionTarget {
    fn new(mtm: MainThreadMarker) -> Retained<Self> {
        let this = mtm.alloc::<Self>();
        let this = this.set_ivars(MenuActionTargetIvars {
            callbacks: RefCell::new(Vec::new()),
        });
        unsafe { msg_send_id![super(this), init] }
    }

    /// Register a callback and return its index. The NSMenuItem's
    /// `tag` is set to this index so `item_clicked:` can route back.
    fn register(&self, cb: Rc<dyn Fn()>) -> isize {
        let mut callbacks = self.ivars().callbacks.borrow_mut();
        let idx = callbacks.len();
        callbacks.push(Some(cb));
        idx as isize
    }
}

// =========================================================================
// NSMenu / NSMenuItem builders
// =========================================================================

fn make_nsmenu(_mtm: MainThreadMarker, title: &str, _delegate: &MenuActionTarget) -> Retained<NSObject> {
    let menu_class = class!(NSMenu);
    let title_ns = NSString::from_str(title);
    let menu: Retained<NSObject> = unsafe {
        let allocated: *mut objc2::runtime::AnyObject = msg_send![menu_class, alloc];
        let inited: *mut objc2::runtime::AnyObject =
            msg_send![allocated, initWithTitle: &*title_ns];
        Retained::from_raw(inited.cast::<NSObject>())
            .expect("NSMenu init returned nil")
    };
    // Auto-enable handles greying out items whose target/action chain
    // doesn't respond; we manage enabled state per-item via setEnabled:
    // explicitly, so turn it off to avoid AppKit overriding our values.
    let _: () = unsafe { msg_send![&*menu, setAutoenablesItems: false] };
    menu
}

fn make_nsmenuitem(
    _mtm: MainThreadMarker,
    label: &str,
    shortcut: Option<&Shortcut>,
    enabled: bool,
) -> Retained<NSObject> {
    let item_class = class!(NSMenuItem);
    let title_ns = NSString::from_str(label);

    // Key equivalent: NSMenuItem stores it as a single-character
    // NSString. Empty string = no shortcut. The character is
    // **lowercase** for the unshifted form; AppKit applies the Shift
    // modifier from the modifier mask, not from a capitalized key.
    let key_string: Retained<NSString> = match shortcut {
        Some(s) => {
            // Lowercase + best-effort. Characters that aren't pure
            // ASCII letters (function keys, arrows, etc.) need
            // different `keyEquivalent` constants — that surface
            // lands as a follow-up; for v1 we pass the literal char
            // through.
            let mut buf = [0u8; 4];
            let lower = s.key.to_ascii_lowercase();
            let s_str: &str = lower.encode_utf8(&mut buf);
            NSString::from_str(s_str)
        }
        None => NSString::from_str(""),
    };

    let item: Retained<NSObject> = unsafe {
        let allocated: *mut objc2::runtime::AnyObject = msg_send![item_class, alloc];
        let inited: *mut objc2::runtime::AnyObject = msg_send![
            allocated,
            initWithTitle: &*title_ns,
            action: sel!(itemClicked:),
            keyEquivalent: &*key_string,
        ];
        Retained::from_raw(inited.cast::<NSObject>())
            .expect("NSMenuItem init returned nil")
    };

    if let Some(s) = shortcut {
        let mask = ns_modifier_mask(s.modifiers);
        let _: () = unsafe { msg_send![&*item, setKeyEquivalentModifierMask: mask] };
    }

    let _: () = unsafe { msg_send![&*item, setEnabled: enabled] };

    item
}

/// Map our `Modifiers` bitflags to AppKit's `NSEventModifierFlags`.
///
/// Bit layout (NSUInteger):
/// - NSEventModifierFlagShift   = 1 << 17
/// - NSEventModifierFlagControl = 1 << 18
/// - NSEventModifierFlagOption  = 1 << 19
/// - NSEventModifierFlagCommand = 1 << 20
fn ns_modifier_mask(m: Modifiers) -> usize {
    let mut mask: usize = 0;
    if m.contains(Modifiers::SHIFT) {
        mask |= 1 << 17;
    }
    if m.contains(Modifiers::CONTROL) {
        mask |= 1 << 18;
    }
    if m.contains(Modifiers::OPTION) {
        mask |= 1 << 19;
    }
    if m.contains(Modifiers::COMMAND) {
        mask |= 1 << 20;
    }
    mask
}

/// Recursively populate `menu` with `items`. Registers each
/// command's callback against `delegate` and wires the
/// corresponding NSMenuItem's `tag` to the registered index.
fn populate_menu(
    menu: &Retained<NSObject>,
    items: &[MenuItem],
    delegate: &Retained<MenuActionTarget>,
    mtm: MainThreadMarker,
) {
    for item in items {
        match item {
            MenuItem::Command(cmd) => {
                let nsitem = build_command_item(cmd, delegate, mtm);
                let _: () = unsafe { msg_send![&**menu, addItem: &*nsitem] };
            }
            MenuItem::Separator => {
                // `+[NSMenuItem separatorItem]` returns an autoreleased
                // sentinel item; retain so it survives the autorelease
                // pool drain at scope exit.
                let item_class = class!(NSMenuItem);
                let sep_ptr: *mut objc2::runtime::AnyObject =
                    unsafe { msg_send![item_class, separatorItem] };
                if !sep_ptr.is_null() {
                    let retained_ptr: *mut objc2::runtime::AnyObject =
                        unsafe { msg_send![sep_ptr, retain] };
                    let sep = unsafe {
                        Retained::from_raw(retained_ptr.cast::<NSObject>())
                    };
                    if let Some(sep) = sep {
                        let _: () = unsafe { msg_send![&**menu, addItem: &*sep] };
                    }
                }
            }
            MenuItem::Submenu(sub) => {
                // A submenu row is an NSMenuItem whose title is the
                // submenu's display label, with a child NSMenu set
                // via `setSubmenu:`. No keyEquivalent (submenus
                // don't take shortcuts directly), no target/action.
                //
                // Construct via plain `alloc/init` then `setTitle:`
                // rather than the `initWithTitle:action:keyEquivalent:`
                // designated initializer, because the latter requires
                // a non-null action SEL (objc2 has no "null SEL"
                // encoding — passing a null `*const Sel` doesn't
                // implement RefEncode). The blank-init path matches
                // what Apple's own sample code does for submenu rows.
                let label = &sub.title;
                let title_ns = NSString::from_str(label);
                let item_class = class!(NSMenuItem);
                let nsitem: Retained<NSObject> = unsafe {
                    let allocated: *mut objc2::runtime::AnyObject =
                        msg_send![item_class, alloc];
                    let inited: *mut objc2::runtime::AnyObject = msg_send![allocated, init];
                    Retained::from_raw(inited.cast::<NSObject>())
                        .expect("NSMenuItem (submenu) init returned nil")
                };
                let _: () = unsafe { msg_send![&*nsitem, setTitle: &*title_ns] };
                let submenu = make_nsmenu(mtm, label, delegate);
                populate_menu(&submenu, &sub.items, delegate, mtm);
                let _: () = unsafe { msg_send![&*nsitem, setSubmenu: &*submenu] };
                let _: () = unsafe { msg_send![&**menu, addItem: &*nsitem] };
            }
        }
    }
}

fn build_command_item(
    cmd: &MenuCommand,
    delegate: &Retained<MenuActionTarget>,
    mtm: MainThreadMarker,
) -> Retained<NSObject> {
    let item = make_nsmenuitem(
        mtm,
        &cmd.label,
        cmd.shortcut.as_ref(),
        cmd.enabled,
    );
    let tag = match &cmd.on_click {
        Some(cb) => delegate.register(cb.clone()),
        None => -1, // No callback registered — itemClicked: short-circuits.
    };
    let _: () = unsafe { msg_send![&*item, setTag: tag] };
    let _: () = unsafe { msg_send![&*item, setTarget: &**delegate] };
    item
}

/// Menu's title accessor — silences `dead_code` since it's only read
/// via the `menu.title` field-access pattern from the lib.rs surface.
/// (Compiler quirk: `populate_menu` borrows `sub.title` directly so
/// `Menu` ends up "used" via field access, but in case future
/// refactors funnel through methods, leaving an `unused_imports`-
/// adjacent allow here documents the intent.)
#[allow(dead_code)]
fn _menu_marker_unused() {
    let _ = Menu::new("");
}
