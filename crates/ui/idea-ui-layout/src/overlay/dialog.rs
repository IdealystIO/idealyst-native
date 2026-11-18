//! [`Dialog`] — a form dialog whose title and verbs stay put while only
//! the body between them scrolls.

use std::rc::Rc;

use icons_lucide::{ARROW_LEFT, X};
use idea_ui::{
    typography_kind, variant, IconButton, IconButtonSize, Modal, ModalPresentation, ModalSection, ModalSlotStyles,
    Typography,
};
use runtime_core::{
    component, empty_absolute_view, rx, stylesheet, ui, AlignItems, Element, FlexDirection, IdealystSchema, Length,
    Reactive, StyleRules, Tokenized,
};

use super::sheet_serves;
use crate::{OptionalSlot, Slot};

/// A [`Dialog`]'s card width where the window has room.
pub const DIALOG_WIDTH: f32 = 520.0;

#[runtime_core::props]
#[derive(IdealystSchema)]
pub struct DialogProps {
    /// Whether the dialog is up. Pass the open-state signal itself; the
    /// dialog stays mounted and plays its exit on `false`.
    pub open: bool,
    /// The title. Live: pass `rx!` to name the record being edited, or
    /// the stage a dialog of stages is on.
    pub title: String,
    /// A back arrow before the title, for a dialog of stages, drawn
    /// while `show_back` says so.
    #[prop(static)]
    pub on_back: Option<Rc<dyn Fn()>>,
    /// Whether the back arrow is drawn now.
    pub show_back: Reactive<bool>,
    /// The header's ✕, the backdrop press and Escape / back: one close.
    #[prop(static)]
    pub on_close: Rc<dyn Fn()>,
    /// The scrolling middle, rebuilt per open.
    #[prop(static)]
    pub body: Slot,
    /// The pinned foot, rebuilt per open: usually a
    /// [`DialogFooter`](super::DialogFooter), with any error line above
    /// it. `None` draws no footer (a read-only dialog).
    #[prop(static)]
    pub footer: OptionalSlot,
    /// The card's width where the window has room.
    #[prop(static)]
    pub width: f32,
}

impl Default for DialogProps {
    fn default() -> Self {
        Self {
            open: Reactive::Static(false),
            title: Reactive::Static(String::new()),
            on_back: None,
            show_back: Reactive::Static(false),
            on_close: Rc::new(|| {}),
            body: Slot::default(),
            footer: OptionalSlot::default(),
            width: DIALOG_WIDTH,
        }
    }
}

/// A form dialog: the title and ✕ pinned at the top, the verbs pinned at
/// the foot, and only the body between them scrolling (rule `dialogs`).
/// Content-sized below the modal's cap, so a short form is a short card.
///
/// A bottom sheet on a phone. From `Sm` a TOP-anchored card rather than
/// a centred one: a form that grows as it is filled would re-centre on
/// every change and move the field being typed in; anchored at the top
/// it grows downward and nothing above moves.
///
/// ```ignore
/// Dialog(
///     open = open,
///     title = "Edit check-in".to_string(),
///     on_close = dismiss.clone(),
///     body = move || ui! { FieldGrid(columns = 2_usize) { … } },
///     footer = move || ui! {
///         DialogFooter {
///             Button(label = "Cancel", variant = variant::Ghost, block = dialog_block(), on_click = dismiss.clone())
///             Button(label = "Save", block = dialog_block(), on_click = save.clone())
///         }
///     },
/// )
/// ```
#[component]
pub fn Dialog(props: &DialogProps) -> Element {
    let open = props.open.clone();
    let title = props.title.clone();
    let on_close = props.on_close.clone();
    let on_back = props.on_back.clone();
    let show_back = props.show_back.clone();
    let body = props.body.clone();
    let dismiss = on_close.clone();
    let header = move || {
        let heading = title.clone();
        let show_back = show_back.clone();
        ui! {
            view(style = DialogHead()) {
                if let Some(back) = on_back.clone() {
                    match show_back.get() {
                        true => {
                            IconButton(
                                icon = Some(ARROW_LEFT),
                                variant = variant::Ghost,
                                size = IconButtonSize::Md,
                                label = Some("Back".to_string()),
                                on_click = back.clone(),
                            )
                        }
                        false => { empty_absolute_view() }
                    }
                }
                view(style = DialogTitle()) {
                    Typography(content = heading, kind = typography_kind::H3)
                }
                IconButton(
                    icon = Some(X),
                    variant = variant::Ghost,
                    size = IconButtonSize::Sm,
                    label = Some("Close".to_string()),
                    on_click = on_close.clone(),
                )
            }
        }
    };
    // A read-only dialog has no foot at all, not an empty ruled strip.
    let footer: ModalSection = match props.footer.0.clone() {
        Some(foot) => (move || {
            let foot = foot();
            ui! { view(style = DialogFoot()) { foot } }
        })
        .into(),
        None => Default::default(),
    };
    ui! {
        Modal(
            open = open,
            presentation = rx!(if sheet_serves() { ModalPresentation::Sheet } else { ModalPresentation::Top }),
            on_dismiss = Some(dismiss.clone()),
            width = props.width,
            styles = section_bleed(),
            header = header,
            content = move || body.build(),
            footer = footer,
        )
    }
}

/// The modal's header and footer slots with their own padding and gap
/// zeroed: the head and foot below carry a dialog's density (`md`
/// vertical, `lg` across). The slots keep their hairlines.
fn section_bleed() -> ModalSlotStyles {
    let zero = || Some(Tokenized::Literal(Length::Px(0.0)));
    let bled = || StyleRules {
        padding_top: zero(),
        padding_right: zero(),
        padding_bottom: zero(),
        padding_left: zero(),
        gap: zero(),
        ..Default::default()
    };
    ModalSlotStyles::default().header(bled()).footer(bled())
}

stylesheet! {
    // The title, then the ✕ hard right.
    DialogHead<idea_ui::IdeaThemeRef> {
        base(t) {
            flex_direction: FlexDirection::Row,
            align_items: AlignItems::Center,
            gap: t.spacing.sm(),
            padding_horizontal: t.spacing.lg(),
            padding_vertical: t.spacing.md(),
        }
    }
}

stylesheet! {
    // Takes the row's width so a long title wraps instead of pushing the
    // ✕ off the card.
    DialogTitle<idea_ui::IdeaThemeRef> {
        base(_t) {
            flex_grow: 1.0,
            flex_shrink: 1.0,
            min_width: Length::Px(0.0),
        }
    }
}

stylesheet! {
    // The verbs. Never scrolls: Save is always where the reader left it.
    DialogFoot<idea_ui::IdeaThemeRef> {
        base(t) {
            flex_direction: FlexDirection::Column,
            gap: t.spacing.sm(),
            padding_horizontal: t.spacing.lg(),
            padding_vertical: t.spacing.md(),
        }
    }
}
