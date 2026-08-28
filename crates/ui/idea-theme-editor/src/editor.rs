//! The control panel — one row per token, grouped by namespace.

use std::rc::Rc;

use idea_ui::typography_kind;
use idea_ui::{Card, Field, FieldSize, Stack, StackAlign, StackAxis, StackGap, Typography};
use runtime_core::{component, rx, ui, Color, Element, StyleApplication, StyleRules, TokenValue};

use crate::draft::ThemeDraft;
use crate::format::{swatch_color, DraftKind};
use crate::styles::Swatch;

// Rows and swatches take a `(draft, name)` pair rather than a cloned
// `DraftEntry`, for two reasons. The draft stays the single source of
// truth — a row cannot be handed an entry that isn't in the draft it
// commits to. And `#[component]` needs its props to be `Default`, which
// a `DraftEntry` can't be: it owns a `Signal`, and manufacturing one
// inside a `Default` impl would create it outside any reactive scope,
// which is exactly the ownership hazard `unscope` exists to prevent.

/// A live control panel for every token in `draft`.
///
/// Editing a control commits that token immediately, so the app behind
/// the panel re-tints as you type. A value that doesn't parse marks its
/// own row and simply isn't committed — the app keeps the last good
/// value instead of flashing through half-typed text.
///
/// The panel renders controls and nothing else: no save/load buttons,
/// no file dialogs. Reading and writing files belongs to the app (and
/// an SDK), so [`ThemeDraft::to_json`], [`ThemeDraft::load_json`], and
/// [`ThemeDraft::to_rust`] are plain methods to wire to whatever your
/// app already uses.
///
/// ```ignore
/// let draft = ThemeDraft::from_live();
/// ui! {
///     Stack(gap = StackGap::Md) {
///         ThemeEditor(draft = draft.clone())
///         Button(label = "Copy as Rust".to_string(), on_click = {
///             let d = draft.clone();
///             Rc::new(move || { if let Some(src) = d.to_rust() { /* … */ } })
///         })
///     }
/// }
/// ```
#[component]
pub fn ThemeEditor(
    /// The tokens to edit. Build it with [`ThemeDraft::from_live`].
    /// Static: the draft's per-token signals carry every change, so
    /// swapping the draft itself is a remount, not an update.
    #[prop(static)]
    draft: ThemeDraft,
) -> Element {
    let namespaces = draft.namespaces();
    ui! {
        Stack(gap = StackGap::Lg) {
            for namespace in namespaces {
                TokenGroup(namespace = namespace.to_string(), draft = draft.clone())
            }
        }
    }
}

/// One namespace's section — a heading plus its rows.
#[component]
fn TokenGroup(
    /// The namespace to render (`color`, `intent`, `spacing`, …).
    #[prop(static)]
    namespace: String,
    /// The draft the rows read and commit into.
    #[prop(static)]
    draft: ThemeDraft,
) -> Element {
    // The group derives its own row list rather than being handed one,
    // so a caller can't pair a heading with the wrong rows.
    let names: Vec<String> = draft
        .entries()
        .iter()
        .filter(|e| e.namespace == namespace)
        .map(|e| e.name.to_string())
        .collect();
    ui! {
        Card() {
            Stack(gap = StackGap::Sm) {
                Typography(content = namespace.clone(), kind = typography_kind::H3)
                for name in names {
                    TokenRow(name = name, draft = draft.clone())
                }
            }
        }
    }
}

/// One token: a swatch when it carries a color, plus the input.
#[component]
fn TokenRow(
    /// The token this row edits.
    #[prop(static)]
    name: String,
    /// The draft to read the entry from and commit into.
    #[prop(static)]
    draft: ThemeDraft,
) -> Element {
    let Some(entry) = draft.find(&name) else {
        // A name with no entry renders nothing rather than panicking:
        // the panel is a dev tool, and a missing row is a far better
        // failure than taking the host app down with it.
        return Element::Fragment(Vec::new());
    };
    let token = entry.name;
    let text = entry.text;
    let is_color = entry.kind == DraftKind::Color;

    // Commit on every keystroke. `commit_text` parses first and leaves
    // the theme untouched on failure, so typing through an intermediate
    // state (`#f` on the way to `#f0f`) never repaints the app with it.
    //
    // `commit_text`, NOT `commit`: the `set` above only STAGES until the
    // world flushes, so a `commit` here would read the text from before
    // this keystroke and the theme would trail the input by one
    // character forever. The handler already has the new text.
    let on_change: Rc<dyn Fn(String)> = {
        let draft = draft.clone();
        Rc::new(move |next| {
            text.set(next.clone());
            let _ = draft.commit_text(token, &next);
        })
    };

    // "This doesn't parse", live. Reads the entry through the draft so
    // the closure owns no borrow of it.
    let error = {
        let draft = draft.clone();
        rx!(draft.find(token).and_then(|e| e.parsed().err()).map(|e| e.to_string()))
    };

    ui! {
        Stack(axis = StackAxis::Row, gap = StackGap::Sm, align = StackAlign::Center) {
            if is_color {
                ColorSwatch(name = token.to_string(), draft = draft.clone())
            }
            Field(
                label = Some(token.to_string()),
                value = text,
                on_change = on_change,
                error = error,
                size = FieldSize::Sm,
            )
        }
    }
}

/// The color preview beside a color row.
///
/// Paints the TOKEN, not the row's text: an inline `Tokenized::token`
/// reference resolves through the same registry the app paints from, so
/// the swatch shows what actually landed. A swatch driven from the
/// input's text would happily show a color the theme never took.
#[component]
fn ColorSwatch(
    /// The token to preview.
    #[prop(static)]
    name: String,
    /// The draft to read it from.
    #[prop(static)]
    draft: ThemeDraft,
) -> Element {
    let Some(entry) = draft.find(&name) else {
        return Element::Fragment(Vec::new());
    };
    let token = entry.name;
    let fallback = match &entry.initial {
        TokenValue::Color(c) => c.clone(),
        // Unreachable through `TokenRow`, which only builds a swatch for
        // a color row — a swatch with no color paints transparent
        // rather than panicking.
        _ => Color("transparent".into()),
    };
    let tint = swatch_color(token, &fallback);

    // The "unparseable" marker is an axis on the swatch's own sheet, not
    // an extra element: the panel is a long column, and a marker that
    // changes a row's box would reflow every row below it on each
    // keystroke.
    let style = move || {
        let invalid = draft.find(token).map(|e| e.parsed().is_err()).unwrap_or(false);
        StyleApplication::new(Swatch::sheet())
            .with("invalid", if invalid { "on" } else { "off" })
            .with_inline(StyleRules { background: Some(tint.clone()), ..Default::default() })
    };

    ui! { view(style = style) {} }
}
