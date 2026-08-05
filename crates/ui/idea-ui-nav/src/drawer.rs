//! `Drawer` — a themed side panel wrapping a swap navigator's outlet.
//!
//! Under the outlet model the drawer is *author layout*, not a navigator kind:
//! wrap `{nav.outlet}` in a `Drawer`, put nav links in its `sidebar`, and the
//! drawer owns its own open/close state. There is no `Custom(DrawerCmd)` on the
//! control plane — `is_open` is a plain `Signal<bool>` the layout toggles (from
//! a hamburger button) and the sidebar's links flip shut after selecting.
//!
//! Rendered mount-once: the panel and scrim are always in the tree; opening
//! slides the panel in (`transform` + `transform_transition`) and fades the
//! scrim (`opacity` + `pointer_events`), driven reactively by `is_open`. No
//! portal — the drawer is the layout root, so its absolutely-positioned panel
//! and scrim overlay the outlet directly. Piggybacks on idea-ui's themed
//! [`Surface`](idea_ui::Surface) for the panel background.

use idea_ui::{Surface, SurfaceColor};
use runtime_core::{
    component, pressable, ui, ChildList, Color, Easing, Element, FlexDirection, IdealystSchema,
    Length, PointerEvents, Position, Reactive, Signal, StyleApplication, StyleRules, StyleSheet,
    Tokenized, Transform, Transition,
};
use std::rc::Rc;

/// Which edge the drawer panel anchors to.
#[derive(Copy, Clone, Debug, PartialEq, Eq, Default, IdealystSchema)]
pub enum DrawerSide {
    /// Leading edge — left in LTR (the default).
    #[default]
    Start,
    /// Trailing edge — right in LTR.
    End,
}

const SLIDE_MS: u32 = 240;

// Sheets are cached via `cached_stylesheet` (NOT per-file `thread_local!`s)
// — on Android every `thread_local!` burns one of bionic's ~128 pthread TLS
// keys, and per-sheet thread_locals exhausted the table (SIGABRT at mount);
// the shared registry keeps the key count flat.
//
// They are real premintable sheets, NOT `with_computed` layers over an
// empty base: a computed layer produces rules at runtime under a key the
// premint dump cannot enumerate, so it can never premint — a Drawer in a
// `--premint-only` app would panic at MOUNT. And the drawer's open state
// is a variant AXIS on each sheet, never part of sheet identity: the dump
// mounts the drawer CLOSED and never interacts, so an "open" sheet would
// first construct on the user's tap — after the crawl — and its class
// would have no CSS (the UNCRAWLED panic; `AppShell` had exactly this bug).

/// Build-time identity shared by the dump binary and the shipped bundle.
/// Runtime addresses must not leak into it (see `AppShell::premint_id`).
fn premint_id(which: &str, width: f32, side: DrawerSide) -> String {
    format!(
        "idea-ui-nav.v1.drawer.{which}|w={}|side={}",
        width.to_bits(),
        match side {
            DrawerSide::Start => "start",
            DrawerSide::End => "end",
        },
    )
}

fn cache_key(width: f32, side: DrawerSide, which: u8) -> usize {
    let mut k = width.to_bits() as usize;
    k = k.wrapping_mul(31).wrapping_add(side as usize);
    k.wrapping_mul(31).wrapping_add(which as usize)
}

fn container_sheet() -> Rc<StyleSheet> {
    static KEY: u8 = 0;
    runtime_core::cached_stylesheet(&KEY as *const u8 as usize, || {
        // Constant rules, no variants — the auto-preminted `r#static`
        // content class is binary-stable, so no explicit identity needed.
        Rc::new(StyleSheet::r#static(StyleRules {
            position: Some(Position::Relative),
            width: Some(Length::Percent(100.0).into()),
            height: Some(Length::Percent(100.0).into()),
            ..Default::default()
        }))
    })
}

fn scrim_sheet() -> Rc<StyleSheet> {
    static KEY: u8 = 0;
    runtime_core::cached_stylesheet(&KEY as *const u8 as usize, || {
        StyleSheet::r#static(StyleRules {
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
        })
        // Identity carries no parameters: every Drawer's scrim is the
        // same full-bleed fade regardless of width/side.
        .premint_as("idea-ui-nav.v1.drawer.scrim")
    })
}

fn panel_sheet(width: f32, side: DrawerSide) -> Rc<StyleSheet> {
    runtime_core::cached_stylesheet(cache_key(width, side, 0), move || {
        let closed_dx = match side {
            DrawerSide::Start => -width,
            DrawerSide::End => width,
        };
        let (left, right) = match side {
            DrawerSide::Start => (Some(Length::Px(0.0).into()), None),
            DrawerSide::End => (None, Some(Length::Px(0.0).into())),
        };
        StyleSheet::r#static(StyleRules {
            position: Some(Position::Absolute),
            top: Some(Length::Px(0.0).into()),
            bottom: Some(Length::Px(0.0).into()),
            left,
            right,
            width: Some(Length::Px(width).into()),
            // The panel must be a flex COLUMN: on web a node with no
            // flex-container property stays `display: block`, which makes
            // the inner `Surface(grow = 1)` (flex-basis: 0) inert — the
            // sidebar collapses to zero height and the open drawer shows
            // an empty panel. Native backends are flex-by-default, so
            // this also keeps the two layouts identical.
            flex_direction: Some(FlexDirection::Column),
            transform: Some(vec![Transform::TranslateX(Length::Px(closed_dx))]),
            transform_transition: Some(Transition::new(SLIDE_MS, Easing::EaseOut)),
            ..Default::default()
        })
        .variant("open", "on", |_| StyleRules {
            transform: Some(vec![Transform::TranslateX(Length::Px(0.0))]),
            ..Default::default()
        })
        .premint_as(&premint_id("panel", width, side))
    })
}

/// Props for [`Drawer`]. `children` is the main content (the navigator outlet);
/// `sidebar` is the panel content.
#[runtime_core::props]
#[cfg_attr(feature = "docs", derive(idea_ui::doc_controls::DocControls))]
#[derive(IdealystSchema)]
pub struct DrawerProps {
    /// The panel content — nav links, header, etc. Wired by the author; a link
    /// typically calls `on_select` then `is_open.set(false)`.
    pub sidebar: Vec<Element>,
    /// The drawer's open state. The drawer OWNS this (not the navigator): a
    /// hamburger button in the layout flips it, the sidebar closes it on select.
    pub is_open: Signal<bool>,
    /// Which edge the panel anchors to — see [`DrawerSide`]. Default `Start`.
    pub side: DrawerSide,
    /// Panel width in logical pixels. Default `280`.
    pub width: f32,
    /// The main content — the navigator outlet. Moved out of props by
    /// `#[component(children)]`.
    pub children: Vec<Element>,
}

impl Default for DrawerProps {
    fn default() -> Self {
        Self {
            sidebar: Vec::new(),
            is_open: runtime_core::signal(false),
            side: Reactive::Static(DrawerSide::Start),
            width: Reactive::Static(280.0),
            children: Vec::new(),
        }
    }
}

/// Renders the drawer: the outlet content, a tap-to-close scrim, and a sliding
/// side panel — all mounted once, their visibility driven reactively by
/// `is_open`.
#[component(children)]
pub fn Drawer(props: DrawerProps) -> Element {
    let is_open = props.is_open;
    let side = props.side.get();
    let width = props.width.get();
    let content = props.children;
    let sidebar = props.sidebar;

    let container_style = move || StyleApplication::new(container_sheet());

    let scrim_style = move || {
        let mut app = StyleApplication::new(scrim_sheet());
        if is_open.get() {
            app = app.with("open", "on");
        }
        app
    };

    let panel_style = move || {
        let mut app = StyleApplication::new(panel_sheet(width, side));
        if is_open.get() {
            app = app.with("open", "on");
        }
        app
    };

    let close = move || is_open.set(false);

    // Scrim: a full-bleed pressable that closes on tap; `pointer_events: None`
    // when closed lets taps fall through to the content. Built via the fn form
    // (`pressable` isn't an attribute primitive).
    let scrim: Element = pressable(Vec::new(), close).with_style(scrim_style).into();

    // Panel: the sliding side panel, its themed background from `Surface`. The
    // sidebar is a received-children Vec, so flatten it into `children` and
    // splat (the canonical container pattern — the macro's bare `children`).
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

    // Container children: the outlet content, then the scrim, then the panel
    // (DOM order = stacking order — panel above scrim above content).
    let mut children: Vec<Element> = Vec::with_capacity(content.len() + 2);
    for c in content {
        ChildList::append_to(c, &mut children);
    }
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
    use runtime_core::{resolve_style, text, StyleApplication};

    /// The panel's REACTIVE style closure (the drawer is
    /// `view { content…, scrim, panel }`; the panel is the last child and
    /// carries the sliding transform on its own reactive style). Returned
    /// as the closure so a test can re-resolve across `is_open` writes.
    fn panel_style_fn(drawer: Element) -> Box<dyn Fn() -> StyleApplication> {
        let mut children = match classify(drawer) {
            P::View { children, .. } => children,
            _ => panic!("Drawer root is a view"),
        };
        let panel = children.pop().expect("panel is the last child");
        match classify(panel) {
            P::View { style, .. } => match style.expect("panel has a style") {
                TStyle::AppFn(f) => f,
                _ => panic!("panel style is reactive (slides with is_open)"),
            },
            _ => panic!("panel is a view"),
        }
    }

    /// The `translateX` (px) an application currently resolves to.
    fn translate_x(app: &StyleApplication) -> f32 {
        let rules = resolve_style(app);
        let transforms = rules.transform.clone().expect("panel sets a transform");
        transforms
            .iter()
            .find_map(|t| match t {
                Transform::TranslateX(Length::Px(x)) => Some(*x),
                _ => None,
            })
            .expect("panel has a translateX")
    }

    // The drawer mounts once; opening must SLIDE the panel (translateX: off-screen
    // → 0) rather than mount/unmount it. Asserting the resolved transform at each
    // `is_open` state is what proves the reactive slide (not a structural swap).
    #[test]
    fn drawer_panel_slides_between_closed_and_open() {
        with_test_world(|| {
            let is_open = runtime_core::signal(false);
            let drawer = Drawer(DrawerProps {
                sidebar: vec![text("SIDEBAR").into()],
                is_open,
                side: Reactive::Static(DrawerSide::Start),
                width: Reactive::Static(280.0),
                children: vec![text("CONTENT").into()],
            });
            let style_fn = panel_style_fn(drawer);

            // Closed: a leading panel sits one width off the left edge.
            assert_eq!(translate_x(&style_fn()), -280.0, "closed panel is off-screen");

            // Open: it slides flush to the edge.
            is_open.set(true);
            commit();
            assert_eq!(translate_x(&style_fn()), 0.0, "open panel is flush");
        });
    }

    // The panel must resolve as a flex COLUMN. The web backend only promotes a
    // node to `display: flex` when its rules carry a flex-container property;
    // without one the panel is `display: block`, the inner `Surface(grow = 1)`
    // (flex-basis: 0) is inert, and the open drawer renders an EMPTY panel over
    // the scrim (the "sidebar content missing" bug).
    #[test]
    fn drawer_panel_is_a_flex_column() {
        with_test_world(|| {
            let drawer = Drawer(DrawerProps {
                sidebar: vec![text("SIDEBAR").into()],
                is_open: runtime_core::signal(true),
                side: Reactive::Static(DrawerSide::Start),
                width: Reactive::Static(280.0),
                children: vec![text("CONTENT").into()],
            });
            let rules = resolve_style(&panel_style_fn(drawer)());
            assert_eq!(
                rules.flex_direction,
                Some(FlexDirection::Column),
                "panel must be a flex column so Surface(grow = 1) can fill it"
            );
        });
    }

    // Every Drawer style must PREMINT, in both open states, off ONE sheet.
    // The old spelling was `with_computed` layers over an empty base — a
    // premint disqualifier, so a Drawer in a `--premint-only` app panicked
    // at MOUNT — and sheet identity keyed on open state would panic
    // UNCRAWLED on first open instead (the dump mounts the drawer closed
    // and never interacts; `AppShell` had exactly that bug). Fails against
    // both wrong spellings: `preminted_class_list()` is `None` for a
    // computed-carrying application, and the base classes differ across
    // states for identity-keyed sheets.
    #[test]
    fn regression_drawer_premints_open_state_as_an_axis() {
        with_test_world(|| {
            let is_open = runtime_core::signal(false);
            let drawer = Drawer(DrawerProps {
                sidebar: vec![text("SIDEBAR").into()],
                is_open,
                side: Reactive::Static(DrawerSide::Start),
                width: Reactive::Static(280.0),
                children: vec![text("CONTENT").into()],
            });
            let style_fn = panel_style_fn(drawer);

            let closed = style_fn()
                .preminted_class_list()
                .expect("closed panel premints");
            is_open.set(true);
            commit();
            let open = style_fn()
                .preminted_class_list()
                .expect("open panel premints");
            let base = closed.split(' ').next().unwrap();
            assert_eq!(
                open.split(' ').next().unwrap(),
                base,
                "one sheet, one base class across open states"
            );
            assert!(
                open.contains(&format!("{base}-open-on")),
                "opening adds only an arm class, got {open}"
            );
        });
    }

    // A trailing (`End`) drawer sits off the RIGHT edge when closed (+width).
    #[test]
    fn drawer_end_side_offsets_positive_when_closed() {
        with_test_world(|| {
            let is_open = runtime_core::signal(false);
            let drawer = Drawer(DrawerProps {
                sidebar: vec![text("SIDEBAR").into()],
                is_open,
                side: Reactive::Static(DrawerSide::End),
                width: Reactive::Static(200.0),
                children: vec![text("CONTENT").into()],
            });
            assert_eq!(
                translate_x(&panel_style_fn(drawer)()),
                200.0,
                "closed trailing panel is off the right"
            );
        });
    }
}
