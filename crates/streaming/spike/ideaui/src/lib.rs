//! idea-ui as a remote component library: remote components that use idea-ui
//! the way an app does, area by area, for `tests/idea_ui.rs`.
//!
//! Each area's tree is a plain fn (`*_demo`). In the bundle build, the remote
//! component renders it and every idea-ui component in it is an import of
//! the app's copy. In the app build the same fn compiles too, so a test can
//! render it in-process and compare the two trees.

// In the app build the remote bodies are compiled out (the bundle has them).
#![cfg_attr(not(idealyst_stream_guest), allow(unused_imports))]

use std::rc::Rc;

use idea_ui::components::card;
use idea_ui::{
    shape, size, tone, typography_kind, variant, Accordion, Autocomplete, AccordionItem, Adornment, Alert, AlertClose, Avatar,
    AvatarColor, AvatarSize, Badge, Breadcrumbs, Button, Calendar, Card, CardPadding, Center, Checkbox, Chip, CivilDate,
    CivilDateTime, CivilTime, Collapsible, Crumb, DateInput, DatePicker, DateRangePicker, DateTimeInput, DateTimePicker, Divider, DividerAxis, Field, FieldAppearance, FieldSize, Grid,
    Icon, IconButton, IconButtonSize, Image, Link, List, ListItem, Menu, MenuEntry, MenuItem, MenuLabel, MenuSeparator,
    Modal, ModalContent, ModalPresentation, Pagination, Popover, Progress, ProgressMode, Radio, RadioAxis, RadioGroup,
    RadioOption, RangeCalendar, SegmentOption, SegmentedControl, Select, SelectOption, SelectSize, Skeleton,
    SkeletonWidth, Slider, Spacer, Spinner, SpinnerSize, Stack, StackAlign, StackAxis, StackGap, StackJustify,
    StackPadding, SubMenu, Surface, SurfaceColor, Switch, Tab, TabIndicator, Table, TableCell, TableRow, Tabs, Tag,
    Textarea, TimeInput, ToastHost, ToastPlacement, Tooltip, Typography, Weekday,
};
use runtime_core::primitives::portal::{AnchorTarget, ElementAlign, ElementSide};
use runtime_core::{component, rx, signal, ui, Element, PressableHandle, Ref, TextAlign};

fn act(f: impl Fn() + 'static) -> Rc<dyn Fn()> {
    Rc::new(f)
}

fn on<T: 'static>(f: impl Fn(T) + 'static) -> Rc<dyn Fn(T)> {
    Rc::new(f)
}

fn s(text: &str) -> String {
    text.to_string()
}

fn date(y: i32, m: u8, d: u8) -> CivilDate {
    CivilDate::new(y, m, d).expect("a real date")
}

// ---------------------------------------------------------------------------
// Layout
// ---------------------------------------------------------------------------

pub fn layout_demo() -> Element {
    ui! {
        Stack(gap = StackGap::Lg, padding = StackPadding::Md) {
            Stack(axis = StackAxis::Row, gap = StackGap::Sm, align = StackAlign::Center, justify = StackJustify::Between, wrap = true) {
                Typography(content = s("left"))
                Spacer()
                Typography(content = s("right"))
            }
            Divider(axis = DividerAxis::Horizontal)
            Center() {
                Typography(content = s("centered"))
            }
            Surface(background = SurfaceColor::SurfaceAlt, padding = StackPadding::Sm) {
                Typography(content = s("on a surface"))
            }
            Grid(columns = 2u32, gap = StackGap::Sm) {
                Typography(content = s("a"))
                Typography(content = s("b"))
                Typography(content = s("c"))
            }
            Card(variant = card::variant::Elevated, padding = CardPadding::Lg, tone = Some(tone::Info.into())) {
                Typography(content = s("in a card"))
            }
        }
    }
}

#[component(remote)]
pub fn LayoutArea() -> Element {
    layout_demo()
}

// ---------------------------------------------------------------------------
// Text and status
// ---------------------------------------------------------------------------

pub fn status_demo() -> Element {
    let closed = signal(0u32);
    ui! {
        Stack(gap = StackGap::Sm) {
            Typography(content = s("Heading"), kind = typography_kind::H1)
            Typography(content = s("muted, centred"), muted = true, align = TextAlign::Center)
            Typography(content = s("danger"), tone = Some(tone::Danger.into()))
            Badge(label = s("Active"), tone = tone::Success, variant = variant::Soft)
            Tag(label = s("rust"), tone = tone::Info, on_remove = Some(act(move || closed.update(|n| n + 1))))
            Chip(label = s("filter"), selected = true, on_select = Some(act(|| {})))
            Avatar(initials = s("AB"), color = AvatarColor::Primary, size = AvatarSize::Lg)
            Spinner(size = SpinnerSize::Large)
            Skeleton(width = SkeletonWidth::Half, height = 12.0f32)
            Progress(value = 0.4f32, tone = tone::Success)
            Progress(mode = ProgressMode::Indeterminate)
            Alert(
                title = s("Unsaved changes"),
                body = Some(s("Save before leaving.")),
                tone = tone::Warning,
                close = AlertClose::Button(act(|| {})),
            )
            Icon(data = icons_lucide::HEART, size = 24.0f32, tone = Some(tone::Danger.into()))
            Link(label = s("docs"), url = s("https://example.com"))
            Image(src = s("https://example.com/a.png"), width = Some(40.0f32), height = Some(40.0f32), rounded = true)
        }
    }
}

#[component(remote)]
pub fn StatusArea() -> Element {
    status_demo()
}

// ---------------------------------------------------------------------------
// Actions
// ---------------------------------------------------------------------------

pub fn actions_demo() -> Element {
    let presses = signal(0u32);
    ui! {
        Stack(axis = StackAxis::Row, gap = StackGap::Sm) {
            Button(label = s("Primary"), on_click = act(move || presses.update(|n| n + 1)), tone = tone::Primary)
            Button(label = s("Outlined"), on_click = act(|| {}), variant = variant::Outlined, size = size::Lg, shape = shape::Pill)
            Button(label = s("Icon"), on_click = act(|| {}), leading_icon = Some(icons_lucide::HEART), block = true)
            Button(label = s("Busy"), on_click = act(|| {}), disabled = true, loading = true)
            IconButton(icon = Some(icons_lucide::SEARCH), on_click = act(|| {}), size = IconButtonSize::Sm, selected = true)
            Typography(content = rx!(format!("pressed {}", presses.get())))
        }
    }
}

#[component(remote)]
pub fn ActionsArea() -> Element {
    actions_demo()
}

// ---------------------------------------------------------------------------
// Forms
// ---------------------------------------------------------------------------

pub fn forms_demo() -> Element {
    let checked = signal(false);
    let picked = signal(true);
    let fruit = signal(s("apple"));
    let on_off = signal(true);
    let level = signal(30.0f32);
    let name = signal(s("Ada"));
    let notes = signal(s(""));
    let size_pick = signal(s("m"));
    let view_pick = signal(s("list"));
    let city = signal(s(""));
    ui! {
        Stack(gap = StackGap::Sm) {
            Checkbox(value = checked, on_change = on(move |v| checked.set(v)), label = Some(s("Accept")))
            Radio(selected = picked, on_select = act(move || picked.set(true)), label = Some(s("Pick me")))
            RadioGroup(
                value = fruit,
                on_change = on(move |v| fruit.set(v)),
                options = vec![
                    RadioOption { id: s("apple"), label: s("Apple").into() },
                    RadioOption { id: s("pear"), label: s("Pear").into() },
                ],
                axis = RadioAxis::Row,
            )
            Switch(value = on_off, on_change = on(move |v| on_off.set(v)), label = Some(s("Wi-Fi")))
            Slider(value = rx!(level.get()), on_change = on(move |v| level.set(v)), min = 0.0f32, max = 100.0f32, step = 10.0f32)
            Field(
                label = Some(s("Name")),
                value = name,
                on_change = on(move |v| name.set(v)),
                placeholder = Some(s("Your name")),
                help = Some(s("As on your ID")),
                size = FieldSize::Lg,
                variant = FieldAppearance::Contained,
                leading = Adornment::Icon(icons_lucide::SEARCH),
            )
            Textarea(label = Some(s("Notes")), value = notes, on_change = on(move |v| notes.set(v)), rows = 3u32)
            Select(
                value = size_pick,
                on_change = on(move |v| size_pick.set(v)),
                options = vec![
                    SelectOption { id: s("s"), label: s("Small").into() },
                    SelectOption { id: s("m"), label: s("Medium").into() },
                ],
                size = SelectSize::Sm,
                placeholder = Some(s("Size")),
            )
            SegmentedControl(
                value = rx!(view_pick.get()),
                on_change = on(move |v| view_pick.set(v)),
                options = vec![
                    SegmentOption { id: s("list"), label: s("List").into() },
                    SegmentOption { id: s("grid"), label: s("Grid").into() },
                ],
            )
            Autocomplete(
                value = city,
                on_change = on(move |v| city.set(v)),
                options = vec![SelectOption { id: s("ldn"), label: s("London").into() }],
                placeholder = Some(s("City")),
            )
        }
    }
}

#[component(remote)]
pub fn FormsArea() -> Element {
    forms_demo()
}

// ---------------------------------------------------------------------------
// Dates
// ---------------------------------------------------------------------------

pub fn dates_demo() -> Element {
    let day = signal(Some(date(2026, 10, 2)));
    let range = signal(Some((date(2026, 10, 2), date(2026, 10, 9))));
    let due = signal(None::<CivilDate>);
    let typed = signal(Some(date(2026, 1, 15)));
    let at = signal(CivilTime::new(9, 30, 0));
    let when = signal(Some(CivilDateTime::new(date(2026, 10, 2), CivilTime::new(14, 0, 0).expect("a real time"))));
    let meeting = signal(None::<CivilDateTime>);
    let trip = signal(None::<(CivilDate, CivilDate)>);
    ui! {
        Stack(gap = StackGap::Sm) {
            Calendar(value = day, on_change = on(move |d| day.set(Some(d))), first_weekday = Weekday::Sunday, min = Some(date(2026, 1, 1)))
            RangeCalendar(value = range, on_change = Rc::new(move |a, b| range.set(Some((a, b)))) as Rc<dyn Fn(CivilDate, CivilDate)>)
            DatePicker(value = due, on_change = on(move |d| due.set(d)), placeholder = Some(s("Due")), clearable = true)
            DateInput(value = typed, on_change = on(move |d| typed.set(d)), label = Some(s("Start")))
            TimeInput(value = at, on_change = on(move |t| at.set(t)), label = Some(s("At")))
            DateTimeInput(value = when, on_change = on(move |v| when.set(v)), label = Some(s("When")))
            DateTimePicker(value = meeting, on_change = on(move |v| meeting.set(v)), placeholder = Some(s("Meeting")))
            DateRangePicker(value = trip, on_change = on(move |v| trip.set(v)), placeholder = Some(s("Trip")))
        }
    }
}

#[component(remote)]
pub fn DatesArea() -> Element {
    dates_demo()
}

// ---------------------------------------------------------------------------
// Navigation
// ---------------------------------------------------------------------------

pub fn navigation_demo() -> Element {
    let tabs = signal(vec![Tab { id: s("one"), label: s("One").into() }, Tab { id: s("two"), label: s("Two").into() }]);
    let active = signal(s("one"));
    let page = signal(1usize);
    ui! {
        Stack(gap = StackGap::Sm) {
            Tabs(tabs = tabs, active = rx!(active.get()), on_change = on(move |t| active.set(t)), indicator = TabIndicator::Dot)
            Breadcrumbs(
                items = vec![
                    Crumb { label: s("Home").into(), on_press: Some(act(|| {})) },
                    Crumb { label: s("Here").into(), on_press: None },
                ],
                separator = s("/"),
            )
            Pagination(page = page, total = 5usize, on_change = on(move |p| page.set(p)))
            List() {
                ListItem(label = s("First"), on_press = Some(act(|| {})), active = true)
                ListItem(label = s("Second"), trailing = Some(ui! { Badge(label = s("2")) }))
            }
        }
    }
}

#[component(remote)]
pub fn NavigationArea() -> Element {
    navigation_demo()
}

// ---------------------------------------------------------------------------
// Overlays
// ---------------------------------------------------------------------------

pub fn overlays_demo() -> Element {
    let trigger: Ref<PressableHandle> = Ref::new();
    let open = signal(true);
    ui! {
        Stack(gap = StackGap::Sm) {
            Tooltip(text = s("A hint")) {
                Button(label = s("Hover me"), on_click = act(|| {}))
            }
            Button(label = s("Menu"), on_click = act(move || open.update(|o| !o)), bind_to = Some(trigger))
            if open.get() {
                Popover(target = Some(AnchorTarget::from(trigger)), side = ElementSide::Below, align = ElementAlign::Start, offset = 6.0f32, on_dismiss = Some(act(move || open.set(false)))) {
                    Typography(content = s("in a popover"))
                }
            }
            Menu(target = Some(AnchorTarget::from(trigger)), on_dismiss = Some(act(|| {}))) {
                MenuLabel(text = s("Actions"))
                MenuItem(label = s("Edit"), on_select = act(|| {}))
                MenuSeparator()
                SubMenu(label = s("More"), items = vec![MenuEntry { label: s("Archive").into(), on_select: act(|| {}), checked: None }])
            }
            ToastHost(placement = ToastPlacement::BottomCenter)
            Modal(
                open = true,
                content = ModalContent(Rc::new(|| ui! { Typography(content = s("in a modal")) })),
                presentation = ModalPresentation::Sheet,
                on_dismiss = Some(act(|| {})),
            )
        }
    }
}

#[component(remote)]
pub fn OverlaysArea() -> Element {
    overlays_demo()
}

// ---------------------------------------------------------------------------
// Data
// ---------------------------------------------------------------------------

pub fn data_demo() -> Element {
    let shown = signal(true);
    ui! {
        Stack(gap = StackGap::Sm) {
            Table() {
                TableRow() {
                    TableCell(header = true, text = Some(s("Name")))
                    TableCell(header = true, text = Some(s("Role")))
                }
                TableRow(on_row_click = Some(act(|| {}))) {
                    TableCell(text = Some(s("Ada")))
                    TableCell() {
                        Badge(label = s("Admin"))
                    }
                }
            }
            Accordion(
                items = vec![
                    AccordionItem { title: s("First").into(), body: ui! { Typography(content = s("first body")) } },
                    AccordionItem { title: s("Second").into(), body: ui! { Typography(content = s("second body")) } },
                ],
                open = signal(vec![true, false]),
            )
            Collapsible(title = s("Details"), value = shown, on_change = on(move |v| shown.set(v))) {
                Typography(content = s("collapsible body"))
            }
        }
    }
}

#[component(remote)]
pub fn DataArea() -> Element {
    data_demo()
}

// ---------------------------------------------------------------------------
// The theme's tokens in remote code's OWN stylesheets (no idea-ui component)
// ---------------------------------------------------------------------------

runtime_core::stylesheet! {
    pub ThemedPage<idea_ui::IdeaThemeRef> {
        base(t) {
            background: t.color.background(),
            color: t.color.text(),
            padding: t.spacing.md(),
        }
    }
}

runtime_core::stylesheet! {
    pub ThemedBanner<idea_ui::IdeaThemeRef> {
        base(t) {
            background: t.intent.primary.solid_bg(),
            color: t.intent.primary.solid_text(),
            padding: t.spacing.sm(),
            border_radius: t.radius.md(),
        }
    }
}

/// Plain primitives styled by sheets that name idea's tokens: the sheets
/// run in the bundle, their token names cross, and the app resolves them
/// against ITS installed theme.
pub fn themed_demo() -> Element {
    ui! {
        view(style = ThemedPage()) {
            view(style = ThemedBanner()) {
                text { "a banner the bundle styled" }
            }
            text { "page text" }
        }
    }
}

#[component(remote)]
pub fn ThemedArea() -> Element {
    themed_demo()
}
