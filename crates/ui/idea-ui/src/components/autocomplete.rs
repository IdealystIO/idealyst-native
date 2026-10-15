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
//!         // Pinned below the rows: offer to create what the user typed.
//!         footer = AutocompleteSlot::new(move |cx| {
//!             let dismiss = cx.dismiss.clone();
//!             ui! {
//!                 Button(label = rx!(format!("Add “{}”", cx.query.get())), on_click = move || {
//!                     // …create the option + commit it host-side, then:
//!                     (dismiss)();
//!                 })
//!             }
//!         }),
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
//!   label OR its optional `description`) and opens it. A row's
//!   description draws as a muted second line under its label, exactly as
//!   in `Select`; the input itself only ever shows the label.
//! - `ArrowDown`/`ArrowUp` move the keyboard cursor, `Enter` commits the
//!   cursor row, `Escape` closes and reverts the text to the committed
//!   selection's label.
//! - The chevron (lucide `chevron-down`, shared with `Select`) toggles the
//!   menu open and focuses the input, and flips 180° while the menu is open.
//! - Picking a row (click or `Enter`) fires `on_change` with the option's
//!   `id` and shows its label.
//! - Dismissing the menu without choosing (blur / `Escape`) reverts the
//!   typed text to the committed selection, so the input can never be left
//!   showing a string that isn't a valid option.
//! - Optional `header` / `footer` slots render pinned above / below the
//!   scrolling row area — they never scroll away with the rows. Each is a
//!   builder invoked when the menu opens with an [`AutocompleteSlotCx`]
//!   carrying the live typed query and a `dismiss` handle, which is what an
//!   "Add ‹query›" footer needs: render the current text reactively and
//!   close the menu after acting. Slots are press surfaces inside the
//!   focus-preserving panel, so pressing them never blurs the input; they
//!   are click-only (the keyboard cursor and `Enter` stay on the rows).
//! - `clearable` adds a trailing ✕ left of the chevron, shown while there is
//!   something to clear (a committed selection or typed text). Pressing it
//!   fires `on_change("")` and empties the input — the one way to get BACK
//!   to "nothing selected", since dismissal otherwise reverts the text to
//!   the committed label. Like the chevron it is `preserves_focus`, so
//!   clearing a focused combobox keeps focus (and an open menu, now listing
//!   every option) where it was.
//!
//! # Multi-select
//!
//! Binding a `selection: Signal<Vec<String>>` switches modes. Everything
//! above still holds except where a list of picks makes it impossible:
//!
//! - Rows carry a leading CHECKBOX, and picking one TOGGLES it without
//!   closing the menu — the same rule a checkable `MenuEntry` follows, so
//!   several matches of one search stack without retyping it. The query is
//!   kept across picks for the same reason.
//! - A picked row never paints solid. "An open menu never shows two solid
//!   rows" cannot survive a list of picks, so the mark carries the
//!   selection and the row style carries the keyboard cursor alone.
//! - The input holds the QUERY, never a committed label, so there is
//!   nothing to revert to: blur / `Escape` clear the search instead, and
//!   the picks are untouched.
//! - `on_selection_change` fires with the WHOLE vec, not the one id that
//!   changed. `value` / `on_change` are ignored.
//! - The `clearable` ✕ empties the whole selection (firing
//!   `on_selection_change` with an empty vec) along with the query.
//! - The picked options are shown as a count summary in the placeholder,
//!   and nothing more: the bordered box is the `text_input` itself so the
//!   focus ring lands on the focusable element, and a filter bar already
//!   shows its selection as removable chips BESIDE the control. Rendering
//!   them is the host's job.
//!
//! ```ignore
//! let picked: Signal<Vec<String>> = signal(Vec::new());
//! ui! {
//!     Autocomplete(
//!         selection = Some(picked),
//!         on_selection_change = Some(Rc::new(move |ids: Vec<String>| refetch(ids))),
//!         placeholder = "Filter by heading…",
//!         options = headings,
//!     )
//! }
//! ```
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
//! itself — exactly like [`Field`](crate::Field) — and the trailing row (✕ +
//! chevron) is absolutely positioned over its right edge. A bordered wrapper would draw
//! a ring that never lights up, because focus is on the inner input.

use std::rc::Rc;

use idea_theme::compat::SignalModify as _;
use runtime_core::primitives::key::{KeyEvent, KeyOutcome};
use runtime_core::primitives::overlay::BackdropMode;
use runtime_core::primitives::portal::{AnchorTarget, ElementAlign, ElementSide};
use runtime_core::{
    component, each_keyed, memo, on_defer, pressable, signal, text, text_input, ui, view,
    EachKey, EachRowBuild, Element, IdealystSchema, IntoElement, Reactive, ReadSignal, Ref, Signal,
    StyleApplication, TextInputHandle, VariantEnum, ViewHandle,
};

use idea_theme::theme::IdeaThemeRef;

use crate::components::date_input::CLEAR_GLYPH;
use crate::components::field::{adornment_icon_color, adornment_icon_px_for};
use crate::components::icon::Icon;
use crate::components::select::{
    live_options, option_text, rotating_chevron, SelectOption, SelectSize, CHEVRON_DOWN,
};
use crate::stylesheets::{
    AutocompleteBox, AutocompleteChevron, AutocompleteClear, AutocompleteEmpty,
    AutocompleteInput, AutocompleteTrailing, SelectOption as SelectOptionStyle,
};

/// Context handed to a menu-slot builder ([`AutocompleteProps::header`] /
/// [`AutocompleteProps::footer`]) each time the menu opens.
#[derive(Clone)]
pub struct AutocompleteSlotCx {
    /// The live text currently typed in the input (read-only — a slot
    /// observes the query, it never edits it). The builder runs once per
    /// open, so render query-dependent content *reactively* off this signal
    /// (`text_fmt!` / a derived read inside the returned element) rather
    /// than snapshotting `.get()` at build time.
    pub query: ReadSignal<String>,
    /// Close the menu and revert the input to the committed selection's
    /// label — the same path as `Escape`, preserving the invariant that the
    /// input never lingers on text that isn't a valid option. Call it from
    /// a slot action (e.g. after "Add ‹query›" creates the option and
    /// commits it host-side).
    pub dismiss: Rc<dyn Fn()>,
}

/// A pinned dropdown slot ([`AutocompleteProps::header`] /
/// [`AutocompleteProps::footer`]): a builder invoked with an
/// [`AutocompleteSlotCx`] each time the menu opens. A builder closure
/// rather than an `Element` because the panel is structurally rebuilt on
/// every open, and an `Element` can only be mounted once (the same shape as
/// `Field`'s [`crate::Adornment::element`] and `Modal`'s content).
#[derive(Clone, runtime_core::Remote)]
pub struct AutocompleteSlot(Rc<dyn Fn(AutocompleteSlotCx) -> Element>);

impl AutocompleteSlot {
    /// Build a slot from a closure:
    /// `AutocompleteSlot::new(move |cx| ui! { … })`.
    pub fn new(build: impl Fn(AutocompleteSlotCx) -> Element + 'static) -> Self {
        Self(Rc::new(build))
    }

    /// Invoke the builder for one menu-open cycle.
    fn build(&self, cx: AutocompleteSlotCx) -> Element {
        (self.0)(cx)
    }
}

/// Default text shown in the menu when nothing matches the query.
const DEFAULT_EMPTY_TEXT: &str = "No results";

// Reactive-by-default: `#[props]` rewrites each scalar-DATA field `T` →
// `Reactive<T>`. AUTO-SKIPPED: `value` (a `Signal` reactive source) and
// `on_change` (an `Rc` handler). `options` is a `Vec`, which the macro skips
// as a list, so it opts in with `#[prop(reactive)]` — rows are DATA, and
// rows that arrive after mount are the common case (see `Select`). `size` routes
// into the reactive `input_style` sink; `placeholder` routes to the
// `text_input`'s reactive placeholder. `empty_text` feeds the dropdown's
// empty-state row — structural list content (see the TODO in the body).
// `header`/`footer` are `#[prop(static)]` for the same reason as `Field`'s
// adornments: they're ELEMENT-BUILDERS (the *children* category), whose
// reactivity is structural/internal via the slot cx, not data-reactive.
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
    /// The rows to offer; the typed query filters this list by label and
    /// by each row's optional [`SelectOption::description`].
    /// Reactive: pass a `Signal<Vec<SelectOption>>` (or `rx!`) and rows that
    /// change after mount show up — in the open menu too, and in the input's
    /// label once the committed id's option arrives. Pass the signal itself,
    /// not `opts.get()` (read once, like any prop).
    #[prop(reactive)]
    #[schema(constraint = "reactive: static Vec<SelectOption> or Signal/rx!")]
    pub options: Vec<SelectOption>,
    /// Input height/density. Default `Md`. Shared with [`Select`](crate::Select).
    pub size: SelectSize,
    /// Placeholder shown when the input is empty. `Reactive<Option<String>>`.
    pub placeholder: Option<String>,
    /// Text shown in the menu when no option matches the query. Defaults to
    /// "No results".
    pub empty_text: Option<String>,
    /// Optional slot pinned ABOVE the scrolling row area (a caption, a
    /// hint, …). Built per menu-open with an [`AutocompleteSlotCx`].
    #[prop(static)]
    pub header: Option<AutocompleteSlot>,
    /// Optional slot pinned BELOW the scrolling row area. The canonical use
    /// is an "Add ‹query›" action when no existing option fits: read
    /// `cx.query` reactively for the label, create + commit the option
    /// host-side on press, then `(cx.dismiss)()`. Built per menu-open.
    #[prop(static)]
    pub footer: Option<AutocompleteSlot>,
    /// MULTI-SELECT. Bind a `Vec<String>` of chosen option ids and the
    /// combobox switches modes: rows carry a checkbox, picking one toggles
    /// it WITHOUT closing the menu, and the input holds the search query
    /// rather than the committed label. `None` (the default) is the
    /// single-select combobox, unchanged.
    ///
    /// `value` / `on_change` are ignored in this mode — the selection is a
    /// list, so it has its own signal and its own callback
    /// ([`AutocompleteProps::on_selection_change`]).
    ///
    /// The component does NOT render the picked options: the input shows a
    /// count summary in its placeholder, and anything richer belongs to the
    /// host, which is where removable chips already live. Two reasons, and
    /// both matter — the bordered box is the `text_input` ITSELF so the
    /// native focus ring lands on the focusable element (chips inside would
    /// force the border onto a wrapper whose ring never lights up), and a
    /// filter bar already shows its selection as chips BESIDE the control.
    #[prop(static)]
    pub selection: Option<Signal<Vec<String>>>,
    /// Fires with the WHOLE selection after a multi-select toggle — the new
    /// vec, not the one id that changed. Only meaningful alongside
    /// [`AutocompleteProps::selection`].
    #[prop(static)]
    pub on_selection_change: Option<Rc<dyn Fn(Vec<String>)>>,
    /// Focus the input once, right after the combobox mounts (HTML
    /// `autofocus`). Focus opens the menu, as a user's focus does.
    /// Mount-time only; the last autofocus field mounted wins. Default
    /// `false`.
    #[prop(static)]
    pub autofocus: bool,
    /// Offer a trailing ✕ (left of the chevron) that clears the selection:
    /// single mode fires `on_change("")`, multi mode fires
    /// `on_selection_change` with an empty vec, and both empty the input.
    /// Shown only while there is something to clear (a committed selection
    /// or typed text). Static: it shapes the trailing row and the input's
    /// right-padding reserve. Default `false`.
    #[prop(static)]
    pub clearable: bool,
}

impl Default for AutocompleteProps {
    fn default() -> Self {
        Self {
            value: runtime_core::signal(String::new()),
            on_change: Rc::new(|_| {}),
            options: Reactive::Static(Vec::new()),
            size: Reactive::Static(SelectSize::default()),
            placeholder: Reactive::Static(None),
            empty_text: Reactive::Static(None),
            header: None,
            footer: None,
            selection: None,
            on_selection_change: None,
            autofocus: false,
            clearable: false,
        }
    }
}

/// The [`AutocompleteSlotCx`] handed to `header`/`footer` builders on each
/// menu-open: the live query (read-only) plus a `dismiss` that closes the
/// menu and reverts the input to the committed selection — the exact
/// `Escape` path, so a slot action can never leave the input showing text
/// that isn't a valid option. Pulled out as a function so the cx contract
/// (live query, close-and-revert dismissal) is unit-tested without a
/// backend.
fn slot_cx(query: Signal<String>, open: Signal<bool>, revert: Rc<dyn Fn()>) -> AutocompleteSlotCx {
    AutocompleteSlotCx {
        query: query.read_only(),
        dismiss: Rc::new(move || {
            open.set(false);
            (revert)();
        }),
    }
}

/// Indices into `labels` that match `query`, in original order.
///
/// Filtering is a case-insensitive substring match on the (trimmed) query,
/// against each option's label OR its description (`descriptions[i]`, the
/// i-th option's description text; a missing entry or `None` is a row
/// without one) — a row whose note says what picking it does is findable
/// by that note.
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
    descriptions: &[Option<String>],
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
        .filter(|(i, l)| {
            l.to_lowercase().contains(&q)
                || descriptions
                    .get(*i)
                    .and_then(|d| d.as_deref())
                    .is_some_and(|d| d.to_lowercase().contains(&q))
        })
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

/// One multi-select toggle: `id` leaves the selection if it is already
/// there, otherwise joins it at the end. Order is INSERTION order, not
/// option order — the host reads this vec back as "what was picked", and
/// re-sorting picks under the user is surprising. Pure so the toggle is
/// unit-tested without a backend.
pub(crate) fn toggled(selection: &[String], id: &str) -> Vec<String> {
    let mut next: Vec<String> = selection.to_vec();
    match next.iter().position(|s| s == id) {
        Some(pos) => {
            next.remove(pos);
        }
        None => next.push(id.to_string()),
    }
    next
}

/// What a multi-select input shows when the user is not typing: nothing
/// picked falls back to the host's own placeholder, ONE pick reads as its
/// label (a count would be a worse way to say "Bolting"), and several read
/// as a count. Pure so the summary is unit-tested without a backend.
pub(crate) fn selection_summary(
    ids: &[String],
    options: &[SelectOption],
    base: Option<String>,
) -> Option<String> {
    match ids.len() {
        0 => base,
        1 => options
            .iter()
            .find(|o| o.id == ids[0])
            .map(|o| o.label.get())
            .or(base),
        n => Some(format!("{n} selected")),
    }
}

/// The ✕ action: empty the selection AND the typed query. Single mode
/// reports `""` through `on_change` (the host owns `value`, as with a row
/// commit); multi mode writes the empty vec to `selection` and reports it.
/// The query is cleared too — otherwise single mode's blur-revert would
/// restore a stale label if the host's write lands after it. Pulled out so
/// both modes are unit-tested without a backend.
pub(crate) fn clear_action(
    query: Signal<String>,
    on_change: Rc<dyn Fn(String)>,
    selection: Option<Signal<Vec<String>>>,
    on_selection_change: Option<Rc<dyn Fn(Vec<String>)>>,
) -> Rc<dyn Fn()> {
    Rc::new(move || {
        query.set(String::new());
        match selection {
            None => (on_change)(String::new()),
            Some(sel) => {
                sel.set(Vec::new());
                if let Some(cb) = &on_selection_change {
                    (cb)(Vec::new());
                }
            }
        }
    })
}

/// The trailing ✕: an icon-sized pressable sized like a Field adornment
/// at the same density. `preserves_focus` for the chevron's reason —
/// pressing it must not blur the input (a blur would close-and-revert the
/// menu mid-clear).
fn clear_button(clear: Rc<dyn Fn()>, size: SelectSize) -> Element {
    let px = adornment_icon_px_for(size.as_variant_str());
    let muted = adornment_icon_color();
    let glyph = ui! { Icon(data = CLEAR_GLYPH, size = px, color = Some(muted)) };
    pressable(vec![glyph], move || (clear)())
        .preserves_focus(true)
        .a11y_label("Clear")
        .with_style(AutocompleteClear())
        .into_element()
}

/// Renders a searchable combobox: a `text_input` that filters an anchored
/// dropdown of [`SelectOption`] rows, with keyboard navigation and
/// constrained (id-only) selection.
#[component]
pub fn Autocomplete(props: AutocompleteProps) -> Element {
    let value = props.value;
    let on_change = props.on_change.clone();
    let size = props.size.clone();
    // TODO(reactive-sweep): `empty_text` is snapshotted here and moved into the
    // `when`/`each_keyed` dropdown closures that build the empty-state row
    // (structural list content). Routing a live `empty_text` would need the
    // empty row to read it from a `Reactive` source inside the `each_keyed`
    // build — flagged, not routed.
    let empty_text = props
        .empty_text
        .get()
        .unwrap_or_else(|| DEFAULT_EMPTY_TEXT.to_string());
    // The live list, as a signal of one shared snapshot (see
    // `select::live_options`). Rows identify their option by ID, never by
    // position: a keyed row survives a list change, and an index it captured
    // would then name a different option.
    let options = live_options(props.options.clone());
    // MULTI-SELECT is the presence of a `selection` signal. It changes four
    // things and nothing else: what a row press does (toggle, menu stays
    // open), what the input holds (the query, never a committed label), what
    // dismissal reverts to (nothing — it clears the query), and how a row
    // marks itself (a checkbox rather than a solid fill).
    let selection = props.selection;
    let multi = selection.is_some();
    let on_selection_change = props.on_selection_change.clone();
    let clearable = props.clearable;

    // --- internal state -----------------------------------------------------
    // `query` is the text in the input; it doubles as the filter. In single
    // mode seed it from the initial committed selection so the input shows the
    // chosen label on first paint (external changes are synced via `on_defer`
    // below). In multi mode the input is a SEARCH BOX and starts empty — there
    // is no one label for a list of picks.
    // The committed id's label in the CURRENT list ("" when it isn't there
    // yet — rows still loading — or nothing is committed). Untracked: the
    // callers are build-time seeding and event/effect bodies.
    let label_of = move |id: &str| -> String {
        options
            .peek()
            .iter()
            .find(|o| o.id == id)
            .map(|o| o.label.get())
            .unwrap_or_default()
    };
    let initial_query = if multi { String::new() } else { label_of(&value.peek()) };
    let query: Signal<String> = signal(initial_query);
    let open: Signal<bool> = signal(false);
    // Keyboard highlight as a position into the *current* filtered list.
    let highlight: Signal<usize> = signal(0usize);

    let input_ref: Ref<TextInputHandle> = Ref::new();
    let wrapper_ref: Ref<ViewHandle> = Ref::new();

    // Filtered option indices, recomputed when the query, the committed
    // value, or any option label or description changes.
    // (No type annotation: `memo` yields a `Memo` — a `Copy` handle with
    // `.get()`.)
    let filtered = {
        memo(move || {
            let q = query.get();
            let options = options.get();
            let labels: Vec<String> = options.iter().map(|o| o.label.get()).collect();
            let descriptions: Vec<Option<String>> = options
                .iter()
                .map(|o| o.description.as_ref().map(|d| d.get()))
                .collect();
            // The "query equals the committed label ⇒ list everything" trick
            // is single-mode only: in multi mode the input never holds a
            // label, so the query is always a genuine search.
            let sel_label = if multi {
                None
            } else {
                let sel_id = value.get();
                options.iter().find(|o| o.id == sel_id).map(|o| o.label.get())
            };
            filter_indices(&labels, &descriptions, &q, sel_label.as_deref())
        })
    };

    // Commit the option with this id. Single mode: report its id, show its
    // label, close the menu. Multi mode: TOGGLE it and report the whole vec,
    // leaving the menu open and the query intact — the same rule a checkable
    // `MenuEntry` follows, so several matches of one search can be stacked
    // without retyping it. An id no longer in the list (it changed under the
    // press) commits nothing.
    let commit: Rc<dyn Fn(&str)> = {
        let on_change = on_change.clone();
        Rc::new(move |id: &str| {
            let Some(o) = options.peek().iter().find(|o| o.id == id).cloned() else { return };
            let Some(sel) = selection else {
                (on_change)(o.id.clone());
                query.set(o.label.get());
                open.set(false);
                return;
            };
            let next = toggled(&sel.get(), &o.id);
            // Writes are staged, so hand the new vec on as a VALUE rather
            // than re-reading the signal in this same pass.
            sel.set(next.clone());
            if let Some(cb) = &on_selection_change {
                (cb)(next);
            }
        })
    };

    // Revert the typed text to the committed selection's label (used on
    // Escape / tap-away so an unmatched query never lingers).
    let revert: Rc<dyn Fn()> = {
        Rc::new(move || {
            // Multi mode has no committed label to revert TO — the field
            // holds the query and the selection lives in the vec — so
            // dismissing just abandons the search. The picks are untouched.
            if multi {
                query.set(String::new());
                return;
            }
            query.set(label_of(&value.get()));
        })
    };

    // Open the menu with the keyboard cursor seeded on the committed
    // selection — opening a combobox highlights what's already chosen, not
    // the first row. Shared by every OPEN path (focus, chevron,
    // ArrowDown-from-closed); typing bypasses it and resets the cursor to
    // the top instead, because the filter just changed.
    let open_menu: Rc<dyn Fn()> = {
        Rc::new(move || {
            open.set(true);
            // Multi mode seeds on the FIRST pick — there is no single
            // committed selection to land on, and the top row is the
            // fallback either way.
            let seed = match selection {
                Some(sel) => sel.get().first().cloned().unwrap_or_default(),
                None => value.get(),
            };
            highlight.set(initial_highlight(&filtered.get(), &options.peek(), &seed));
        })
    };

    // Keep the input text in sync when the host changes `value` out of band
    // (skips the initial run — we seeded `query` above). Body is untracked,
    // so reading `open` here doesn't subscribe; we skip while the menu is
    // open to avoid clobbering active typing.
    let _sync = on_defer(value, move |new_id, _| {
        // Multi mode ignores `value` entirely, and the input holds a
        // query that an out-of-band write must never clobber.
        if multi || open.get() {
            return;
        }
        query.set(label_of(new_id));
    });
    // Same for a NEW option list: a committed id whose option arrives after
    // mount (rows loaded by a fetch) seeded an empty input, and its label
    // must show once the row exists. Same skips as above.
    let _sync_options = on_defer(options, move |_, _| {
        if multi || open.get() {
            return;
        }
        query.set(label_of(&value.peek()));
    });

    // The input is empty whenever the user isn't actively searching, so in
    // multi mode the PLACEHOLDER is where the selection shows: one pick reads
    // as its label, several as a count. Anything richer (removable chips) is
    // the host's, for the reasons on `AutocompleteProps::selection`.
    let placeholder = match selection {
        None => props.placeholder.clone(),
        Some(sel) => {
            let base = props.placeholder.clone();
            Reactive::derive(move || selection_summary(&sel.get(), &options.get(), base.get()))
        }
    };

    // --- input --------------------------------------------------------------
    // `size` is read LIVE inside the style closure so a reactive `size`
    // re-resolves the input height in place; a `Static` one keeps the
    // build-time fast path (no per-input apply-style Effect).
    let size_is_reactive = !size.is_static();
    let make_input_style = {
        let size = size.clone();
        let trailing = if clearable { "clear" } else { "chevron" };
        move || {
            StyleApplication::new(AutocompleteInput::sheet())
                .with("size", size.get().as_variant_str().to_string())
                .with("trailing", trailing.to_string())
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
    .placeholder_reactive(placeholder)
    .autofocus(props.autofocus);
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
                        let id = options.peek().get(idxs[pos]).map(|o| o.id.clone());
                        if let Some(id) = id {
                            (key_commit)(&id);
                        }
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
    // Lucide `chevron-down`, flipping 180° while the menu is open — the same
    // glyph + tween as Select's trigger, sized like the ✕ beside it.
    let chevron_glyph = rotating_chevron(
        CHEVRON_DOWN,
        adornment_icon_px_for(size.get().as_variant_str()),
        open,
    );
    let chevron = pressable(vec![chevron_glyph], {
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

    // --- clear ✕ -------------------------------------------------------------
    // Shown only while there's something to clear: a committed selection, or
    // text typed over an empty one.
    let clear = clear_action(query, on_change.clone(), selection, props.on_selection_change.clone());
    let show_clear = memo(move || {
        let committed = match selection {
            Some(sel) => !sel.get().is_empty(),
            None => !value.get().is_empty(),
        };
        committed || !query.get().is_empty()
    });
    let trailing = ui! {
        view(style = AutocompleteTrailing()) {
            if clearable {
                if show_clear.get() {
                    clear_button(clear.clone(), size.get())
                }
            }
            chevron
        }
    };

    let wrapper = view(vec![input_node, trailing])
        .with_style(AutocompleteBox())
        .bind(wrapper_ref)
        .into_element();

    // --- dropdown -----------------------------------------------------------
    let menu_commit = commit.clone();
    // The id under the keyboard cursor, in the CURRENT filtered list.
    let cursor_id = move || -> Option<String> {
        let idxs = filtered.get();
        let opts = options.get();
        idxs.get(highlight.get()).and_then(|&i| opts.get(i)).map(|o| o.id.clone())
    };
    let menu_revert = revert.clone();
    let header_slot = props.header.clone();
    let footer_slot = props.footer.clone();
    let build_panel = move || {
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
            // Tracks the list too: a new list with the same matching
            // POSITIONS still has to re-key (the ids changed).
            let opts = options.get();
            idxs.iter()
                .filter_map(|&oi| opts.get(oi).cloned())
                .map(|o| {
                    let key = EachKey::new(o.id.clone());
                    let commit_row = snapshot_commit.clone();
                    let build: EachRowBuild = Box::new(move || {
                        vec![row(o, commit_row, cursor_id, value, selection)]
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
        // the input's `on_focus` close-on-blur). Header/footer slots are
        // built fresh per open (the panel is a structural rebuild) and
        // pinned outside the scrolling row area.
        let cx = slot_cx(query, open, menu_revert.clone());
        let header = header_slot.as_ref().map(|s| s.build(cx.clone()));
        let footer = footer_slot.as_ref().map(|s| s.build(cx));
        let menu = crate::components::menu_panel::combobox_menu_panel(
            vec![rows],
            AnchorTarget::from(wrapper_ref),
            header,
            footer,
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
    };


    ui! {
        view {
            wrapper
            if open.get() {
                build_panel()
            }
        }
    }
}

/// One menu row. The `active` variant is resolved reactively from the id
/// under the keyboard cursor in the *current* filtered list plus the
/// committed selection, so moving the keyboard cursor or filtering the list
/// re-styles rows in place without rebuilding them. Selection and cursor
/// are DISTINCT looks: the committed row paints solid (`on`), the keyboard
/// cursor paints the subtle hover surface (`cursor`) — an open menu never
/// shows two solid rows. When the cursor rests on the committed row (the
/// seeded state right after opening), `on` wins.
///
/// MULTI-SELECT rows carry a leading checkbox instead, and never paint
/// solid: "never two solid rows" cannot survive a list of picks, and a panel
/// of them would be unreadable. The mark carries the selection, the row
/// style is left to carry the cursor alone.
fn row(
    o: SelectOption,
    commit: Rc<dyn Fn(&str)>,
    // The id under the keyboard cursor, or `None`. A tracked getter so
    // moving the cursor or re-filtering re-styles the row in place; by ID
    // because a keyed row outlives the list positions it was built at.
    cursor_id: impl Fn() -> Option<String> + 'static,
    value: Signal<String>,
    selection: Option<Signal<Vec<String>>>,
) -> Element {
    let id = o.id.clone();
    let label = o.label.clone();
    let description = o.description.clone();
    let mut kids = Vec::with_capacity(2);
    if let Some(sel) = selection {
        let id = id.clone();
        kids.push(crate::components::menu::menu_checkbox(Reactive::derive(move || {
            sel.get().iter().any(|s| s == &id)
        })));
    }
    // Label, plus the muted description line when the option has one —
    // shared with `Select`'s rows. Multi rows never paint solid, so their
    // description never needs the solid-row contrast swap.
    let text_id = id.clone();
    kids.push(option_text(label, description, move || {
        selection.is_none() && value.get() == text_id
    }));
    let press_id = id.clone();
    pressable(kids, move || (commit)(&press_id))
        .with_style(move || {
            let _ = idea_theme::active_theme_untracked()
                .downcast_ref::<IdeaThemeRef>()
                .expect("idea-ui: no IdeaTheme installed — call install_idea_theme(...) first");
            let highlighted = cursor_id().as_deref() == Some(id.as_str());
            let selected = selection.is_none() && value.get() == id;
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
            assert_eq!(filter_indices(&ls, &[], "", None), vec![0, 1, 2]);
            // Whitespace-only is also "empty" after trim.
            assert_eq!(filter_indices(&ls, &[], "   ", None), vec![0, 1, 2]);
    });
    }

    #[test]
    fn substring_match_is_case_insensitive() {
        with_test_world(|| {
            let ls = labels(&["Apple", "Pear", "Pineapple"]);
            // "ap" hits Apple + Pineapple, not Pear.
            assert_eq!(filter_indices(&ls, &[], "ap", None), vec![0, 2]);
            // Case folds both ways.
            assert_eq!(filter_indices(&ls, &[], "PEAR", None), vec![1]);
    });
    }

    #[test]
    fn no_match_returns_empty() {
        with_test_world(|| {
            let ls = labels(&["Apple", "Pear"]);
            assert!(filter_indices(&ls, &[], "zzz", None).is_empty());
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
                filter_indices(&ls, &[], "Apple", Some("Apple")),
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
            assert_eq!(filter_indices(&ls, &[], "plu", Some("Apple")), vec![2]);
    });
    }

    /// The typed query matches an option's DESCRIPTION as well as its
    /// label: "one-line" finds the row whose note reads "as its one-line
    /// label" even though its label is "Address".
    #[test]
    fn filter_matches_description() {
        with_test_world(|| {
            let ls = labels(&["Address", "Phone", "Email"]);
            let ds = vec![Some("as its one-line label".to_string()), None, Some("Work inbox".to_string())];
            assert_eq!(filter_indices(&ls, &ds, "one-LINE", None), vec![0]);
            assert_eq!(filter_indices(&ls, &ds, "inbox", None), vec![2]);
            // Labels still match, and a row matching on both counts once.
            assert_eq!(filter_indices(&ls, &ds, "e", None), vec![0, 1, 2]);
            assert!(filter_indices(&ls, &ds, "zzz", None).is_empty());
        });
    }

    /// An Autocomplete row draws its description as a second, muted text
    /// under the label; a row without one stays the bare label.
    #[test]
    fn row_draws_description_only_when_set() {
        with_test_world(|| {
            idea_theme::theme::install_idea_theme(idea_theme::theme::light_theme());
            let value: Signal<String> = runtime_core::signal(String::new());
            let body = |o: SelectOption| {
                let P::Pressable { mut children, .. } =
                    classify(row(o, Rc::new(|_: &str| {}), || None, value, None))
                else {
                    panic!("a menu row is a pressable");
                };
                assert_eq!(children.len(), 1, "a single-mode row holds one text body");
                classify(children.remove(0))
            };
            let P::View { children, .. } = body(
                SelectOption::new("form_entry.address", "Address")
                    .with_description("as its one-line label"),
            ) else {
                panic!("a described row's body is a [label, description] view");
            };
            let texts: Vec<Option<String>> = children
                .into_iter()
                .map(|c| match classify(c) {
                    P::Text { text, .. } => text,
                    _ => panic!("body children are texts"),
                })
                .collect();
            assert_eq!(
                texts,
                vec![Some("Address".to_string()), Some("as its one-line label".to_string())]
            );
            let P::Text { text, .. } = body(SelectOption::new("p", "Plain")) else {
                panic!("an undescribed row's body is the label text alone");
            };
            assert_eq!(text.as_deref(), Some("Plain"));
        });
    }

    // Close-on-blur is only safe because the widget's press
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
                options: Reactive::Static(vec![SelectOption::new("apple", "Apple")]),
                ..Default::default()
            };
            // root view [input wrapper, dropdown panel] → wrapper [input,
            // trailing row] → the chevron is the trailing row's last child.
            let mut children = match classify(Autocomplete(props)) {
                P::View { children, .. } => children,
                _ => panic!("Autocomplete renders a view wrapper"),
            };
            let mut wrapper_children = match classify(children.remove(0)) {
                P::View { children, .. } => children,
                _ => panic!("first child is the input wrapper view"),
            };
            assert_eq!(wrapper_children.len(), 2, "wrapper holds [input, trailing row]");
            let mut trailing = classify(wrapper_children.remove(1)).children();
            let chevron = trailing.pop().expect("the trailing row ends with the chevron");
            let P::Pressable { preserves_focus, children, .. } = classify(chevron) else {
                panic!("chevron is a pressable");
            };
            assert!(preserves_focus, "the chevron must not blur the input when pressed");
            // The glyph is lucide `chevron-down` inside Select's rotating
            // view (bound for the RotateZ flip), not a text caret.
            let rotator = classify(children.into_iter().next().unwrap()).children();
            let P::Icon { data, .. } = classify(rotator.into_iter().next().unwrap()) else {
                panic!("the chevron's rotating view wraps an icon");
            };
            assert_eq!(data.paths, CHEVRON_DOWN.paths, "lucide chevron-down, shared with Select");

            // Same rule for the clear ✕: a blur mid-clear would close the
            // menu and revert the input to the label being cleared.
            let P::Pressable { preserves_focus, .. } =
                classify(clear_button(Rc::new(|| {}), SelectSize::default()))
            else {
                panic!("the ✕ is a pressable");
            };
            assert!(preserves_focus, "the ✕ must not blur the input when pressed");
    });
    }

    /// `autofocus` reaches the combobox's `text_input`, and defaults off.
    #[test]
    fn autofocus_forwards_to_the_text_input() {
        fn input_autofocus(props: AutocompleteProps) -> bool {
            let mut children = match classify(Autocomplete(props)) {
                P::View { children, .. } => children,
                _ => panic!("Autocomplete renders a view wrapper"),
            };
            let mut wrapper_children = match classify(children.remove(0)) {
                P::View { children, .. } => children,
                _ => panic!("first child is the input+chevron wrapper view"),
            };
            let P::TextInput { autofocus, .. } = classify(wrapper_children.remove(0)) else {
                panic!("the wrapper's first child is the input");
            };
            autofocus
        }
        with_test_world(|| {
            idea_theme::theme::install_idea_theme(idea_theme::theme::light_theme());
            assert!(input_autofocus(AutocompleteProps { autofocus: true, ..Default::default() }));
            assert!(!input_autofocus(AutocompleteProps::default()));
        });
    }

    /// The ✕ action in single mode reports `""` through `on_change` (the
    /// host owns `value`) and empties the query so a later blur-revert has
    /// no stale label to restore.
    #[test]
    fn clear_action_single_commits_empty_and_clears_query() {
        with_test_world(|| {
            let query: Signal<String> = signal("Apple".to_string());
            let got = Rc::new(std::cell::RefCell::new(None::<String>));
            let sink = got.clone();
            let clear = clear_action(
                query,
                Rc::new(move |id| *sink.borrow_mut() = Some(id)),
                None,
                None,
            );
            (clear)();
            idea_theme::testing::commit();
            assert_eq!(got.borrow().as_deref(), Some(""), "on_change fires with \"\"");
            assert_eq!(query.get(), "", "the input empties");
        });
    }

    /// Multi mode: the ✕ empties the WHOLE selection and reports the empty
    /// vec; single-mode `on_change` stays silent.
    #[test]
    fn clear_action_multi_empties_selection_and_reports_it() {
        with_test_world(|| {
            let query: Signal<String> = signal("pl".to_string());
            let selection: Signal<Vec<String>> = signal(labels(&["a", "b"]));
            let reported = Rc::new(std::cell::RefCell::new(None::<Vec<String>>));
            let sink = reported.clone();
            let single_fired = Rc::new(std::cell::Cell::new(false));
            let single_flag = single_fired.clone();
            let clear = clear_action(
                query,
                Rc::new(move |_| single_flag.set(true)),
                Some(selection),
                Some(Rc::new(move |ids| *sink.borrow_mut() = Some(ids))),
            );
            (clear)();
            idea_theme::testing::commit();
            assert!(selection.get().is_empty(), "every pick is cleared");
            assert_eq!(reported.borrow().as_deref(), Some(&[][..]), "reports the empty vec");
            assert!(!single_fired.get(), "multi mode never fires on_change");
            assert_eq!(query.get(), "", "the search empties too");
        });
    }

    /// The input's right-padding reserve covers the trailing row at every
    /// size — the SVG chevron alone, or chevron + ✕ when `clearable` — so
    /// typed text never runs under a glyph.
    #[test]
    fn clearable_reserves_room_for_the_x() {
        with_test_world(|| {
            idea_theme::theme::install_idea_theme(idea_theme::theme::light_theme());
            let pad = |size: &str, trailing: &str| {
                let app = StyleApplication::new(AutocompleteInput::sheet())
                    .with("size", size.to_string())
                    .with("trailing", trailing.to_string());
                runtime_core::resolve_style(&app).padding_right.clone().map(|t| t.resolve())
            };
            use crate::stylesheets::{
                AUTOCOMPLETE_CLEARABLE_PAD_LG_PX, AUTOCOMPLETE_CLEARABLE_PAD_MD_PX,
                AUTOCOMPLETE_CLEARABLE_PAD_SM_PX, AUTOCOMPLETE_PAD_LG_PX, AUTOCOMPLETE_PAD_MD_PX,
                AUTOCOMPLETE_PAD_SM_PX,
            };
            for (size, chevron_only, clearable) in [
                ("sm", AUTOCOMPLETE_PAD_SM_PX, AUTOCOMPLETE_CLEARABLE_PAD_SM_PX),
                ("md", AUTOCOMPLETE_PAD_MD_PX, AUTOCOMPLETE_CLEARABLE_PAD_MD_PX),
                ("lg", AUTOCOMPLETE_PAD_LG_PX, AUTOCOMPLETE_CLEARABLE_PAD_LG_PX),
            ] {
                assert_eq!(pad(size, "chevron"), Some(runtime_core::Length::Px(chevron_only)), "{size}");
                assert_eq!(pad(size, "clear"), Some(runtime_core::Length::Px(clearable)), "{size}");
                // Both reserves must cover the glyphs they sit beside.
                let glyph = crate::components::field::adornment_icon_px_for(size);
                assert!(chevron_only > glyph, "{size}: chevron reserve clears the chevron");
                assert!(clearable > chevron_only + glyph, "{size}: clearable reserve clears the ✕ too");
            }
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

    #[test]
    fn multi_toggle_adds_then_removes_keeping_insertion_order() {
        with_test_world(|| {
            let picked = toggled(&[], "b");
            assert_eq!(picked, labels(&["b"]));
            let picked = toggled(&picked, "a");
            assert_eq!(picked, labels(&["b", "a"]), "picks keep INSERTION order");
            let picked = toggled(&picked, "b");
            assert_eq!(picked, labels(&["a"]), "toggling a picked id removes it");
            assert_eq!(toggled(&picked, "a"), Vec::<String>::new());
    });
    }

    /// The multi-select input shows its selection in the PLACEHOLDER (the
    /// box itself holds the search query). One pick reads as its label —
    /// "1 selected" would be a worse way to say "Bolting" — and several as
    /// a count. Nothing picked falls back to the host's placeholder.
    #[test]
    fn multi_placeholder_summarises_the_selection() {
        with_test_world(|| {
            let opts =
                vec![SelectOption::new("a", "Alpha"), SelectOption::new("b", "Beta")];
            let base = || Some("Search…".to_string());

            assert_eq!(selection_summary(&[], &opts, base()), base());
            assert_eq!(
                selection_summary(&labels(&["b"]), &opts, base()),
                Some("Beta".to_string()),
                "a lone pick reads as its own label"
            );
            assert_eq!(
                selection_summary(&labels(&["a", "b"]), &opts, base()),
                Some("2 selected".to_string())
            );
            // An id with no matching option can't be labelled — fall back
            // rather than showing a blank box.
            assert_eq!(selection_summary(&labels(&["gone"]), &opts, base()), base());
    });
    }

    /// A multi-select row carries a leading checkbox and NEVER paints solid:
    /// "an open menu never shows two solid rows" cannot survive a list of
    /// picks, so the mark carries the selection and the row style is left to
    /// carry the keyboard cursor alone.
    #[test]
    fn multi_rows_carry_a_checkbox_and_never_paint_solid() {
        with_test_world(|| {
            idea_theme::theme::install_idea_theme(idea_theme::theme::light_theme());

            let highlight: Signal<usize> = runtime_core::signal(1);
            // Rows "a", "b"; the cursor sits on `highlight`'s id.
            let cursor = move || ["a", "b"].get(highlight.get()).map(|s| s.to_string());
            let value: Signal<String> = runtime_core::signal(String::new());
            let selection: Signal<Vec<String>> = runtime_core::signal(labels(&["a"]));
            let el = row(
                SelectOption::new("a", "A"),
                Rc::new(|_: &str| {}),
                cursor,
                value,
                Some(selection),
            );
            let P::Pressable { style: Some(style), children, .. } = classify(el) else {
                panic!("a menu row is a styled pressable");
            };
            assert_eq!(children.len(), 2, "a multi row is [checkbox, label]");
            let TStyle::AppFn(style) = style else {
                panic!("row style is reactive (cursor resolves live)");
            };
            // Row 0 IS selected but the cursor is on row 1, so it must not
            // paint solid — in single mode this same state would be `on`.
            let bg = runtime_core::resolve_style(&style()).background.clone();
            let selected_bg = bg.map(|b| b.resolve());

            let single = row(
                SelectOption::new("a", "A"),
                Rc::new(|_: &str| {}),
                cursor,
                runtime_core::signal("a".to_string()),
                None,
            );
            let P::Pressable { style: Some(TStyle::AppFn(single_style)), children: single_kids, .. } =
                classify(single)
            else {
                panic!("a single-mode row is a styled pressable");
            };
            assert_eq!(single_kids.len(), 1, "a single-mode row is the label alone");
            let single_bg = runtime_core::resolve_style(&single_style())
                .background
                .clone()
                .map(|b| b.resolve());
            assert_ne!(
                selected_bg, single_bg,
                "a picked multi row must not borrow single mode's solid selection paint"
            );
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

            let highlight: Signal<usize> = runtime_core::signal(0);
            // Rows "a", "b"; the cursor sits on `highlight`'s id.
            let cursor = move || ["a", "b"].get(highlight.get()).map(|s| s.to_string());
            let value: Signal<String> = runtime_core::signal("b".to_string());
            let el = row(
                SelectOption::new("a", "A"),
                Rc::new(|_: &str| {}),
                cursor,
                value,
                None,
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

    // The slot-cx contract handed to `header`/`footer` builders: the query
    // is LIVE (a slot renders "Add ‹query›" reactively, not a snapshot at
    // open time) and `dismiss` takes the Escape path — close AND revert —
    // so a slot action can never leave the input showing text that isn't a
    // valid option.
    #[test]
    fn slot_cx_exposes_live_query_and_dismiss_closes_and_reverts() {
        with_test_world(|| {
            let query: Signal<String> = signal("Che".to_string());
            let open: Signal<bool> = signal(true);
            let reverted = Rc::new(std::cell::Cell::new(false));
            let revert_flag = reverted.clone();
            let cx = slot_cx(query, open, Rc::new(move || revert_flag.set(true)));

            assert_eq!(cx.query.get(), "Che");
            query.set("Cherry".to_string());
            idea_theme::testing::commit();
            assert_eq!(cx.query.get(), "Cherry", "the slot cx reads the query LIVE");

            (cx.dismiss)();
            idea_theme::testing::commit();
            assert!(!open.get(), "dismiss closes the menu");
            assert!(reverted.get(), "dismiss reverts the input, same as Escape");
    });
    }

    // A tree with both menu slots wired must build without panicking, and
    // the slot builders must NOT run at build time — they're invoked per
    // menu-OPEN (the collapsed tree has no panel to put them in).
    #[test]
    fn builds_collapsed_tree_with_menu_slots() {
        with_test_world(|| {
            idea_theme::theme::install_idea_theme(idea_theme::theme::light_theme());
            let built = Rc::new(std::cell::Cell::new(0u32));
            let header_built = built.clone();
            let footer_built = built.clone();
            let props = AutocompleteProps {
                options: Reactive::Static(vec![SelectOption::new("apple", "Apple")]),
                header: Some(AutocompleteSlot::new(move |_cx| {
                    header_built.set(header_built.get() + 1);
                    runtime_core::text("Fruits".to_string()).into_element()
                })),
                footer: Some(AutocompleteSlot::new(move |cx| {
                    footer_built.set(footer_built.get() + 1);
                    let dismiss = cx.dismiss.clone();
                    pressable(vec![text("Add…".to_string()).into_element()], move || {
                        (dismiss)()
                    })
                    .into_element()
                })),
                ..Default::default()
            };
            match classify(Autocomplete(props)) {
                P::View { children, .. } => {
                    assert_eq!(children.len(), 2, "wrapper view + dropdown panel");
                }
                _ => panic!("Autocomplete renders a view wrapper"),
            }
            assert_eq!(built.get(), 0, "slot builders run per menu-open, not at build");
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
                options: Reactive::Static(vec![
                    SelectOption::new("apple", "Apple"),
                    SelectOption::new("pear", "Pear"),
                ]),
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
