//! Browser observers feeding `runtime_shared::app_state`: window focus,
//! page visibility, the reduce-motion preference, the primary pointer and
//! the first user activation. Installed once per boot (`start` /
//! `hydrate`), removed by `stop`. Every report from a browser callback is
//! followed by `schedule_flush` so the world's `app_*` signals commit.
//!
//! | fact | read at boot | followed by |
//! |---|---|---|
//! | `Focused` | `document.hasFocus()` | window `focus` / `blur` |
//! | `Visible` | `document.visibilityState` | document `visibilitychange` |
//! | `ReduceMotion` | `(prefers-reduced-motion: reduce)` | its `change` |
//! | `PrimaryPointer` | `(pointer: fine)` / `(pointer: coarse)` | their `change` |
//! | `UserActivated` | `navigator.userActivation.hasBeenActive` (absent → `false`) | first capture-phase `pointerdown` / `mousedown` / `keydown` / `touchend` |
//!
//! The activation events are the ones the HTML spec's "activation
//! triggering input events" list (plus `mousedown` for engines that
//! predate pointer events); after the first one the listeners are removed
//! — activation is sticky, there is nothing more to learn.

use std::cell::RefCell;

use runtime_shared::app_state::{self, AppStateChange};
use web_glue::dom::{Document, Listener, ListenerOptions, MediaQueryList, Window};

use crate::app_pointer_policy as policy;

const REDUCE_MOTION_QUERY: &str = "(prefers-reduced-motion: reduce)";
const POINTER_FINE_QUERY: &str = "(pointer: fine)";
const POINTER_COARSE_QUERY: &str = "(pointer: coarse)";
const ACTIVATION_EVENTS: [&str; 4] = ["pointerdown", "mousedown", "keydown", "touchend"];

thread_local! {
    /// Long-lived observers (focus, visibility, media queries).
    static SOURCE: RefCell<Vec<Listener>> = const { RefCell::new(Vec::new()) };
    /// The first-activation listeners, dropped once one fires.
    static ACTIVATION: RefCell<Vec<Listener>> = const { RefCell::new(Vec::new()) };
}

fn report(change: AppStateChange) {
    app_state::report(change);
    crate::newcore::schedule_flush();
}

fn primary_pointer(win: &Window) -> runtime_shared::app_state::PointerKind {
    let m = |q| win.match_media(q).ok().flatten().is_some_and(|l: MediaQueryList| l.matches());
    policy::primary_pointer(m(POINTER_FINE_QUERY), m(POINTER_COARSE_QUERY))
}

/// Report the boot values and attach the observers. Idempotent: a second
/// call replaces the first set.
pub(crate) fn install() {
    remove();
    let Some(win) = web_glue::dom::window() else { return };
    let Some(doc) = win.document() else { return };

    // Boot values. Installed after the mount (both boot paths), so the
    // first build saw the defaults; the flush scheduled at the end commits
    // the real values in a microtask — before the browser's first paint.
    app_state::report(AppStateChange::Focused(doc.has_focus()));
    app_state::report(AppStateChange::Visible(policy::is_visible(&doc.visibility_state())));
    app_state::report(AppStateChange::PrimaryPointer(primary_pointer(&win)));
    let reduce = win.match_media(REDUCE_MOTION_QUERY).ok().flatten();
    if let Some(mql) = &reduce {
        app_state::report(AppStateChange::ReduceMotion(mql.matches()));
    }
    let activated = win.navigator().user_activation_has_been_active().unwrap_or(false);
    app_state::report(AppStateChange::UserActivated(activated));

    let mut ls = Vec::new();
    let opts = ListenerOptions::default();
    ls.push(crate::glue_dom::listen_fn(&win, "focus", opts, |_| report(AppStateChange::Focused(true))));
    ls.push(crate::glue_dom::listen_fn(&win, "blur", opts, |_| report(AppStateChange::Focused(false))));
    let d = doc.clone();
    ls.push(crate::glue_dom::listen_fn(&doc, "visibilitychange", opts, move |_| {
        report(AppStateChange::Visible(policy::is_visible(&d.visibility_state())));
    }));
    if let Some(mql) = reduce {
        let m = mql.clone();
        ls.push(crate::glue_dom::listen_fn(&mql, "change", opts, move |_| {
            report(AppStateChange::ReduceMotion(m.matches()));
        }));
    }
    for q in [POINTER_FINE_QUERY, POINTER_COARSE_QUERY] {
        if let Some(mql) = win.match_media(q).ok().flatten() {
            let w = win.clone();
            ls.push(crate::glue_dom::listen_fn(&mql, "change", opts, move |_| {
                report(AppStateChange::PrimaryPointer(primary_pointer(&w)));
            }));
        }
    }
    SOURCE.with(|s| *s.borrow_mut() = ls);
    crate::newcore::schedule_flush();

    if !activated {
        install_activation(&doc);
    }
}

fn install_activation(doc: &Document) {
    let ls = ACTIVATION_EVENTS
        .into_iter()
        .map(|ty| {
            crate::glue_dom::listen_fn(doc, ty, ListenerOptions::CAPTURE, |_| {
                report(AppStateChange::UserActivated(true));
                // Dropping a listener from inside its own callback is safe
                // (web-glue defers the free until the call returns).
                let done = ACTIVATION.with(|a| std::mem::take(&mut *a.borrow_mut()));
                drop(done);
            })
        })
        .collect();
    ACTIVATION.with(|a| *a.borrow_mut() = ls);
}

/// Detach every observer (`stop`).
pub(crate) fn remove() {
    let a = SOURCE.with(|s| std::mem::take(&mut *s.borrow_mut()));
    let b = ACTIVATION.with(|s| std::mem::take(&mut *s.borrow_mut()));
    drop((a, b));
}
