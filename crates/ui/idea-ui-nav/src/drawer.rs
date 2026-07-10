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
    component, pressable, ui, ChildList, Color, Easing, Element, IdealystSchema, Length,
    PointerEvents, Position, Reactive, Signal, StyleApplication, StyleRules, StyleSheet, Tokenized,
    Transform, Transition,
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

// One shared empty base sheet; the reactive per-state rules ride a
// `with_computed` layer keyed by open/closed so the resolution cache stays
// stable across renders. Cached via `cached_stylesheet` (NOT a per-file
// `thread_local!`) — on Android every `thread_local!` burns one of bionic's
// ~128 pthread TLS keys, and per-sheet thread_locals exhausted the table
// (SIGABRT at mount); the shared registry keeps the key count flat.
fn base_sheet() -> Rc<StyleSheet> {
    static KEY: u8 = 0;
    runtime_core::cached_stylesheet(&KEY as *const u8 as usize, || {
        Rc::new(StyleSheet::r#static(StyleRules::default()))
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
            is_open: Signal::new(false),
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

    // Off-screen offset when closed: negative for a leading panel, positive for
    // a trailing one.
    let closed_dx = match side {
        DrawerSide::Start => -width,
        DrawerSide::End => width,
    };

    let container_style = {
        move || {
            StyleApplication::new(base_sheet()).with_computed("drawer_container", || StyleRules {
                position: Some(Position::Relative),
                width: Some(Length::Percent(100.0).into()),
                height: Some(Length::Percent(100.0).into()),
                ..Default::default()
            })
        }
    };

    let scrim_style = {
        move || {
            let open = is_open.get();
            let key = if open { "drawer_scrim_open" } else { "drawer_scrim_closed" };
            StyleApplication::new(base_sheet()).with_computed(key, move || StyleRules {
                position: Some(Position::Absolute),
                top: Some(Length::Px(0.0).into()),
                left: Some(Length::Px(0.0).into()),
                right: Some(Length::Px(0.0).into()),
                bottom: Some(Length::Px(0.0).into()),
                background: Some(Tokenized::Literal(Color("#000000".into()))),
                opacity: Some(Tokenized::Literal(if open { 0.45 } else { 0.0 })),
                pointer_events: Some(if open { PointerEvents::Auto } else { PointerEvents::None }),
                opacity_transition: Some(Transition::new(SLIDE_MS, Easing::EaseOut)),
                ..Default::default()
            })
        }
    };

    let panel_style = {
        move || {
            let open = is_open.get();
            let dx = if open { 0.0 } else { closed_dx };
            let key = if open { "drawer_panel_open" } else { "drawer_panel_closed" };
            let (left, right) = match side {
                DrawerSide::Start => (Some(Length::Px(0.0).into()), None),
                DrawerSide::End => (None, Some(Length::Px(0.0).into())),
            };
            StyleApplication::new(base_sheet()).with_computed(key, move || StyleRules {
                position: Some(Position::Absolute),
                top: Some(Length::Px(0.0).into()),
                bottom: Some(Length::Px(0.0).into()),
                left: left.clone(),
                right: right.clone(),
                width: Some(Length::Px(width).into()),
                transform: Some(vec![Transform::TranslateX(Length::Px(dx))]),
                transform_transition: Some(Transition::new(SLIDE_MS, Easing::EaseOut)),
                ..Default::default()
            })
        }
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
    use runtime_core::{resolve_style, text, StyleSource};

    /// The `translateX` (px) the panel's reactive style currently resolves to.
    /// The drawer is `view { content…, scrim, panel }`; the panel is the last
    /// child and carries the sliding transform on its own reactive style.
    fn panel_translate_x(drawer: &Element) -> f32 {
        let children = match drawer {
            Element::View { children, .. } => children,
            _ => panic!("Drawer root is a view"),
        };
        let panel = children.last().expect("panel is the last child");
        let style = match panel {
            Element::View { style, .. } => style.as_ref().expect("panel has a style"),
            _ => panic!("panel is a view"),
        };
        let app = match style {
            StyleSource::Reactive(f) => f(),
            _ => panic!("panel style is reactive (slides with is_open)"),
        };
        let rules = resolve_style(&app);
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
        let is_open = Signal::new(false);
        let drawer = Drawer(DrawerProps {
            sidebar: vec![text("SIDEBAR").into()],
            is_open,
            side: Reactive::Static(DrawerSide::Start),
            width: Reactive::Static(280.0),
            children: vec![text("CONTENT").into()],
        });

        // Closed: a leading panel sits one width off the left edge.
        assert_eq!(panel_translate_x(&drawer), -280.0, "closed panel is off-screen");

        // Open: it slides flush to the edge.
        is_open.set(true);
        assert_eq!(panel_translate_x(&drawer), 0.0, "open panel is flush");
    }

    // A trailing (`End`) drawer sits off the RIGHT edge when closed (+width).
    #[test]
    fn drawer_end_side_offsets_positive_when_closed() {
        let is_open = Signal::new(false);
        let drawer = Drawer(DrawerProps {
            sidebar: vec![text("SIDEBAR").into()],
            is_open,
            side: Reactive::Static(DrawerSide::End),
            width: Reactive::Static(200.0),
            children: vec![text("CONTENT").into()],
        });
        assert_eq!(panel_translate_x(&drawer), 200.0, "closed trailing panel is off the right");
    }
}
