//! [`AdaptivePopover`] — free-form content beside its trigger with a
//! pointer, in a bottom sheet on a phone.

use std::rc::Rc;

use idea_ui::{typography_kind, ElementAlign, ElementSide, Popover, Stack, StackGap, Typography};
use runtime_core::{component, rx, signal, ui, AnchorTarget, Element, IdealystSchema, Signal};

use super::{sheet_serves, Sheet};
use crate::Slot;

#[runtime_core::props]
#[derive(IdealystSchema)]
pub struct AdaptivePopoverProps {
    /// The open state. The surface closes itself through it on an
    /// outside press, Escape or back.
    pub open: Signal<bool>,
    /// What the popover opens beside. The sheet ignores it.
    #[prop(static)]
    pub target: Option<AnchorTarget>,
    /// The sheet's heading: on a phone the trigger is no longer beside
    /// the content to say what it is. The popover draws none.
    #[prop(static)]
    pub title: Option<String>,
    /// Builds the content, on every open.
    #[prop(static)]
    pub content: Slot,
    /// Runs after the surface closes itself.
    #[prop(static)]
    pub on_dismiss: Option<Rc<dyn Fn()>>,
    /// Which side of the target the popover sits on.
    #[prop(static)]
    pub side: ElementSide,
    /// The popover's alignment along the target's edge.
    #[prop(static)]
    pub align: ElementAlign,
}

impl Default for AdaptivePopoverProps {
    fn default() -> Self {
        Self {
            open: signal(false),
            target: None,
            title: None,
            content: Slot::default(),
            on_dismiss: None,
            side: ElementSide::Below,
            align: ElementAlign::Start,
        }
    }
}

/// A popover that becomes a bottom sheet on a phone (rule `overlays`):
/// a search field, a filter panel, a date window's presets. The content
/// builder is the same for both, so what it holds must work at a
/// sheet's full width and at a popover's.
///
/// Mount it once, ungated, and drive `open`.
///
/// ```ignore
/// AdaptivePopover(
///     open = open,
///     target = Some(AnchorTarget::from(filter_button)),
///     title = Some("Filters".to_string()),
///     content = move || ui! { FilterRows(…) },
/// )
/// ```
#[component]
pub fn AdaptivePopover(props: &AdaptivePopoverProps) -> Element {
    let open = props.open;
    let target = props.target.clone();
    let title = props.title.clone();
    let content = props.content.clone();
    let side = props.side;
    let align = props.align;
    let after = props.on_dismiss.clone();
    let on_dismiss: Rc<dyn Fn()> = Rc::new(move || {
        open.set(false);
        if let Some(after) = &after {
            after();
        }
    });
    ui! {
        match sheet_serves() {
            true => {
                {
                    let title = title.clone();
                    let content = content.clone();
                    let sheet: Rc<dyn Fn() -> Element> = Rc::new(move || {
                        let title = title.clone();
                        let body = content.build();
                        ui! {
                            Stack(gap = StackGap::Md) {
                                if let Some(title) = title {
                                    Typography(content = title, kind = typography_kind::H3)
                                }
                                body
                            }
                        }
                    });
                    ui! {
                        Sheet(open = rx!(open.get()), on_dismiss = on_dismiss.clone(), content = { let sheet = sheet.clone(); move || sheet() })
                    }
                }
            }
            false => {
                {
                    // Gated on `open`: mounting a `Popover` is opening it.
                    let content = content.clone();
                    let dismiss = on_dismiss.clone();
                    let target = target.clone();
                    ui! {
                        if open.get() {
                            Popover(
                                target = target.clone(),
                                side = side,
                                align = align,
                                on_dismiss = Some(dismiss.clone()),
                                // `sheet_serves` above is the one switch;
                                // the Popover never picks a sheet itself.
                                sheet_on_phone = false,
                            ) {
                                { content.build() }
                            }
                        }
                    }
                }
            }
        }
    }
}
