//! Course chrome components — `CodePanel`, `Callout`, `ReactNote`,
//! `DocsLink`, `StepNav`, `LessonPage`. Each is a `#[component]` so its tag and props
//! struct are wired into `ui!` dispatch automatically (the macro emits
//! `pub type Foo = FooProps` + the matching `BuildElement` impl).
//!
//! Author paragraphs and sub-headings as `Typography(content = …, kind
//! = …)` directly — there's no point hiding it behind a wrapper.

use codeblock::rust::Palette;
use std::rc::Rc;

use runtime_core::primitives::overlay::BackdropMode;
use runtime_core::{
    component, memo, signal, switch, ui, Breakpoint, Color, Element, IntoElement, Route, Signal,
    StyleApplication, Tokenized, ViewportPlacement,
};

/// From this breakpoint up the lesson workspace docks beside the prose at
/// a fixed width; below it, it is a drawer. One step above the sidebar's
/// pin (`Lg`), so a pinned sidebar, the prose and the workspace never
/// share a width too narrow for all three.
const WORKSPACE_DOCK_AT: Breakpoint = Breakpoint::Xl;
// `IdeaTheme as _` brings the theme trait into scope so `theme_is_dark`
// below can call `colors()` on the downcast `IdeaThemeRef` carrier.
use idea_ui::{Button, ButtonSize, ButtonVariant, IdeaTheme as _, Stack, StackGap, Typography};

use crate::course;
use crate::styles::{
    Callout as CalloutBox,
    CodePanel as CodePanelBox,
    CodeText,
    CopyCorner,
    DocsLink as DocsLinkBox,
    Eyebrow,
    EyebrowRow,
    EyebrowSpacer,
    HeaderNext,
    HeaderNextText,
    LearnArrow,
    LearnCard,
    LearnRow,
    LearnText,
    LessonMeta,
    LessonMetaRow,
    LessonProse,
    LessonSplit,
    LessonWorkspace,
    DrawerBar,
    DrawerOpener,
    DrawerPanel,
    DrawerScrim,
    ReactNote as ReactNoteBox,
    PagePad,
    StepNavLink,
    StepNavRow,
};

/// GitHub blob bases for the deep-dive reference docs. The tutorial
/// teaches the concept; these links point at the verbose reference.
const REPO_BASE: &str = "https://github.com/IdealystIO/idealyst-native/blob/master";
const DOCS_BASE: &str = "https://github.com/IdealystIO/idealyst-native/blob/master/docs/";

// =============================================================================
// CodePanel — theme-aware, syntax-tinted code block.
// =============================================================================

#[derive(Default)]
pub struct CodePanelProps {
    pub src: String,
    /// A terminal transcript or plain listing: one ink, no Rust tints
    /// (a `for` in a comment column is not a keyword).
    pub shell: bool,
}

/// Read the active theme's background and decide whether we're in a dark
/// theme. idea-ui's light themes start with near-white backgrounds; dark
/// themes with near-black, so a luminance check survives palette tweaks on
/// either side.
///
/// Reads via [`idea_ui::active_theme`] (a tracked signal read) rather than
/// `Tokenized::resolve()`: the Tokenized registry read neither updates on
/// `set_idea_theme` nor subscribes the `switch` scrutinee below (Tokenized
/// registry reads are not world-tracked), which would leave dark
/// code panels wearing the light palette's ink.
pub(crate) fn theme_is_dark() -> bool {
    if idea_ui::theme_installed() {
        let theme = idea_ui::active_theme();
        if let Some(t) = theme.downcast_ref::<idea_ui::IdeaThemeRef>() {
            return is_dark_color(&t.colors().background.value().0);
        }
    }
    let bg: Color =
        Tokenized::<Color>::token("color-background", Color("#ffffff".into())).resolve();
    is_dark_color(&bg.0)
}

fn is_dark_color(s: &str) -> bool {
    let hex = s.trim_start_matches('#');
    if hex.len() < 6 {
        return false;
    }
    let r = u8::from_str_radix(&hex[0..2], 16).unwrap_or(255) as f32;
    let g = u8::from_str_radix(&hex[2..4], 16).unwrap_or(255) as f32;
    let b = u8::from_str_radix(&hex[4..6], 16).unwrap_or(255) as f32;
    let luma = 0.2126 * r + 0.7152 * g + 0.0722 * b;
    luma < 128.0
}

/// Copy `text` to the system clipboard. Fire-and-forget: a failure (an
/// insecure origin on the web) leaves the clipboard as it was, and the
/// code is still on screen to select by hand.
pub(crate) fn copy_to_clipboard(text: String) {
    runtime_core::spawn_then(async move { clipboard::set_text(&text).await }, |_| {});
}

#[component]
pub fn CodePanel(props: &CodePanelProps) -> Element {
    let src = props.src.clone();
    let shell = props.shell;
    let for_copy = src.clone();
    let copy: std::rc::Rc<dyn Fn()> = std::rc::Rc::new(move || copy_to_clipboard(for_copy.clone()));
    // idealyst-lint-disable-next-line prefer-ui-control-flow -- keyed rebuild of one shape: the code block per palette
    let dynamic = switch(theme_is_dark, move |&is_dark| {
        let palette = if is_dark { Palette::DARK } else { Palette::LIGHT };
        let spans = if shell {
            vec![(src.clone(), Color(palette.ink.into()))]
        } else {
            codeblock::rust::spans(&src, &palette)
        };
        codeblock::code_block(spans)
            .with_style(move || StyleApplication::new(CodeText::sheet()))
            .into_element()
    });
    ui! {
        view(style = CodePanelBox()) {
            dynamic
            view(style = CopyCorner()) {
                Button(
                    label = "Copy",
                    on_click = copy,
                    variant = ButtonVariant::Outlined,
                    size = ButtonSize::Sm,
                    tone = idea_ui::tone::Neutral,
                )
            }
        }
    }
}

// =============================================================================
// Callout — tinted block: short label + body children.
// =============================================================================

#[derive(Default)]
pub struct CalloutProps {
    pub label: String,
    pub children: Vec<Element>,
}

#[component]
pub fn Callout(props: CalloutProps) -> Element {
    let style = CalloutBox();
    let label = props.label;
    let children = props.children;
    // `children` is a bare identifier (no braces). The `ui!` parser
    // treats it as a Rust expression in child position and feeds it
    // through `ChildList::append_to`, so the caller's body splats in
    // beside the label. Wrapping it as `{ children }` would instead be
    // consumed as the preceding component's child block.
    ui! {
        view(style = style) {
            Typography(content = label, kind = idea_ui::typography_kind::Overline)
            children
        }
    }
}

/// Where a lesson's workspace is.
#[derive(Clone, Copy, PartialEq)]
enum WorkspacePlace {
    /// Beside the prose, from `WORKSPACE_DOCK_AT` up.
    Docked,
    /// Narrower: in the open drawer.
    Drawer,
    /// Narrower, drawer closed: only the button that opens it shows.
    Closed,
}

/// The workspace in its place. A `match` rather than an `if` chain: each
/// arm is its own closure, so each takes its own handle on the builder.
fn workspace_region(build: Workspace, drawer_open: Signal<bool>) -> Element {
    let docked = build.clone();
    let drawer = build;
    let place = memo(move || {
        if runtime_core::current_breakpoint().get().is_at_least(WORKSPACE_DOCK_AT) {
            WorkspacePlace::Docked
        } else if drawer_open.get() {
            WorkspacePlace::Drawer
        } else {
            WorkspacePlace::Closed
        }
    });
    ui! {
        match place.get() {
            WorkspacePlace::Docked => {
                view(style = LessonWorkspace()) {
                    { docked() }
                }
            }
            WorkspacePlace::Drawer => { workspace_drawer(drawer.clone(), drawer_open) }
            WorkspacePlace::Closed => { drawer_opener(drawer_open) }
        }
    }
}

/// The workspace in a drawer over the whole window, from its right edge:
/// the scrim dims everything (sidebar included), and a tap on it, Escape,
/// back or the Close button closes it.
fn workspace_drawer(build: Workspace, open: Signal<bool>) -> Element {
    let close: Rc<dyn Fn()> = Rc::new(move || open.set(false));
    ui! {
        overlay(
            placement = ViewportPlacement::Right,
            backdrop = BackdropMode::Dismiss,
            on_dismiss = move || open.set(false),
            backdrop_style = DrawerScrim(),
            style = DrawerPanel(),
        ) {
            view(style = DrawerBar()) {
                Button(
                    label = "Close",
                    on_click = close,
                    variant = ButtonVariant::Ghost,
                    size = ButtonSize::Sm,
                    tone = idea_ui::tone::Neutral,
                )
            }
            { build() }
        }
    }
}

/// The button, floating at the page's bottom-right, that opens the drawer.
fn drawer_opener(open: Signal<bool>) -> Element {
    let open_it: Rc<dyn Fn()> = Rc::new(move || open.set(true));
    ui! {
        view(style = DrawerOpener()) {
            Button(label = "Open the editor \u{2192}", on_click = open_it, size = ButtonSize::Sm)
        }
    }
}

// =============================================================================
// ReactNote — a "Coming from React" aside. The course landing page points
// React developers at these; they map a React habit onto the framework.
// =============================================================================

#[derive(Default)]
pub struct ReactNoteProps {
    pub children: Vec<Element>,
}

#[component]
pub fn ReactNote(props: ReactNoteProps) -> Element {
    let children = props.children;
    ui! {
        view(style = ReactNoteBox()) {
            Typography(content = "Coming from React".to_string(), kind = idea_ui::typography_kind::Overline)
            children
        }
    }
}

// =============================================================================
// DocsLink — "read more in the docs" cross-link card.
// =============================================================================

#[derive(Default)]
pub struct DocsLinkProps {
    pub summary: String,
    pub link_label: String,
    pub doc_file: String,
}

#[component]
pub fn DocsLink(props: &DocsLinkProps) -> Element {
    let style = CalloutBox();
    // `docs/`-relative by default; a leading `/` names a path from the
    // repository root (`/README.md`, `/crates/…`).
    let url = match props.doc_file.strip_prefix('/') {
        Some(from_root) => format!("{REPO_BASE}/{from_root}"),
        None => format!("{DOCS_BASE}{}", props.doc_file),
    };
    let link_style = move || StyleApplication::new(DocsLinkBox::sheet());
    let link_text = format!("{} \u{2197}", props.link_label);
    let summary = props.summary.clone();
    ui! {
        view(style = style) {
            Typography(content = summary, muted = true)
            link(external = url) { text(style = link_style) { link_text } }
        }
    }
}

// =============================================================================
// StepNav — prev / next bar derived from the linear step order in `routes`.
// =============================================================================

#[derive(Default)]
pub struct StepNavProps {
    pub current: &'static str,
}

fn step_link(route: &'static Route<()>, label: String) -> Element {
    let style = move || StyleApplication::new(StepNavLink::sheet());
    ui! {
        link(route = route, params = ()) {
            text(style = style) { label }
        }
    }
}

#[component]
pub fn StepNav(props: &StepNavProps) -> Element {
    let (prev, next) = course::neighbors(props.current);
    let row = StepNavRow();
    // Each side resolves to a link OR an empty view — deliberately not an
    // in-macro `if`, whose empty branch is laid out `position: absolute`.
    // The row is `justify-content: space-between`, so an absolute (i.e.
    // out-of-flow) placeholder would let a lone "next →" drift to the left
    // edge on the first and last steps. The empty view keeps the slot.
    let prev_el = match prev {
        Some((n, l)) => step_link(l.route.expect("written"), format!("\u{2190} {n} {}", l.title)),
        None => runtime_core::view(Vec::<Element>::new()).into_element(),
    };
    let next_el = match next {
        Some((n, l)) => step_link(l.route.expect("written"), format!("{n} {} \u{2192}", l.title)),
        None => runtime_core::view(Vec::<Element>::new()).into_element(),
    };
    ui! {
        view(style = row) {
            prev_el
            next_el
        }
    }
}

// =============================================================================
// LessonPage — top-level frame for a step. Renders the title block, the
// supplied body as children, and the prev/next bar, all inside the padded
// reading column. By-value so it can move children out.
// =============================================================================

/// Builds a lesson's workspace; see [`LessonPageProps::workspace`].
pub type Workspace = Rc<dyn Fn() -> Element>;

/// A lesson's workspace from its builder:
/// `workspace = workspace(|| ui! { Fiddle(source = …) })`.
pub fn workspace(build: impl Fn() -> Element + 'static) -> Option<Workspace> {
    Some(Rc::new(build))
}

#[derive(Default)]
pub struct LessonPageProps {
    pub current: &'static str,
    pub title: String,
    pub lead: String,
    /// "You'll learn" bullets, shown in a card under the lead.
    pub learn: Vec<&'static str>,
    /// The lesson's workspace — a `Fiddle` or a `Terminal` — as a builder
    /// (make one with [`workspace`]). Wide, it is a fixed-width column
    /// beside the prose that stays put while the prose scrolls; narrow, it
    /// opens in a drawer over the whole window. A builder because the two
    /// places are different parents: the drawer is a viewport `overlay`,
    /// which a docked element cannot move into.
    pub workspace: Option<Workspace>,
    pub children: Vec<Element>,
}

#[component]
pub fn LessonPage(props: LessonPageProps) -> Element {
    let current = props.current;
    let children = props.children;
    let learn = props.learn;
    let workspace = props.workspace;
    let listed = course::find(current);
    // A lesson listed in the course takes its title from the outline, so
    // the page, the sidebar and the landing page cannot name it differently.
    let title = listed.map_or(props.title, |(_, _, lesson)| lesson.title.to_string());
    let lead = props.lead;
    let eyebrow = listed
        .map(|(c, i, _)| format!("Lesson {}", course::lesson_number(c, i)))
        .unwrap_or_default();
    // "12 min   Builds on 0.6 Anatomy of a project" — the lesson before
    // this one in the course.
    let minutes = listed.map(|(_, _, lesson)| format!("{} min", lesson.minutes)).unwrap_or_default();
    let builds_on = listed
        .and_then(|_| course::neighbors(current).0)
        .map(|(n, prev)| format!("Builds on {n} {}", prev.title))
        .unwrap_or_default();
    let has_learn = !learn.is_empty();
    // The workspace docks beside the prose from `WORKSPACE_DOCK_AT` up;
    // narrower, it opens in a drawer from the window's right edge. A
    // Fiddle keeps its draft across the rebuild either move causes (see
    // `fiddle::Fiddle`).
    let drawer_open: Signal<bool> = signal(false);
    // The header's Next button: the next written lesson, if there is one.
    let next = course::neighbors(current).1.and_then(|(n, lesson)| {
        lesson.route.map(|route| (route, format!("Next: {n} {} \u{2192}", lesson.title)))
    });

    ui! {
        view(style = LessonSplit()) {
            scroll_view(style = LessonProse()) {
                view(style = PagePad()) {
                    Stack(gap = StackGap::Sm) {
                        view(style = EyebrowRow()) {
                            { crate::shell::sidebar_toggle() }
                            text(style = Eyebrow()) { eyebrow }
                            view(style = EyebrowSpacer()) {}
                            if let Some((route, label)) = next {
                                link(route = route, params = ()) {
                                    view(style = HeaderNext()) {
                                        text(style = HeaderNextText()) { label }
                                    }
                                }
                            }
                        }
                        Typography(content = title, kind = idea_ui::typography_kind::H1)
                        view(style = LessonMetaRow()) {
                            text(style = LessonMeta()) { minutes }
                            text(style = LessonMeta()) { builds_on }
                        }
                    }
                    Typography(content = lead, kind = idea_ui::typography_kind::BodyLg, muted = true)
                    if has_learn {
                        view(style = LearnCard()) {
                            text(style = Eyebrow()) { "You'll learn" }
                            for item in learn {
                                view(style = LearnRow()) {
                                    text(style = LearnArrow()) { "\u{2192}" }
                                    text(style = LearnText()) { item }
                                }
                            }
                        }
                    }
                    children
                    StepNav(current = current)
                }
            }
            if let Some(build) = workspace {
                workspace_region(build, drawer_open)
            }
        }
    }
}
