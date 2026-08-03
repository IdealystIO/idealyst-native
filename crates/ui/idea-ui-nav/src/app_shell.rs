//! `AppShell` — the responsive "pinned sidebar on desktop, drawer on mobile"
//! shell, mount-once on the outlet model.
//!
//! This is the common shape every real app re-derives by hand: a sidebar
//! that sits pinned and in view at desktop widths and becomes an off-canvas
//! drawer (hamburger + scrim) below them. Hand-rolling it means getting the
//! breakpoint read, the scroll containment, and the height chain right — and,
//! naively, building the sidebar TWICE (once for the drawer panel, once for
//! the pinned column), each run re-firing its resource fetches.
//!
//! `AppShell` builds everything exactly once. The sidebar lives in a single
//! always-mounted panel; "pinned vs drawer" is STATIC breakpoint styling —
//! `__bp_*` overlays that compile to real `@media (min-width: …)` rules on
//! web and SSR, so a server-rendered page is viewport-correct on first
//! paint with no JS, and a live resize re-pins with zero Rust work. Only
//! the drawer's `is_open` is reactive. Crossing the breakpoint never
//! remounts anything, so sidebar state and in-flight work survive resizes
//! — and the main content (where you splat `{nav.outlet}`) never moves,
//! which is exactly the stable-outlet pattern the one-shot outlet requires.
//!
//! ```ignore
//! SwapNavigator::new(&HOME)
//!     .screen(HOME, |_| Screen::new(/* … */))
//!     .layout(|nav| ui! {
//!         AppShell(sidebar = vec![/* nav links, header … */], is_open = drawer_open) {
//!             // Stable spot for the one-shot outlet:
//!             { nav.outlet }
//!         }
//!     })
//! ```
//!
//! The author's hamburger (in a header inside `children`) toggles `is_open`
//! and hides itself when [`sidebar_pinned`] reports true; sidebar links close
//! the drawer only when unpinned (`if !sidebar_pinned(pin_at) { is_open.set(false) }`).

use idea_ui::{Surface, SurfaceColor};
use runtime_core::{
    component, current_breakpoint, pressable, ui, Breakpoint, ChildList, Color, Easing, Element,
    FlexDirection, IdealystSchema, Length, PointerEvents, Position, Reactive, Signal,
    StyleApplication, StyleRules, StyleSheet, Tokenized, Transform, Transition,
};
use std::rc::Rc;

const SLIDE_MS: u32 = 240;

/// Reactive read: is an [`AppShell`] sidebar with this pin threshold currently
/// pinned in-flow? Call inside `rx!` / an effect so it re-fires on breakpoint
/// change — e.g. to hide the hamburger, or to close the drawer after a
/// sidebar link only when it's actually a drawer:
///
/// ```ignore
/// if !sidebar_pinned(Breakpoint::Lg) { is_open.set(false); }
/// ```
pub fn sidebar_pinned(pin_at: Breakpoint) -> bool {
    current_breakpoint().get().is_at_least(pin_at)
}

/// Props for [`AppShell`]. `children` is the main content (put `{nav.outlet}`
/// here); `sidebar` is the panel content, built exactly once.
#[runtime_core::props]
#[cfg_attr(feature = "docs", derive(idea_ui::doc_controls::DocControls))]
#[derive(IdealystSchema)]
pub struct AppShellProps {
    /// The sidebar content — nav links, header, footer. Built ONCE; the same
    /// nodes serve both the pinned column and the off-canvas drawer.
    pub sidebar: Vec<Element>,
    /// Drawer open state — only meaningful below the pin breakpoint (a pinned
    /// sidebar is always visible). The author's hamburger toggles it; the
    /// scrim and (typically) sidebar links close it.
    pub is_open: Signal<bool>,
    /// The breakpoint at (and above) which the sidebar pins in-flow.
    /// Default [`Breakpoint::Lg`] (≥ 1024 dp with the default table).
    pub pin_at: Breakpoint,
    /// Sidebar width in logical pixels. Default `280`.
    pub width: f32,
    /// The main content — the navigator outlet and everything around it.
    /// Moved out of props by `#[component(children)]`.
    pub children: Vec<Element>,
}

impl Default for AppShellProps {
    fn default() -> Self {
        Self {
            sidebar: Vec::new(),
            is_open: runtime_core::signal(false),
            pin_at: Reactive::Static(Breakpoint::Lg),
            width: Reactive::Static(280.0),
            children: Vec::new(),
        }
    }
}

/// Renders the shell: main content, a tap-to-close scrim, and ONE sidebar
/// panel — pinned in view at/above `pin_at`, an off-canvas drawer below it.
/// All three are always mounted; pinning is static breakpoint styling
/// (`@media` on web/SSR), only the drawer's `is_open` is reactive.
#[component(children)]
pub fn AppShell(props: AppShellProps) -> Element {
    let is_open = props.is_open;
    let pin_at = props.pin_at.get();
    let width = props.width.get();
    let content = props.children;
    let sidebar = props.sidebar;

    // The pin threshold as a breakpoint-overlay axis. Pinning is STATIC
    // breakpoint styling (not a reactive `current_breakpoint()` read):
    // on web + SSR the overlays compile to real `@media (min-width: …)`
    // rules, so a server-rendered page is viewport-correct on first
    // paint with no JS — and a live resize re-pins with no Rust work.
    // Only the drawer's `is_open` stays reactive. `Xs` has no overlay
    // axis (mobile-first base IS the xs layout); treat it as "always
    // pinned" by overlaying at every width via the base itself.
    let pin_axis: Option<&'static str> = pin_at.axis_name();

    // Sheets are cached per (width, pin axis) — the width is baked into
    // rule values, so each distinct width mints its own sheet key. The
    // key packs the width bits with the axis discriminant.
    fn cache_key(width: f32, pin_axis: Option<&'static str>, which: u8) -> usize {
        let mut k = width.to_bits() as usize;
        k = k.wrapping_mul(31).wrapping_add(match pin_axis {
            None => 0,
            Some(a) => a.as_ptr() as usize,
        });
        k.wrapping_mul(31).wrapping_add(which as usize)
    }

    /// Build-time identity for the same sheet `cache_key` addresses.
    /// Deliberately NOT `cache_key`: that folds in `pin_axis.as_ptr()`, a
    /// runtime address that differs between the dump binary and the
    /// shipped wasm, so the two halves would derive different classes and
    /// every shell style would silently miss its CSS.
    ///
    /// Safe to premint because the shell mounts on the INITIAL route — the
    /// dump mounts the app, so these sheets are constructed while it is
    /// collecting. A sheet that only appears on a later route would not be
    /// (see `StyleSheet::premint_as`). The same contract is why NOTHING
    /// here may key sheet identity on `is_open`: the dump mounts the shell
    /// CLOSED and never interacts, so an "open" sheet would first construct
    /// on the user's tap — after the crawl — and its class would have no
    /// CSS (the UNCRAWLED panic under `--premint-only`). Open state is a
    /// variant AXIS on one sheet, so both arms register during the crawl.
    fn premint_id(width: f32, pin_axis: Option<&'static str>, which: &str) -> String {
        format!(
            "idea-ui-nav.v1.app_shell.{which}|w={}|pin={}",
            width.to_bits(),
            pin_axis.unwrap_or("-"),
        )
    }

    // The container's rules are CONSTANT, so they belong in a sheet, not a
    // `with_computed` layer over an empty one. A computed layer produces
    // rules at runtime under a key the dump cannot enumerate, so it can
    // never premint — `--premint-report` flagged this as
    // `computed=app_shell_container`.
    let container_sheet = runtime_core::cached_stylesheet(
        cache_key(width, pin_axis, 5),
        move || {
            StyleSheet::r#static(StyleRules {
                position: Some(Position::Relative),
                width: Some(Length::Percent(100.0).into()),
                height: Some(Length::Percent(100.0).into()),
                ..Default::default()
            })
            .premint_as(&premint_id(width, pin_axis, "container"))
        },
    );
    let container_style = move || StyleApplication::new(container_sheet.clone());

    // Main content: fills the shell; the pinned-sidebar offset is a
    // margin applied by the breakpoint overlay (the panel itself stays
    // absolute in BOTH modes, so pin/unpin never reflows the panel
    // subtree — only this wrapper's margin animates).
    let content_sheet = runtime_core::cached_stylesheet(
        cache_key(width, pin_axis, 0),
        move || {
            let mut sheet = StyleSheet::r#static(StyleRules {
                height: Some(Length::Percent(100.0).into()),
                min_height: Some(Length::Px(0.0).into()),
                min_width: Some(Length::Px(0.0).into()),
                margin_left: Some(Length::Px(if pin_axis.is_none() { width } else { 0.0 }).into()),
                margin_left_transition: Some(Transition::new(SLIDE_MS, Easing::EaseOut)),
                ..Default::default()
            });
            if let Some(axis) = pin_axis {
                sheet = sheet.variant(axis, "on", move |_| StyleRules {
                    margin_left: Some(Length::Px(width).into()),
                    ..Default::default()
                });
            }
            sheet.premint_as(&premint_id(width, pin_axis, "content"))
        },
    );
    let content_style = StyleApplication::new(content_sheet);

    // Scrim: base = closed (invisible, inert); opening is the `open` AXIS,
    // never a second sheet (see `premint_id`). The pinned overlay forces it
    // inert at every width past the threshold (nothing to dismiss).
    //
    // Precedence: single-axis arms merge in BTreeMap-alphabetical axis
    // order, and `__bp_*` sorts before `open` — so a bare `open` arm would
    // BEAT the pinned overlay, and an `is_open` left true across a resize
    // past the threshold would dim and intercept a pinned layout. The
    // pin-beats-open invariant therefore rides a COMPOUND arm
    // (open=on ∧ pinned=on), which merges after all single-axis arms.
    let scrim_sheet = runtime_core::cached_stylesheet(
        cache_key(width, pin_axis, 1),
        move || {
            let mut sheet = StyleSheet::r#static(StyleRules {
                position: Some(Position::Absolute),
                top: Some(Length::Px(0.0).into()),
                left: Some(Length::Px(0.0).into()),
                right: Some(Length::Px(0.0).into()),
                bottom: Some(Length::Px(0.0).into()),
                background: Some(Tokenized::Literal(Color("#000000".into()))),
                opacity: Some(Tokenized::Literal(0.0)),
                pointer_events: Some(PointerEvents::None),
                opacity_transition: Some(Transition::new(SLIDE_MS, Easing::EaseOut)),
                ..Default::default()
            })
            .variant("open", "on", |_| StyleRules {
                opacity: Some(Tokenized::Literal(0.45)),
                pointer_events: Some(PointerEvents::Auto),
                ..Default::default()
            });
            if let Some(axis) = pin_axis {
                sheet = sheet
                    .variant(axis, "on", |_| StyleRules {
                        opacity: Some(Tokenized::Literal(0.0)),
                        pointer_events: Some(PointerEvents::None),
                        ..Default::default()
                    })
                    .compound(vec![("open", "on"), (axis, "on")], |_| StyleRules {
                        opacity: Some(Tokenized::Literal(0.0)),
                        pointer_events: Some(PointerEvents::None),
                        ..Default::default()
                    });
            }
            sheet.premint_as(&premint_id(width, pin_axis, "scrim"))
        },
    );
    let scrim_style = {
        let scrim_sheet = scrim_sheet.clone();
        move || {
            let mut app = StyleApplication::new(scrim_sheet.clone());
            if is_open.get() {
                app = app.with("open", "on");
            }
            app
        }
    };

    // Panel: always absolute on the leading edge; the pinned overlay
    // slides it permanently in. Keeping ONE positioning mode (rather
    // than toggling in-flow/absolute) is what makes pin/unpin a pure
    // style flip with no remount, and — with no z-index in StyleRules —
    // the panel must stay the LAST sibling to paint above the scrim.
    // Open state is the `open` axis here too. No compound needed: the
    // `open` arm and the pinned arm agree (translateX(0)), so merge order
    // between them cannot matter.
    let panel_sheet = runtime_core::cached_stylesheet(
        cache_key(width, pin_axis, 2),
        move || {
            let mut sheet = StyleSheet::r#static(StyleRules {
                position: Some(Position::Absolute),
                top: Some(Length::Px(0.0).into()),
                bottom: Some(Length::Px(0.0).into()),
                left: Some(Length::Px(0.0).into()),
                width: Some(Length::Px(width).into()),
                // Explicit flex container: the web backend emits
                // `display: flex` only when a flex-container property
                // is present, and without it the inner `Surface(grow
                // = 1)` has no flex context — it sizes to content and
                // overflows the panel's bounded height, so an author
                // sidebar `scroll_view` never clamps and can't
                // scroll.
                flex_direction: Some(FlexDirection::Column),
                transform: Some(vec![Transform::TranslateX(Length::Px(-width))]),
                transform_transition: Some(Transition::new(SLIDE_MS, Easing::EaseOut)),
                ..Default::default()
            })
            .variant("open", "on", |_| StyleRules {
                transform: Some(vec![Transform::TranslateX(Length::Px(0.0))]),
                ..Default::default()
            });
            if let Some(axis) = pin_axis {
                sheet = sheet.variant(axis, "on", |_| StyleRules {
                    transform: Some(vec![Transform::TranslateX(Length::Px(0.0))]),
                    ..Default::default()
                });
            }
            sheet.premint_as(&premint_id(width, pin_axis, "panel"))
        },
    );
    let panel_style = {
        let panel_sheet = panel_sheet.clone();
        move || {
            let mut app = StyleApplication::new(panel_sheet.clone());
            if is_open.get() {
                app = app.with("open", "on");
            }
            app
        }
    };

    let close = move || is_open.set(false);
    let scrim: Element = pressable(Vec::new(), close).with_style(scrim_style).into();

    // The ONE sidebar build (received children, flattened and splatted).
    let panel: Element = {
        let mut children: Vec<Element> = Vec::with_capacity(sidebar.len());
        for c in sidebar {
            ChildList::append_to(c, &mut children);
        }
        ui! {
            view(style = panel_style) {
                Surface(background = SurfaceColor::Surface, grow = 1.0) {
                    children
                }
            }
        }
    };

    let content_wrapper: Element = {
        let mut children: Vec<Element> = Vec::with_capacity(content.len());
        for c in content {
            ChildList::append_to(c, &mut children);
        }
        ui! {
            view(style = content_style) {
                children
            }
        }
    };

    // Container children: content, then scrim, then panel — DOM order is
    // stacking order (no z-index in StyleRules), so the panel paints above
    // the scrim above the content. Same assembled-parts shape as `Drawer`.
    let mut children: Vec<Element> = Vec::with_capacity(3);
    children.push(content_wrapper);
    children.push(scrim);
    children.push(panel);

    ui! {
        view(style = container_style) {
            children
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use idea_theme::testing::{commit, with_test_world};
    use idea_ui::test_support::{classify, P, TStyle};
    use runtime_core::{resolve_style, text};

    /// Resolve a part's style at the MOBILE base (no breakpoint overlay
    /// active) or with the `pin_at` overlay forced on — simulating
    /// "at/above the pin breakpoint" without a viewport, since pinning
    /// is static `__bp_*` overlay styling now.
    fn resolve(style: &TStyle, pinned: bool) -> Rc<StyleRules> {
        let app = match style {
            TStyle::AppFn(f) => f(),
            TStyle::App(app) => app.clone(),
            _ => panic!("unexpected style source"),
        };
        let app = if pinned { app.with("__bp_lg", "on") } else { app };
        resolve_style(&app)
    }

    /// `(content, scrim, panel)` STYLES of a built shell (the styles are
    /// extracted so tests can re-resolve after `is_open` writes).
    fn parts(shell: Element) -> (TStyle, TStyle, TStyle) {
        let children = match classify(shell) {
            P::View { children, .. } => children,
            _ => panic!("AppShell root is a view"),
        };
        assert_eq!(children.len(), 3, "content, scrim, panel");
        let mut styles = children.into_iter().map(|c| match classify(c) {
            P::View { style, .. } => style.expect("styled view"),
            P::Pressable { style, .. } => style.expect("styled pressable"),
            _ => panic!("unexpected element shape"),
        });
        let content = styles.next().unwrap();
        let scrim = styles.next().unwrap();
        let panel = styles.next().unwrap();
        (content, scrim, panel)
    }

    fn shell(is_open: Signal<bool>) -> Element {
        AppShell(AppShellProps {
            sidebar: vec![text("SIDEBAR").into()],
            is_open,
            pin_at: Reactive::Static(Breakpoint::Lg),
            width: Reactive::Static(280.0),
            children: vec![text("CONTENT").into()],
        })
    }

    fn translate_x(rules: &StyleRules) -> f32 {
        rules
            .transform
            .as_ref()
            .expect("panel sets a transform")
            .iter()
            .find_map(|t| match t {
                Transform::TranslateX(Length::Px(x)) => Some(*x),
                _ => None,
            })
            .expect("panel has a translateX")
    }

    // At/above the pin breakpoint (the `__bp_lg` overlay) the SAME panel
    // is permanently slid in (even with is_open=false), the scrim is
    // inert, and the content is offset by the sidebar width — pinned
    // mode is static overlay styling, not a remount, so the SSR first
    // paint carries BOTH layouts and `@media` picks one.
    #[test]
    fn pinned_overlay_shows_panel_and_inerts_scrim() {
        with_test_world(|| {
            let (content, scrim, panel) = parts(shell(runtime_core::signal(false)));

            assert_eq!(translate_x(&resolve(&panel, true)), 0.0, "pinned panel is in view");
            assert_eq!(
                resolve(&scrim, true).pointer_events,
                Some(PointerEvents::None),
                "pinned: scrim never intercepts"
            );
            assert_eq!(
                resolve(&content, true).margin_left,
                Some(Length::Px(280.0).into()),
                "pinned: content offset by the sidebar width"
            );
        });
    }

    // The panel must be an explicit flex column. The web backend emits
    // `display: flex` only when a flex-container property is present in
    // the rules; without one the panel renders `display: block`, the
    // inner `Surface(grow = 1)`'s flex sizing is inert, and the sidebar
    // chain sizes to content — overflowing the panel's bounded height so
    // an author sidebar `scroll_view` never clamps and cannot scroll
    // (website sidebar: nav list taller than the viewport was simply
    // clipped, in both pinned and drawer modes).
    #[test]
    fn regression_panel_is_flex_column_so_sidebar_scrollers_clamp() {
        with_test_world(|| {
            let (_, _, panel) = parts(shell(runtime_core::signal(false)));

            for pinned in [false, true] {
                assert_eq!(
                    resolve(&panel, pinned).flex_direction,
                    Some(FlexDirection::Column),
                    "panel declares itself a flex column (pinned = {pinned})"
                );
            }
        });
    }

    // THE PREMINT CRAWL CONTRACT: sheet identity must not depend on
    // `is_open`. The dump mounts the shell CLOSED and never interacts, so
    // a second "open" sheet would first construct on the user's tap —
    // after the crawl — and its class would have no CSS (`--premint-only`
    // panics UNCRAWLED; opening the mobile docs drawer was the repro).
    // Open state must be an AXIS: the same base class in both states, the
    // open state adding only an arm class minted alongside the base.
    #[test]
    fn regression_open_state_is_an_axis_not_a_second_sheet() {
        with_test_world(|| {
            let is_open = runtime_core::signal(false);
            let (_, scrim, panel) = parts(shell(is_open));
            let app_of = |s: &TStyle| match s {
                TStyle::AppFn(f) => f(),
                _ => panic!("scrim/panel styles are reactive closures"),
            };
            for (which, style) in [("scrim", &scrim), ("panel", &panel)] {
                is_open.set(false);
                commit();
                let closed = app_of(style)
                    .preminted_class_list()
                    .unwrap_or_else(|| panic!("closed {which} premints"));
                is_open.set(true);
                commit();
                let open = app_of(style)
                    .preminted_class_list()
                    .unwrap_or_else(|| panic!("open {which} premints"));
                let base = closed.split(' ').next().unwrap();
                assert_eq!(
                    open.split(' ').next().unwrap(),
                    base,
                    "{which}: one sheet, one base class across open states"
                );
                assert!(
                    open.contains(&format!("{base}-open-on")),
                    "{which}: opening adds only an arm class, got {open}"
                );
            }
        });
    }

    // An `is_open` left true across a resize past the pin threshold must
    // NOT dim/intercept the pinned layout. Single-axis arms merge in
    // alphabetical axis order and `__bp_*` sorts before `open`, so the
    // open arm would win — the compound (open ∧ pinned) arm re-asserts
    // the inert scrim after all single-axis arms.
    #[test]
    fn regression_pinned_wins_over_lingering_open_scrim() {
        with_test_world(|| {
            let is_open = runtime_core::signal(true);
            let (_, scrim, _) = parts(shell(is_open));
            let rules = resolve(&scrim, true);
            assert_eq!(
                rules.pointer_events,
                Some(PointerEvents::None),
                "pinned scrim never intercepts, even while is_open lingers true"
            );
            assert_eq!(
                rules.opacity,
                Some(Tokenized::Literal(0.0)),
                "pinned scrim stays invisible, even while is_open lingers true"
            );
        });
    }

    // The mobile-first BASE (no overlay) behaves as a drawer: panel
    // slides with is_open, scrim dims/intercepts only while open,
    // content is full-bleed.
    #[test]
    fn mobile_base_behaves_as_drawer() {
        with_test_world(|| {
            let is_open = runtime_core::signal(false);
            let (content, scrim, panel) = parts(shell(is_open));

            assert_eq!(translate_x(&resolve(&panel, false)), -280.0, "closed drawer is off-screen");
            assert_eq!(resolve(&scrim, false).pointer_events, Some(PointerEvents::None));
            assert_eq!(
                resolve(&content, false).margin_left,
                Some(Length::Px(0.0).into()),
                "mobile base: content is full-bleed"
            );

            is_open.set(true);
            commit();
            assert_eq!(translate_x(&resolve(&panel, false)), 0.0, "open drawer slides in");
            assert_eq!(
                resolve(&scrim, false).pointer_events,
                Some(PointerEvents::Auto),
                "open drawer scrim intercepts (tap to close)"
            );
        });
    }
}
