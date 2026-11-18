//! [`ToolbarButton`] — a toolbar control that opens something: Filter,
//! Columns, a date window.

use std::rc::Rc;

use idea_ui::{tone, variant, Button, IconButton};
use runtime_core::{
    component, rx, stylesheet, ui, Element, FlexDirection, IconData, IdealystSchema, Length, PointerEvents, Position,
    PressableHandle, Reactive, Ref,
};

use crate::width::is_phone;

/// The "something is on" dot over the glyph, in px.
const BADGE_PX: f32 = 8.0;

#[runtime_core::props]
#[derive(IdealystSchema)]
pub struct ToolbarButtonProps {
    /// What the control is: "Filter".
    #[prop(static)]
    pub label: String,
    /// Its glyph. With one, a phone draws the glyph alone; without one
    /// the label stays, whole.
    #[prop(static)]
    pub icon: Option<IconData>,
    /// How many things are on behind it (filters applied). Zero draws
    /// nothing; more is counted in the label and marked on the glyph.
    pub count: usize,
    /// Open what it opens.
    #[prop(static)]
    pub on_press: Rc<dyn Fn()>,
    /// Filled with the button's handle, to anchor what it opens (an
    /// [`AdaptivePopover`](crate::AdaptivePopover)'s `target`).
    #[prop(static)]
    pub bind_to: Option<Ref<PressableHandle>>,
    /// The button's robot/E2E id.
    #[prop(static)]
    pub test_id: Option<&'static str>,
}

impl Default for ToolbarButtonProps {
    fn default() -> Self {
        Self {
            label: String::new(),
            icon: None,
            count: Reactive::Static(0),
            on_press: Rc::new(|| {}),
            bind_to: None,
            test_id: None,
        }
    }
}

/// A toolbar control that opens a panel (rule `filter-panel`). From 640
/// up, an outlined button with its glyph and label, the count after it
/// ("Filter · 2"). On a phone, the glyph alone with a dot while
/// anything is on: a toolbar control collapses to its glyph, it never
/// takes a shorter label.
///
/// ```ignore
/// ToolbarButton(
///     label = "Filter".to_string(),
///     icon = Some(LIST_FILTER),
///     count = rx!(filters.get().len()),
///     on_press = open_filters.clone(),
///     bind_to = Some(filter_button),
/// )
/// ```
#[component]
pub fn ToolbarButton(props: &ToolbarButtonProps) -> Element {
    let label = props.label.clone();
    let glyph = props.icon;
    let count = props.count.clone();
    let on_press = props.on_press.clone();
    let bind_to = props.bind_to;
    let test_id = props.test_id;
    let counted: Reactive<String> = {
        let (label, count) = (label.clone(), count.clone());
        rx!(match count.get() {
            0 => label.clone(),
            n => format!("{label} · {n}"),
        })
    };
    let marked: Reactive<bool> = rx!(count.get() > 0);
    // The two forms hold different elements, so they are a `match`.
    ui! {
        match (is_phone(), glyph) {
            (true, Some(glyph)) => {
                view(style = ToolbarGlyphSeat()) {
                    IconButton(
                        icon = Some(*glyph),
                        variant = variant::Outlined,
                        tone = tone::Neutral,
                        on_click = on_press.clone(),
                        bind_to = bind_to,
                        label = Some(label.clone()),
                        test_id = test_id,
                    )
                    BadgeDot(on = marked.clone())
                }
            }
            _ => {
                Button(
                    label = counted.clone(),
                    leading_icon = glyph,
                    variant = variant::Outlined,
                    tone = tone::Neutral,
                    on_click = on_press.clone(),
                    bind_to = bind_to,
                    test_id = test_id,
                )
            }
        }
    }
}

#[runtime_core::props]
#[derive(IdealystSchema)]
pub(super) struct BadgeDotProps {
    /// Whether the dot shows.
    pub on: bool,
}

impl Default for BadgeDotProps {
    fn default() -> Self {
        Self { on: Reactive::Static(false) }
    }
}

/// The dot on a toolbar glyph that says something is on behind it.
///
/// ```ignore
/// BadgeDot(on = rx!(!search.get().is_empty()))
/// ```
#[component]
pub(super) fn BadgeDot(props: &BadgeDotProps) -> Element {
    let on = props.on.clone();
    ui! {
        if on.get() {
            view(style = ToolbarBadge())
        }
    }
}

stylesheet! {
    // The glyph's own box, so the dot has a corner to stand in.
    ToolbarGlyphSeat<idea_ui::IdeaThemeRef> {
        base(_t) {
            flex_direction: FlexDirection::Row,
            position: Position::Relative,
            flex_shrink: 0.0,
        }
    }
}

stylesheet! {
    // Out of flow and deaf: it marks the button, it is not part of it.
    ToolbarBadge<idea_ui::IdeaThemeRef> {
        base(t) {
            position: Position::Absolute,
            top: Length::Px(-BADGE_PX / 4.0),
            right: Length::Px(-BADGE_PX / 4.0),
            width: Length::Px(BADGE_PX),
            height: Length::Px(BADGE_PX),
            border_radius: t.radius.pill(),
            background: t.intent.primary.solid_bg(),
            pointer_events: PointerEvents::None,
        }
    }
}
