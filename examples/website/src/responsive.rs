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

use std::cell::OnceCell;
use std::rc::Rc;

use drawer_navigator::DrawerHandle;
use idea_ui::{current_breakpoint, Breakpoint};
use runtime_core::{memo, Ref, Signal, StyleApplication, StyleSheet};

/// Width (dp) below which the sidebar collapses into the drawer
/// overlay. Single source of truth for the collapse breakpoint:
/// [`build_css`] generates the `@media (max-width: …)` rule from it,
/// and the mobile header keys its visibility on the same value via
/// [`sidebar_collapsed`] / [`collapse_responsive_style`]. The header
/// MUST collapse at this exact width (not at the lower content-tighten
/// breakpoint used by [`responsive_variant`]) or the band between the
/// two becomes an un-navigable dead zone — see the regression test.
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
/// ui! { view(style = hero_style) { /* … */ } }
/// ```
pub fn responsive_style(
    sheet: Rc<StyleSheet>,
) -> impl Fn() -> StyleApplication + Clone + 'static {
    move || {
        StyleApplication::new(sheet.clone())
            .with("size", responsive_variant().to_string())
    }
}

// ---------------------------------------------------------------------------
// Collapse-keyed `size` variant — for chrome whose visibility must
// track the *sidebar-collapse* breakpoint, NOT the content-tighten
// breakpoint above.
//
// The mobile header (hamburger) is the only thing that can open the
// drawer once the sidebar overlays itself. It MUST therefore appear at
// exactly the width where the sidebar collapses. Keying it on
// `responsive_variant`'s Sm/Md cutoff (768 dp) instead left a
// 768–899 dp dead zone: the sidebar was already overlaid off-screen
// (CSS `@media (max-width: 899px)`) but the hamburger hadn't appeared
// yet, so there was no way to navigate. These helpers key on
// `SIDEBAR_COLLAPSE_PX` so the two flip together.
// ---------------------------------------------------------------------------

/// Pure classifier: is the sidebar collapsed into the drawer overlay
/// at `width`? True for widths strictly below [`SIDEBAR_COLLAPSE_PX`],
/// which is exactly the set the CSS `@media (max-width: {COLLAPSE-1}px)`
/// rule (built in [`build_css`]) matches.
pub fn sidebar_collapsed_at(width: f32) -> bool {
    width < SIDEBAR_COLLAPSE_PX as f32
}

/// Map collapse-state to the stylesheet `size` variant. `narrow` is
/// the visible 56-px header; `wide` collapses it to zero height.
fn collapse_variant(collapsed: bool) -> &'static str {
    if collapsed {
        "narrow"
    } else {
        "wide"
    }
}

thread_local! {
    /// Memoized `Signal<bool>` — `true` while the sidebar is collapsed.
    /// Derived from [`runtime_core::viewport_size`] like
    /// [`idea_ui::current_breakpoint`], so subscribers only re-fire when
    /// the viewport crosses [`SIDEBAR_COLLAPSE_PX`], not on every pixel
    /// of a resize drag. Lazily created on first read.
    static COLLAPSED_MEMO: OnceCell<Signal<bool>> = const { OnceCell::new() };
}

/// Reactive flag: is the sidebar collapsed into the drawer overlay?
/// Read inside a style closure / effect to subscribe to crossings of
/// the collapse breakpoint.
pub fn sidebar_collapsed() -> Signal<bool> {
    COLLAPSED_MEMO.with(|cell| {
        // Root-anchor this thread-lifetime cached memo so it isn't owned
        // by whatever transient scope first touches it (e.g. an SSR
        // deferred chrome build) — otherwise the cached signal id dangles
        // when that scope drops and its slot recycles.
        *cell.get_or_init(|| {
            runtime_core::unscope(|| {
                memo(|| sidebar_collapsed_at(runtime_core::viewport_size().get().width))
            })
        })
    })
}

/// Build a reactive style closure for a `variant size { wide, narrow }`
/// stylesheet whose visibility tracks the sidebar-collapse breakpoint
/// (e.g. the mobile header). Reads [`sidebar_collapsed`] on every fire
/// so a resize across [`SIDEBAR_COLLAPSE_PX`] re-applies the matching
/// variant.
pub fn collapse_responsive_style(
    sheet: Rc<StyleSheet>,
) -> impl Fn() -> StyleApplication + Clone + 'static {
    move || {
        let variant = collapse_variant(sidebar_collapsed().get());
        StyleApplication::new(sheet.clone()).with("size", variant.to_string())
    }
}

// Open-drawer dispatcher + active-route mirror used to live here as
// thread-locals while the website built its chrome inside per-screen
// `layout()` calls and had no direct access to the navigator's slot
// signals. After the slot refactor, the navigator hands `SlotProps`
// (with `open_drawer` / `active_route` / `is_open` directly) to the
// `top_with` / `leading_with` / etc. closures, so the thread-locals
// became dead intermediaries and were removed.

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
        let css = build_css();
        style.set_text_content(Some(css.as_str()));
        let _ = head.append_child(&style);
    }
}

/// Assemble the responsive stylesheet text. The sidebar-collapse media
/// query is generated from [`SIDEBAR_COLLAPSE_PX`] — `max-width` is one
/// below the threshold so the rule matches exactly the widths
/// [`sidebar_collapsed_at`] reports as collapsed. The const is the
/// single source of truth; the mobile header
/// ([`collapse_responsive_style`]) keys off the same value, so they
/// can never drift apart.
fn build_css() -> String {
    format!(
        "{BACKDROP_CSS}@media (max-width: {}px){{{COLLAPSE_RULES}}}",
        SIDEBAR_COLLAPSE_PX - 1,
    )
}

/// Backdrop overlay rules — outside the media query (the backdrop is
/// inert until `.is-on` is toggled, regardless of viewport width).
/// Trailing `\n` separates it from the generated media block.
const BACKDROP_CSS: &str = "\
.web-site-backdrop{position:fixed;inset:0;background:rgba(0,0,0,0);\
transition:background 220ms ease;pointer-events:none;z-index:998;}\
.web-site-backdrop.is-on{background:rgba(0,0,0,0.42);pointer-events:auto;}\
\n";

/// Rules that live *inside* the sidebar-collapse media query.
///
/// The sidebar pin/modal collapse itself now lives in the navigator's
/// shared stylesheet (`css::navigator_layout_css`, driven by
/// [`drawer_navigator::install_navigator_pin_width`] — which `app()` pins
/// to [`SIDEBAR_COLLAPSE_PX`]). That sheet is emitted by BOTH the live web
/// backend and SSR, so the collapse is correct on the static first paint —
/// the whole point of the migration. We no longer duplicate those rules
/// here (a duplicate, wasm-only `!important` block would diverge from the
/// SSR sheet and flash on hydration).
///
/// What remains is purely a content concern: wrap long code lines on
/// narrow screens. Without this, `<pre>`'s default `white-space: pre`
/// keeps single-line snippets at their full natural width — and even with
/// `min-width: 0` on the panel wrapper letting the panel shrink, the
/// visible text would be clipped by `overflow: hidden`. Wrapping preserves
/// readability; `word-break: break-word` lets very long identifiers / URLs
/// break mid-token rather than pushing the line wider than the viewport.
const COLLAPSE_RULES: &str = "\
  pre{white-space:pre-wrap!important;word-break:break-word;}";

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
        use runtime_core::watch;
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
        // This observer is wired up **outside the component tree** — the
        // sidebar build closure that installs it runs in a microtask after
        // init, so there is no reactive scope to own it. That is exactly
        // what `watch` is for: it returns a caller-owned `Subscription`.
        // We `.leak()` it because the observer should live for the whole
        // page (the `INSTALLED` guard above makes this once-only), the
        // honest replacement for the old `Effect::persist()` pin.
        let backdrop_for_effect = backdrop;
        watch(move || {
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
        })
        .leak();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Regression: the 768–899 dp "navigation dead zone".
    ///
    /// The mobile header (hamburger) used to key its visible/hidden
    /// `size` variant on the shared Sm/Md content breakpoint (768 dp),
    /// while the sidebar collapses into the drawer overlay at
    /// `SIDEBAR_COLLAPSE_PX` (900). Across 768–899 dp the sidebar was
    /// translated off-screen *and* the hamburger was gone, so the
    /// drawer could not be opened at all. Both must flip at the same
    /// width: whenever the sidebar is collapsed, the header must be in
    /// the visible (`narrow`) variant.
    #[test]
    fn regression_no_navigation_dead_zone() {
        for w in 0..=2000u32 {
            let width = w as f32;
            let collapsed = sidebar_collapsed_at(width);
            let header_visible = collapse_variant(collapsed) == "narrow";
            assert_eq!(
                collapsed, header_visible,
                "width {width}: sidebar collapsed={collapsed} but header \
                 visible={header_visible} — navigation dead zone reopened"
            );
        }

        // Explicit anchor inside the old dead zone: the sidebar is
        // overlaid here, so the hamburger MUST be present.
        let dead_zone_width = 800.0;
        assert!(sidebar_collapsed_at(dead_zone_width));
        assert_eq!(collapse_variant(sidebar_collapsed_at(dead_zone_width)), "narrow");
    }

    /// The collapse threshold is exclusive: `SIDEBAR_COLLAPSE_PX` itself
    /// is the first width that is NOT collapsed, and one below it is.
    #[test]
    fn collapse_boundary_is_exclusive_at_const() {
        assert!(sidebar_collapsed_at((SIDEBAR_COLLAPSE_PX - 1) as f32));
        assert!(!sidebar_collapsed_at(SIDEBAR_COLLAPSE_PX as f32));
    }

    /// The generated CSS media query is derived from the same const the
    /// header keys on, so the sidebar's collapse and the hamburger's
    /// appearance can never drift to different pixel values.
    #[test]
    fn css_media_query_matches_collapse_const() {
        let css = build_css();
        let expected = format!("@media (max-width: {}px)", SIDEBAR_COLLAPSE_PX - 1);
        assert!(
            css.contains(&expected),
            "generated CSS should contain `{expected}`, got:\n{css}"
        );
    }
}
