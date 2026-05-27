//! Responsive chrome — drawer-sidebar overlay, click-to-close
//! backdrop, hamburger button.
//!
//! All three live OUTSIDE the framework's render tree:
//!
//! - The CSS `<style>` block injects rules keyed on the navigator
//!   helpers' stable classes (`.ui-nav-drawer-sidebar`,
//!   `.ui-nav-drawer-root`, `.ui-nav-drawer-body`) plus our own
//!   `.web-site-backdrop` / `.web-site-hamburger` classes.
//! - The backdrop and hamburger are plain `<div>` / `<button>`
//!   elements appended to `<body>` once on init. The framework owns
//!   `#app` and below; these float above it.
//! - A reactive effect mirrors the SDK's `is_open` signal onto the
//!   `.drawer-open` class on `.ui-nav-drawer-root` (drives the CSS
//!   transform) and `.is-on` on the backdrop (dims + catches
//!   clicks).
//!
//! Author-side responsive work — TOC visibility, content padding,
//! Hero column layout — uses [`idea_ui::current_breakpoint`]
//! directly inside stylesheet closures. This module owns the
//! sidebar/overlay chrome; the rest is reactive style in
//! [`crate::shell`] / [`crate::styles`].
//!
//! Everything except `set_open_drawer` is a no-op outside `wasm32`.

use std::cell::{OnceCell, RefCell};
use std::rc::Rc;

use drawer_navigator::DrawerHandle;
use idea_ui::{current_breakpoint, Breakpoint};
use runtime_core::{Effect, Ref, Signal, StyleApplication, StyleSheet};

/// Width below which the sidebar collapses into an overlay. The
/// stylesheet variant choices in [`crate::shell`] and
/// [`crate::styles`] cross-reference this through
/// `idea_ui::Breakpoint`; keep both in sync.
pub const SIDEBAR_COLLAPSE_PX: u32 = 900;

// ---------------------------------------------------------------------------
// Reactive `size` variant helpers — for stylesheets with
// `variant size { wide, narrow }`.
// ---------------------------------------------------------------------------

/// Pick `"wide"` or `"narrow"` based on the active breakpoint. Read
/// inside an `effect!` / `move || ...` style closure so the
/// surrounding scope subscribes to the breakpoint signal and the
/// style re-applies on resize across the cutoff.
///
/// Cutoff: `Md` and above is wide, `Sm` and below is narrow. The Md
/// threshold (768 dp) is just under the sidebar-collapse breakpoint
/// (900 dp), so the content tightens *before* the sidebar collapses
/// — gives the body width to breathe in the transition zone.
pub fn responsive_variant() -> &'static str {
    match current_breakpoint().get() {
        Breakpoint::Xs | Breakpoint::Sm => "narrow",
        _ => "wide",
    }
}

/// Build a reactive style closure for a stylesheet that declares
/// `variant size { wide, narrow }`. Reads
/// [`current_breakpoint`] on every fire so a resize across the
/// `Sm`/`Md` boundary re-applies the matching variant.
///
/// ```ignore
/// let hero_style = responsive_style(Hero::sheet());
/// ui! { View(style = hero_style) { /* … */ } }
/// ```
pub fn responsive_style(
    sheet: Rc<StyleSheet>,
) -> impl Fn() -> StyleApplication + Clone + 'static {
    move || {
        StyleApplication::new(sheet.clone())
            .with("size", responsive_variant().to_string())
    }
}

thread_local! {
    /// Captures the "open the drawer" closure derived from the bound
    /// `Ref<DrawerHandle>`. The in-tree mobile-header menu button
    /// reads this on press. `RefCell<Option<...>>` so a hot-reload
    /// rebuild can overwrite the previous binding.
    static OPEN_FN: RefCell<Option<Rc<dyn Fn()>>> = const { RefCell::new(None) };

    /// Long-lived `Signal<&'static str>` mirroring the SDK's
    /// `active_route`. Lazily allocated on first read so the mobile
    /// header's reactive text closure can subscribe to a *stable*
    /// signal handle from any screen scope, even before the
    /// sidebar-build closure has run and installed the mirror
    /// observer. After `install_active_route_observer` runs, an
    /// effect copies the SDK's signal value into this signal on
    /// every change.
    ///
    /// We can't store the SDK's `Signal<&'static str>` directly
    /// here and have screens subscribe to it: at mobile-header
    /// mount time the SDK's signal might not yet be set, the
    /// header's text closure would read `None` and never
    /// subscribe (closures only subscribe to signals they actually
    /// `.get()`). The mirror is the standard "place an indirection
    /// signal whose value is updated by an effect" pattern.
    static ACTIVE_ROUTE_MIRROR: OnceCell<Signal<&'static str>> = const { OnceCell::new() };
}

/// Stash the "open" closure for the imperative hamburger DOM
/// element to call. Invoked once from `app()` after the navigator
/// is built.
///
/// We flip the SDK's shared `is_open` signal directly rather than
/// going through `nav.open()` — the SDK's `DrawerHandle::open` /
/// `close` dispatch the SDK's `DrawerCmd` enum, but the web
/// navigator-helper's command dispatcher downcasts to its OWN
/// (distinct-Rust-type) `DrawerCmd`, so the SDK-dispatched
/// commands are silently dropped. See memory note
/// `project_drawer_helpers_cmd_enum`. Flipping the signal is
/// equivalent to what the helper's dispatcher would do on a
/// successful match (it just `is_open.set(true)` + fires
/// `open_changed` — but `open_changed` is itself just another
/// `is_open.set(...)` in the wiring at `web.rs` line ~138).
pub fn set_open_drawer(nav: Ref<DrawerHandle>) {
    let opener: Rc<dyn Fn()> = Rc::new(move || {
        // Pull the signal handle out via `nav.with` (which holds an
        // ARENA borrow) BEFORE calling `.set(true)` — `set` fires
        // dependent effects synchronously, and our observer effect
        // reads from the same ARENA via `query_selector` /
        // `class_list` JS interop that's safe but also re-enters Rust
        // signal reads. Doing both inside `nav.with` triggers a
        // RefCell already-borrowed panic in `reactive.rs`.
        let sig = nav.with(|h| h.is_open_signal());
        if let Some(sig) = sig {
            sig.set(true);
        }
    });
    OPEN_FN.with(|slot| *slot.borrow_mut() = Some(opener));
}

/// Trigger an open of the drawer from anywhere — called by the
/// in-tree mobile header's menu-button press handler. No-ops if
/// the navigator hasn't been bound yet (pre-mount).
pub fn open_drawer() {
    let cb = OPEN_FN.with(|slot| slot.borrow().clone());
    if let Some(cb) = cb {
        cb();
    }
}

/// Stable `Signal<&'static str>` that always returns the current
/// active route. The signal is lazily allocated on first call;
/// subsequent calls return the same handle. The mobile header
/// reads via this so its text closure subscribes to a known
/// signal even before the SDK's `active_route` becomes available
/// through `install_active_route_observer`.
pub fn active_route_signal() -> Signal<&'static str> {
    ACTIVE_ROUTE_MIRROR.with(|cell| *cell.get_or_init(|| Signal::new("")))
}

/// Mirror the SDK's `active_route` into our long-lived signal.
/// Called once from inside the sidebar builder closure (an active
/// reactive scope), passing `slot.active_route`. The effect copies
/// every SDK update into the mirror; mobile-header subscribers
/// re-fire on each navigation.
///
/// The effect is **forgotten** — it must outlive the sidebar
/// builder closure that installed it. Same posture as
/// [`install_drawer_open_observer`].
pub fn install_active_route_observer(source: Signal<&'static str>) {
    let target = active_route_signal();
    let effect = Effect::new(move || {
        let next = source.get();
        if target.get() != next {
            target.set(next);
        }
    });
    std::mem::forget(effect);
}

// ---------------------------------------------------------------------------
// CSS injection — keyed on stable nav-helper classes + our own DOM
// ---------------------------------------------------------------------------

/// Inject the responsive `<style>` block once. Subsequent calls
/// no-op. Targets `.ui-nav-drawer-sidebar` / `.ui-nav-drawer-root` /
/// `.ui-nav-drawer-body` (owned by `web-navigator-helpers`) plus
/// the two classes we attach ourselves below.
pub fn install_responsive_css() {
    #[cfg(target_arch = "wasm32")]
    {
        use std::cell::Cell;
        thread_local! { static INSTALLED: Cell<bool> = const { Cell::new(false) }; }
        if INSTALLED.with(|c| c.get()) {
            return;
        }
        INSTALLED.with(|c| c.set(true));

        let Some(win) = web_sys::window() else { return };
        let Some(doc) = win.document() else { return };
        let Some(head) = doc.head() else { return };
        let Ok(style) = doc.create_element("style") else { return };
        style.set_text_content(Some(CSS));
        let _ = head.append_child(&style);
    }
}

/// Static stylesheet text. Sidebar-collapse breakpoint kept in sync
/// with [`SIDEBAR_COLLAPSE_PX`] via the literal `899px` below — the
/// const is intentional documentation; if you change one, change
/// the other.
const CSS: &str = "\
.web-site-backdrop{position:fixed;inset:0;background:rgba(0,0,0,0);\
transition:background 220ms ease;pointer-events:none;z-index:998;}\
.web-site-backdrop.is-on{background:rgba(0,0,0,0.42);pointer-events:auto;}\
\n\
@media (max-width: 899px){\
  .ui-nav-drawer-sidebar{position:fixed!important;top:0;left:0;height:100%;\
    width:min(82vw,300px);transform:translateX(-100%);\
    transition:transform 240ms cubic-bezier(0.2,0.0,0.0,1.0);\
    z-index:1000;box-shadow:6px 0 28px rgba(0,0,0,0.22);}\
  .ui-nav-drawer-root.drawer-open .ui-nav-drawer-sidebar{transform:translateX(0);}\
  .ui-nav-drawer-body{width:100%!important;}\
  /* Wrap long code lines on narrow screens. Without this, `<pre>`'s \
     default `white-space: pre` keeps single-line snippets at their \
     full natural width — and even with `min-width: 0` on the panel \
     wrapper letting the panel shrink, the visible text would be \
     clipped by `overflow: hidden`. Wrapping preserves readability; \
     `word-break: break-word` lets very long identifiers / URLs \
     break mid-token rather than pushing the line wider than the \
     viewport. */\
  pre{white-space:pre-wrap!important;word-break:break-word;}\
}\
";

// ---------------------------------------------------------------------------
// DOM elements (backdrop) and reactive class mirror
// ---------------------------------------------------------------------------

/// Append the backdrop to `<body>`, wire its click → close
/// handler, and install a reactive effect that toggles the
/// `.drawer-open` class on `.ui-nav-drawer-root` (CSS gates the
/// sidebar transform) plus `.is-on` on the backdrop (CSS gates the
/// dim + pointer-events) whenever `is_open` changes.
///
/// The menu button itself lives **inside the framework tree** as
/// part of the mobile header (see [`crate::shell::mobile_header`]) —
/// it doesn't need this observer to render. This function only
/// owns the backdrop overlay + the class-mirror so the open / close
/// CSS transitions stay in sync with the SDK's signal.
///
/// Idempotent — second call no-ops (a hot-reload rebuild would
/// otherwise stack a second backdrop DOM node and leak another
/// reactive effect each cycle).
pub fn install_drawer_open_observer(
    #[allow(unused)] is_open: Signal<bool>,
    #[allow(unused)] nav: Ref<DrawerHandle>,
) {
    #[cfg(target_arch = "wasm32")]
    {
        use runtime_core::Effect;
        use std::cell::Cell;
        use wasm_bindgen::closure::Closure;
        use wasm_bindgen::JsCast;

        thread_local! { static INSTALLED: Cell<bool> = const { Cell::new(false) }; }
        if INSTALLED.with(|c| c.get()) {
            return;
        }
        INSTALLED.with(|c| c.set(true));

        let Some(win) = web_sys::window() else { return };
        let Some(doc) = win.document() else { return };
        let Some(body) = doc.body() else { return };

        // --- backdrop ---
        let Ok(backdrop) = doc.create_element("div") else { return };
        let _ = backdrop.set_attribute("class", "web-site-backdrop");
        let _ = body.append_child(&backdrop);

        // Same SDK-enum-mismatch workaround as `set_open_drawer`:
        // flip `is_open` directly instead of `nav.close()`. Same
        // RefCell-borrow split (get the signal under `.with`, then
        // `.set` outside the ARENA borrow).
        let close_nav = nav;
        let close_cb = Closure::wrap(Box::new(move |_: web_sys::Event| {
            let sig = close_nav.with(|h| h.is_open_signal());
            if let Some(sig) = sig {
                sig.set(false);
            }
        }) as Box<dyn FnMut(web_sys::Event)>);
        let _ = backdrop.add_event_listener_with_callback(
            "click",
            close_cb.as_ref().unchecked_ref(),
        );
        close_cb.forget();

        // --- reactive class mirror ---
        // Re-resolves `.ui-nav-drawer-root` on every fire because the
        // navigator's container is created during the SDK handler's
        // `init` (synchronously) but the sidebar build closure that
        // installs this observer runs in a microtask AFTER init. So
        // by the time the effect's first fire runs, the root exists.
        // For safety against future re-orderings, `query_selector`
        // returning `None` is silently skipped — the next signal
        // change will retry.
        //
        // The effect is **forgotten** intentionally: the observer
        // outlives the sidebar build closure that installed it (the
        // closure returns; without leaking the Effect handle it
        // would Drop and stop firing). Page-lifetime is the intended
        // scope.
        let backdrop_for_effect = backdrop;
        let effect = Effect::new(move || {
            let open = is_open.get();
            let Some(win) = web_sys::window() else { return };
            let Some(doc) = win.document() else { return };
            if let Ok(Some(root)) = doc.query_selector(".ui-nav-drawer-root") {
                let class_list = root.class_list();
                if open {
                    let _ = class_list.add_1("drawer-open");
                } else {
                    let _ = class_list.remove_1("drawer-open");
                }
            }
            let class_list = backdrop_for_effect.class_list();
            if open {
                let _ = class_list.add_1("is-on");
            } else {
                let _ = class_list.remove_1("is-on");
            }
        });
        std::mem::forget(effect);
    }
}
