//! [`DrillSheet`] — a sheet with levels, and the [`DrillCx`] its
//! builders move between them with.

use std::rc::Rc;

use icons_lucide::CHEVRON_LEFT;
use idea_ui::{tone, typography_kind, variant, IconButton, Stack, StackAlign, StackAxis, StackGap, Typography};
use runtime_core::{component, signal, ui, Element, IdealystSchema, Reactive, Signal};

use super::Sheet;

/// What a [`DrillSheet`]'s builders move between levels with.
#[derive(Clone)]
pub struct DrillCx {
    /// Push the pane at this index over the root.
    pub push: Rc<dyn Fn(usize)>,
    /// Pop to the root.
    pub back: Rc<dyn Fn()>,
    /// Close the sheet.
    pub done: Rc<dyn Fn()>,
}

#[runtime_core::props]
#[derive(IdealystSchema)]
pub struct DrillSheetProps {
    /// Whether the sheet is up: see [`SheetProps::open`](super::SheetProps).
    pub open: bool,
    /// Close the sheet.
    #[prop(static)]
    pub on_dismiss: Rc<dyn Fn()>,
    /// Which level is showing: `None` the root, `Some(i)` pane `i`.
    /// Owned by the caller and created outside whatever rebuilds this:
    /// where the sheet lands on open (the root, or one pane) is the
    /// caller's decision.
    pub level: Signal<Option<usize>>,
    /// Builds the root: the push rows and anything beside them.
    /// `cx.push(i)` is what a row's press does.
    #[prop(static)]
    pub root: Rc<dyn Fn(DrillCx) -> Element>,
    /// The title over pane `i`, beside the back chevron.
    #[prop(static)]
    pub pane_title: Rc<dyn Fn(usize) -> String>,
    /// Builds pane `i`'s body, under its title. `cx.back()` pops;
    /// `cx.done()` closes.
    #[prop(static)]
    pub pane: Rc<dyn Fn(usize, DrillCx) -> Element>,
}

impl Default for DrillSheetProps {
    fn default() -> Self {
        Self {
            open: Reactive::Static(false),
            on_dismiss: Rc::new(|| {}),
            level: signal(None),
            root: Rc::new(|_| ui! { view {} }),
            pane_title: Rc::new(|_| String::new()),
            pane: Rc::new(|_, _| ui! { view {} }),
        }
    }
}

fn drill_cx(level: Signal<Option<usize>>, dismiss: Rc<dyn Fn()>) -> DrillCx {
    DrillCx {
        push: Rc::new(move |i| level.set(Some(i))),
        back: Rc::new(move || level.set(None)),
        done: Rc::new(move || dismiss()),
    }
}

/// A sheet with levels: the phone form of a menu with submenus (rule
/// `menus`). The root is a list of [`SheetPushRow`](super::SheetPushRow)s
/// and whatever the caller puts beside them; pressing one replaces the
/// root with its pane under a titled back chevron.
///
/// ```ignore
/// DrillSheet(
///     open = rx!(open.get()),
///     on_dismiss = dismiss.clone(),
///     level = level,
///     root = Rc::new(move |cx| root_rows(cx)),
///     pane_title = Rc::new(move |i| dims[i].label.clone()),
///     pane = Rc::new(move |i, cx| pane_body(i, cx)),
/// )
/// ```
#[component]
pub fn DrillSheet(props: &DrillSheetProps) -> Element {
    let open = props.open.clone();
    let on_dismiss = props.on_dismiss.clone();
    let level = props.level;
    let root = props.root.clone();
    let pane_title = props.pane_title.clone();
    let pane = props.pane.clone();
    let content_dismiss = on_dismiss.clone();
    let content: Rc<dyn Fn() -> Element> = Rc::new(move || {
        let root = root.clone();
        let pane_title = pane_title.clone();
        let pane = pane.clone();
        let dismiss = content_dismiss.clone();
        // The two levels hold different elements, so they are a `match`.
        ui! {
            match level.get() {
                None => { root(drill_cx(level, dismiss.clone())) }
                Some(i) => {
                    {
                        let cx = drill_cx(level, dismiss.clone());
                        let title = pane_title(*i);
                        let body = pane(*i, cx.clone());
                        let back = cx.back.clone();
                        ui! {
                            Stack(gap = StackGap::Md) {
                                Stack(axis = StackAxis::Row, gap = StackGap::Xs, align = StackAlign::Center) {
                                    IconButton(
                                        icon = Some(CHEVRON_LEFT),
                                        variant = variant::Ghost,
                                        tone = tone::Neutral,
                                        label = Some("Back".to_string()),
                                        on_click = back.clone(),
                                    )
                                    Typography(content = title, kind = typography_kind::H3)
                                }
                                body
                            }
                        }
                    }
                }
            }
        }
    });
    ui! {
        Sheet(open = open, on_dismiss = on_dismiss.clone(), content = { let content = content.clone(); move || content() })
    }
}
