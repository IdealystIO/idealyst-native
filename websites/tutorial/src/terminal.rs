//! `Terminal` — the workspace for a command-line lesson.
//!
//! The lesson's commands are a numbered list of steps under a terminal
//! screen. **Run next** plays the next step into the screen — its command
//! after a `$` prompt, then the output that command prints — and Reset
//! clears it. Each step's command has a Copy button, so the reader can run
//! the real thing in their own terminal alongside.
//!
//! Commands are the real ones, written for the operating system picked in
//! the header where they genuinely differ (installing rustup, chaining two
//! commands). Output is illustrative: it is modeled on what the real tools
//! print, trimmed. A lesson can also show "I want to build for" chips;
//! a step reads the chosen targets (`idealyst doctor` checks follow them).
//!
//! Steps are data — [`TermStep`] with plain `fn` pointers — so a lesson
//! declares them as a `const` slice and the transcript is a pure function
//! of (steps run, OS, targets), tested below without a UI.

use std::cell::RefCell;
use std::rc::Rc;

use codeblock::code_block;
use codeblock::rust::Palette;
use idea_ui::{
    Button, ButtonSize, ButtonVariant, IdeaThemeRef, SegmentOption, SegmentedControl,
    SegmentedControlSize,
};
use runtime_core::{
    component, derived, pressable, signal, stylesheet, switch, text, ui, AlignItems, Color,
    Element, FlexDirection, IntoElement, Length, ScrollAxis, Signal, TextAlign, after_animation_frame, Ref, ScheduledTask, ScrollViewHandle,
};

use crate::common::{copy_to_clipboard, theme_is_dark};

/// The reader's operating system, as the header's tabs pick it.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum Os {
    Macos,
    Linux,
    Windows,
}

impl Os {
    fn id(self) -> &'static str {
        match self {
            Os::Macos => "macos",
            Os::Linux => "linux",
            Os::Windows => "windows",
        }
    }

    fn from_id(id: &str) -> Os {
        match id {
            "linux" => Os::Linux,
            "windows" => Os::Windows,
            _ => Os::Macos,
        }
    }
}

/// The platforms the reader wants to build for ("I want to build for").
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub struct Targets {
    pub web: bool,
    pub ios: bool,
    pub android: bool,
}

impl Default for Targets {
    /// Web alone: it needs no tooling beyond Rust, so it is where the
    /// course starts.
    fn default() -> Self {
        Targets { web: true, ios: false, android: false }
    }
}

/// One step of a command-line lesson.
pub struct TermStep {
    pub title: &'static str,
    /// The command to type, for this OS and these targets.
    pub command: fn(Os, Targets) -> String,
    /// What the command prints — illustrative, modeled on the real tool.
    /// Empty for a command that prints nothing.
    pub output: fn(Os, Targets) -> String,
}

/// What a run of transcript text is, for tinting.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub(crate) enum Run {
    Prompt,
    Command,
    Output,
}

/// The terminal's text after the first `ran` steps: each step's prompt,
/// command and output, then an idle prompt with a block cursor.
pub(crate) fn transcript(steps: &[TermStep], ran: usize, os: Os, targets: Targets) -> Vec<(String, Run)> {
    let mut out = Vec::new();
    for step in steps.iter().take(ran) {
        out.push(("$ ".to_string(), Run::Prompt));
        out.push((format!("{}\n", (step.command)(os, targets)), Run::Command));
        let printed = (step.output)(os, targets);
        if !printed.is_empty() {
            out.push((format!("{}\n", printed.trim_end()), Run::Output));
        }
        out.push(("\n".to_string(), Run::Output));
    }
    out.push(("$ ".to_string(), Run::Prompt));
    out.push(("\u{2588}".to_string(), Run::Command));
    out
}

/// The transcript's row height (`TermText`'s `line_height`).
const TERM_LINE_HEIGHT: f32 = 20.0;

/// The line the next command is printed on, after `ran` steps have run:
/// where the idle prompt sits now.
fn new_command_line(steps: &[TermStep], ran: usize, os: Os, targets: Targets) -> usize {
    transcript(steps, ran, os, targets)
        .iter()
        .map(|(text, _)| text.matches('\n').count())
        .sum()
}

#[derive(Default)]
pub struct TerminalProps {
    pub steps: &'static [TermStep],
    /// Show the "I want to build for" chips. Only a lesson whose steps
    /// read the targets needs them.
    pub show_targets: bool,
}

#[component]
pub fn Terminal(props: &TerminalProps) -> Element {
    let steps = props.steps;
    let show_targets = props.show_targets;
    let os: Signal<String> = signal(String::from(Os::Macos.id()));
    let targets: Signal<Targets> = signal(Targets::default());
    // How many steps have been played into the screen.
    let ran: Signal<usize> = signal(0);
    let finished: Signal<bool> = signal(steps.is_empty());

    // The screen's scroller, and the pending scroll (dropping a scheduled
    // task cancels it, so it is held until it fires or the next run).
    let screen_scroll: Ref<ScrollViewHandle> = Ref::new();
    let pending_scroll: Rc<RefCell<Option<ScheduledTask>>> = Rc::new(RefCell::new(None));
    let run_next: Rc<dyn Fn()> = Rc::new(move || {
        let done = ran.get();
        let next = (done + 1).min(steps.len());
        ran.set(next);
        finished.set(next >= steps.len());
        // Bring the command just run to the top of the screen, its output
        // under it. It starts on the line the idle prompt was on, which is
        // the line count of what was already shown. Deferred a frame: the
        // write commits at the end of this handler and the transcript
        // re-renders after that.
        let line = new_command_line(steps, done, Os::from_id(&os.get()), targets.get());
        let y = line as f32 * TERM_LINE_HEIGHT;
        *pending_scroll.borrow_mut() = Some(after_animation_frame(move || {
            screen_scroll.with(|h| h.scroll_to(0.0, y));
        }));
    });
    let reset: Rc<dyn Fn()> = Rc::new(move || {
        ran.set(0);
        finished.set(steps.is_empty());
    });
    let on_os: Rc<dyn Fn(String)> = Rc::new(move |id| os.set(id));

    // The screen re-renders whenever what it shows changes.
    // idealyst-lint-disable-next-line prefer-ui-control-flow -- keyed rebuild of one shape: the transcript per (steps run, OS, targets, theme)
    let screen = switch(
        move || (ran.get(), os.get(), targets.get(), theme_is_dark()),
        move |(ran, os, targets, dark): &(usize, String, Targets, bool)| {
            let palette = if *dark { Palette::DARK } else { Palette::LIGHT };
            let runs = transcript(steps, *ran, Os::from_id(os), *targets)
                .into_iter()
                .map(|(text, kind)| {
                    let color = match kind {
                        Run::Prompt => palette.keyword,
                        Run::Command => palette.ink,
                        Run::Output => palette.comment,
                    };
                    (text, Color(color.into()))
                })
                .collect();
            code_block(runs).with_style(TermText()).into_element()
        },
    );

    ui! {
        view(style = TermFrame()) {
            view(style = TermHeader()) {
                text(style = TermChip()) { "Terminal" }
                SegmentedControl(
                    value = os,
                    on_change = on_os,
                    size = SegmentedControlSize::Sm,
                    options = vec![
                        SegmentOption::new(Os::Macos.id(), "macOS"),
                        SegmentOption::new(Os::Linux.id(), "Linux"),
                        SegmentOption::new(Os::Windows.id(), "Windows"),
                    ],
                )
                view(style = TermSpacer()) {}
                Button(
                    label = "Reset",
                    on_click = reset,
                    variant = ButtonVariant::Ghost,
                    size = ButtonSize::Sm,
                )
                Button(
                    label = "Run next \u{21b5}",
                    on_click = run_next,
                    size = ButtonSize::Sm,
                    disabled = finished,
                )
            }
            scroll_view(axis = ScrollAxis::Both, style = TermScreen(), bind = screen_scroll) {
                screen
            }
            if show_targets {
                view(style = TermTargets()) {
                    text(style = TermTargetsLabel()) { "I want to build for" }
                    { target_chip("Web", targets, |t| t.web, |t| t.web = !t.web) }
                    { target_chip("iOS", targets, |t| t.ios, |t| t.ios = !t.ios) }
                    { target_chip("Android", targets, |t| t.android, |t| t.android = !t.android) }
                }
            }
            scroll_view(style = TermSteps()) {
                for (i, step) in steps.iter().enumerate() {
                    { step_row(i, step, ran, os, targets) }
                }
            }
            text(style = TermFootnote()) {
                "Commands are real; output is illustrative."
            }
        }
    }
}

/// One "I want to build for" chip: a toggle over one field of `targets`.
fn target_chip(
    label: &'static str,
    targets: Signal<Targets>,
    on: fn(&Targets) -> bool,
    flip: fn(&mut Targets),
) -> Element {
    let style = TermTargetChip().on(derived(move || {
        if on(&targets.get()) { TermTargetChipOn::Yes } else { TermTargetChipOn::No }
    }));
    let face = text(move || {
        if on(&targets.get()) { format!("\u{2713} {label}") } else { label.to_string() }
    })
    .into_element();
    pressable(vec![face], move || {
        targets.update(|t| {
            let mut next = *t;
            flip(&mut next);
            next
        })
    })
    .with_style(style)
    .into_element()
}

fn step_row(
    i: usize,
    step: &'static TermStep,
    ran: Signal<usize>,
    os: Signal<String>,
    targets: Signal<Targets>,
) -> Element {
    let marker = TermStepMarker().state(derived(move || {
        let done = ran.get();
        if i < done {
            TermStepMarkerState::Done
        } else if i == done {
            TermStepMarkerState::Next
        } else {
            TermStepMarkerState::Later
        }
    }));
    let command = move || (step.command)(Os::from_id(&os.get()), targets.get());
    let copy: Rc<dyn Fn()> = Rc::new(move || copy_to_clipboard(command()));
    ui! {
        view(style = TermStepRow()) {
            text(style = marker) {
                move || if i < ran.get() { "\u{2713}".to_string() } else { (i + 1).to_string() }
            }
            view(style = TermStepBody()) {
                text(style = TermStepTitle()) { step.title }
                text(style = TermStepCommand()) { move || command() }
            }
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

// ---- Styles, local to this component ----------------------------------------
//
// Controls (the header, the target chips, the footnote) sit straight on the
// page card's white; the screen and the step list are cards of their own.

stylesheet! {
    pub TermFrame<IdeaThemeRef> {
        base(t) {
            flex_direction: FlexDirection::Column,
            flex_grow: 1.0,
            min_height: 0.0,
            min_width: 0.0,
            gap: t.spacing.md(),
        }
    }
}

stylesheet! {
    pub TermHeader<IdeaThemeRef> {
        base(t) {
            flex_direction: FlexDirection::Row,
            align_items: AlignItems::Center,
            gap: t.spacing.sm(),
            flex_shrink: 0.0,
        }
    }
}

stylesheet! {
    pub TermChip<IdeaThemeRef> {
        base(t) {
            font_family: "ui-monospace, SFMono-Regular, Menlo, monospace",
            font_size: 13.0,
            font_weight: runtime_core::FontWeight::SemiBold,
            padding_vertical: 4.0,
            padding_horizontal: 10.0,
            border_radius: 6.0,
            background: t.color.surface_alt(),
            color: t.color.text(),
        }
    }
}

stylesheet! {
    pub TermSpacer<()> {
        base(_t) { flex_grow: 1.0 }
    }
}

stylesheet! {
    pub TermScreen<IdeaThemeRef> {
        base(t) {
            flex_grow: 1.0,
            flex_shrink: 1.0,
            flex_basis: 0.0,
            min_height: 160.0,
            min_width: 0.0,
            background: t.color.surface_alt(),
            border_width: 1.0,
            border_color: t.color.border(),
            border_radius: t.radius.lg(),
        }
    }
}

stylesheet! {
    pub TermText<()> {
        base(_t) {
            font_family: "ui-monospace, SFMono-Regular, Menlo, monospace",
            font_size: 13.0,
            line_height: TERM_LINE_HEIGHT,
            padding: 18.0,
        }
    }
}

stylesheet! {
    pub TermTargets<IdeaThemeRef> {
        base(t) {
            flex_direction: FlexDirection::Row,
            flex_wrap: runtime_core::FlexWrap::Wrap,
            align_items: AlignItems::Center,
            gap: 8.0,
            flex_shrink: 0.0,
        }
    }
}

stylesheet! {
    pub TermTargetsLabel<IdeaThemeRef> {
        base(t) {
            font_size: 13.0,
            color: t.color.text_muted(),
            margin_right: 4.0,
        }
    }
}

stylesheet! {
    pub TermTargetChip<IdeaThemeRef> {
        base(t) {
            padding_vertical: 4.0,
            padding_horizontal: 12.0,
            border_radius: 999.0,
            border_width: 1.0,
            border_color: t.color.border(),
            font_size: 13.0,
            font_weight: runtime_core::FontWeight::SemiBold,
            color: t.color.text_muted(),
            cursor: runtime_core::Cursor::Pointer,
        }
        variant on {
            #[default]
            no(_t) {}
            yes(t) {
                background: t.intent.primary.soft_bg(),
                border_color: t.intent.primary.soft_bg(),
                color: t.intent.primary.solid_bg(),
            }
        }
    }
}

stylesheet! {
    pub TermSteps<IdeaThemeRef> {
        base(t) {
            flex_shrink: 0.0,
            max_height: 340.0,
            border_width: 1.0,
            border_color: t.color.border(),
            border_radius: t.radius.lg(),
        }
    }
}

stylesheet! {
    pub TermStepRow<IdeaThemeRef> {
        base(t) {
            flex_direction: FlexDirection::Row,
            align_items: AlignItems::Center,
            gap: 14.0,
            padding_vertical: 10.0,
            padding_horizontal: 16.0,
            border_bottom_width: 1.0,
            border_bottom_color: t.color.border(),
        }
    }
}

stylesheet! {
    pub TermStepMarker<IdeaThemeRef> {
        base(t) {
            width: 26.0,
            height: 26.0,
            flex_shrink: 0.0,
            border_radius: 999.0,
            border_width: 1.5,
            border_color: t.color.border(),
            font_size: 13.0,
            line_height: 23.0,
            text_align: TextAlign::Center,
            color: t.color.text_muted(),
        }
        variant state {
            #[default]
            later(_t) {}
            next(t) {
                background: t.intent.primary.solid_bg(),
                border_color: t.intent.primary.solid_bg(),
                color: t.intent.primary.solid_text(),
            }
            done(t) {
                border_color: t.intent.success.solid_bg(),
                color: t.intent.success.solid_bg(),
            }
        }
    }
}

stylesheet! {
    pub TermStepBody<()> {
        base(_t) {
            flex_direction: FlexDirection::Column,
            gap: 2.0,
            flex_grow: 1.0,
            flex_shrink: 1.0,
            min_width: 0.0,
        }
    }
}

stylesheet! {
    pub TermStepTitle<IdeaThemeRef> {
        base(t) {
            font_size: 15.0,
            font_weight: runtime_core::FontWeight::Medium,
            color: t.color.text(),
        }
    }
}

stylesheet! {
    pub TermStepCommand<IdeaThemeRef> {
        base(t) {
            font_family: "ui-monospace, SFMono-Regular, Menlo, monospace",
            font_size: 12.0,
            line_height: 18.0,
            color: t.color.text_muted(),
        }
    }
}

stylesheet! {
    pub TermFootnote<IdeaThemeRef> {
        base(t) {
            flex_shrink: 0.0,
            font_size: 12.0,
            color: t.color.text_muted(),
            width: Length::pct(100.0),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn echo(_: Os, _: Targets) -> String {
        "echo hi".to_string()
    }
    fn hi(_: Os, _: Targets) -> String {
        "hi".to_string()
    }
    fn nothing(_: Os, _: Targets) -> String {
        String::new()
    }

    /// The scroll target for "Run next": the next command prints where the
    /// idle prompt is, after every line already shown. A step with output
    /// takes three lines (command, output, gap); one without takes two.
    #[test]
    fn the_next_command_prints_on_the_prompt_line() {
        static STEPS: &[TermStep] = &[
            TermStep { title: "a", command: echo, output: hi },
            TermStep { title: "b", command: echo, output: nothing },
            TermStep { title: "c", command: echo, output: hi },
        ];
        let at = |ran| new_command_line(STEPS, ran, Os::Macos, Targets::default());
        assert_eq!(at(0), 0);
        assert_eq!(at(1), 3);
        assert_eq!(at(2), 5);
    }
    fn shell_os(os: Os, _: Targets) -> String {
        if os == Os::Windows { "dir".to_string() } else { "ls".to_string() }
    }

    const STEPS: &[TermStep] = &[
        TermStep { title: "Say hi", command: echo, output: hi },
        TermStep { title: "List", command: shell_os, output: nothing },
    ];

    fn text_of(runs: &[(String, Run)]) -> String {
        runs.iter().map(|(t, _)| t.as_str()).collect()
    }

    #[test]
    fn nothing_run_is_an_idle_prompt() {
        let runs = transcript(STEPS, 0, Os::Macos, Targets::default());
        assert_eq!(text_of(&runs), "$ \u{2588}");
        assert_eq!(runs[0].1, Run::Prompt);
    }

    #[test]
    fn a_run_step_shows_its_command_then_its_output() {
        let text = text_of(&transcript(STEPS, 1, Os::Macos, Targets::default()));
        assert_eq!(text, "$ echo hi\nhi\n\n$ \u{2588}");
    }

    #[test]
    fn a_silent_command_prints_only_its_prompt_line() {
        let text = text_of(&transcript(STEPS, 2, Os::Linux, Targets::default()));
        assert!(text.contains("$ ls\n\n$ \u{2588}"), "{text}");
    }

    #[test]
    fn commands_follow_the_os() {
        let text = text_of(&transcript(STEPS, 2, Os::Windows, Targets::default()));
        assert!(text.contains("$ dir\n"), "{text}");
    }

    #[test]
    fn running_past_the_end_shows_every_step_once() {
        let text = text_of(&transcript(STEPS, 9, Os::Macos, Targets::default()));
        assert_eq!(text.matches("$ echo hi").count(), 1);
    }

    #[test]
    fn os_ids_round_trip() {
        for os in [Os::Macos, Os::Linux, Os::Windows] {
            assert_eq!(Os::from_id(os.id()), os);
        }
    }
}
