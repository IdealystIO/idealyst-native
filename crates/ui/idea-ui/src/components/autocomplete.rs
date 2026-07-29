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
//! - Focusing the input opens the menu (with the full option list when the
//!   text equals the committed selection's label); losing focus — Tab away,
//!   click elsewhere — closes it and reverts unmatched typing.
//! - Opening seeds the keyboard cursor on the committed selection (typing
//!   re-seeds it to the top row — the filter changed). The committed row and
//!   the cursor row are styled differently: solid (`active: on`) vs the
//!   subtle hover surface (`active: cursor`).
//! - Typing filters the menu (case-insensitive substring on the option
//!   label) and opens it.
//! - `ArrowDown`/`ArrowUp` move the keyboard cursor, `Enter` commits the
//!   cursor row, `Escape` closes and reverts the text to the committed
//!   selection's label.
//! - The chevron toggles the menu open and focuses the input.
//! - Picking a row (click or `Enter`) fires `on_change` with the option's
//!   `id` and shows its label.
//! - Dismissing the menu without choosing (blur / `Escape`) reverts the
//!   typed text to the committed selection, so the input can never be left
//!   showing a string that isn't a valid option.
//!
//! The dropdown deliberately reuses `Select`'s menu/row styling so the two
//! controls drop visually identical menus, with two combobox-specific
//! adjustments (see `menu_panel::combobox_menu_panel`): the panel's width is
//! floored at the input's width so filtering doesn't resize it under the
//! field, and the panel + chevron are `preserves_focus` press regions so
//! interacting with them never blurs the input (which is what makes the
//! close-on-blur above safe — a row press can't dismiss-and-unmount the row
//! before its own click commits).
//!
//! Rationale for the input-carries-the-chrome layout (vs. a bordered
//! wrapper around a bare input): the native focus ring must land on the
//! *focusable* element, so the bordered box lives on the `text_input`
//! itself — exactly like [`Field`](crate::Field) — and the chevron is
//! absolutely positioned over its right edge. A bordered wrapper would draw
//! a ring that never lights up, because focus is on the inner input.

use std::rc::Rc;

use idea_theme::compat::SignalModify as _;
use runtime_core::primitives::key::{KeyEvent, KeyOutcome};
use runtime_core::primitives::overlay::BackdropMode;
use runtime_core::primitives::portal::{AnchorTarget, ElementAlign, ElementSide};
use runtime_core::{
    component, each_keyed, memo, on_defer, pressable, signal, text, text_input, ui, view, when,
    EachKey, EachRowBuild, Element, IdealystSchema, IntoElement, Reactive, ReadSignal, Ref, Signal,
    StyleApplication, TextInputHandle, VariantEnum, ViewHandle,
};

use idea_theme::theme::IdeaThemeRef;

use crate::components::select::{SelectOption, SelectSize};
use crate::stylesheets::{
    AutocompleteBox, AutocompleteChevron, AutocompleteEmpty, AutocompleteInput,
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
            value: runtime_core::signal(String::new()),
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

/// The keyboard-cursor seed when the menu OPENS (focus / chevron /
/// ArrowDown-from-closed): the committed selection's position within the
/// current filtered list, falling back to the top row when the selection
/// isn't among the matches (or nothing is committed). Typing instead
/// re-seeds the cursor to 0 — the filter just changed, so the previous
/// position is meaningless. Pulled out as a pure function so the
/// regression (opening always put the cursor on row 0, painting the top
/// row highlighted regardless of the selection) stays unit-tested without
/// a backend.
pub(crate) fn initial_highlight(
    filtered: &[usize],
    options: &[SelectOption],
    selected_id: &str,
) -> usize {
    filtered
        .iter()
        .position(|&oi| options.get(oi).map(|o| o.id == selected_id).unwrap_or(false))
        .unwrap_or(0)
}

/// Renders a searchable combobox: a `text_input` that filters an anchored
/// dropdown of [`SelectOption`] rows, with keyboard navigation and
/// constrained (id-only) selection.
///
/// **Cargo features:** requires `prim-text-input` + `prim-portal` (both in idea-ui's
/// default set). A restricted `--primitives` / `default-features = false`
/// build without them compiles this component out, so using it is a
/// compile error naming the missing feature — see the 0.4→0.5
/// migration guide.
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
    let query: Signal<String> = signal(initial_query);
    let open: Signal<bool> = signal(false);
    // Keyboard highlight as a position into the *current* filtered list.
    let highlight: Signal<usize> = signal(0usize);

    let input_ref: Ref<TextInputHandle> = Ref::new();
    let wrapper_ref: Ref<ViewHandle> = Ref::new();

    // Filtered option indices, recomputed when the query, the committed
    // value, or any option label changes.
    // (No type annotation: `memo` yields `ReadSignal` on the old core
    // and `Memo` on the new one — both `Copy` handles with `.get()`.)
    let filtered = {
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

    // Open the menu with the keyboard cursor seeded on the committed
    // selection — opening a combobox highlights what's already chosen, not
    // the first row. Shared by every OPEN path (focus, chevron,
    // ArrowDown-from-closed); typing bypasses it and resets the cursor to
    // the top instead, because the filter just changed.
    let open_menu: Rc<dyn Fn()> = {
        let options = options.clone();
        Rc::new(move || {
            open.set(true);
            highlight.set(initial_highlight(&filtered.get(), &options, &value.get()));
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
    let blur_revert = revert.clone();
    let input = text_input(query, move |v: String| {
        query.set(v);
        open.set(true);
        highlight.set(0);
    })
    .bind(input_ref)
    // `placeholder` is routed LIVE: a reactive source updates the native
    // placeholder in place; a `Static` one sets it once.
    .placeholder_reactive(placeholder);
    let input = input
        // Focus opens the menu (a combobox invites browsing the moment the
        // field activates); losing focus dismisses + reverts, same as
        // Escape / tap-away. This close-on-blur is only safe because every
        // press surface belonging to the widget — the chevron and the
        // anchored menu — is `preserves_focus`-marked, so pressing them
        // never blurs the input: a blur here is always a GENUINE departure
        // (Tab away, click elsewhere), never a mid-commit row press about
        // to be unmounted by its own dismissal.
        .on_focus({
            let open_menu = open_menu.clone();
            move |focused: bool| {
                if focused {
                    (open_menu)();
                } else {
                    open.set(false);
                    (blur_revert)();
                }
            }
        })
        .on_key_down({
            let open_menu = open_menu.clone();
            move |e: &KeyEvent| match e.key.as_str() {
            "ArrowDown" => {
                if !open.get() {
                    // Opening keystroke: seed the cursor on the selection;
                    // the NEXT ArrowDown starts moving it.
                    (open_menu)();
                } else {
                    let len = filtered.get().len();
                    if len > 0 {
                        highlight.modify(|h| *h = (*h + 1).min(len - 1));
                    }
                }
                KeyOutcome::PreventDefault
            }
            "ArrowUp" => {
                highlight.modify(|h| *h = h.saturating_sub(1));
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
        }});
    // Reactive when `size` is live (re-resolves the input height in place);
    // else the build-time fast path.
    let input_node = if size_is_reactive {
        input.with_style(make_input_style).into_element()
    } else {
        input.with_style(make_input_style()).into_element()
    };

    // --- chevron ------------------------------------------------------------
    // `preserves_focus`: pressing the chevron must not blur the input — a
    // blur would close-on-blur the menu BEFORE this toggle runs, flipping
    // `open` back to true and breaking "chevron closes an open menu".
    let chevron = pressable(vec![text(CHEVRON.to_string()).into_element()], {
        let open_menu = open_menu.clone();
        move || {
            if open.get() {
                open.set(false);
            } else {
                (open_menu)();
                if let Some(h) = input_ref.get() {
                    h.focus();
                }
            }
        }
    })
    .preserves_focus(true)
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
                            Box::new(move || {
                                vec![row(o, oi, commit_row, move || filtered.get(), highlight, value)]
                            });
                        (key, build)
                    })
                    .collect()
            });
            // Cap + scroll the filtered list so a long set of matches scrolls
            // within a bounded panel instead of running off the viewport. The
            // combobox shape additionally floors the panel's width at the
            // input's (so filtering doesn't make it jump) and marks it
            // focus-preserving (so row presses don't blur the input — see
            // the input's `on_focus` close-on-blur).
            let menu = crate::components::menu_panel::combobox_menu_panel(
                vec![rows],
                AnchorTarget::from(wrapper_ref),
            );
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

/// One menu row. The `active` variant is resolved reactively from the
/// *current* filtered list (`filtered[highlight] == this option`) plus the
/// committed selection, so moving the keyboard cursor or filtering the list
/// re-styles rows in place without rebuilding them. Selection and cursor
/// are DISTINCT looks: the committed row paints solid (`on`), the keyboard
/// cursor paints the subtle hover surface (`cursor`) — an open menu never
/// shows two solid rows. When the cursor rests on the committed row (the
/// seeded state right after opening), `on` wins.
fn row(
    o: SelectOption,
    oi: usize,
    commit: Rc<dyn Fn(usize)>,
    // A tracked getter rather than a `ReadSignal` handle: `memo` yields
    // `ReadSignal` on the old core and `Memo` on the new one, and the
    // row only ever `.get()`s — the closure form is the shared shape.
    filtered: impl Fn() -> Vec<usize> + 'static,
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
            let highlighted = filtered().get(highlight.get()).copied() == Some(oi);
            let selected = value.get() == id_for_style;
            let variant = if selected {
                "on"
            } else if highlighted {
                "cursor"
            } else {
                "off"
            };
            StyleApplication::new(SelectOptionStyle::sheet()).with("active", variant.to_string())
        })
        .into_element()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::{classify, P, TStyle};
    use idea_theme::testing::with_test_world;

    fn labels(v: &[&str]) -> Vec<String> {
        v.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn empty_query_matches_everything() {
        with_test_world(|| {
            let ls = labels(&["Apple", "Pear", "Plum"]);
            assert_eq!(filter_indices(&ls, "", None), vec![0, 1, 2]);
            // Whitespace-only is also "empty" after trim.
            assert_eq!(filter_indices(&ls, "   ", None), vec![0, 1, 2]);
    });
    }

    #[test]
    fn substring_match_is_case_insensitive() {
        with_test_world(|| {
            let ls = labels(&["Apple", "Pear", "Pineapple"]);
            // "ap" hits Apple + Pineapple, not Pear.
            assert_eq!(filter_indices(&ls, "ap", None), vec![0, 2]);
            // Case folds both ways.
            assert_eq!(filter_indices(&ls, "PEAR", None), vec![1]);
    });
    }

    #[test]
    fn no_match_returns_empty() {
        with_test_world(|| {
            let ls = labels(&["Apple", "Pear"]);
            assert!(filter_indices(&ls, "zzz", None).is_empty());
    });
    }

    // The committed-selection special case: when the query equals the
    // selected option's label, the field is showing an untouched selection,
    // so the whole list comes back (not just the self-containing row).
    #[test]
    fn query_equal_to_selected_label_shows_all() {
        with_test_world(|| {
            let ls = labels(&["Apple", "Pear", "Plum"]);
            assert_eq!(
                filter_indices(&ls, "Apple", Some("Apple")),
                vec![0, 1, 2],
                "reopening a combobox on its committed value should list every option"
            );
    });
    }

    // ...but once the user edits away from the committed label, normal
    // filtering resumes even though a selection exists.
    #[test]
    fn editing_away_from_selection_filters_normally() {
        with_test_world(|| {
            let ls = labels(&["Apple", "Pear", "Plum"]);
            assert_eq!(filter_indices(&ls, "plu", Some("Apple")), vec![2]);
    });
    }

    /// Destructure a built Autocomplete into the pieces the behavioral
    /// regressions poke at: the input's `value` signal + `on_change` +
    /// `on_focus` notifier, the chevron pressable, and the dropdown
    /// `when`'s live `open` condition.
    ///
    /// Old-core only: the mirror collapses `on_focus` to presence (`bool`)
    /// and the dropdown `when`'s live condition is an opaque `Dyn` hole on
    /// the new core, so the invoke-the-handlers behavioral tests below
    /// can't be expressed through the build tree there.
    #[cfg(not(feature = "new-core"))]
    #[allow(clippy::type_complexity)]
    fn dissect(
        tree: Element,
    ) -> (
        Signal<String>,
        Rc<dyn Fn(String)>,
        Rc<dyn Fn(bool)>,
        Element,
        Rc<dyn Fn() -> bool>,
    ) {
        let Element::View { children, .. } = tree else {
            panic!("Autocomplete renders a view wrapper");
        };
        let mut children = children.into_iter();
        let wrapper = children.next().expect("wrapper view");
        let panel = children.next().expect("dropdown panel");
        let Element::View { children: wrapper_children, .. } = wrapper else {
            panic!("first child is the input+chevron wrapper view");
        };
        let mut wrapper_children = wrapper_children.into_iter();
        let input = wrapper_children.next().expect("text input");
        let chevron = wrapper_children.next().expect("chevron pressable");
        let Element::TextInput { value, on_change, on_focus, .. } = input else {
            panic!("wrapper's first child is the text_input");
        };
        let on_focus = on_focus.expect("Autocomplete must install an on_focus notifier");
        let Element::When { cond, .. } = panel else {
            panic!("dropdown panel is a `when`");
        };
        (value, on_change, on_focus, chevron, cond.compute)
    }

    // REGRESSION: focusing the input did nothing — the menu only opened on
    // typing / ArrowDown / the chevron. A combobox must invite browsing the
    // moment the field activates.
    // Old-core only: drives the on_focus handler + reads the `when` cond —
    // both opaque through the new-core build tree (see `dissect`).
    #[cfg(not(feature = "new-core"))]
    #[test]
    fn regression_focusing_the_input_opens_the_menu() {
        with_test_world(|| {
            idea_theme::theme::install_idea_theme(idea_theme::theme::light_theme());
            let props = AutocompleteProps {
                value: runtime_core::signal("pear".to_string()),
                options: vec![
                    SelectOption::new("apple", "Apple"),
                    SelectOption::new("pear", "Pear"),
                ],
                ..Default::default()
            };
            let (_query, _on_change, on_focus, _chevron, open) = dissect(Autocomplete(props));

            assert!(!open(), "menu starts closed");
            (on_focus)(true);
            assert!(open(), "focusing the input must open the menu");
    });
    }

    // REGRESSION: blurring the input left the menu dangling open (only
    // Escape / a committed row closed it), and the typed filter text
    // lingered. Losing focus must dismiss AND revert unmatched typing to
    // the committed selection's label — same contract as Escape.
    // Old-core only: drives the on_focus handler + reads the `when` cond —
    // both opaque through the new-core build tree (see `dissect`).
    #[cfg(not(feature = "new-core"))]
    #[test]
    fn regression_blurring_the_input_closes_the_menu_and_reverts() {
        with_test_world(|| {
            idea_theme::theme::install_idea_theme(idea_theme::theme::light_theme());
            let props = AutocompleteProps {
                value: runtime_core::signal("pear".to_string()),
                options: vec![
                    SelectOption::new("apple", "Apple"),
                    SelectOption::new("pear", "Pear"),
                ],
                ..Default::default()
            };
            let (query, on_change, on_focus, _chevron, open) = dissect(Autocomplete(props));

            (on_focus)(true);
            (on_change)("zzz".to_string()); // user types an unmatched filter
            assert!(open());
            assert_eq!(query.get(), "zzz");

            (on_focus)(false);
            assert!(!open(), "blurring the input must close the menu");
            assert_eq!(
                query.get(),
                "Pear",
                "blur must revert unmatched typing to the committed selection's label"
            );
    });
    }

    // The close-on-blur above is only safe because the widget's press
    // surfaces never blur the input: the chevron must carry the
    // `preserves_focus` mark (the menu panel's mark is pinned by
    // `menu_panel::tests`). Without it, pressing the chevron blurs →
    // close-on-blur flips `open` → the chevron's toggle re-opens instead
    // of closing.
    #[test]
    fn chevron_is_a_focus_preserving_pressable() {
        with_test_world(|| {
            idea_theme::theme::install_idea_theme(idea_theme::theme::light_theme());
            let props = AutocompleteProps {
                options: vec![SelectOption::new("apple", "Apple")],
                ..Default::default()
            };
            // wrapper view [input+chevron wrapper, dropdown panel] → the
            // chevron is the wrapper's second child.
            let mut children = match classify(Autocomplete(props)) {
                P::View { children, .. } => children,
                _ => panic!("Autocomplete renders a view wrapper"),
            };
            let mut wrapper_children = match classify(children.remove(0)) {
                P::View { children, .. } => children,
                _ => panic!("first child is the input+chevron wrapper view"),
            };
            assert!(wrapper_children.len() >= 2, "wrapper holds [input, chevron]");
            let chevron = wrapper_children.remove(1);
            let P::Pressable { preserves_focus, .. } = classify(chevron) else {
                panic!("chevron is a pressable");
            };
            assert!(preserves_focus, "the chevron must not blur the input when pressed");
    });
    }

    // REGRESSION: opening the menu always put the keyboard cursor on row 0,
    // so the TOP row painted highlighted regardless of the committed
    // selection. Opening must seed the cursor on the selection's position
    // in the filtered list; a selection not among the matches (or none)
    // falls back to the top.
    #[test]
    fn regression_open_seeds_cursor_on_committed_selection() {
        with_test_world(|| {
            let options = vec![
                SelectOption::new("apple", "Apple"),
                SelectOption::new("pear", "Pear"),
                SelectOption::new("plum", "Plum"),
            ];
            // Full list open: selection sits at its own index.
            assert_eq!(initial_highlight(&[0, 1, 2], &options, "pear"), 1);
            // Filtered list: position is WITHIN the filtered indices, not the
            // option's global index.
            assert_eq!(initial_highlight(&[1, 2], &options, "plum"), 1);
            // Selection filtered out / nothing committed → top row.
            assert_eq!(initial_highlight(&[0], &options, "plum"), 0);
            assert_eq!(initial_highlight(&[0, 1, 2], &options, ""), 0);
            // Empty match list → 0 (Enter guards on non-empty separately).
            assert_eq!(initial_highlight(&[], &options, "pear"), 0);
    });
    }

    // REGRESSION: the keyboard-cursor row and the committed-selection row
    // used the same `active: on` style, so an open menu showed two solid
    // rows. The three states must resolve to three distinct paints:
    // selection = solid, cursor = subtle hover surface, rest = transparent.
    #[test]
    fn regression_cursor_and_selected_rows_paint_differently() {
        with_test_world(|| {
            idea_theme::theme::install_idea_theme(idea_theme::theme::light_theme());

            let filtered = memo(|| vec![0, 1]);
            let highlight: Signal<usize> = runtime_core::signal(0);
            let value: Signal<String> = runtime_core::signal("b".to_string());
            let el = row(
                SelectOption::new("a", "A"),
                0,
                Rc::new(|_| {}),
                move || filtered.get(),
                highlight,
                value,
            );
            let P::Pressable { style: Some(style), .. } = classify(el) else {
                panic!("a menu row is a styled pressable");
            };
            let TStyle::AppFn(style) = style else {
                panic!("row style is reactive (cursor/selection resolve live)");
            };
            let bg = |app: StyleApplication| {
                runtime_core::resolve_style(&app).background.clone().map(|b| b.resolve())
            };

            // highlight=0 → cursor rests on this row; committed value is "b".
            let cursor_bg = bg(style());
            // Move the cursor off the row → plain row.
            highlight.set(1);
            idea_theme::testing::commit();
            let off_bg = bg(style());
            // Commit this row's id → selection paint.
            value.set("a".to_string());
            idea_theme::testing::commit();
            let selected_bg = bg(style());

            assert_ne!(cursor_bg, off_bg, "the cursor row must paint against a plain row");
            assert_ne!(cursor_bg, selected_bg, "cursor and selection must be distinct looks");
            assert_ne!(selected_bg, off_bg, "the selection must paint against a plain row");
    });
    }

    // The whole reactive tree (seeded query signal, filter memo, input +
    // chevron, `when` panel, external-value `on_defer`) must build without
    // panicking. Guards against a regression where any of those wiring steps
    // touches the arena/scope in a way that aborts at construction. Renders
    // to the closed (`open == false`) state: a wrapper view + the `when`
    // panel placeholder.
    #[test]
    fn builds_collapsed_tree() {
        with_test_world(|| {
            idea_theme::theme::install_idea_theme(idea_theme::theme::light_theme());
            let value = runtime_core::signal("pear".to_string());
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
            match classify(tree) {
                P::View { children, .. } => {
                    assert_eq!(children.len(), 2, "wrapper view + dropdown panel");
                }
                _ => panic!("Autocomplete renders a view wrapper"),
            }
    });
    }
}
