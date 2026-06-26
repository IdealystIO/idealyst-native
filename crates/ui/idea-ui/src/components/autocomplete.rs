//! `Autocomplete` — a searchable combobox: a text input that filters a
//! list of [`SelectOption`]s as you type, with an anchored dropdown of
//! matches. The constrained-selection sibling of [`Select`](crate::Select)
//! — the value committed to the bound signal is always one of the
//! `options`' `id`s, never free text.
//!
//! ```ignore
//! ui! {
//!     Autocomplete(
//!         value = value,
//!         on_change = on_change,
//!         placeholder = "Search fruit…",
//!         options = vec![
//!             SelectOption::new("apple", "Apple"),
//!             SelectOption::new("pear", "Pear"),
//!             SelectOption::new("plum", "Plum"),
//!         ],
//!     )
//! }
//! ```
//!
//! Behaviour:
//! - Typing filters the menu (case-insensitive substring on the option
//!   label) and opens it.
//! - `ArrowDown`/`ArrowUp` move the keyboard highlight, `Enter` commits the
//!   highlighted row, `Escape` closes and reverts the text to the committed
//!   selection's label.
//! - The chevron toggles the menu open and focuses the input.
//! - Picking a row (click or `Enter`) fires `on_change` with the option's
//!   `id` and shows its label.
//! - Dismissing the menu without choosing (tap-away / `Escape`) reverts the
//!   typed text to the committed selection, so the input can never be left
//!   showing a string that isn't a valid option.
//!
//! The dropdown deliberately reuses `Select`'s menu/row styling so the two
//! controls drop visually identical menus.
//!
//! Rationale for the input-carries-the-chrome layout (vs. a bordered
//! wrapper around a bare input): the native focus ring must land on the
//! *focusable* element, so the bordered box lives on the `text_input`
//! itself — exactly like [`Field`](crate::Field) — and the chevron is
//! absolutely positioned over its right edge. A bordered wrapper would draw
//! a ring that never lights up, because focus is on the inner input.

use std::rc::Rc;

use runtime_core::primitives::key::{KeyEvent, KeyOutcome};
use runtime_core::primitives::overlay::BackdropMode;
use runtime_core::primitives::portal::{AnchorTarget, ElementAlign, ElementSide};
use runtime_core::{
    component, each_keyed, memo, on_defer, pressable, signal, text, text_input, ui, view, when,
    EachKey, EachRowBuild, Element, IdealystSchema, IntoElement, Reactive, Ref, Signal,
    StyleApplication, TextInputHandle, VariantEnum, ViewHandle,
};

use idea_theme::theme::IdeaThemeRef;

use crate::components::select::{SelectOption, SelectSize};
use crate::stylesheets::{
    AutocompleteBox, AutocompleteChevron, AutocompleteEmpty, AutocompleteInput, SelectMenu,
    SelectOption as SelectOptionStyle,
};

/// Disclosure caret glyph (▾) shown at the input's right edge.
const CHEVRON: &str = "\u{25BE}";

/// Default text shown in the menu when nothing matches the query.
const DEFAULT_EMPTY_TEXT: &str = "No results";

// Reactive-by-default: `#[props]` rewrites each scalar-DATA field `T` →
// `Reactive<T>`. AUTO-SKIPPED: `value` (a `Signal` reactive source),
// `on_change` (an `Rc` handler), and `options` (a `Vec` LIST). `size` routes
// into the reactive `input_style` sink; `placeholder` routes to the
// `text_input`'s reactive placeholder. `empty_text` feeds the dropdown's
// empty-state row — structural list content (see the TODO in the body).
#[runtime_core::props]
#[derive(IdealystSchema)]
pub struct AutocompleteProps {
    /// Controlled selected value — the `id` of the chosen [`SelectOption`].
    /// The host owns the signal; committing a row sets it via `on_change`.
    /// The input text always reflects the matching option's label (reverting
    /// any unmatched typing on dismiss), so this stays one of `options`' ids.
    pub value: Signal<String>,
    /// Fires with the chosen option's `id` when the user commits a row.
    pub on_change: Rc<dyn Fn(String)>,
    /// The rows to offer; the typed query filters this list by label.
    pub options: Vec<SelectOption>,
    /// Input height/density. Default `Md`. Shared with [`Select`](crate::Select).
    pub size: SelectSize,
    /// Placeholder shown when the input is empty. `Reactive<Option<String>>`.
    pub placeholder: Option<String>,
    /// Text shown in the menu when no option matches the query. Defaults to
    /// "No results".
    pub empty_text: Option<String>,
}

impl Default for AutocompleteProps {
    fn default() -> Self {
        Self {
            value: Signal::new(String::new()),
            on_change: Rc::new(|_| {}),
            options: Vec::new(),
            size: Reactive::Static(SelectSize::default()),
            placeholder: Reactive::Static(None),
            empty_text: Reactive::Static(None),
        }
    }
}

/// Indices into `labels` that match `query`, in original order.
///
/// Filtering is a case-insensitive substring match on the (trimmed) query.
/// An empty query matches everything. The one subtlety:
/// `selected_label` — when the query *equals* the currently-committed
/// option's label, the field is showing an untouched selection rather than a
/// search, so we treat it as an empty query and list ALL options. That makes
/// reopening a combobox with a value show the whole menu (with the selection
/// highlighted), not just the single row whose label happens to contain
/// itself. Pulled out as a pure function so the filter behaviour is unit
/// tested without a backend.
pub(crate) fn filter_indices(
    labels: &[String],
    query: &str,
    selected_label: Option<&str>,
) -> Vec<usize> {
    let showing_selection = selected_label == Some(query);
    let q = if showing_selection { "" } else { query.trim() };
    if q.is_empty() {
        return (0..labels.len()).collect();
    }
    let q = q.to_lowercase();
    labels
        .iter()
        .enumerate()
        .filter(|(_, l)| l.to_lowercase().contains(&q))
        .map(|(i, _)| i)
        .collect()
}

/// Renders a searchable combobox: a `text_input` that filters an anchored
/// dropdown of [`SelectOption`] rows, with keyboard navigation and
/// constrained (id-only) selection.
#[component]
pub fn Autocomplete(props: AutocompleteProps) -> Element {
    let value = props.value;
    let on_change = props.on_change.clone();
    let size = props.size.clone();
    let placeholder = props.placeholder.clone();
    // TODO(reactive-sweep): `empty_text` is snapshotted here and moved into the
    // `when`/`each_keyed` dropdown closures that build the empty-state row
    // (structural list content). Routing a live `empty_text` would need the
    // empty row to read it from a `Reactive` source inside the `each_keyed`
    // build — flagged, not routed.
    let empty_text = props
        .empty_text
        .get()
        .unwrap_or_else(|| DEFAULT_EMPTY_TEXT.to_string());
    let options = Rc::new(props.options);

    // --- internal state -----------------------------------------------------
    // `query` is the text in the input; it doubles as the filter. Seed it
    // from the initial committed selection so the input shows the chosen
    // label on first paint (external changes are synced via `on_defer` below).
    let initial_query = options
        .iter()
        .find(|o| o.id == value.get())
        .map(|o| o.label.get())
        .unwrap_or_default();
    let query: Signal<String> = signal!(initial_query);
    let open: Signal<bool> = signal!(false);
    // Keyboard highlight as a position into the *current* filtered list.
    let highlight: Signal<usize> = signal!(0usize);

    let input_ref: Ref<TextInputHandle> = Ref::new();
    let wrapper_ref: Ref<ViewHandle> = Ref::new();

    // Filtered option indices, recomputed when the query, the committed
    // value, or any option label changes.
    let filtered: Signal<Vec<usize>> = {
        let options = options.clone();
        memo(move || {
            let q = query.get();
            let labels: Vec<String> = options.iter().map(|o| o.label.get()).collect();
            let sel_id = value.get();
            let sel_label = options.iter().find(|o| o.id == sel_id).map(|o| o.label.get());
            filter_indices(&labels, &q, sel_label.as_deref())
        })
    };

    // Commit option `oi`: report its id, show its label, close the menu.
    let commit: Rc<dyn Fn(usize)> = {
        let options = options.clone();
        let on_change = on_change.clone();
        Rc::new(move |oi: usize| {
            if let Some(o) = options.get(oi) {
                (on_change)(o.id.clone());
                query.set(o.label.get());
                open.set(false);
            }
        })
    };

    // Revert the typed text to the committed selection's label (used on
    // Escape / tap-away so an unmatched query never lingers).
    let revert: Rc<dyn Fn()> = {
        let options = options.clone();
        Rc::new(move || {
            let label = options
                .iter()
                .find(|o| o.id == value.get())
                .map(|o| o.label.get())
                .unwrap_or_default();
            query.set(label);
        })
    };

    // Keep the input text in sync when the host changes `value` out of band
    // (skips the initial run — we seeded `query` above). Body is untracked,
    // so reading `open` here doesn't subscribe; we skip while the menu is
    // open to avoid clobbering active typing.
    let _sync = {
        let options = options.clone();
        on_defer(value, move |new_id, _| {
            if open.get() {
                return;
            }
            let label = options
                .iter()
                .find(|o| &o.id == new_id)
                .map(|o| o.label.get())
                .unwrap_or_default();
            query.set(label);
        })
    };

    // --- input --------------------------------------------------------------
    // `size` is read LIVE inside the style closure so a reactive `size`
    // re-resolves the input height in place; a `Static` one keeps the
    // build-time fast path (no per-input apply-style Effect).
    let size_is_reactive = !size.is_static();
    let make_input_style = {
        let size = size.clone();
        move || {
            StyleApplication::new(AutocompleteInput::sheet())
                .with("size", size.get().as_variant_str().to_string())
        }
    };

    let key_commit = commit.clone();
    let key_revert = revert.clone();
    let mut input = text_input(query, move |v: String| {
        query.set(v);
        open.set(true);
        highlight.set(0);
    })
    .bind(input_ref)
    // `placeholder` is routed LIVE: a reactive source updates the native
    // placeholder in place; a `Static` one sets it once.
    .placeholder_reactive(placeholder);
    let input = input
        .on_key_down(move |e: &KeyEvent| match e.key.as_str() {
            "ArrowDown" => {
                open.set(true);
                let len = filtered.get().len();
                if len > 0 {
                    highlight.update(|h| *h = (*h + 1).min(len - 1));
                }
                KeyOutcome::PreventDefault
            }
            "ArrowUp" => {
                highlight.update(|h| *h = h.saturating_sub(1));
                KeyOutcome::PreventDefault
            }
            "Enter" => {
                if open.get() {
                    let idxs = filtered.get();
                    if !idxs.is_empty() {
                        let pos = highlight.get().min(idxs.len() - 1);
                        (key_commit)(idxs[pos]);
                    }
                    KeyOutcome::PreventDefault
                } else {
                    KeyOutcome::Default
                }
            }
            "Escape" => {
                if open.get() {
                    open.set(false);
                    (key_revert)();
                    KeyOutcome::PreventDefault
                } else {
                    KeyOutcome::Default
                }
            }
            _ => KeyOutcome::Default,
        });
    // Reactive when `size` is live (re-resolves the input height in place);
    // else the build-time fast path.
    let input_node = if size_is_reactive {
        input.with_style(make_input_style).into_element()
    } else {
        input.with_style(make_input_style()).into_element()
    };

    // --- chevron ------------------------------------------------------------
    let chevron = pressable(vec![text(CHEVRON.to_string()).into_element()], move || {
        let now = !open.get();
        open.set(now);
        if now {
            if let Some(h) = input_ref.get() {
                h.focus();
            }
        }
    })
    .with_style(AutocompleteChevron())
    .into_element();

    let wrapper = view(vec![input_node, chevron])
        .with_style(AutocompleteBox())
        .bind(wrapper_ref)
        .into_element();

    // --- dropdown -----------------------------------------------------------
    let menu_options = options.clone();
    let menu_commit = commit.clone();
    let menu_revert = revert.clone();
    let panel = when(
        move || open.get(),
        move || {
            let snapshot_options = menu_options.clone();
            let snapshot_commit = menu_commit.clone();
            let empty_text = empty_text.clone();
            let rows = each_keyed(move || {
                let idxs = filtered.get();
                if idxs.is_empty() {
                    let empty_text = empty_text.clone();
                    let build: EachRowBuild = Box::new(move || {
                        vec![text(empty_text).with_style(AutocompleteEmpty()).into_element()]
                    });
                    return vec![(EachKey::new("__empty".to_string()), build)];
                }
                idxs.iter()
                    .map(|&oi| {
                        let o = snapshot_options[oi].clone();
                        let key = EachKey::new(o.id.clone());
                        let commit_row = snapshot_commit.clone();
                        let build: EachRowBuild =
                            Box::new(move || vec![row(o, oi, commit_row, filtered, highlight, value)]);
                        (key, build)
                    })
                    .collect()
            });
            let menu = ui! { view(style = SelectMenu()) { rows } };
            let dismiss_revert = menu_revert.clone();
            runtime_core::anchored_overlay(AnchorTarget::from(wrapper_ref), vec![menu])
                .side(ElementSide::Below)
                .align(ElementAlign::Start)
                .offset(4.0)
                .backdrop(BackdropMode::None)
                .trap_focus(false)
                .on_dismiss(move || {
                    open.set(false);
                    (dismiss_revert)();
                })
                .into_element()
        },
        || ui! { view {} }.into_element(),
    );

    ui! {
        view {
            wrapper
            panel
        }
    }
}

/// One menu row. The "active" highlight is resolved reactively from the
/// *current* filtered list (`filtered[highlight] == this option`) plus the
/// committed selection, so moving the keyboard cursor or filtering the list
/// re-styles rows in place without rebuilding them.
fn row(
    o: SelectOption,
    oi: usize,
    commit: Rc<dyn Fn(usize)>,
    filtered: Signal<Vec<usize>>,
    highlight: Signal<usize>,
    value: Signal<String>,
) -> Element {
    let id_for_style = o.id.clone();
    let label = o.label.clone();
    pressable(vec![text(label).into_element()], move || (commit)(oi))
        .with_style(move || {
            let _ = idea_theme::active_theme_untracked()
                .downcast_ref::<IdeaThemeRef>()
                .expect("idea-ui: no IdeaTheme installed — call install_idea_theme(...) first");
            let highlighted = filtered.get().get(highlight.get()).copied() == Some(oi);
            let selected = value.get() == id_for_style;
            let variant = if highlighted || selected { "on" } else { "off" };
            StyleApplication::new(SelectOptionStyle::sheet()).with("active", variant.to_string())
        })
        .into_element()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn labels(v: &[&str]) -> Vec<String> {
        v.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn empty_query_matches_everything() {
        let ls = labels(&["Apple", "Pear", "Plum"]);
        assert_eq!(filter_indices(&ls, "", None), vec![0, 1, 2]);
        // Whitespace-only is also "empty" after trim.
        assert_eq!(filter_indices(&ls, "   ", None), vec![0, 1, 2]);
    }

    #[test]
    fn substring_match_is_case_insensitive() {
        let ls = labels(&["Apple", "Pear", "Pineapple"]);
        // "ap" hits Apple + Pineapple, not Pear.
        assert_eq!(filter_indices(&ls, "ap", None), vec![0, 2]);
        // Case folds both ways.
        assert_eq!(filter_indices(&ls, "PEAR", None), vec![1]);
    }

    #[test]
    fn no_match_returns_empty() {
        let ls = labels(&["Apple", "Pear"]);
        assert!(filter_indices(&ls, "zzz", None).is_empty());
    }

    // The committed-selection special case: when the query equals the
    // selected option's label, the field is showing an untouched selection,
    // so the whole list comes back (not just the self-containing row).
    #[test]
    fn query_equal_to_selected_label_shows_all() {
        let ls = labels(&["Apple", "Pear", "Plum"]);
        assert_eq!(
            filter_indices(&ls, "Apple", Some("Apple")),
            vec![0, 1, 2],
            "reopening a combobox on its committed value should list every option"
        );
    }

    // ...but once the user edits away from the committed label, normal
    // filtering resumes even though a selection exists.
    #[test]
    fn editing_away_from_selection_filters_normally() {
        let ls = labels(&["Apple", "Pear", "Plum"]);
        assert_eq!(filter_indices(&ls, "plu", Some("Apple")), vec![2]);
    }

    // The whole reactive tree (seeded query signal, filter memo, input +
    // chevron, `when` panel, external-value `on_defer`) must build without
    // panicking. Guards against a regression where any of those wiring steps
    // touches the arena/scope in a way that aborts at construction. Renders
    // to the closed (`open == false`) state: a wrapper view + the `when`
    // panel placeholder.
    #[test]
    fn builds_collapsed_tree() {
        idea_theme::theme::install_idea_theme(idea_theme::theme::light_theme());
        let value = Signal::new("pear".to_string());
        let props = AutocompleteProps {
            value,
            options: vec![
                SelectOption::new("apple", "Apple"),
                SelectOption::new("pear", "Pear"),
            ],
            placeholder: Reactive::Static(Some("Search…".to_string())),
            ..Default::default()
        };
        let tree = Autocomplete(props);
        match tree {
            Element::View { children, .. } => {
                assert_eq!(children.len(), 2, "wrapper view + dropdown panel");
            }
            _ => panic!("Autocomplete renders a view wrapper"),
        }
    }
}
