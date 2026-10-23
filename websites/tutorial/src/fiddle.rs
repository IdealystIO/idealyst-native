//! `Fiddle` — an editable example whose preview is the code, built.
//!
//! The editor holds a complete `app.rs`. **Run** (or ⌘/Ctrl-Enter) posts
//! it to the fiddle server (`examples/fiddle`), which builds it with
//! `idealyst build --web` into a real web app and answers with the
//! bundle's URL; the preview frames that URL. So what a reader sees is
//! not a simulation of their code — it is their code, compiled and
//! running on the web backend, the same bundle an app would ship.
//!
//! A compile error comes back as diagnostics in the reader's own file
//! names: each is underlined at its position in the editor and printed
//! under the panes, rustc's rendering included. A panic at run time
//! arrives from the frame as a `postMessage` (the server injects the
//! bridge into every bundle) and prints there too.
//!
//! The server address is `http://127.0.0.1:8787` unless the tutorial was
//! built with `TUTORIAL_FIDDLE_URL` set. With no server reachable, the
//! panel says how to start one; the code stays editable.
//!
//! Every example compiles on first view. The server caches by content,
//! so an unedited example is answered from its cache after the first
//! reader builds it.

use std::cell::Cell;
use std::rc::Rc;

use codeblock::rust::{self, Palette};
use codeblock::{code_block, code_editor, Decoration, Underline};
use idea_ui::{Button, ButtonSize, ButtonVariant};
use runtime_core::primitives::text_area::TextAreaHandle;
use runtime_core::{
    component, derived, memo, signal, spawn_then_in, switch, ui, Color, Element, IntoElement, KeyEvent,
    KeyOutcome, Ref, ScopeAlive, ScrollAxis, Signal, TouchEvent, TouchPhase, TouchResponse,
};
use serde::Deserialize;
use webview::{url, WebView, WebViewProps};

use crate::common::theme_is_dark;
use crate::styles::{
    BrowserFrame, FiddleEditor, FiddleEditorPane, FiddleEditorRow,
    FiddleFileName, FiddleFrame, FiddleGutter, FiddleOutput, FiddleOutputText, FiddlePreview,
    FiddleDivider, FiddleDividerGrip, FiddleDividerGripActive, FiddlePreviewCard,
    FiddlePreviewSection, FiddleStatus,
    FiddleStatusTone, FiddleToolbar,
};

/// Where the compile server listens. A deployed tutorial is built with
/// `TUTORIAL_FIDDLE_URL=https://…` pointing at its own server.
const FIDDLE_SERVER: &str = match option_env!("TUTORIAL_FIDDLE_URL") {
    Some(url) => url,
    None => "http://127.0.0.1:8787",
};

/// The file a submission's code goes in — the server's required entry.
const ENTRY_FILE: &str = "app.rs";

const EDITOR_FONT: &str = "ui-monospace, SFMono-Regular, Menlo, monospace";
const EDITOR_FONT_SIZE: f32 = 13.0;
/// Integral, so both editor layers land each row on the same pixel.
const EDITOR_LINE_HEIGHT: f32 = 20.0;
const EDITOR_PADDING: f32 = 14.0;
const ERROR_UNDERLINE: &str = "#e5484d";
/// The preview card's height before the reader drags the divider, and the
/// range a drag stays in (the editor's own minimum takes over above it).
const PREVIEW_HEIGHT: f32 = 300.0;
const PREVIEW_MIN_HEIGHT: f32 = 120.0;
const PREVIEW_MAX_HEIGHT: f32 = 1200.0;
/// Two presses on the divider closer than this reset the split.
const DOUBLE_PRESS_NS: u64 = 400_000_000;
const WARNING_UNDERLINE: &str = "#d4a017";

/// One compiler message, as `POST /compile` reports it
/// (`examples/fiddle/src/compile.rs`).
#[derive(Clone, Debug, Deserialize, PartialEq)]
struct Diagnostic {
    level: String,
    message: String,
    file: Option<String>,
    line: Option<u32>,
    column: Option<u32>,
    rendered: String,
}

#[derive(Deserialize)]
#[serde(tag = "status", rename_all = "snake_case")]
enum Reply {
    Built { url: String, ms: u64, cached: bool, diagnostics: Vec<Diagnostic> },
    Failed { diagnostics: Vec<Diagnostic>, log: String },
}

#[derive(Clone, Debug, PartialEq)]
enum Status {
    Compiling,
    Built { ms: u64, cached: bool },
    Failed,
    /// The request never got a reply: no server, or a refused submission.
    Unreachable(String),
}

async fn compile(source: String) -> Result<Reply, String> {
    let body = serde_json::json!({ "files": { ENTRY_FILE: source } });
    let response = net::Client::new()
        .post(format!("{FIDDLE_SERVER}/compile"))
        .json(&body)
        .send()
        .await
        .map_err(|e| e.to_string())?;
    let status = response.status();
    let text = response.text().await.map_err(|e| e.to_string())?;
    if status != 200 {
        let message = serde_json::from_str::<serde_json::Value>(&text)
            .ok()
            .and_then(|v| v.get("error").and_then(|e| e.as_str()).map(String::from))
            .unwrap_or(text);
        return Err(message);
    }
    serde_json::from_str(&text).map_err(|e| format!("unreadable reply: {e}"))
}

thread_local! {
    /// The reader's edits, per example, for the life of the page. A
    /// lesson's workspace is rebuilt when it moves (docked ⇄ drawer) and
    /// each time the drawer opens, and a reader who navigates away and
    /// back expects their code where they left it. Keyed by the starter's
    /// address: each example's source is its own `include_str!` constant.
    static DRAFTS: std::cell::RefCell<std::collections::HashMap<usize, String>> =
        std::cell::RefCell::new(std::collections::HashMap::new());
}

fn draft_key(source: &'static str) -> usize {
    source.as_ptr() as usize
}

/// The code to start an example with: the reader's draft, else the starter.
fn draft(source: &'static str) -> String {
    DRAFTS.with(|d| d.borrow().get(&draft_key(source)).cloned()).unwrap_or_else(|| source.to_string())
}

fn save_draft(source: &'static str, code: &str) {
    DRAFTS.with(|d| d.borrow_mut().insert(draft_key(source), code.to_string()));
}

/// Reset: the example starts from its starter again.
fn forget_draft(source: &'static str) {
    DRAFTS.with(|d| d.borrow_mut().remove(&draft_key(source)));
}

/// The byte range to underline for a diagnostic at 1-based (`line`,
/// `column`) — rustc counts columns in characters. It covers the
/// identifier starting there, or one character when none does.
fn diagnostic_range(src: &str, line: u32, column: u32) -> Option<std::ops::Range<usize>> {
    let line_start = if line <= 1 {
        0
    } else {
        src.match_indices('\n').nth(line as usize - 2)?.0 + 1
    };
    let line_text = &src[line_start..];
    let line_text = &line_text[..line_text.find('\n').unwrap_or(line_text.len())];
    let (offset, _) = line_text.char_indices().nth(column.saturating_sub(1) as usize)?;
    let rest = &line_text[offset..];
    // A position on a string literal (an f-string's unknown name, a bad
    // format spec) underlines the literal through its closing quote.
    if let Some(body) = rest.strip_prefix('"') {
        let mut escaped = false;
        for (i, c) in body.char_indices() {
            match c {
                '\\' if !escaped => escaped = true,
                '"' if !escaped => {
                    let start = line_start + offset;
                    return Some(start..start + 1 + i + 1);
                }
                _ => escaped = false,
            }
        }
    }
    let word = rest
        .char_indices()
        .find(|(_, c)| !(c.is_alphanumeric() || *c == '_'))
        .map_or(rest.len(), |(i, _)| i);
    let len = if word == 0 { rest.chars().next().map_or(0, char::len_utf8) } else { word };
    let start = line_start + offset;
    (len > 0).then(|| start..start + len)
}

/// A JSON message from the error bridge in the framed bundle, or `None`
/// for any other message the frame posts.
fn bridge_error(message: &str) -> Option<String> {
    let v: serde_json::Value = serde_json::from_str(message).ok()?;
    if v.get("source")?.as_str()? != "idealyst-fiddle" {
        return None;
    }
    v.get("text")?.as_str().map(String::from)
}

/// An editable, runnable example. `source` is a whole `app.rs`: it must
/// define `pub fn app() -> Element`. The reader's edits are kept per
/// example for the life of the page (see `DRAFTS`), so the editor comes
/// back with them when it is rebuilt.
#[component]
pub fn Fiddle(#[prop(static)] source: &'static str) -> Element {
    let code: Signal<String> = signal(draft(source));
    let status: Signal<Status> = signal(Status::Compiling);
    let preview_url: Signal<String> = signal(String::from("about:blank"));
    let diagnostics: Signal<Vec<Diagnostic>> = signal(Vec::new());
    // The code `diagnostics` describe: an edit since that compile moves the
    // text under the reported positions, so the underlines hide until the
    // next run instead of marking the wrong characters.
    let checked: Signal<String> = signal(String::new());
    let runtime_error: Signal<Option<String>> = signal(None);
    let busy: Signal<bool> = signal(true);
    // Taken here, not in the handler: the result must land only while
    // this panel is mounted (a reader may navigate away mid-build).
    let alive = ScopeAlive::current();

    // Compile `src` and show the result. Takes the source rather than
    // reading `code`, because Reset writes `code` and compiles in the same
    // handler — and a write is staged until the handler returns.
    let compile_source: Rc<dyn Fn(String)> = Rc::new(move |src: String| {
        status.set(Status::Compiling);
        busy.set(true);
        runtime_error.set(None);
        spawn_then_in(&alive, compile(src.clone()), move |reply| {
            busy.set(false);
            match reply {
                Ok(Reply::Built { url, ms, cached, diagnostics: found }) => {
                    preview_url.set(format!("{FIDDLE_SERVER}{url}"));
                    status.set(Status::Built { ms, cached });
                    diagnostics.set(found);
                }
                Ok(Reply::Failed { diagnostics: mut found, log }) => {
                    if found.is_empty() {
                        found.push(Diagnostic {
                            level: "error".into(),
                            message: "the build failed".into(),
                            file: None,
                            line: None,
                            column: None,
                            rendered: log,
                        });
                    }
                    status.set(Status::Failed);
                    diagnostics.set(found);
                }
                Err(reason) => {
                    status.set(Status::Unreachable(reason));
                    diagnostics.set(Vec::new());
                }
            }
            checked.set(src);
        });
    });
    compile_source(source.to_string());

    let run: Rc<dyn Fn()> = {
        let compile_source = compile_source.clone();
        Rc::new(move || compile_source(code.get()))
    };
    let reset: Rc<dyn Fn()> = Rc::new(move || {
        code.set(source.to_string());
        forget_draft(source);
        compile_source(source.to_string());
    });

    // Syntax colors, then the last compile's underlines while the code is
    // still what was compiled.
    let decorations = memo(move || {
        let src = code.get();
        let palette = if theme_is_dark() { Palette::DARK } else { Palette::LIGHT };
        let mut out = rust::decorations(&src, &palette);
        if checked.get() == src {
            for d in diagnostics.get().iter() {
                if d.file.as_deref() != Some(ENTRY_FILE) {
                    continue;
                }
                let (Some(line), Some(column)) = (d.line, d.column) else { continue };
                if let Some(range) = diagnostic_range(&src, line, column) {
                    let color = if d.level == "error" { ERROR_UNDERLINE } else { WARNING_UNDERLINE };
                    out.push(Decoration::underline(range, Underline::solid().colored(color)));
                }
            }
        }
        out
    });

    let handle: Ref<TextAreaHandle> = Ref::new();
    let run_for_keys = run.clone();
    let on_key_down = move |ev: &KeyEvent| {
        if ev.key == "Enter" && (ev.meta || ev.ctrl) {
            run_for_keys();
            KeyOutcome::PreventDefault
        } else if ev.key == "Tab" && !(ev.shift || ev.ctrl || ev.alt || ev.meta) {
            if let Some(h) = handle.get() {
                h.insert_text("    ");
            }
            KeyOutcome::PreventDefault
        } else {
            KeyOutcome::Default
        }
    };
    let on_key_down = Rc::new(on_key_down);

    // The editor's plain-text ink is a build-time metric, so a theme flip
    // rebuilds the editor; its text lives in `code`, which survives.
    // idealyst-lint-disable-next-line prefer-ui-control-flow -- keyed rebuild of one shape: the editor per theme
    let editor = switch(theme_is_dark, move |&dark| {
        let palette = if dark { Palette::DARK } else { Palette::LIGHT };
        let on_key_down = on_key_down.clone();
        code_editor(code, move |next| {
            save_draft(source, &next);
            code.set(next);
        })
            .decorations(decorations.read_only())
            .on_key_down(move |ev| on_key_down(ev))
            .bind(handle)
            .font(EDITOR_FONT, EDITOR_FONT_SIZE)
            .line_height(EDITOR_LINE_HEIGHT)
            .padding(EDITOR_PADDING)
            .text_color(palette.ink)
            .with_style(FiddleEditor())
            .into_element()
    });

    let status_text = move || match status.get() {
        Status::Compiling => "Compiling…".to_string(),
        Status::Built { cached: true, .. } => "Built (cached)".to_string(),
        Status::Built { ms, .. } => format!("Built in {:.1} s", ms as f64 / 1000.0),
        Status::Failed => {
            let n = diagnostics.get().iter().filter(|d| d.level == "error").count();
            if n == 1 { "1 error".to_string() } else { format!("{n} errors") }
        }
        Status::Unreachable(_) => "Preview offline".to_string(),
    };
    let status_style = FiddleStatus().tone(derived(move || match status.get() {
        Status::Built { .. } => FiddleStatusTone::Good,
        Status::Failed | Status::Unreachable(_) => FiddleStatusTone::Bad,
        Status::Compiling => FiddleStatusTone::Quiet,
    }));

    let web_preview = || {
        WebView(WebViewProps {
            url: url(move || preview_url.get()),
            on_message: Some(Rc::new(move |message: String| {
                if let Some(text) = bridge_error(&message) {
                    runtime_error.set(Some(text));
                }
            })),
            ..Default::default()
        })
        .with_style(FiddlePreview())
        .into_element()
    };

    // The editor/preview split: the divider sets the preview card's height
    // and the editor takes the rest. Deltas come from `window_position`:
    // the divider itself moves as the split changes, so a position local
    // to it would chase the drag.
    let preview_height: Signal<f32> = signal(PREVIEW_HEIGHT);
    let dragging: Signal<bool> = signal(false);
    let drag_from = Rc::new(Cell::new((0.0_f32, 0.0_f32)));
    let last_press_ns = Rc::new(Cell::new(0_u64));
    let on_divider = move |ev: &TouchEvent| -> TouchResponse {
        match ev.phase {
            TouchPhase::Began => {
                let previous = last_press_ns.replace(ev.timestamp_ns);
                if previous != 0 && ev.timestamp_ns.saturating_sub(previous) < DOUBLE_PRESS_NS {
                    last_press_ns.set(0);
                    preview_height.set(PREVIEW_HEIGHT);
                    return TouchResponse::CLAIMED;
                }
                drag_from.set((ev.window_position.y, preview_height.peek()));
                dragging.set(true);
                TouchResponse::CLAIMED
            }
            TouchPhase::Moved => {
                let (y0, h0) = drag_from.get();
                // Dragging up grows the preview.
                let h = (h0 - (ev.window_position.y - y0)).clamp(PREVIEW_MIN_HEIGHT, PREVIEW_MAX_HEIGHT);
                preview_height.set(h);
                TouchResponse::CLAIMED
            }
            TouchPhase::Ended | TouchPhase::Cancelled => {
                dragging.set(false);
                TouchResponse::CLAIMED
            }
            TouchPhase::Hovered => TouchResponse::IGNORED,
        }
    };
    let grip_style = FiddleDividerGrip().active(derived(move || {
        if dragging.get() { FiddleDividerGripActive::On } else { FiddleDividerGripActive::Off }
    }));
    let preview_card_style = move || {
        FiddlePreviewCard()
            .into_style_application()
            .with_inline(runtime_core::StyleRules {
                height: Some(runtime_core::Length::Px(preview_height.get()).into()),
                ..Default::default()
            })
    };

    // Line numbers beside the editor, rebuilt when the line count (or the
    // theme) changes. Same font, line height and top inset as the editor,
    // so each number sits on its line.
    // idealyst-lint-disable-next-line prefer-ui-control-flow -- keyed rebuild of one shape: the gutter per (line count, theme)
    let gutter = switch(
        move || (code.get().split('\n').count(), theme_is_dark()),
        |&(lines, dark): &(usize, bool)| {
            let palette = if dark { Palette::DARK } else { Palette::LIGHT };
            let numbers = (1..=lines).map(|n| n.to_string()).collect::<Vec<_>>().join("\n");
            code_block(vec![(numbers, Color(palette.comment.into()))])
                .with_style(FiddleGutter())
                .into_element()
        },
    );


    // What prints under the panes: compiler output, a runtime panic, or how
    // to start the server.
    // Everything printed under the panes, as code_block runs: compiler
    // output keeps rustc's line breaks and column alignment, which plain
    // text would collapse.
    let output = memo(move || -> Vec<(String, Color)> {
        let palette = if theme_is_dark() { Palette::DARK } else { Palette::LIGHT };
        let ink = Color(palette.ink.into());
        let red = Color(ERROR_UNDERLINE.into());
        let mut runs: Vec<(String, Color)> = match status.get() {
            Status::Unreachable(reason) => vec![
                (format!("The live preview could not reach the fiddle server at {FIDDLE_SERVER}\n({reason}).\n\n"), red.clone()),
                ("Start it from the framework checkout, then press Run:\n\n    cargo run -p fiddle\n".to_string(), ink.clone()),
            ],
            _ => diagnostics
                .get()
                .iter()
                .map(|d| {
                    let color = if d.level == "error" { red.clone() } else { ink.clone() };
                    (format!("{}\n\n", d.rendered.trim_end()), color)
                })
                .collect(),
        };
        if let Some(panic) = runtime_error.get() {
            runs.push((format!("The app failed while running:\n{panic}\n"), red));
        }
        runs
    });
    // Built inside the `if` below, which rebuilds it each time output appears.
    let output_block = move || {
        // idealyst-lint-disable-next-line prefer-ui-control-flow -- keyed rebuild of one shape: the output block per report
        switch(move || output.get(), |runs: &Vec<(String, Color)>| {
            code_block(runs.clone()).with_style(FiddleOutputText()).into_element()
        })
    };

    ui! {
        view(style = FiddleFrame()) {
            view(style = FiddleToolbar()) {
                text(style = FiddleFileName()) { "src/app.rs" }
                text(style = status_style) { move || status_text() }
                Button(
                    label = "Reset",
                    on_click = reset,
                    variant = ButtonVariant::Ghost,
                    size = ButtonSize::Sm,
                )
                Button(
                    label = "Run \u{2318}\u{21b5}",
                    on_click = run.clone(),
                    size = ButtonSize::Sm,
                    disabled = busy,
                )
            }
            scroll_view(axis = ScrollAxis::Both, style = FiddleEditorPane()) {
                view(style = FiddleEditorRow()) {
                    gutter
                    editor
                }
            }
            if !output.get().is_empty() {
                scroll_view(style = FiddleOutput()) {
                    { output_block() }
                }
            }
            view(style = FiddleDivider()) {
                view(style = grip_style) {}
            }.on_touch(on_divider)
            view(style = FiddlePreviewSection()) {
                view(style = preview_card_style) {
                    view(style = BrowserFrame()) {
                        { web_preview() }
                    }
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_diagnostic_underlines_the_identifier_at_its_position() {
        let src = "fn a() {}\nlet x = oops + 1;\n";
        let range = diagnostic_range(src, 2, 9).expect("in range");
        assert_eq!(&src[range], "oops");
    }

    #[test]
    fn columns_count_characters_not_bytes() {
        let src = "text { \"héllo\" } bad";
        let range = diagnostic_range(src, 1, 18).expect("in range");
        assert_eq!(&src[range], "bad");
    }

    #[test]
    fn a_position_on_punctuation_underlines_one_character() {
        let src = "let x = (1;";
        let range = diagnostic_range(src, 1, 9).expect("in range");
        assert_eq!(&src[range], "(");
    }

    #[test]
    fn a_position_on_a_string_literal_underlines_the_whole_literal() {
        let src = r#"text { "typo: {nmae} \"q\"" } x"#;
        let range = diagnostic_range(src, 1, 8).expect("in range");
        assert_eq!(&src[range], r#""typo: {nmae} \"q\"""#);
    }

    #[test]
    fn a_position_past_the_text_underlines_nothing() {
        assert_eq!(diagnostic_range("abc", 4, 1), None);
        assert_eq!(diagnostic_range("abc", 1, 9), None);
    }

    #[test]
    fn a_draft_outlives_its_editor_until_reset() {
        static STARTER: &str = "fn app() {}";
        assert_eq!(draft(STARTER), STARTER, "no draft: the starter");
        save_draft(STARTER, "fn app() { /* edited */ }");
        assert_eq!(draft(STARTER), "fn app() { /* edited */ }", "a rebuilt editor gets the edit");
        forget_draft(STARTER);
        assert_eq!(draft(STARTER), STARTER, "Reset goes back to the starter");
    }

    #[test]
    fn only_the_fiddle_bridge_reports_runtime_errors() {
        let bridged = r#"{"source":"idealyst-fiddle","kind":"error","text":"panicked at app.rs:3"}"#;
        assert_eq!(bridge_error(bridged).as_deref(), Some("panicked at app.rs:3"));
        assert_eq!(bridge_error(r#"{"source":"other","text":"x"}"#), None);
        assert_eq!(bridge_error("not json"), None);
    }
}
