//! Escape closes the topmost overlay: the open portals that have an
//! `on_dismiss`, in mount order, for the key paths that have no
//! Escape-to-dismiss of their own.
//!
//! Every backend's user-facing behavior is the same — an Escape that
//! nothing consumed calls the `on_dismiss` of the topmost shown portal, and
//! only that one. Web (one window `keydown` listener over the backend's
//! own portal stack, `primitives::portal::EscapeStack`), macOS
//! (`field_keys`), Windows, Android (`RustModalOverlay`) and Linux (a key
//! controller on the portal container) get there through their toolkit's
//! own key routing. Two paths have no such routing and ask this registry
//! instead:
//!
//! - the robot's `key` verb in its `dispatch` mechanism (iOS, Linux), after
//!   the focused field's `on_key_down` and the app's `on_key` listeners
//!   left the Escape unconsumed;
//! - iOS's hardware-keyboard path: UIKit has no Escape-to-dismiss for the
//!   window-level views a portal mounts as, so the backend's key responders
//!   call [`dismiss_topmost`] for an unconsumed Escape.
//!
//! The portal handler ([`crate::handlers::portal`]) registers each portal
//! with an `on_dismiss` at mount and unregisters it at teardown; a portal
//! hidden because its screen is not the active route is skipped, and so is
//! one whose subtree is leaving (held mounted for an exit animation). The
//! backend's own stacks still call a leaving portal's `on_dismiss` when it
//! is their topmost; the handler hands that Escape to [`dismiss_topmost`],
//! which reaches the live portal beneath. "Topmost"
//! is the most recently mounted, the same rule macOS's `field_keys` and
//! the web's portal stack use: an overlay opened from inside another one
//! mounts after it and sits above it.

use std::cell::{Cell, RefCell};
use std::rc::Rc;

struct Entry {
    id: u64,
    hidden: Rc<Cell<bool>>,
    /// Set once the portal's subtree starts leaving ([`Registration::leave`]).
    leaving: bool,
    dismiss: Rc<dyn Fn()>,
}

impl Entry {
    /// Does an Escape stop here? Not while the portal's screen is hidden,
    /// and not once it is on its way out.
    fn live(&self) -> bool {
        !self.leaving && !self.hidden.get()
    }
}

#[derive(Default)]
struct Stack {
    next_id: u64,
    entries: Vec<Entry>,
}

thread_local! {
    static STACK: RefCell<Stack> = RefCell::new(Stack::default());
}

/// A portal's place in the stack; dropping it removes the portal.
#[must_use = "dropping the registration removes the portal from the Escape stack"]
pub struct Registration {
    id: u64,
}

impl Registration {
    /// The portal is leaving: its subtree was switched out and is held
    /// mounted only for an exit animation. From now on Escape passes over
    /// it to the portal beneath, as if it were already gone. A second
    /// Escape pressed while a submenu fades must close the menu under it,
    /// not land on the fading flyout and do nothing.
    pub fn leave(&self) {
        let id = self.id;
        let _ = STACK.try_with(|s| {
            if let Some(e) = s.borrow_mut().entries.iter_mut().find(|e| e.id == id) {
                e.leaving = true;
            }
        });
    }
}

impl Drop for Registration {
    fn drop(&mut self) {
        let id = self.id;
        // `try_with`: a registration dropped during thread-local teardown
        // has nothing left to remove from.
        let _ = STACK.try_with(|s| s.borrow_mut().entries.retain(|e| e.id != id));
    }
}

/// Put a portal on top of the stack. `hidden` is the portal's own
/// "not shown" flag (its screen is not the active route); while it is
/// set, Escape passes over the portal to the one beneath.
pub fn register(dismiss: Rc<dyn Fn()>, hidden: Rc<Cell<bool>>) -> Registration {
    STACK.with(|s| {
        let mut s = s.borrow_mut();
        s.next_id += 1;
        let id = s.next_id;
        s.entries.push(Entry { id, hidden, leaving: false, dismiss });
        Registration { id }
    })
}

/// Whether a shown portal with an `on_dismiss` is open — what an Escape
/// would close.
pub fn has_dismissable() -> bool {
    STACK.with(|s| s.borrow().entries.iter().any(Entry::live))
}

/// Call the topmost shown portal's `on_dismiss`, as an unconsumed Escape
/// does. Returns whether there was one — the Escape was used, and the key
/// path swallows it. The handler runs with no borrow of the stack held: it
/// closes the portal, whose teardown unregisters it.
pub fn dismiss_topmost() -> bool {
    let top = STACK.with(|s| s.borrow().entries.iter().rev().find(|e| e.live()).map(|e| e.dismiss.clone()));
    match top {
        Some(dismiss) => {
            dismiss();
            true
        }
        None => false,
    }
}
