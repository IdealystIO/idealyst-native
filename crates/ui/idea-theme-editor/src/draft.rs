//! The editor's working set — one editable entry per live token.
//!
//! [`ThemeDraft`] is the model, and it is deliberately independent of
//! the UI: it can be driven from a control panel, a hot-reload watcher,
//! or a test. The component in [`crate::editor`] is one consumer.

use std::collections::BTreeMap;
use std::rc::Rc;

use idea_theme::{token_descriptors, TokenEntry, TokenValue};
use runtime_core::{signal, update_tokens, Signal};

use crate::format::{
    format_value, parse_value, read_json, rust_color, rust_f32, rust_token_value, write_json,
    DraftKind, ParseError,
};

/// The namespace extension tokens are grouped under — they have no
/// accessor path to take a namespace from.
pub const EXTENSION_NAMESPACE: &str = "extension";

/// One editable token.
#[derive(Clone)]
pub struct DraftEntry {
    /// The registry key — what `update_tokens` writes and
    /// `token_value` reads.
    pub name: &'static str,
    /// The grouping an editor lays out by: the accessor namespace
    /// (`color`, `intent`, `spacing`, …) or [`EXTENSION_NAMESPACE`].
    pub namespace: &'static str,
    /// The `IdeaThemeDefaults` field this token reads from, when it has
    /// one — the thing [`ThemeDraft::to_rust`] assigns to. `None` for
    /// `radius-pill` (no field by design) and for every extension
    /// token (no accessor at all).
    pub field_path: Option<&'static str>,
    /// Which control this token wants, and how its text parses back.
    pub kind: DraftKind,
    /// The value when the draft was created. The baseline
    /// [`ThemeDraft::to_rust`] diffs against, so generated source is
    /// the edits rather than a restatement of the whole palette.
    pub initial: TokenValue,
    /// The control's live text. The editor's inputs write it; nothing
    /// reaches the theme until it is committed.
    pub text: Signal<String>,
}

impl DraftEntry {
    /// This entry's text as a token value, or why it can't be one.
    ///
    /// Reads the signal, so it reports on the last FLUSHED text — a
    /// reactive read (`rx!`) re-runs after the flush and sees the new
    /// value, which is what the row's error marker wants.
    pub fn parsed(&self) -> Result<TokenValue, ParseError> {
        parse_value(self.kind, &self.text.get())
    }

    /// Whether the entry's text still parses to the value it started
    /// at. Unparseable text counts as changed — it is certainly not
    /// the initial value.
    pub fn is_changed(&self) -> bool {
        self.parsed().map(|v| v != self.initial).unwrap_or(true)
    }
}

/// Every live token, as editable state.
///
/// Cheap to clone — entries live behind an `Rc` and the per-entry
/// signals are `Copy` handles, so every clone drives the same controls.
///
/// Requires an installed theme: [`ThemeDraft::from_live`] reads the
/// world's token table, which is empty until `install_idea_theme` (or
/// `install_theme`) has run.
#[derive(Clone, Default)]
pub struct ThemeDraft {
    entries: Rc<Vec<DraftEntry>>,
}

impl ThemeDraft {
    /// Read the live theme into a fresh draft.
    ///
    /// Covers the vocabulary in declaration order (so the editor lays
    /// out neutrals, intents, then the scales), then any remaining name
    /// the world holds — the extension tokens, which have no accessor
    /// and would otherwise be uneditable. A vocabulary token the world
    /// has no value for is skipped rather than invented: the editor
    /// shows what this app actually installed.
    ///
    /// Must be called where signal creation is legal (a component body
    /// or effect), since it creates one signal per token.
    pub fn from_live() -> Self {
        let mut entries = Vec::new();
        let mut seen: BTreeMap<&'static str, ()> = BTreeMap::new();

        for d in token_descriptors() {
            let Some(value) = runtime_core::token_value(d.name) else { continue };
            seen.insert(d.name, ());
            entries.push(DraftEntry {
                name: d.name,
                namespace: d.namespace,
                field_path: d.field_path,
                kind: d.kind.into(),
                text: signal(format_value(&value)),
                initial: value,
            });
        }

        // Extension tokens: in the world, absent from the vocabulary.
        // Sorted, because `token_names()` is hash-ordered and a control
        // panel that reshuffles between mounts is unusable.
        let mut extras: Vec<&'static str> =
            runtime_core::token_names().into_iter().filter(|n| !seen.contains_key(n)).collect();
        extras.sort_unstable();
        for name in extras {
            let Some(value) = runtime_core::token_value(name) else { continue };
            entries.push(DraftEntry {
                name,
                namespace: EXTENSION_NAMESPACE,
                field_path: None,
                kind: DraftKind::of(&value),
                text: signal(format_value(&value)),
                initial: value,
            });
        }

        Self { entries: Rc::new(entries) }
    }

    /// The entries, in layout order.
    pub fn entries(&self) -> &[DraftEntry] {
        &self.entries
    }

    /// The entry for `name`, if the draft has one.
    pub fn find(&self, name: &str) -> Option<&DraftEntry> {
        self.entries.iter().find(|e| e.name == name)
    }

    /// Entry names grouped by namespace, in layout order — what an
    /// editor lays sections out from.
    pub fn namespaces(&self) -> Vec<&'static str> {
        let mut out: Vec<&'static str> = Vec::new();
        for e in self.entries.iter() {
            if !out.contains(&e.namespace) {
                out.push(e.namespace);
            }
        }
        out
    }

    /// Push one entry's COMMITTED text to the live theme.
    ///
    /// A parse failure leaves the theme untouched and reports why —
    /// half-typed text in a control must not repaint the app.
    ///
    /// **Reads the signal, so it sees the last FLUSHED text.** Signal
    /// writes stage until the owning world flushes, so
    /// `entry.text.set(t); draft.commit(name)` in one turn applies the
    /// value from before `t` — a keystroke handler doing that would
    /// leave the theme one character behind the input forever. Use
    /// [`commit_text`](Self::commit_text) from an input handler, where
    /// the new text is in hand; use this when the text is already
    /// committed (a button press, a later turn).
    pub fn commit(&self, name: &str) -> Result<(), ParseError> {
        let entry = self.find(name).ok_or_else(|| ParseError::UnknownToken(name.to_string()))?;
        let value = entry.parsed()?;
        update_tokens(&[TokenEntry { name: entry.name, value }]);
        Ok(())
    }

    /// Push `text` to the live theme as `name`'s value, without reading
    /// the entry's signal.
    ///
    /// The input path: an `on_change` handler has the new text before
    /// the staged `set` is observable, so passing it in is what makes
    /// the edit land on THIS keystroke rather than the next one. See
    /// [`commit`](Self::commit) for the staging rule this sidesteps.
    pub fn commit_text(&self, name: &str, text: &str) -> Result<(), ParseError> {
        let entry = self.find(name).ok_or_else(|| ParseError::UnknownToken(name.to_string()))?;
        let value = parse_value(entry.kind, text)?;
        update_tokens(&[TokenEntry { name: entry.name, value }]);
        Ok(())
    }

    /// Push every entry that parses, and report the ones that don't.
    ///
    /// Reads the entries' committed text — see [`commit`](Self::commit)
    /// for what that means. Called from a button handler (a turn after
    /// the typing that staged the values), which is the normal case.
    ///
    /// One `update_tokens` call rather than one per entry: the call
    /// batches its signal fires, so a whole-theme apply re-runs each
    /// styled effect once instead of once per token it reads.
    pub fn commit_all(&self) -> Vec<(&'static str, ParseError)> {
        let mut batch = Vec::new();
        let mut failures = Vec::new();
        for e in self.entries.iter() {
            match e.parsed() {
                Ok(value) => batch.push(TokenEntry { name: e.name, value }),
                Err(err) => failures.push((e.name, err)),
            }
        }
        if !batch.is_empty() {
            update_tokens(&batch);
        }
        failures
    }

    /// Put every control back to the value the draft opened with, and
    /// apply it.
    pub fn revert(&self) {
        for e in self.entries.iter() {
            e.text.set(format_value(&e.initial));
        }
        let batch: Vec<TokenEntry> =
            self.entries.iter().map(|e| TokenEntry { name: e.name, value: e.initial.clone() }).collect();
        if !batch.is_empty() {
            update_tokens(&batch);
        }
    }

    /// The whole draft as a flat JSON object of `name → text`, in
    /// layout order.
    ///
    /// Every token, not just the edits: a save file is a theme, and one
    /// that carried only the diff would depend on the palette it was
    /// saved against still being what the reader loads it onto.
    pub fn to_json(&self) -> String {
        let pairs: Vec<(&str, String)> =
            self.entries.iter().map(|e| (e.name, e.text.get())).collect();
        write_json(&pairs)
    }

    /// Load a save file into the controls and apply it.
    ///
    /// Returns how many tokens were applied. Names the draft doesn't
    /// have are reported rather than ignored, so loading a file saved
    /// from a different theme tells you what was left behind instead of
    /// half-applying in silence.
    ///
    /// Nothing is written until the whole file parses: a malformed file
    /// leaves the theme exactly as it was.
    pub fn load_json(&self, src: &str) -> Result<usize, LoadReport> {
        let pairs = read_json(src).map_err(|e| LoadReport {
            applied: 0,
            error: Some(e),
            unknown: Vec::new(),
            invalid: Vec::new(),
        })?;

        // Resolve everything BEFORE touching a signal, so a file with a
        // bad value in the middle doesn't leave half a theme applied.
        let mut resolved = Vec::new();
        let mut unknown = Vec::new();
        let mut invalid = Vec::new();
        for (name, text) in &pairs {
            let Some(entry) = self.find(name) else {
                unknown.push(name.clone());
                continue;
            };
            match parse_value(entry.kind, text) {
                Ok(value) => resolved.push((entry.clone(), text.clone(), value)),
                Err(e) => invalid.push((name.clone(), e)),
            }
        }
        if !unknown.is_empty() || !invalid.is_empty() {
            return Err(LoadReport { applied: 0, error: None, unknown, invalid });
        }

        for (entry, text, _) in &resolved {
            entry.text.set(text.clone());
        }
        let batch: Vec<TokenEntry> =
            resolved.iter().map(|(e, _, v)| TokenEntry { name: e.name, value: v.clone() }).collect();
        if !batch.is_empty() {
            update_tokens(&batch);
        }
        Ok(batch.len())
    }

    /// The edits as Rust source, ready to paste into an app's theme
    /// setup.
    ///
    /// Only what CHANGED since the draft opened, and deliberately not a
    /// whole `light_theme()` restatement: the app's base theme is
    /// whatever it installed, which this crate cannot know, so the
    /// snippet assigns onto a `theme` binding the caller already has.
    ///
    /// Tokens with no theme field — `radius-pill`, and every extension
    /// token — can't be an assignment, so they are emitted as an
    /// `update_tokens` call after the assignments rather than dropped.
    ///
    /// Returns `None` when nothing has changed.
    pub fn to_rust(&self) -> Option<String> {
        let changed: Vec<(&DraftEntry, TokenValue)> = self
            .entries
            .iter()
            .filter_map(|e| e.parsed().ok().filter(|v| *v != e.initial).map(|v| (e, v)))
            .collect();
        if changed.is_empty() {
            return None;
        }

        let mut assignments = Vec::new();
        let mut loose = Vec::new();
        for (entry, value) in &changed {
            match (entry.field_path, value) {
                (Some(path), TokenValue::Color(c)) => {
                    assignments.push(format!("theme.{path} = {};", rust_color(c)));
                }
                // Only a px length is an `f32` field. A `%`/`auto`/
                // `full` value has no number to assign, so it routes to
                // the loose block instead of generating source that
                // doesn't compile.
                (Some(path), TokenValue::Length(runtime_core::Length::Px(n))) => {
                    assignments.push(format!("theme.{path} = {};", rust_f32(*n)));
                }
                _ => loose.push((entry.name, value.clone())),
            }
        }

        let mut out = String::new();
        out.push_str(
            "// Generated by idea-theme-editor. These are the EDITS — assign them\n\
             // onto the theme this app already installs, then install it.\n",
        );
        for line in &assignments {
            out.push('\n');
            out.push_str(line);
        }
        if !assignments.is_empty() {
            out.push('\n');
        }
        if !loose.is_empty() {
            out.push_str(
                "\n// No theme field backs these, so they are applied directly:\n\
                 // `radius-pill` is Length::Full by design, and extension tokens\n\
                 // (a `tone!`'s `tokens = [...]`) have no accessor at all.\n\
                 update_tokens(&[\n",
            );
            for (name, value) in &loose {
                out.push_str(&format!(
                    "    TokenEntry {{ name: {name:?}, value: {} }},\n",
                    rust_token_value(value)
                ));
            }
            out.push_str("]);\n");
        }
        Some(out)
    }
}

/// What went wrong loading a save file — and, when nothing did, how
/// much was applied.
///
/// Carries the specifics rather than one flattened message because a
/// theme editor's most common load failure is a file from a DIFFERENT
/// theme, and "these six names aren't in this theme" is the actionable
/// form of that.
#[derive(Clone, Debug, PartialEq)]
pub struct LoadReport {
    /// How many tokens were applied. Zero on any failure — a load is
    /// all-or-nothing.
    pub applied: usize,
    /// The file didn't parse at all.
    pub error: Option<ParseError>,
    /// Names in the file that this theme has no token for.
    pub unknown: Vec<String>,
    /// Names whose value couldn't be read as that token's kind.
    pub invalid: Vec<(String, ParseError)>,
}

impl std::fmt::Display for LoadReport {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        if let Some(e) = &self.error {
            return write!(f, "{e}");
        }
        if !self.unknown.is_empty() {
            write!(f, "not in this theme: {}", self.unknown.join(", "))?;
        }
        if !self.invalid.is_empty() {
            if !self.unknown.is_empty() {
                write!(f, "; ")?;
            }
            let parts: Vec<String> =
                self.invalid.iter().map(|(n, e)| format!("{n} ({e})")).collect();
            write!(f, "unreadable: {}", parts.join(", "))?;
        }
        Ok(())
    }
}

impl std::error::Error for LoadReport {}
