//! [`ActionMenu`] — a set of verbs behind one trigger: an anchored menu
//! with a pointer, an action sheet on a phone.

use std::rc::Rc;

use icons_lucide::ELLIPSIS_VERTICAL;
use idea_ui::{
    variant, Icon, IconButton, IconButtonSize, IdeaThemeRef, Menu, MenuItem, MenuLabel, MenuSlotStyles, Stack,
    StackAxis, StackGap, StackJustify, ToneRef,
};
use runtime_core::{
    component, rx, signal, ui, AnchorTarget, Element, IconData, IdealystSchema, PressableHandle, Reactive, Ref,
    ScopeAlive, Signal,
};

use super::{sheet_serves, Sheet, SheetActionRow};
use crate::Action;

/// A menu row's leading glyph, in px.
const MENU_ICON_PX: f32 = 16.0;
/// A sheet row's leading glyph, in px: a touch row is taller.
const SHEET_ICON_PX: f32 = 18.0;

#[runtime_core::props]
#[derive(IdealystSchema)]
pub struct ActionMenuProps {
    /// A label row above the entries: what the actions apply to
    /// ("3 people selected"). `None` draws none.
    pub heading: Option<String>,
    /// The entries, in order. An empty list draws nothing, trigger
    /// included.
    #[prop(static)]
    pub actions: Vec<Action>,
    /// The trigger's glyph. `None`: ⋮.
    #[prop(static)]
    pub icon: Option<IconData>,
    /// The trigger's accessible name.
    #[prop(static)]
    pub label: String,
    /// The trigger's size.
    #[prop(static)]
    pub size: IconButtonSize,
    /// The trigger's robot/E2E id.
    #[prop(static)]
    pub test_id: Option<&'static str>,
}

impl Default for ActionMenuProps {
    fn default() -> Self {
        Self {
            heading: Reactive::Static(None),
            actions: Vec::new(),
            icon: None,
            label: "More actions".to_string(),
            size: IconButtonSize::Sm,
            test_id: None,
        }
    }
}

/// A ⋮ trigger and the verbs behind it (rule `overlays`): an anchored
/// menu on a tablet or desktop, an action sheet on a phone, the same
/// entries in the same order. Self-contained: each instance owns its
/// open state, so a rebuilt table row mounts a fresh, closed menu.
///
/// ```ignore
/// ActionMenu(actions = vec![
///     Action::new("Rename", rename).icon(PENCIL),
///     Action::new("Delete", delete).icon(TRASH),
/// ])
/// ```
#[component]
pub fn ActionMenu(props: &ActionMenuProps) -> Element {
    let actions = props.actions.clone();
    let has_actions = !actions.is_empty();
    let heading = props.heading.clone();
    let glyph = props.icon.unwrap_or(ELLIPSIS_VERTICAL);
    let label = props.label.clone();
    let size = props.size;
    let test_id = props.test_id;
    let open = signal(false);
    let trigger: Ref<PressableHandle> = Ref::new();
    let on_open: Rc<dyn Fn()> = Rc::new(move || open.set(true));
    ui! {
        // No gap: the open surface is a second child of this row. Its
        // content is a portal, but the node holding it is a zero-size
        // flex item, and a gap beside it would shift the trigger as the
        // menu opens.
        Stack(axis = StackAxis::Row, justify = StackJustify::Center, gap = StackGap::None) {
            if has_actions {
                IconButton(
                    icon = Some(glyph),
                    variant = variant::Ghost,
                    size = size,
                    on_click = on_open.clone(),
                    bind_to = Some(trigger),
                    label = Some(label.clone()),
                    test_id = test_id,
                )
            }
            ActionMenuSurface(
                open = open,
                target = Some(AnchorTarget::from(trigger)),
                heading = heading.clone(),
                actions = actions.clone(),
            )
        }
    }
}

#[runtime_core::props]
#[derive(IdealystSchema)]
pub struct ActionMenuSurfaceProps {
    /// The open state. The surface closes itself through it: on a
    /// dismissal, and before an entry's handler runs.
    pub open: Signal<bool>,
    /// What the anchored menu opens beside. The sheet ignores it.
    #[prop(static)]
    pub target: Option<AnchorTarget>,
    /// A label row above the entries.
    pub heading: Option<String>,
    /// The entries, in order.
    #[prop(static)]
    pub actions: Vec<Action>,
}

impl Default for ActionMenuSurfaceProps {
    fn default() -> Self {
        Self {
            open: signal(false),
            target: None,
            heading: Reactive::Static(None),
            actions: Vec::new(),
        }
    }
}

/// The surface of an [`ActionMenu`] without its trigger, for a menu a
/// screen opens from something of its own: a right-click, a long press,
/// a row's visible button.
///
/// Mount it once, ungated, and drive `open`: the sheet form has to stay
/// mounted to play its exit.
///
/// ```ignore
/// ActionMenuSurface(open = open, target = Some(AnchorTarget::from(row_ref)), actions = actions.clone())
/// ```
#[component]
pub fn ActionMenuSurface(props: &ActionMenuSurfaceProps) -> Element {
    let open = props.open;
    let target = props.target.clone();
    let heading = props.heading.clone();
    // Entries run inside THIS component's scope, not the pressed row's:
    // closing the surface tears down the subtree that mounted the row,
    // and work an entry spawns there would die before its callback ran.
    let anchor = ScopeAlive::current();
    let actions: Vec<Action> = props
        .actions
        .iter()
        .map(|a| {
            let run = a.guarded_press();
            let anchor = anchor.clone();
            let mut a = a.clone();
            a.on_press = Rc::new(move || {
                open.set(false);
                anchor.run_within(|| run());
            });
            a
        })
        .collect();
    let on_dismiss: Rc<dyn Fn()> = Rc::new(move || open.set(false));
    ui! {
        match sheet_serves() {
            true => {
                {
                    let heading = heading.clone();
                    let actions = actions.clone();
                    let content: Rc<dyn Fn() -> Element> = Rc::new(move || {
                        ui! {
                            Stack(gap = StackGap::Xs) {
                                MenuHeading(source = heading.clone())
                                // A static list: keyed for identity only.
                                for a in actions.iter(), key = a.label.clone() {
                                    SheetEntry(action = a.clone())
                                }
                            }
                        }
                    });
                    ui! {
                        Sheet(open = rx!(open.get()), on_dismiss = on_dismiss.clone(), content = { let content = content.clone(); move || content() })
                    }
                }
            }
            false => {
                {
                    // Gated on `open`, unlike the sheet: a `Menu` has no
                    // `open` prop (mounting it IS opening it), while the
                    // sheet takes one and must stay mounted through its
                    // exit.
                    let actions = actions.clone();
                    let heading = heading.clone();
                    let dismiss = on_dismiss.clone();
                    let target = target.clone();
                    ui! {
                        if open.get() {
                            Menu(target = target.clone(), on_dismiss = Some(dismiss.clone())) {
                                MenuHeading(source = heading.clone())
                                for a in actions.iter(), key = a.label.clone() {
                                    MenuEntry(action = a.clone())
                                }
                            }
                        }
                    }
                }
            }
        }
    }
}

#[runtime_core::props]
#[derive(IdealystSchema)]
struct MenuHeadingProps {
    /// The live heading; nothing is drawn while it reads `None`.
    pub source: Option<String>,
}

impl Default for MenuHeadingProps {
    fn default() -> Self {
        Self { source: Reactive::Static(None) }
    }
}

/// The label row over the entries, drawn only while there is one.
///
/// ```ignore
/// MenuHeading(source = heading.clone())
/// ```
#[component]
fn MenuHeading(props: &MenuHeadingProps) -> Element {
    let gate = props.source.clone();
    let text = props.source.clone();
    ui! {
        if gate.get().is_some() {
            LiveMenuLabel(source = text.clone())
        }
    }
}

#[runtime_core::props]
#[derive(IdealystSchema)]
struct LiveMenuLabelProps {
    /// The live label text; `None` reads as empty.
    pub source: Option<String>,
}

impl Default for LiveMenuLabelProps {
    fn default() -> Self {
        Self { source: Reactive::Static(None) }
    }
}

/// The label row itself, its text following the live heading.
///
/// ```ignore
/// LiveMenuLabel(source = text.clone())
/// ```
#[component]
fn LiveMenuLabel(props: &LiveMenuLabelProps) -> Element {
    let source = props.source.clone();
    ui! {
        MenuLabel(text = rx!(source.get().unwrap_or_default()))
    }
}

#[runtime_core::props]
#[derive(IdealystSchema, Default)]
struct SheetEntryProps {
    /// The entry this row presses.
    #[prop(static)]
    pub action: Action,
}

/// One entry as a sheet row. Not a `MenuItem`: that takes its keyboard
/// and roving focus from a `Menu` above it, and a sheet has none.
///
/// ```ignore
/// SheetEntry(action = a.clone())
/// ```
#[component]
fn SheetEntry(props: &SheetEntryProps) -> Element {
    let action = props.action.clone();
    let tone = action.tone.clone();
    let leading = action.icon.map(|d| ui! { Icon(data = d, size = SHEET_ICON_PX, tone = tone.clone()) });
    ui! {
        SheetActionRow(
            label = action.label.clone(),
            leading = leading,
            on_press = action.on_press.clone(),
            tone = action.tone.clone(),
        )
    }
}

#[runtime_core::props]
#[derive(IdealystSchema, Default)]
struct MenuEntryProps {
    /// The entry this row presses.
    #[prop(static)]
    pub action: Action,
}

/// One entry as an anchored menu's row.
///
/// ```ignore
/// MenuEntry(action = a.clone())
/// ```
#[component]
fn MenuEntry(props: &MenuEntryProps) -> Element {
    let action = props.action.clone();
    let tone = action.tone.clone();
    let leading = action.icon.map(|d| ui! { Icon(data = d, size = MENU_ICON_PX, tone = tone.clone()) });
    ui! {
        MenuItem(
            label = action.label.clone(),
            on_select = action.on_press.clone(),
            leading = leading,
            test_id = action.test_id,
            styles = toned_item(action.tone.as_ref()),
        )
    }
}

/// A menu row's label in its action's tone: the tone's ink, as a toned
/// `Typography` draws it (a token, so it follows a theme swap). No tone,
/// no override.
fn toned_item(tone: Option<&ToneRef>) -> MenuSlotStyles {
    let ink = tone.and_then(|tone| {
        let theme = idea_ui::active_theme_untracked();
        theme.downcast_ref::<IdeaThemeRef>().map(|th| tone.stroke_fg(th))
    });
    match ink {
        Some(color) => MenuSlotStyles::default().item_label(runtime_core::StyleRules {
            color: Some(color),
            ..Default::default()
        }),
        None => MenuSlotStyles::default(),
    }
}
