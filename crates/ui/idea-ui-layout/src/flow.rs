//! [`Flow`] — a sequence of steps a person finishes, one at a time,
//! with its actions pinned at the bottom (rule `layout-flow`).

use std::rc::Rc;

use icons_lucide::{CHECK, CHEVRON_DOWN};
use idea_ui::{
    tone, typography_kind, variant, Button, Divider, Icon, Modal, Spacer, Stack, StackAlign, StackAxis, StackGap,
    Typography,
};
use runtime_core::Tokens as _;
use runtime_core::{
    component, pressable, rx, signal, stylesheet, ui, AlignItems, AlignSelf, ChildList, Cursor, Element,
    FlexDirection, IdealystSchema, IntoElement, JustifyContent, Length, Overflow, Position, Reactive, Ref,
    StyleRules, Tokenized, ViewHandle,
};

use crate::overlay::dialog_presentation;
use crate::page::{watch_height, Page, PageFrame, PageVariant};
use crate::split_view::{SplitMode, SplitView, SPLIT_NAV_WIDTH};
use crate::LayoutTheme;

/// The widest a step's heading and body run: a form line across a wide
/// monitor is unreadable.
const FLOW_COLUMN_MAX_WIDTH: f32 = 760.0;
/// The widest the step itself runs beside its list.
const FLOW_PANE_MAX_WIDTH: f32 = 720.0;
/// The room between the step list and the step.
const FLOW_RAIL_GAP: f32 = 40.0;
/// The progress bar's thickness under the step line.
const FLOW_PROGRESS_HEIGHT: f32 = 3.0;
/// A step's number badge, and the check drawn in it once done.
const FLOW_BADGE_PX: f32 = 24.0;
const FLOW_BADGE_CHECK_PX: f32 = 14.0;
/// A step row's least height in the phone's sheet: a touch target.
const FLOW_TOUCH_ROW_MIN_HEIGHT: f32 = 44.0;

/// One step of a [`Flow`]: what the step list and the heading say about
/// it.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct FlowStep {
    /// "Shifts": the heading over the step, and its row in the list.
    pub title: String,
    /// The muted line under the heading: what the step is for.
    pub subtitle: Option<String>,
    /// The one-line answer under the step's row once it has one ("Day,
    /// Night").
    pub status: Option<String>,
}

impl FlowStep {
    /// A step named `title`.
    pub fn new(title: impl Into<String>) -> Self {
        Self { title: title.into(), subtitle: None, status: None }
    }

    /// The muted line under the step's heading.
    pub fn subtitle(mut self, subtitle: impl Into<String>) -> Self {
        self.subtitle = Some(subtitle.into());
        self
    }

    /// The answer the step holds so far.
    pub fn status(mut self, status: Option<String>) -> Self {
        self.status = status;
        self
    }
}

/// The words a [`Flow`] draws itself, for an app in another language.
#[derive(Clone)]
pub struct FlowLabels {
    /// "Step 2 of 5", from the step's number (from one) and the count.
    pub step_of: Rc<dyn Fn(usize, usize) -> String>,
    /// The step sheet's heading.
    pub steps: String,
}

impl FlowLabels {
    pub fn english() -> Self {
        Self { step_of: Rc::new(|step, count| format!("Step {step} of {count}")), steps: "Steps".to_string() }
    }
}

/// The installed [`LayoutLabels::flow`](crate::LayoutLabels::flow).
impl Default for FlowLabels {
    fn default() -> Self {
        crate::layout_labels().flow.clone()
    }
}

#[runtime_core::props]
#[derive(IdealystSchema)]
pub struct FlowProps {
    /// The screen's header, its one exit (Cancel) among the actions.
    pub header: Element,
    /// Every step, in order. Live: a status that arrives with a save
    /// redraws the list.
    pub steps: Reactive<Vec<FlowStep>>,
    /// The index of the step on show.
    pub current: Reactive<usize>,
    /// Go back to a DONE step (one before `current`). Steps ahead never
    /// respond: a flow that saves on each Next would skip the write the
    /// step owes. `None`: the list is read-only.
    #[prop(static)]
    pub on_step: Option<Rc<dyn Fn(usize)>>,
    /// Under the heading: the step's error, as an `Alert`.
    pub notice: Option<Element>,
    /// A fixed row between the heading and the step, for a control that
    /// narrows what the step scrolls (a search). It draws its own
    /// gutter, and nothing at all for a step without one.
    pub toolbar: Option<Element>,
    /// The step's content: the one thing that scrolls. Key it on the
    /// step, so moving on rebuilds it and nothing else does.
    pub body: Element,
    /// The closing actions: an [`ActionBar`](crate::ActionBar) with
    /// `pinned = true` (Back, Next or the final verb).
    pub actions: Element,
    /// What the reader reads from while doing the step (the sections
    /// filled before it): a [`SplitMode::Detail`] panel beside the step
    /// from `Md`, stacked under it below. With one, the step list keeps
    /// its narrow form at every width, because the panel takes the room
    /// the list would.
    pub companion: Option<Element>,
    /// The words the flow draws itself.
    #[prop(static)]
    pub labels: FlowLabels,
    /// The screen's dialogs, mounted beside it.
    pub children: Vec<Element>,
}

impl Default for FlowProps {
    fn default() -> Self {
        Self {
            header: ui! { view {} },
            steps: Reactive::Static(Vec::new()),
            current: Reactive::Static(0),
            on_step: None,
            notice: None,
            toolbar: None,
            body: ui! { view {} },
            actions: ui! { view {} },
            companion: None,
            labels: FlowLabels::default(),
            children: Vec::new(),
        }
    }
}

/// A flow: the header, the step's heading, the step scrolling under it,
/// and its actions pinned at the foot. From `Md` the step list sits
/// beside the step, done steps leading back to themselves; below `Md`
/// it collapses to "Step 2 of 5 · Shifts" over a progress bar, which
/// opens the list as a sheet. Crossing `Md` restyles; the step is never
/// rebuilt.
///
/// ```ignore
/// Flow(
///     header = header,
///     steps = rx!(steps(w)),
///     current = rx!(w.step.get()),
///     on_step = Some(jump_back),
///     body = ui! { match w.step.get() { i => { StepBody(step = *i) } } },
///     actions = ui! { ActionBar(primary = Some(next), secondary = vec![back], pinned = true) },
/// ) {
///     ShiftEditor(…)
/// }
/// ```
#[component(children)]
pub fn Flow(props: FlowProps) -> Element {
    let FlowProps { header, steps, current, on_step, notice, toolbar, body, actions, companion, labels, children } =
        props;
    // The `#[component(children)]` slot's flatten, not a staged list.
    let mut overlays = Vec::with_capacity(children.len());
    for c in children {
        ChildList::append_to(c, &mut overlays);
    }
    let sheet_open = signal(false);
    let open_sheet: Rc<dyn Fn()> = Rc::new(move || sheet_open.set(true));
    let close_sheet: Rc<dyn Fn()> = Rc::new(move || sheet_open.set(false));

    let at = {
        let steps = steps.clone();
        let current = current.clone();
        move || {
            let all = steps.get();
            let i = current.get().min(all.len().saturating_sub(1));
            (all.get(i).cloned().unwrap_or_default(), i, all.len())
        }
    };
    let count_line = {
        let at = at.clone();
        let step_of = labels.step_of.clone();
        move || {
            let (_, i, n) = at();
            step_of(i + 1, n)
        }
    };
    let step_line = {
        let at = at.clone();
        let step_of = labels.step_of.clone();
        move || {
            let (step, i, n) = at();
            format!("{} · {}", step_of(i + 1, n), step.title)
        }
    };
    let title = {
        let at = at.clone();
        move || at().0.title
    };
    let subtitle = {
        let at = at.clone();
        move || at().0.subtitle.unwrap_or_default()
    };
    let progress = {
        let at = at.clone();
        move || {
            let (_, i, n) = at();
            fill_rules((i + 1) as f32 / n.max(1) as f32 * 100.0)
        }
    };
    // The list redraws when a step's status changes or the walk moves;
    // the rows hold nothing of their own.
    let list_key = {
        let steps = steps.clone();
        let current = current.clone();
        move || (steps.get(), current.get())
    };
    let sheet_key = list_key.clone();
    let rail_jump = on_step.clone();
    let sheet_jump = on_step;
    let sheet_title = labels.steps.clone();
    // With a companion the step line serves at every width.
    let line = companion.is_some();
    let (count_list, line_list) = match line {
        true => (FlowCountList::Line, FlowStepLineList::Line),
        false => (FlowCountList::Beside, FlowStepLineList::Beside),
    };
    let (layout_list, rail_list) = match line {
        true => (FlowLayoutList::Line, FlowRailList::Line),
        false => (FlowLayoutList::Beside, FlowRailList::Beside),
    };
    let column = ui! {
        view(style = FlowColumn()) {
            view(style = FlowLayout().list(layout_list)) {
                view(style = FlowRail().list(rail_list), test_id = "flow-steps") {
                    match list_key() {
                        (all, i) => {
                            FlowStepList(steps = all.clone(), current = *i, on_step = rail_jump.clone(), touch = false)
                        }
                    }
                }
                view(style = FlowPane(), test_id = "flow-step") { body }
            }
        }
    };
    // With a companion the split sits in the scroller, so below `Md`
    // the panel stacks under the step in the one scroll; beside, it
    // pins to the scroller's top and is never taller than the
    // scrollport.
    let scroller = match companion {
        None => ui! {
            scroll_view(style = FlowScroll()) {
                view(style = FlowScrollBody()) { column }
            }
        },
        Some(companion) => {
            let frame = PageFrame::new();
            let port: Ref<ViewHandle> = Ref::new();
            watch_height(port, frame.port_height);
            // Beside, the step's side is at least the scrollport's
            // height, so the panel and its rule run to the actions.
            let reach = move || StyleRules {
                flex_direction: Some(FlexDirection::Column),
                min_height: SplitMode::Detail
                    .beside()
                    .then(|| Tokenized::Literal(Length::Px(frame.port_height.get()))),
                ..Default::default()
            };
            ui! {
                view(style = FlowScroll(), bind = port) {
                    scroll_view(style = FlowScroll(), bind = frame.scroller) {
                        SplitView(
                            mode = SplitMode::Detail,
                            frame = Some(frame),
                            first = ui! {
                                view(style = reach) {
                                    view(style = FlowScrollBody()) { column }
                                }
                            },
                            second = companion,
                        )
                    }
                }
            }
        }
    };

    let frame = ui! {
        view(style = FlowFrame()) {
            view(style = FlowHead()) {
                view(style = FlowColumn()) {
                    Stack(gap = StackGap::Xs) {
                        view(style = FlowCount().list(count_list)) {
                            Typography(content = rx!(count_line()), kind = typography_kind::BodySm, muted = true)
                        }
                        Typography(content = rx!(title()), kind = typography_kind::H2)
                        Typography(content = rx!(subtitle()), muted = true)
                        if let Some(notice) = notice {
                            { notice }
                        }
                        view(style = FlowStepLine().list(line_list), test_id = "flow-step-line") {
                            Stack(axis = StackAxis::Row, gap = StackGap::Xs, align = StackAlign::Center) {
                                Button(
                                    label = rx!(step_line()),
                                    variant = variant::Ghost,
                                    tone = tone::Neutral,
                                    trailing_icon = Some(CHEVRON_DOWN),
                                    on_click = open_sheet.clone(),
                                    test_id = Some("flow-step-line-open"),
                                )
                                Spacer()
                            }
                            view(style = FlowProgressTrack()) {
                                view(style = progress) {}
                            }
                        }
                    }
                }
            }
            Divider()
            view(style = FlowToolbarSeat()) {
                if let Some(toolbar) = toolbar {
                    { toolbar }
                }
            }
            scroller
            actions
        }
    };

    ui! {
        Page(variant = PageVariant::Blank, header = header, body = frame)
        overlays
        Modal(
            open = sheet_open,
            presentation = dialog_presentation(),
            on_dismiss = Some(close_sheet.clone()),
            content = move || {
                let key = sheet_key.clone();
                let jump = sheet_jump.clone();
                let close = close_sheet.clone();
                let heading = sheet_title.clone();
                ui! {
                    Stack(gap = StackGap::Md) {
                        Typography(content = heading, kind = typography_kind::H3)
                        match key() {
                            (all, i) => {
                                FlowStepList(
                                    steps = all.clone(),
                                    current = *i,
                                    on_step = jump.clone().map(|f| {
                                        let close = close.clone();
                                        let f: Rc<dyn Fn(usize)> = Rc::new(move |n| {
                                            f(n);
                                            close();
                                        });
                                        f
                                    }),
                                    touch = true,
                                )
                            }
                        }
                    }
                }
            },
        )
    }
}

#[runtime_core::props]
#[derive(IdealystSchema)]
struct FlowStepListProps {
    /// Every step, in order.
    #[prop(static)]
    pub steps: Vec<FlowStep>,
    /// The index of the step on show.
    #[prop(static)]
    pub current: usize,
    /// Back to a done step.
    #[prop(static)]
    pub on_step: Option<Rc<dyn Fn(usize)>>,
    /// Touch-height rows, for the step sheet.
    #[prop(static)]
    pub touch: bool,
}

impl Default for FlowStepListProps {
    fn default() -> Self {
        Self { steps: Vec::new(), current: 0, on_step: None, touch: false }
    }
}

/// The steps, one row each: a number that becomes a check once done,
/// the title, and the step's status under it.
///
/// ```ignore
/// FlowStepList(steps = steps, current = 2_usize, on_step = Some(jump), touch = false)
/// ```
#[component]
fn FlowStepList(props: &FlowStepListProps) -> Element {
    let current = props.current;
    let on_step = props.on_step.clone();
    let touch = props.touch;
    let steps = props.steps.clone();
    ui! {
        Stack(gap = StackGap::Xs) {
            // Keyed on the index: the walk is fixed for the life of the
            // run.
            for row in steps.iter().enumerate(), key = row.0 {
                FlowStepRow(
                    number = row.0 + 1,
                    title = row.1.title.clone(),
                    status = row.1.status.clone(),
                    done = row.0 < current,
                    active = row.0 == current,
                    on_jump = on_step.clone().filter(|_| row.0 < current).map(|f| {
                        let index = row.0;
                        let jump: Rc<dyn Fn()> = Rc::new(move || f(index));
                        jump
                    }),
                    touch = touch,
                )
            }
        }
    }
}

#[runtime_core::props]
#[derive(IdealystSchema)]
struct FlowStepRowProps {
    /// The step's number, from one.
    #[prop(static)]
    pub number: usize,
    /// The step's name.
    #[prop(static)]
    pub title: String,
    /// The answer the step holds so far.
    #[prop(static)]
    pub status: Option<String>,
    /// The step is behind the one on show.
    #[prop(static)]
    pub done: bool,
    /// The step is the one on show.
    #[prop(static)]
    pub active: bool,
    /// Back to this step; `Some` only for a done step.
    #[prop(static)]
    pub on_jump: Option<Rc<dyn Fn()>>,
    /// A touch-height row, for the step sheet.
    #[prop(static)]
    pub touch: bool,
}

impl Default for FlowStepRowProps {
    fn default() -> Self {
        Self { number: 1, title: String::new(), status: None, done: false, active: false, on_jump: None, touch: false }
    }
}

/// One step's row. Every row draws the same badge, title and status, so
/// a half-walked list lines up; what differs is the wrapper: a done row
/// is a press target with a hover wash, the rest are labels.
///
/// ```ignore
/// FlowStepRow(number = 2_usize, title = "Shifts".to_string(), done = true, on_jump = Some(back_to_shifts))
/// ```
#[component]
fn FlowStepRow(props: &FlowStepRowProps) -> Element {
    let (done, active) = (props.done, props.active);
    let (done_size, size) = if props.touch {
        (FlowStepRowDoneSize::Touch, FlowStepRowFrameSize::Touch)
    } else {
        (FlowStepRowDoneSize::Compact, FlowStepRowFrameSize::Compact)
    };
    let state = if props.active { FlowStepRowFrameState::Active } else { FlowStepRowFrameState::Idle };
    let number = props.number;
    let title = props.title.clone();
    let status = props.status.clone();
    match props.on_jump.clone() {
        // `pressable` takes its children by value: `ui!` owns no
        // pressable tag, so the vec is the primitive's signature.
        Some(jump) => pressable(
            vec![ui! { FlowStepRowBody(number = number, title = title, status = status, done = done, active = active) }],
            move || jump(),
        )
        .with_style(FlowStepRowDone().size(done_size).into_style_application())
        .into_element(),
        None => ui! {
            view(style = FlowStepRowFrame().size(size).state(state)) {
                FlowStepRowBody(number = number, title = title, status = status, done = done, active = active)
            }
        },
    }
}

#[runtime_core::props]
#[derive(IdealystSchema)]
struct FlowStepRowBodyProps {
    /// The step's number, from one.
    #[prop(static)]
    pub number: usize,
    /// The step's name.
    #[prop(static)]
    pub title: String,
    /// The answer the step holds so far.
    #[prop(static)]
    pub status: Option<String>,
    /// The step is behind the one on show.
    #[prop(static)]
    pub done: bool,
    /// The step is the one on show.
    #[prop(static)]
    pub active: bool,
}

impl Default for FlowStepRowBodyProps {
    fn default() -> Self {
        Self { number: 1, title: String::new(), status: None, done: false, active: false }
    }
}

/// A row's badge (the number, or a check once the step is done) and its
/// title over its status.
///
/// ```ignore
/// FlowStepRowBody(number = 2_usize, title = "Shifts".to_string(), done = true)
/// ```
#[component]
fn FlowStepRowBody(props: &FlowStepRowBodyProps) -> Element {
    let done = props.done;
    let badge = if done {
        FlowStepNumberState::Done
    } else if props.active {
        FlowStepNumberState::Active
    } else {
        FlowStepNumberState::Idle
    };
    let number = props.number.to_string();
    let title = props.title.clone();
    let status = props.status.clone();
    ui! {
        view(style = FlowStepRowInner()) {
            view(style = FlowStepNumber().state(badge)) {
                if done {
                    Icon(data = CHECK, size = FLOW_BADGE_CHECK_PX)
                } else {
                    Typography(content = number, kind = typography_kind::BodySm)
                }
            }
            Stack(gap = StackGap::Xs) {
                Typography(content = title)
                if let Some(status) = status {
                    Typography(content = status, kind = typography_kind::BodySm, muted = true)
                }
            }
        }
    }
}

/// The progress fill: a percentage width, so not a static sheet.
fn fill_rules(pct: f32) -> StyleRules {
    let t = idea_ui::tokens();
    StyleRules {
        height: Some(Length::pct(100.0).into()),
        width: Some(Length::pct(pct).into()),
        background: Some(t.color.text()),
        ..Default::default()
    }
}

// The flow's gutter is the page's (rule `gutter`), so the step's
// heading stands under the page title. The step list's threshold is
// `SplitMode::Nav`'s, `md`: below it there is no room for the list
// beside a usable step.

stylesheet! {
    // The frame under the header. `height: 100%` and `flex_grow` both,
    // and `min_height: 0`, so the scroller in the middle can be clamped
    // and the actions stay on screen.
    FlowFrame<idea_ui::IdeaThemeRef> {
        base(t) {
            flex_direction: FlexDirection::Column,
            background: t.color.surface(),
            height: Length::pct(100.0),
            width: Length::pct(100.0),
            flex_grow: 1.0,
            min_height: Length::Px(0.0),
        }
    }
}

stylesheet! {
    // The step's heading: never scrolls, so the reader always sees
    // which step they are on.
    FlowHead<idea_ui::IdeaThemeRef> {
        base(t) {
            flex_direction: FlexDirection::Column,
            flex_shrink: 0.0,
            padding_left: LayoutTheme::tokens().gutter.phone(),
            padding_right: LayoutTheme::tokens().gutter.phone(),
            padding_top: t.spacing.lg(),
            padding_bottom: t.spacing.md(),
        }
        breakpoint sm(t) {
            padding_left: LayoutTheme::tokens().gutter.tablet(),
            padding_right: LayoutTheme::tokens().gutter.tablet(),
        }
        breakpoint lg(t) {
            padding_left: LayoutTheme::tokens().gutter.desktop(),
            padding_right: LayoutTheme::tokens().gutter.desktop(),
        }
    }
}

stylesheet! {
    // The column a step is read in. Left-aligned, under the page title.
    FlowColumn<idea_ui::IdeaThemeRef> {
        base(_t) {
            flex_direction: FlexDirection::Column,
            width: Length::pct(100.0),
            max_width: Length::Px(FLOW_COLUMN_MAX_WIDTH),
            align_self: AlignSelf::FlexStart,
        }
    }
}

stylesheet! {
    // "Step 2 of 6" over the heading, where the list is beside the
    // step. Collapsed by height below `md`, where the step line says it.
    FlowCount<idea_ui::IdeaThemeRef> {
        base(_t) {
            flex_direction: FlexDirection::Column,
            height: Length::Px(0.0),
            min_height: Length::Px(0.0),
            overflow: Overflow::Hidden,
        }
        breakpoint md(_t) {
            height: Length::Auto,
            min_height: Length::Auto,
            overflow: Overflow::Visible,
        }
        // The step line serves at every width. Variant arms sort after
        // the breakpoint arms, so this wins over `md`.
        variant list {
            #[default]
            beside(_t) {}
            line(_t) {
                height: Length::Px(0.0),
                min_height: Length::Px(0.0),
                overflow: Overflow::Hidden,
            }
        }
    }
}

stylesheet! {
    // The narrow form of the list: "Step 1 of 6 · Basics ▾" over its
    // progress bar. Collapsed from `md`, where the list is beside.
    FlowStepLine<idea_ui::IdeaThemeRef> {
        base(t) {
            flex_direction: FlexDirection::Column,
            gap: t.spacing.xs(),
            width: Length::pct(100.0),
        }
        breakpoint md(_t) {
            height: Length::Px(0.0),
            min_height: Length::Px(0.0),
            overflow: Overflow::Hidden,
        }
        variant list {
            #[default]
            beside(_t) {}
            line(_t) {
                height: Length::Auto,
                min_height: Length::Auto,
                overflow: Overflow::Visible,
            }
        }
    }
}

stylesheet! {
    FlowProgressTrack<idea_ui::IdeaThemeRef> {
        base(t) {
            height: Length::Px(FLOW_PROGRESS_HEIGHT),
            width: Length::pct(100.0),
            background: t.color.border(),
            border_radius: t.radius.sm(),
            overflow: Overflow::Hidden,
        }
    }
}

stylesheet! {
    // A step's fixed row. No padding: a step without one draws nothing,
    // which must measure zero.
    FlowToolbarSeat<idea_ui::IdeaThemeRef> {
        base(_t) {
            flex_direction: FlexDirection::Column,
            flex_shrink: 0.0,
        }
    }
}

stylesheet! {
    // The one scroller: grows into what the heading and the actions
    // leave and is clamped to it (`min_height: 0`), so a long step
    // scrolls rather than pushing the actions off the screen.
    FlowScroll<idea_ui::IdeaThemeRef> {
        base(_t) {
            flex_direction: FlexDirection::Column,
            flex_grow: 1.0,
            flex_shrink: 1.0,
            min_height: Length::Px(0.0),
        }
    }
}

stylesheet! {
    // What the scroller scrolls, at its own height (`flex_shrink: 0`),
    // in the page gutter.
    FlowScrollBody<idea_ui::IdeaThemeRef> {
        base(t) {
            flex_direction: FlexDirection::Column,
            flex_shrink: 0.0,
            padding_left: LayoutTheme::tokens().gutter.phone(),
            padding_right: LayoutTheme::tokens().gutter.phone(),
            padding_top: t.spacing.md(),
            padding_bottom: t.spacing.lg(),
        }
        breakpoint sm(t) {
            padding_left: LayoutTheme::tokens().gutter.tablet(),
            padding_right: LayoutTheme::tokens().gutter.tablet(),
        }
        breakpoint lg(t) {
            padding_left: LayoutTheme::tokens().gutter.desktop(),
            padding_right: LayoutTheme::tokens().gutter.desktop(),
        }
    }
}

stylesheet! {
    // The list beside the step from `md`; one column below it.
    FlowLayout<idea_ui::IdeaThemeRef> {
        base(t) {
            flex_direction: FlexDirection::Column,
            gap: t.spacing.md(),
            width: Length::pct(100.0),
        }
        breakpoint md(_t) {
            flex_direction: FlexDirection::Row,
            align_items: AlignItems::FlexStart,
            gap: Length::Px(FLOW_RAIL_GAP),
        }
        variant list {
            #[default]
            beside(_t) {}
            line(t) {
                flex_direction: FlexDirection::Column,
                align_items: AlignItems::Stretch,
                gap: t.spacing.md(),
            }
        }
    }
}

stylesheet! {
    // The step list. Collapsed by HEIGHT below `md` (a row in a column
    // there), with `min_height: 0` or the content floors it; from `md`
    // as wide as a split's nav and sticky, so it stays with the reader
    // down a long step.
    FlowRail<idea_ui::IdeaThemeRef> {
        base(_t) {
            flex_direction: FlexDirection::Column,
            height: Length::Px(0.0),
            min_height: Length::Px(0.0),
            overflow: Overflow::Hidden,
        }
        breakpoint md(_t) {
            height: Length::Auto,
            min_height: Length::Auto,
            width: Length::Px(SPLIT_NAV_WIDTH),
            flex_shrink: 0.0,
            overflow: Overflow::Visible,
            position: Position::Sticky,
            top: Length::Px(0.0),
        }
        variant list {
            #[default]
            beside(_t) {}
            line(_t) {
                height: Length::Px(0.0),
                min_height: Length::Px(0.0),
                width: Length::Auto,
                overflow: Overflow::Hidden,
                position: Position::Relative,
            }
        }
    }
}

stylesheet! {
    // The step, capped where a form line stops being readable.
    FlowPane<idea_ui::IdeaThemeRef> {
        base(t) {
            flex_direction: FlexDirection::Column,
            gap: t.spacing.md(),
            width: Length::pct(100.0),
            min_width: Length::Px(0.0),
        }
        breakpoint md(_t) {
            max_width: Length::Px(FLOW_PANE_MAX_WIDTH),
        }
    }
}

stylesheet! {
    // A step's row that does not respond: the step on show takes a soft
    // fill.
    FlowStepRowFrame<idea_ui::IdeaThemeRef> {
        base(t) {
            flex_direction: FlexDirection::Row,
            align_items: AlignItems::FlexStart,
            padding_top: t.spacing.xs(),
            padding_bottom: t.spacing.xs(),
            padding_left: t.spacing.sm(),
            padding_right: t.spacing.sm(),
            border_radius: t.radius.md(),
        }
        variant size {
            #[default]
            compact(_t) {}
            touch(_t) { min_height: Length::Px(FLOW_TOUCH_ROW_MIN_HEIGHT), align_items: AlignItems::Center }
        }
        variant state {
            #[default]
            idle(_t) {}
            active(t) { background: t.color.surface_alt() }
        }
    }
}

stylesheet! {
    // A done step's row: the same geometry as `FlowStepRowFrame`, as a
    // link back. Its own sheet because a `state` block cannot see a
    // variant, and a hover wash on the rows ahead would light rows that
    // do not respond.
    FlowStepRowDone<idea_ui::IdeaThemeRef> {
        base(t) {
            flex_direction: FlexDirection::Row,
            align_items: AlignItems::FlexStart,
            padding_top: t.spacing.xs(),
            padding_bottom: t.spacing.xs(),
            padding_left: t.spacing.sm(),
            padding_right: t.spacing.sm(),
            border_radius: t.radius.md(),
            cursor: Cursor::Pointer,
        }
        variant size {
            #[default]
            compact(_t) {}
            touch(_t) { min_height: Length::Px(FLOW_TOUCH_ROW_MIN_HEIGHT), align_items: AlignItems::Center }
        }
        state hovered(t) { background: t.color.surface_alt() }
        state pressed(t) { background: t.color.surface_alt(), opacity: 0.8 }
        transitions { background: 120ms EaseOut }
    }
}

stylesheet! {
    // Inside a row: the badge beside the title and status.
    FlowStepRowInner<idea_ui::IdeaThemeRef> {
        base(t) {
            flex_direction: FlexDirection::Row,
            align_items: AlignItems::FlexStart,
            gap: t.spacing.sm(),
            min_width: Length::Px(0.0),
        }
    }
}

stylesheet! {
    // The step's number, a filled check once done: "active" and "done"
    // are different answers.
    FlowStepNumber<idea_ui::IdeaThemeRef> {
        base(t) {
            width: Length::Px(FLOW_BADGE_PX),
            height: Length::Px(FLOW_BADGE_PX),
            flex_shrink: 0.0,
            border_radius: t.radius.pill(),
            border_width: 1.0,
            border_color: t.color.border(),
            background: t.color.surface(),
            align_items: AlignItems::Center,
            justify_content: JustifyContent::Center,
        }
        variant state {
            #[default]
            idle(_t) {}
            active(t) { border_color: t.color.text() }
            done(t) {
                background: t.color.text(),
                border_color: t.color.text(),
                color: t.color.surface(),
            }
        }
    }
}
