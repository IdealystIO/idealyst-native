//! The system check: one screen that exercises every backend capability a
//! user would notice missing, two ways at once.
//!
//! - **Automated.** Each section holds a fixture tagged with a `test_id`;
//!   the `capabilities` suite (`suites.rs`) drives it through the robot
//!   and its verdict shows on the section as the run reaches it (the
//!   result listener writes [`Results`]).
//! - **Manual.** What only a person can judge — the cursor shape, whether
//!   an animation animates, Enter in a text field — is a checklist row:
//!   instructions plus Pass / Fail / Skip. "Report" logs every verdict as
//!   one `[CHECK-RESULT]` JSON line, the same way the suite ends with
//!   `[E2E-RESULT]`, so runs on two platforms can be compared.
//!
//! The capabilities are the ones whose trait defaults silently do nothing
//! (`crates/dev/capability-coverage` finds those statically): hover and
//! press states, the cursor, text input values written from code, focus
//! and blur, placeholders, text areas, virtualized lists, scroll-end,
//! external links, presence animations.

use std::collections::BTreeMap;

use runtime_macros::{component, ui};
use runtime_shared::{Color, Cursor, Length, StyleRules, StyleSheet};
use runtime_shared::primitives::key::{KeyEvent, KeyOutcome};
use runtime_vocabulary::builders::{pressable, text_area, text_input};
use runtime_vocabulary::glue::primitives::flat_list::{fixed_size, flat_list};
use runtime_vocabulary::glue::primitives::presence::presence;
use runtime_vocabulary::glue::primitives::scroll_view::scroll_view;
use runtime_vocabulary::glue::{button, signal, text, view, Element, IntoElement, Signal};

use crate::screens::NavCell;

/// The automated verdict of one test, as the result listener reports it.
#[derive(Clone, Debug, PartialEq)]
pub struct AutoResult {
    pub test: String,
    /// `"pass"`, `"fail"` or `"skip"`.
    pub status: &'static str,
    pub detail: Option<String>,
}

/// Results that outlive the screen (the suite runs before anyone opens
/// it): automated verdicts as they arrive, manual marks by check id.
#[derive(Clone, Copy)]
pub struct Results {
    pub automated: Signal<Vec<AutoResult>>,
    pub manual: Signal<BTreeMap<&'static str, &'static str>>,
}

impl Results {
    pub fn new() -> Self {
        Results { automated: signal(Vec::new()), manual: signal(BTreeMap::new()) }
    }
}

/// One manual check: what to do and what should happen. `tests` names
/// the automated tests covering the same capability, shown beside it.
struct Manual {
    id: &'static str,
    title: &'static str,
    steps: &'static str,
    tests: &'static [&'static str],
}

/// Section order on screen.
const HOVER: Manual = Manual {
    id: "hover",
    title: "Hover, press and cursor",
    steps: "Move the mouse over the box: it turns light blue and the cursor becomes a pointing hand. \
            Hold the button down on it: it turns dark blue until you let go, even if you drag off it first.",
    tests: &["hover lights the hovered state", "press lights the pressed state"],
};
const INPUT: Manual = Manual {
    id: "text-input",
    title: "Text input",
    steps: "The empty field shows \"Type here\"; \"Swap placeholder\" changes it to \"Something else\". \
            Type, then press Enter: the submit count goes up. \"Clear from code\" empties the field.",
    tests: &["a value written from code reaches the field", "focus and blur reach the app"],
};
const AREA: Manual = Manual {
    id: "text-area",
    title: "Text area",
    steps: "Type two lines into the box: both stay visible and the echo below follows.",
    tests: &["a text area is editable"],
};
const LIST: Manual = Manual {
    id: "list",
    title: "Virtualized list",
    steps: "The list shows rows; \"Add row\" adds one at the end.",
    tests: &["a flat_list renders rows and follows its data"],
};
const SCROLL: Manual = Manual {
    id: "scroll",
    title: "Scroll view",
    steps: "Scroll the box with the wheel or trackpad: a scrollbar shows while scrolling. \
            Reaching the bottom bumps \"end reached\".",
    tests: &["reaching the end of a scroll view is reported"],
};
const LINK: Manual = Manual {
    id: "link",
    title: "External link",
    steps: "\"Open example.com\" opens the page in the system browser.",
    tests: &["external links have an opener"],
};
const PRESENCE: Manual = Manual {
    id: "presence",
    title: "Presence animation",
    steps: "\"Toggle\" fades the box out and back in over a quarter second; it must not snap.",
    tests: &[],
};
const KEYBOARD: Manual = Manual {
    id: "keyboard",
    title: "Keyboard focus",
    steps: "Click the first text field, then press Tab: focus moves to the next control.",
    tests: &[],
};

const MANUAL: [&Manual; 8] = [&HOVER, &INPUT, &AREA, &LIST, &SCROLL, &LINK, &PRESENCE, &KEYBOARD];

/// Indexes into [`MANUAL`], so a section names its check with a plain
/// (static, `Default`) prop.
const HOVER_I: usize = 0;
const INPUT_I: usize = 1;
const AREA_I: usize = 2;
const LIST_I: usize = 3;
const SCROLL_I: usize = 4;
const LINK_I: usize = 5;
const PRESENCE_I: usize = 6;
const KEYBOARD_I: usize = 7;

/// The fixture's hover / press look. `cursor` is on the base so the hand
/// shows whenever the pointer is over it.
fn stateful_box() -> std::rc::Rc<StyleSheet> {
    let fill = |hex: &str| StyleRules { background: Some(Color(hex.into()).into()), ..Default::default() };
    std::rc::Rc::new(
        StyleSheet::new(|_| StyleRules {
            padding_top: Some(Length::Px(16.0).into()),
            padding_bottom: Some(Length::Px(16.0).into()),
            padding_left: Some(Length::Px(16.0).into()),
            padding_right: Some(Length::Px(16.0).into()),
            background: Some(Color("#e5e7eb".into()).into()),
            cursor: Some(Cursor::Pointer),
            ..Default::default()
        })
        .variant("__state_hovered", "on", move |_| fill("#bfdbfe"))
        .variant("__state_pressed", "on", move |_| fill("#2563eb")),
    )
}

fn fixed_height(px: f32) -> StyleRules {
    StyleRules { height: Some(Length::Px(px).into()), ..Default::default() }
}

fn section() -> StyleRules {
    StyleRules {
        padding_top: Some(Length::Px(12.0).into()),
        padding_bottom: Some(Length::Px(12.0).into()),
        gap: Some(Length::Px(8.0).into()),
        ..Default::default()
    }
}

/// The screen. A plain fn like the other pages: it takes the shared nav
/// cell, which is not a prop type.
pub(crate) fn system_check_page(results: Results, nav: NavCell) -> Element {
    let back = move || {
        if let Some(h) = nav.borrow().as_ref() {
            h.pop();
        }
    };
    let report = move || report(results);
    let (automated, manual) = (results.automated, results.manual);
    ui! {
        scroll_view(test_id = "check-page") {
            view(style = section()) {
                text(test_id = "check-marker") { "System check" }
                Summary(automated = automated, manual = manual)
                button(label = "Report", test_id = "check-report", on_click = report)
                button(label = "Back", test_id = "check-back", on_click = back)
            }
            CheckSection(index = HOVER_I, automated = automated, manual = manual) { HoverFixture() }
            CheckSection(index = INPUT_I, automated = automated, manual = manual) { InputFixture() }
            CheckSection(index = AREA_I, automated = automated, manual = manual) { AreaFixture() }
            CheckSection(index = LIST_I, automated = automated, manual = manual) { ListFixture() }
            CheckSection(index = SCROLL_I, automated = automated, manual = manual) { ScrollFixture() }
            CheckSection(index = LINK_I, automated = automated, manual = manual) { LinkFixture() }
            CheckSection(index = PRESENCE_I, automated = automated, manual = manual) { PresenceFixture() }
            CheckSection(index = KEYBOARD_I, automated = automated, manual = manual) { KeyboardFixture() }
        }
    }
}

/// Totals for the automated run and the manual marks.
#[component]
fn Summary(automated: Signal<Vec<AutoResult>>, manual: Signal<BTreeMap<&'static str, &'static str>>) -> Element {
    let line = move || {
        let auto = automated.get();
        let count = |s: &str| auto.iter().filter(|r| r.status == s).count();
        let marked = manual.get().len();
        format!(
            "automated: {} pass, {} fail, {} skip · manual: {marked}/{} marked",
            count("pass"),
            count("fail"),
            count("skip"),
            MANUAL.len()
        )
    };
    ui! { text(test_id = "check-summary") { line } }
}

/// One capability: title, its automated verdicts, the fixture, the manual
/// steps and the Pass / Fail / Skip row.
#[component]
fn CheckSection(
    #[prop(static)] index: usize,
    automated: Signal<Vec<AutoResult>>,
    manual: Signal<BTreeMap<&'static str, &'static str>>,
    children: Vec<Element>,
) -> Element {
    let check = MANUAL[index];
    let id = check.id;
    let mark = move |verdict: &'static str| {
        move || {
            manual.update(|m| {
                let mut m = m.clone();
                m.insert(id, verdict);
                m
            })
        }
    };
    let verdicts = move || {
        let auto = automated.get();
        let lines: Vec<String> = check
            .tests
            .iter()
            .map(|t| match auto.iter().find(|r| r.test == *t) {
                Some(r) => match &r.detail {
                    Some(d) => format!("{}: {} — {d}", r.status, r.test),
                    None => format!("{}: {}", r.status, r.test),
                },
                None => format!("not run: {t}"),
            })
            .collect();
        if lines.is_empty() { "manual only".to_string() } else { lines.join("\n") }
    };
    let marked = move || format!("manual: {}", manual.get().get(id).copied().unwrap_or("unmarked"));
    ui! {
        view(style = section()) {
            text { check.title }
            text { verdicts }
            children
            text { check.steps }
            view(style = StyleRules { flex_direction: Some(runtime_shared::FlexDirection::Row), gap: Some(Length::Px(8.0).into()), ..Default::default() }) {
                button(label = "Pass", on_click = mark("pass"))
                button(label = "Fail", on_click = mark("fail"))
                button(label = "Skip", on_click = mark("skip"))
                text { marked }
            }
        }
    }
}

// `pressable`, `text_area` and the full `text_input` surface (`on_focus`,
// `on_blur`, a dynamic placeholder) are built with the builders: `ui!`
// lowers only the leaf primitives listed in CLAUDE.md §9.2, and these are
// not among them.

#[component]
fn HoverFixture() -> Element {
    let presses = signal(0);
    let target = pressable(move || presses.update(|n| n + 1))
        .test_id("cap-hover")
        .style(stateful_box())
        .child(ui! { text { "Hover or press me" } })
        .build();
    ui! {
        view {
            target
            // A view, not a bare label: labels are hit-transparent on some
            // backends (macOS), so the pointer would land on the parent.
            view(test_id = "cap-elsewhere", style = section()) {
                text { move || format!("presses: {}", presses.get()) }
            }
        }
    }
}

#[component]
fn InputFixture() -> Element {
    let value = signal(String::new());
    let focus = signal(0);
    let blur = signal(0);
    let submits = signal(0);
    let alt_placeholder = signal(false);
    let on_key = move |ev: &KeyEvent| {
        if ev.key == "Enter" {
            submits.update(|n| n + 1);
        }
        KeyOutcome::Default
    };
    let field = text_input()
        .test_id("cap-input")
        .value(value)
        .on_change(move |s| value.set(s))
        .placeholder_dyn(move || Some(if alt_placeholder.get() { "Something else" } else { "Type here" }.to_string()))
        // Focus changes arrive here both ways; `on_blur` is a veto hook
        // ("keep focus?"), not the notification.
        .on_focus(move |on| if on { focus.update(|n| n + 1) } else { blur.update(|n| n + 1) })
        .on_key_down(on_key)
        .build();
    let second = text_input().test_id("cap-input-2").build();
    ui! {
        view {
            field
            second
            text(test_id = "cap-input-echo") { move || format!("value: {}", value.get()) }
            text(test_id = "cap-input-events") {
                move || format!("focus: {} blur: {} submit: {}", focus.get(), blur.get(), submits.get())
            }
            button(label = "Clear from code", test_id = "cap-input-clear", on_click = move || value.set(String::new()))
            button(label = "Swap placeholder", on_click = move || alt_placeholder.update(|b| !b))
        }
    }
}

#[component]
fn AreaFixture() -> Element {
    let value = signal(String::new());
    let area = text_area().test_id("cap-area").value(value).on_change(move |s| value.set(s)).min_rows(2).build();
    ui! {
        view {
            area
            text(test_id = "cap-area-echo") { move || format!("area: {}", value.get().replace('\n', "⏎")) }
        }
    }
}

#[component]
fn ListFixture() -> Element {
    let rows: Signal<Vec<u64>> = signal(vec![1, 2, 3]);
    let add = move || rows.update(|r| {
        let mut r = r.clone();
        r.push(r.len() as u64 + 1);
        r
    });
    ui! {
        view {
            view(style = fixed_height(200.0)) {
                flat_list(
                    data = rows,
                    key = |_idx, r| *r,
                    size = fixed_size(32.0),
                    render = |_idx, r| ui! { text(test_id = "cap-row") { format!("row {r}") } },
                )
            }
            button(label = "Add row", test_id = "cap-add-row", on_click = add)
        }
    }
}

#[component]
fn ScrollFixture() -> Element {
    let ends = signal(0);
    let lines: Vec<u32> = (1..=30).collect();
    ui! {
        view {
            scroll_view(test_id = "cap-scroll", style = fixed_height(120.0), on_end_reached = move || ends.update(|n| n + 1)) {
                for i in lines {
                    text { format!("line {i}") }
                }
            }
            text(test_id = "cap-scroll-end") { move || format!("end reached: {}", ends.get()) }
        }
    }
}

#[component]
fn LinkFixture() -> Element {
    ui! {
        button(label = "Open example.com", test_id = "cap-open-link", on_click = || runtime_shared::open_url("https://example.com"))
    }
}

#[component]
fn PresenceFixture() -> Element {
    use runtime_shared::{Easing, PresenceAnim, PresenceState};
    let shown = signal(true);
    let fade = || PresenceAnim::new(PresenceState { opacity: Some(0.0), ..Default::default() }, 250, Easing::EaseOut);
    let boxed = presence(|| ui! { view(style = stateful_box()) { text { "I fade" } } })
        .present(move || shown.get())
        .enter(fade())
        .exit(fade())
        .into_element();
    ui! {
        view {
            button(label = "Toggle", on_click = move || shown.update(|b| !b))
            view(style = fixed_height(56.0)) { boxed }
        }
    }
}

#[component]
fn KeyboardFixture() -> Element {
    let field = text_input().build();
    ui! {
        view {
            field
            button(label = "Next control", on_click = || {})
        }
    }
}

/// Log every verdict as one `[CHECK-RESULT]` line.
fn report(results: Results) {
    use runtime_shared::__serde_json::{json, Map, Value};
    let automated: Vec<Value> = results
        .automated
        .get()
        .iter()
        .map(|r| json!({ "test": r.test, "status": r.status, "detail": r.detail }))
        .collect();
    let manual: Map<String, Value> = MANUAL
        .iter()
        .map(|m| {
            let v = results.manual.get().get(m.id).copied().unwrap_or("unmarked");
            (m.id.to_string(), Value::String(v.into()))
        })
        .collect();
    let line = json!({
        "platform": format!("{:?}", runtime_shared::platform()),
        "automated": automated,
        "manual": manual,
    });
    runtime_shared::log_info!("[CHECK-RESULT] {line}");
}
