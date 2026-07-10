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
//! always-mounted panel; "pinned vs drawer" is purely reactive styling on
//! that panel (and its scrim + the content offset), driven by
//! [`current_breakpoint`]. Crossing the breakpoint never remounts anything,
//! so sidebar state and in-flight work survive resizes — and the main
//! content (where you splat `{nav.outlet}`) never moves, which is exactly
//! the stable-outlet pattern the one-shot outlet requires.
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
    IdealystSchema, Length, PointerEvents, Position, Reactive, Signal, StyleApplication,
    StyleRules, StyleSheet, Tokenized, Transform, Transition,
};
use std::rc::Rc;

const SLIDE_MS: u32 = 240;

// Shared empty base sheet via `cached_stylesheet` — a per-file `thread_local!`
// would burn one of Android/bionic's ~128 pthread TLS keys per sheet.
fn base_sheet() -> Rc<StyleSheet> {
    static KEY: u8 = 0;
    runtime_core::cached_stylesheet(&KEY as *const u8 as usize, || {
        Rc::new(StyleSheet::r#static(StyleRules::default()))
    })
}

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
            is_open: Signal::new(false),
            pin_at: Reactive::Static(Breakpoint::Lg),
            width: Reactive::Static(280.0),
            children: Vec::new(),
        }
    }
}

/// Renders the shell: main content, a tap-to-close scrim, and ONE sidebar
/// panel — pinned in view at/above `pin_at`, an off-canvas drawer below it.
/// All three are always mounted; responsiveness is reactive styling only.
#[component(children)]
pub fn AppShell(props: AppShellProps) -> Element {
    let is_open = props.is_open;
    let pin_at = props.pin_at.get();
    let width = props.width.get();
    let content = props.children;
    let sidebar = props.sidebar;

    let container_style = {
        move || {
            StyleApplication::new(base_sheet()).with_computed("app_shell_container", || StyleRules {
                position: Some(Position::Relative),
                width: Some(Length::Percent(100.0).into()),
                height: Some(Length::Percent(100.0).into()),
                ..Default::default()
            })
        }
    };

    // Main content: fills the shell, offset by the sidebar width while
    // pinned. The offset is a margin (the panel itself stays absolute in
    // BOTH modes) so pin/unpin never reflows the panel subtree — only this
    // wrapper's margin animates.
    let content_style = {
        move || {
            let pinned = sidebar_pinned(pin_at);
            let key = if pinned { "app_shell_content_pinned" } else { "app_shell_content" };
            StyleApplication::new(base_sheet()).with_computed(key, move || StyleRules {
                height: Some(Length::Percent(100.0).into()),
                min_height: Some(Length::Px(0.0).into()),
                min_width: Some(Length::Px(0.0).into()),
                margin_left: Some(Length::Px(if pinned { width } else { 0.0 }).into()),
                margin_left_transition: Some(Transition::new(SLIDE_MS, Easing::EaseOut)),
                ..Default::default()
            })
        }
    };

    // Scrim: active only as a drawer (unpinned + open); inert and
    // click-through otherwise.
    let scrim_style = {
        move || {
            let dimming = is_open.get() && !sidebar_pinned(pin_at);
            let key = if dimming { "app_shell_scrim_on" } else { "app_shell_scrim_off" };
            StyleApplication::new(base_sheet()).with_computed(key, move || StyleRules {
                position: Some(Position::Absolute),
                top: Some(Length::Px(0.0).into()),
                left: Some(Length::Px(0.0).into()),
                right: Some(Length::Px(0.0).into()),
                bottom: Some(Length::Px(0.0).into()),
                background: Some(Tokenized::Literal(Color("#000000".into()))),
                opacity: Some(Tokenized::Literal(if dimming { 0.45 } else { 0.0 })),
                pointer_events: Some(if dimming { PointerEvents::Auto } else { PointerEvents::None }),
                opacity_transition: Some(Transition::new(SLIDE_MS, Easing::EaseOut)),
                ..Default::default()
            })
        }
    };

    // Panel: always absolute on the leading edge; pinned just means
    // permanently slid in. Keeping ONE positioning mode (rather than
    // toggling in-flow/absolute) is what makes pin/unpin a pure style flip
    // with no remount, and — with no z-index in StyleRules — the panel must
    // stay the LAST sibling to paint above the scrim.
    let panel_style = {
        move || {
            let shown = is_open.get() || sidebar_pinned(pin_at);
            let key = if shown { "app_shell_panel_open" } else { "app_shell_panel_closed" };
            StyleApplication::new(base_sheet()).with_computed(key, move || StyleRules {
                position: Some(Position::Absolute),
                top: Some(Length::Px(0.0).into()),
                bottom: Some(Length::Px(0.0).into()),
                left: Some(Length::Px(0.0).into()),
                width: Some(Length::Px(width).into()),
                transform: Some(vec![Transform::TranslateX(Length::Px(if shown {
                    0.0
                } else {
                    -width
                }))]),
                transform_transition: Some(Transition::new(SLIDE_MS, Easing::EaseOut)),
                ..Default::default()
            })
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
    use runtime_core::{resolve_style, set_viewport_size, text, StyleSource, ViewportSize};

    fn resolve(el: &Element) -> Rc<StyleRules> {
        let style = match el {
            Element::View { style, .. } => style.as_ref().expect("styled view"),
            Element::Pressable { style, .. } => style.as_ref().expect("styled pressable"),
            _ => panic!("unexpected element shape"),
        };
        let app = match style {
            StyleSource::Reactive(f) => f(),
            _ => panic!("app-shell styles are reactive"),
        };
        resolve_style(&app)
    }

    /// `(content, scrim, panel)` of a built shell.
    fn parts(shell: &Element) -> (&Element, &Element, &Element) {
        let children = match shell {
            Element::View { children, .. } => children,
            _ => panic!("AppShell root is a view"),
        };
        assert_eq!(children.len(), 3, "content, scrim, panel");
        (&children[0], &children[1], &children[2])
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

    // At/above the pin breakpoint the SAME panel is permanently slid in
    // (even with is_open=false), the scrim is inert, and the content is
    // offset by the sidebar width — pinned mode is styling, not a remount.
    #[test]
    fn pinned_panel_shows_without_open_and_scrim_is_inert() {
        set_viewport_size(ViewportSize::new(1280.0, 800.0)); // >= Lg
        let shell = shell(Signal::new(false));
        let (content, scrim, panel) = parts(&shell);

        assert_eq!(translate_x(&resolve(panel)), 0.0, "pinned panel is in view");
        assert_eq!(
            resolve(scrim).pointer_events,
            Some(PointerEvents::None),
            "pinned: scrim never intercepts"
        );
        assert_eq!(
            resolve(content).margin_left,
            Some(Length::Px(280.0).into()),
            "pinned: content offset by the sidebar width"
        );
    }

    // Below the pin breakpoint the same nodes behave as a drawer: panel
    // slides with is_open, scrim dims/intercepts only while open, content
    // is full-bleed.
    #[test]
    fn unpinned_behaves_as_drawer() {
        set_viewport_size(ViewportSize::new(400.0, 800.0)); // Xs
        let is_open = Signal::new(false);
        let shell = shell(is_open);
        let (content, scrim, panel) = parts(&shell);

        assert_eq!(translate_x(&resolve(panel)), -280.0, "closed drawer is off-screen");
        assert_eq!(resolve(scrim).pointer_events, Some(PointerEvents::None));
        assert_eq!(
            resolve(content).margin_left,
            Some(Length::Px(0.0).into()),
            "unpinned: content is full-bleed"
        );

        is_open.set(true);
        assert_eq!(translate_x(&resolve(panel)), 0.0, "open drawer slides in");
        assert_eq!(
            resolve(scrim).pointer_events,
            Some(PointerEvents::Auto),
            "open drawer scrim intercepts (tap to close)"
        );
    }
}
