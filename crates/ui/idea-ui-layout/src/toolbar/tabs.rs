//! [`ToolbarTabs`] — a screen's tabs as a row of pills that never
//! scrolls: the ones that do not fit move into an "N more" menu.

use std::cell::RefCell;
use std::rc::Rc;

use icons_lucide::CHEVRON_DOWN;
use idea_ui::{tone, AnchorTarget, Button, ButtonSize, ButtonVariant, Tab, Tabs, TabsSlotStyles};
use runtime_core::{
    after_animation_frame, after_ms_detached, component, effect, on_scope_drop, rx, signal, stylesheet, ui,
    AlignContent, AlignItems, Element, FlexDirection, FlexWrap, IdealystSchema, LayoutSubscription, Length,
    Overflow, Reactive, Ref, ScheduledTask, ScopeAlive, Signal, StyleRules, Tokenized, ViewHandle,
};

use crate::overlay::ActionMenuSurface;
use crate::Action;

/// The step between pills, and before "N more".
const TAB_GAP: f32 = 6.0;
/// The least room "N more" is given when deciding what fits beside it,
/// before it has been measured.
const MORE_WIDTH: f32 = 92.0;
/// One line of pills.
const ROW_HEIGHT: f32 = 28.0;
/// How far the pill row's clip reaches below the pills.
const CLIP_SLACK: f32 = 2.0;
/// A width change smaller than this is layout noise, not a resize.
const WIDTH_EPSILON: f32 = 0.5;

#[runtime_core::props]
#[derive(IdealystSchema)]
pub struct ToolbarTabsProps {
    /// The tabs, in order. Live: a tab arriving or leaving rebuilds the
    /// row.
    pub tabs: Signal<Vec<Tab>>,
    /// The open tab's id.
    pub active: Reactive<String>,
    /// A tab was picked, from the row or from the menu.
    #[prop(static)]
    pub on_change: Rc<dyn Fn(String)>,
    /// The overflow button's words, from how many tabs it holds:
    /// "3 more".
    #[prop(static)]
    pub more_label: Rc<dyn Fn(usize) -> String>,
}

impl Default for ToolbarTabsProps {
    fn default() -> Self {
        Self {
            tabs: signal(Vec::new()),
            active: Reactive::Static(String::new()),
            on_change: Rc::new(|_| {}),
            more_label: Rc::new(|n| format!("{n} more")),
        }
    }
}

/// A screen's tabs (rule `screen-tabs`), with idea-ui `Tabs`' three
/// props. As many pills as the row has room for are drawn; the rest are
/// listed under an "N more" button at the row's end, which reads as the
/// open one while the open tab is in its menu. Nothing scrolls
/// sideways, so the same strip serves a phone and a wide window, and a
/// reader can always see that there are more.
///
/// Tabs switch views of ONE screen: Back leaves the screen, and the
/// header does not change between them.
///
/// Give it a seat that hands it the row's width: a [`Toolbar`]'s
/// `start` slot does.
///
/// [`Toolbar`]: super::Toolbar
///
/// ```ignore
/// ToolbarTabs(tabs = tabs, active = rx!(open.get()), on_change = pick)
/// ```
#[component]
pub fn ToolbarTabs(props: &ToolbarTabsProps) -> Element {
    let tabs = props.tabs;
    let active = props.active.clone();
    let on_change = props.on_change.clone();
    let count_label = props.more_label.clone();
    // The ids of the tabs the row has no room for.
    let hidden: Signal<Vec<String>> = signal(Vec::new());
    let menu_open = signal(false);
    let anchor: Ref<ViewHandle> = Ref::new();
    // The row's own width: the room the pills and "N more" share.
    let room = signal(0.0_f32);
    let row: Ref<ViewHandle> = Ref::new();
    let subs: Rc<RefCell<Option<LayoutSubscription>>> = Rc::new(RefCell::new(None));
    let held = subs.clone();
    // The detached timers below can fire after the strip is gone; a
    // signal read then would be through a freed handle.
    let alive = ScopeAlive::current();
    let task = after_animation_frame(move || {
        if let Some(Some(frame)) = row.with(|h| h.frame()) {
            room.set(frame.width);
        }
        *held.borrow_mut() = row.with(|h| {
            // Layout callbacks run outside a turn, so the write is
            // deferred into one.
            let alive = alive.clone();
            h.on_layout(move |w, _| {
                let alive = alive.clone();
                after_ms_detached(0, move || {
                    if alive.get() && (room.peek() - w).abs() > WIDTH_EPSILON {
                        room.set(w);
                    }
                })
            })
        });
    });
    on_scope_drop(move || {
        drop(task);
        drop(subs);
    });
    // "N more", or the open tab's own name when it is in the menu and
    // the row has room for one tab at most: there, a count would leave
    // the reader with no word for where they are.
    let more_label: Reactive<String> = {
        let active = active.clone();
        rx!({
            let hidden = hidden.get();
            let all = tabs.get();
            let open = active.get();
            let shown = all.len().saturating_sub(hidden.len());
            match all.iter().find(|t| t.id == open) {
                Some(tab) if shown <= 1 && hidden.contains(&open) => tab.label.get(),
                _ => count_label(hidden.len()),
            }
        })
    };
    let more_variant: Reactive<ButtonVariant> = {
        let active = active.clone();
        rx!(if hidden.get().contains(&active.get()) { ButtonVariant::Soft } else { ButtonVariant::Outlined })
    };
    // The "N more" button's laid-out width, which the fit leaves room
    // for. A tab's name is wider than a count: fitted against the
    // count's room, the pill beside it would be cut.
    let more_w = signal(MORE_WIDTH);
    {
        let (label, alive) = (more_label.clone(), ScopeAlive::current());
        let task: Rc<RefCell<Option<ScheduledTask>>> = Rc::new(RefCell::new(None));
        let held = task.clone();
        effect!({
            let _ = (hidden.get(), label.get());
            let alive = alive.clone();
            *held.borrow_mut() = Some(after_animation_frame(move || {
                if !alive.get() {
                    return;
                }
                if let Some(Some(frame)) = anchor.with(|h| h.frame()) {
                    if frame.width > 0.0 && (more_w.peek() - frame.width).abs() > WIDTH_EPSILON {
                        more_w.set(frame.width);
                    }
                }
            }));
        });
        on_scope_drop(move || drop(task));
    }
    let open_menu: Rc<dyn Fn()> = Rc::new(move || menu_open.set(!menu_open.peek()));
    let ids = move || tabs.get().iter().map(|t| t.id.clone()).collect::<Vec<String>>();
    let pick = on_change.clone();
    // The overflow as verbs: an anchored menu with a pointer, an action
    // sheet on a phone.
    let overflow = move || -> Vec<Action> {
        let hidden = hidden.get();
        tabs.peek()
            .into_iter()
            .filter(|t| hidden.contains(&t.id))
            .map(|t| {
                let (id, pick) = (t.id.clone(), pick.clone());
                Action::new(t.label.get(), Rc::new(move || pick(id.clone())))
            })
            .collect()
    };
    ui! {
        view(style = TabsRow(), bind = row, test_id = "toolbar-tabs") {
            match ids() {
                _ids => {
                    TabsFit(
                        tabs = tabs.peek(),
                        active = active.clone(),
                        on_change = on_change.clone(),
                        hidden = hidden,
                        room = room,
                        more_w = more_w,
                    )
                }
            }
            if !hidden.get().is_empty() {
                view(style = TabsMoreSeat(), bind = anchor) {
                    // A button, not a tab: it carries the chevron that
                    // says it opens a list. Filled, like an open tab,
                    // while the open tab is in its menu.
                    Button(
                        label = more_label.clone(),
                        trailing_icon = Some(CHEVRON_DOWN),
                        variant = more_variant.clone(),
                        tone = tone::Neutral,
                        // The small button is a pill's height, so it
                        // stands on the pills' line.
                        size = ButtonSize::Sm,
                        on_click = open_menu.clone(),
                        test_id = Some("toolbar-tabs-more"),
                    )
                }
            }
            match overflow() {
                actions => {
                    ActionMenuSurface(open = menu_open, target = Some(AnchorTarget::from(anchor)), actions = actions.clone())
                }
            }
        }
    }
}

#[runtime_core::props]
#[derive(IdealystSchema)]
struct TabsFitProps {
    /// The tabs, in order.
    #[prop(static)]
    tabs: Vec<Tab>,
    /// The open tab's id.
    active: Reactive<String>,
    /// A pill was picked.
    #[prop(static)]
    on_change: Rc<dyn Fn(String)>,
    /// Written with the ids of the pills that did not fit.
    hidden: Signal<Vec<String>>,
    /// The width the pills and "N more" have between them.
    room: Signal<f32>,
    /// "N more"'s own width, as last laid out.
    more_w: Signal<f32>,
}

impl Default for TabsFitProps {
    fn default() -> Self {
        Self {
            tabs: Vec::new(),
            active: Reactive::Static(String::new()),
            on_change: Rc::new(|_| {}),
            hidden: signal(Vec::new()),
            room: signal(0.0),
            more_w: signal(MORE_WIDTH),
        }
    }
}

/// How many pills show, and the width the pill row is held to (`None`
/// while every pill fits). The pills that fit beside "N more", and at
/// least one.
fn fit_count(widths: &[f32], room: f32, more: f32) -> (usize, Option<f32>) {
    let run = |n: usize| widths[..n].iter().sum::<f32>() + TAB_GAP * n.saturating_sub(1) as f32;
    if run(widths.len()) <= room {
        return (widths.len(), None);
    }
    let n = (1..widths.len()).rev().find(|n| run(*n).ceil() + 1.0 + TAB_GAP + more <= room).unwrap_or(1);
    // A pixel over: the widths are fractional, and a row held to their
    // exact sum can round a pill onto the next line.
    (n, Some(run(n).ceil() + 1.0))
}

/// The pills, in a one-line row that WRAPS and clips. Every pill is
/// built and keeps its own width, fitting or not, so the widths can be
/// read; the row is then held to exactly the pills that fit, which
/// sends the rest to a second line nobody sees and leaves "N more"
/// standing right after the last one showing.
///
/// ```ignore
/// TabsFit(tabs = tabs, active = active, on_change = pick, hidden = hidden, room = room, more_w = more_w)
/// ```
#[component]
fn TabsFit(props: &TabsFitProps) -> Element {
    let active = props.active.clone();
    let on_change = props.on_change.clone();
    let hidden = props.hidden;
    let room = props.room;
    let more_w = props.more_w;
    let items: Vec<(Tab, Ref<ViewHandle>)> = props.tabs.iter().map(|t| (t.clone(), Ref::new())).collect();
    // The row's width: the pills that fit, or no limit while all do.
    let held_to: Signal<Option<f32>> = signal(None);
    let fit: Rc<dyn Fn()> = {
        let seats: Vec<(String, Ref<ViewHandle>)> = items.iter().map(|(t, r)| (t.id.clone(), *r)).collect();
        Rc::new(move || {
            let room = room.peek();
            let widths: Vec<f32> =
                seats.iter().map(|(_, seat)| seat.with(|h| h.frame()).flatten().map_or(0.0, |f| f.width)).collect();
            // A seat not laid out yet has no width; a zero would count
            // it as fitting.
            if room <= 0.0 || widths.is_empty() || widths.iter().any(|w| *w <= 0.0) {
                return;
            }
            let (shown, limit) = fit_count(&widths, room, more_w.peek().max(MORE_WIDTH));
            let next: Vec<String> = seats[shown..].iter().map(|(id, _)| id.clone()).collect();
            if hidden.peek() != next {
                hidden.set(next);
            }
            if held_to.peek() != limit {
                held_to.set(limit);
            }
        })
    };
    // Whenever a pill's width lands or changes, and whenever the room
    // does. Not once on the first frame: on iOS the pills may not be
    // laid out by then.
    let alive = ScopeAlive::current();
    let subs: Rc<RefCell<Vec<LayoutSubscription>>> = Rc::new(RefCell::new(Vec::new()));
    let task = {
        let (fit, subs, alive) = (fit.clone(), subs.clone(), alive.clone());
        let seats: Vec<Ref<ViewHandle>> = items.iter().map(|(_, r)| *r).collect();
        after_animation_frame(move || {
            fit();
            for seat in &seats {
                let (fit, alive) = (fit.clone(), alive.clone());
                if let Some(sub) = seat.with(|h| {
                    h.on_layout(move |_, _| {
                        let (fit, alive) = (fit.clone(), alive.clone());
                        after_ms_detached(0, move || {
                            if alive.get() {
                                fit();
                            }
                        })
                    })
                }) {
                    subs.borrow_mut().push(sub);
                }
            }
        })
    };
    on_scope_drop(move || {
        drop(task);
        drop(subs);
    });
    let on_room = fit.clone();
    effect!({
        let _ = (room.get(), more_w.get());
        let (fit, alive) = (on_room.clone(), alive.clone());
        let _ = after_ms_detached(0, move || {
            if alive.get() {
                fit();
            }
        });
    });
    let row_style = move || {
        let limit = held_to.get();
        TabsFitRow().into_style_application().with_inline_merged(StyleRules {
            max_width: limit.map(|w| Tokenized::Literal(Length::Px(w))),
            ..Default::default()
        })
    };
    ui! {
        view(style = row_style) {
            for item in items.iter(), key = item.0.id.clone() {
                view(style = TabSeat(), bind = item.1) {
                    Tabs(
                        tabs = signal(vec![item.0.clone()]),
                        active = active.clone(),
                        on_change = on_change.clone(),
                        // A pill is its own height: no bar padding, and
                        // not the strip host's floor, which would seat
                        // the pill below the row's top.
                        styles = TabsSlotStyles::default()
                            .host(StyleRules {
                                min_height: Some(Tokenized::Literal(Length::Px(0.0))),
                                ..Default::default()
                            })
                            .bar(StyleRules {
                                padding_top: Some(Tokenized::Literal(Length::Px(0.0))),
                                padding_bottom: Some(Tokenized::Literal(Length::Px(0.0))),
                                ..Default::default()
                            }),
                    )
                }
            }
        }
    }
}

stylesheet! {
    // From the top, as the pills' own line is: centring here would set
    // "N more" against the row's full height while the pills centre in
    // the first of two wrapped lines.
    TabsRow<idea_ui::IdeaThemeRef> {
        base(_t) {
            flex_direction: FlexDirection::Row,
            align_items: AlignItems::FlexStart,
            gap: Length::Px(TAB_GAP),
            flex_grow: 1.0,
            min_width: Length::Px(0.0),
        }
    }
}

stylesheet! {
    // One line of pills; what wraps past it is clipped. As wide as its
    // pills and no wider, so "N more" follows the last one.
    //
    // The clip is CLIP_SLACK taller than the pills, handed back with a
    // negative margin so the row still lays out at ROW_HEIGHT: a clip
    // that fits exactly lands on a fractional y and shaves the pills'
    // bottom edge. The row gap keeps a wrapped line below the slack.
    TabsFitRow<idea_ui::IdeaThemeRef> {
        base(_t) {
            flex_direction: FlexDirection::Row,
            flex_wrap: FlexWrap::Wrap,
            align_items: AlignItems::FlexStart,
            align_content: AlignContent::FlexStart,
            column_gap: Length::Px(TAB_GAP),
            row_gap: Length::Px(TAB_GAP),
            height: Length::Px(ROW_HEIGHT + CLIP_SLACK),
            margin_bottom: Length::Px(-CLIP_SLACK),
            overflow: Overflow::Hidden,
            flex_grow: 0.0,
            flex_shrink: 1.0,
            min_width: Length::Px(0.0),
        }
    }
}

stylesheet! {
    // A pill at its own width: it wraps before it squeezes.
    TabSeat<idea_ui::IdeaThemeRef> {
        base(_t) {
            flex_shrink: 0.0,
        }
    }
}

stylesheet! {
    // "N more", on the pills' line.
    TabsMoreSeat<idea_ui::IdeaThemeRef> {
        base(_t) {
            flex_shrink: 0.0,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const PILL: f32 = 80.0;

    #[test]
    fn every_pill_shows_while_they_all_fit() {
        assert_eq!(fit_count(&[PILL; 3], 400.0, MORE_WIDTH), (3, None));
    }

    /// The pills that do not fit leave room for "N more" beside the
    /// ones that do: fitting against the bare row would cut the last
    /// pill under the button.
    #[test]
    fn the_overflow_button_is_counted_in_the_room() {
        // Three pills and their gaps are 252; the room is 250. Two pills
        // are 166, and with the gap and the button 264: over. So one.
        let (shown, limit) = fit_count(&[PILL; 3], 250.0, MORE_WIDTH);
        assert_eq!(shown, 1);
        assert_eq!(limit, Some(PILL + 1.0));
    }

    #[test]
    fn at_least_one_pill_always_shows() {
        assert_eq!(fit_count(&[PILL; 4], 10.0, MORE_WIDTH).0, 1);
    }

    #[test]
    fn a_wider_overflow_button_costs_a_pill() {
        let widths = [PILL; 5];
        let narrow = fit_count(&widths, 360.0, MORE_WIDTH).0;
        let wide = fit_count(&widths, 360.0, 180.0).0;
        assert!(wide < narrow, "{wide} < {narrow}");
    }
}
