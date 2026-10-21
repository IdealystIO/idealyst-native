//! `Select` — controlled dropdown. Ported to the extensible
//! namespace; same API as the closed-enum
//! [`crate::components::select`]. The component's only stylable axis
//! today is `size` (trigger height) — adding a Tone trait would let
//! authors color the trigger when the value carries semantic meaning
//! (e.g. selecting an alert level). Not wired yet; lands here when
//! there's a concrete use case.
//!
//! `disabled` (reactive) makes the Select read-only: the trigger's
//! pressable is disabled (it won't open, and it leaves keyboard focus), the
//! trigger sheet's `state disabled` overlay dims it, and an open menu
//! closes if the Select goes disabled under it.
//!
//! ```ignore
//! ui! {
//!     Select(
//!         value = value,
//!         on_change = on_change,
//!         options = vec![
//!             SelectOption::new("apple", "Apple"),
//!             SelectOption::new("pear", "Pear"),
//!         ],
//!     )
//! }
//! ```
//!
//! A row can carry a muted second line under its label —
//! `SelectOption::new("pear", "Pear").with_description("Soft once ripe")`.
//! It is drawn in the menu only; the trigger shows the label.
//!
//! `options` is reactive: pass a `Signal<Vec<SelectOption>>` (or `rx!`)
//! and rows that arrive later — a fetch that lands after mount — appear in
//! the trigger label and the menu, even while it is open. Pass the signal
//! itself, not `opts.get()`: a `.get()` at the call site is read once, like
//! any prop.

use std::rc::Rc;

use std::time::Duration;

use runtime_core::animation::{AnimProp, AnimatedValue, TweenTo};
use runtime_core::stylesheet;
use runtime_core::primitives::overlay::{overlay, BackdropMode};
use runtime_core::primitives::portal::{AnchorTarget, ElementAlign, ElementSide, ViewportPlacement};
use runtime_core::{
    component, each_keyed, effect, signal, ui, EachKey, EachRowBuild, Color, Element, FillRule, IconData, IdealystSchema,
    IntoElement, Length, Position, PressableHandle, Reactive, Ref, Signal, StyleApplication, StyleSheet, VariantEnum, ViewHandle
};

use idea_theme::theme::IdeaThemeRef;

// The `icon` primitive builder — the Select trigger's chevron.
use runtime_core::icon;
use crate::stylesheets::{
    SelectOption as SelectOptionStyle, SelectOptionBody, SelectOptionDescription, SelectOptionLabel, SelectTrigger,
};

/// Full-bleed, fully-transparent backdrop for the open menu: it catches the
/// outside click (and a re-press of the trigger) that dismisses the dropdown,
/// without dimming the page. Mirrors `Popover`'s backdrop. `backdrop_style`
/// *replaces* the backdrop's styling, so this must set the full-viewport inset
/// itself — a background-only sheet would collapse to zero and catch nothing.
fn transparent_backdrop_sheet() -> std::rc::Rc<StyleSheet> {
    SelectBackdropSheet::sheet()
}

// `stylesheet!` (LINK-time), not `premint_as`: the dropdown constructs on
// OPEN, which the premint dump's crawl never does — see `popover.rs`.
stylesheet! {
    SelectBackdropSheet<()> {
        base(_t) {
            position: Position::Absolute,
            top: Length::Px(0.0),
            left: Length::Px(0.0),
            right: Length::Px(0.0),
            bottom: Length::Px(0.0),
            background: Color("transparent".into()),
        }
    }
}

pub use crate::stylesheets::SelectTriggerSize as SelectSize;
use idea_theme::tokens;

/// Default trigger affordance: lucide's `chevron-down`, rotated when the
/// menu opens. Authors override it via [`SelectProps::icon`]. Shared with
/// `Autocomplete`'s disclosure chevron.
pub(crate) const CHEVRON_DOWN: IconData = IconData {
    view_box: (24, 24),
    paths: &["M6 9l6 6 6-6"],
    fill_rule: FillRule::NonZero,
    filled: false
};

/// How fast the trigger chevron flips between its closed (0°) and open
/// (180°) positions. Short + eased-out so it feels snappy, not floaty.
const CHEVRON_SPIN_MS: u64 = 130;

/// Point size of the Select trigger's chevron at every trigger size.
const SELECT_CHEVRON_PX: f32 = 16.0;

/// A muted `glyph` that rotates 180° while `open` is true, tweening over
/// [`CHEVRON_SPIN_MS`]. The disclosure affordance shared by `Select`'s
/// trigger and `Autocomplete`'s chevron button, so both flip identically.
/// Wrapped in a bound view because `RotateZ` animates a `ViewHandle`.
pub(crate) fn rotating_chevron(glyph: IconData, size_px: f32, open: Signal<bool>) -> Element {
    let chevron_ref: Ref<ViewHandle> = Ref::new();
    let rot = AnimatedValue::new(0.0);
    rot.bind(chevron_ref, AnimProp::RotateZ);
    effect!({
        let target = if open.get() { 180.0 } else { 0.0 };
        rot.animate(TweenTo::new(target, Duration::from_millis(CHEVRON_SPIN_MS)).ease_out());
    });
    let glyph = icon(glyph)
        .size(size_px)
        .color(|| tokens().color.text_muted().resolve())
        .into_element();
    runtime_core::view(vec![glyph]).bind(chevron_ref).into_element()
}

/// One published snapshot of an option list. Compares by IDENTITY: a
/// signal's guarded `set` skips equal writes, `SelectOption` can't be
/// compared (its label is `Reactive`), and every publish is a fresh `Rc`,
/// so each new list from the host counts as a change.
#[derive(Clone)]
pub(crate) struct OptionList(Rc<Vec<SelectOption>>);

impl PartialEq for OptionList {
    fn eq(&self, other: &Self) -> bool {
        Rc::ptr_eq(&self.0, &other.0)
    }
}

impl std::ops::Deref for OptionList {
    type Target = [SelectOption];
    fn deref(&self) -> &[SelectOption] {
        &self.0
    }
}

/// The live option list behind [`Select`] and `Autocomplete`, as a signal
/// of a shared snapshot. Reading it is an `Rc` clone rather than a `Vec`
/// clone (a `Reactive::Static` `.get()` clones the whole list), and every
/// reader in one pass sees the same snapshot. A static list gets a signal
/// nothing writes; a live one gets a scope-owned effect that republishes
/// each change. Must run in a component body (the effect's owner).
pub(crate) fn live_options(
    options: Reactive<Vec<SelectOption>>,
) -> runtime_core::ReadSignal<OptionList> {
    let list = signal(OptionList(Rc::new(options.get_untracked())));
    if !options.is_static() {
        options.bind(move |v| list.set(OptionList(Rc::new(v))));
    }
    list.read_only()
}

/// One selectable row in a [`Select`] (and in `Autocomplete`'s dropdown).
/// `id` is the value committed to the bound signal when chosen; `label` is
/// what the user sees; the optional `description` is a muted second line
/// drawn under the label in the menu row — a hint, or a note on what
/// picking the row does.
///
/// ```ignore
/// SelectOption::new("form_entry.address", "Address")
///     .with_description("as its one-line label")
/// ```
///
/// The description belongs to the MENU ROW only: the Select trigger, the
/// Autocomplete input, and the multi-select summary keep showing just the
/// label. `Autocomplete`'s filter matches it as well as the label.
#[derive(Clone, IdealystSchema, runtime_core::Remote)]
pub struct SelectOption {
    /// Stable value committed to the `Select`'s `value` signal when this
    /// row is chosen. Compared against the current value to mark the
    /// selected row.
    pub id: String,
    /// Row label. `Reactive<String>` — static or live (signal/`rx!`).
    pub label: Reactive<String>,
    /// Optional muted second line under the label in the menu row (smaller
    /// type, the theme's muted text color). `None` (the default from
    /// [`SelectOption::new`]) draws the label alone. Static or live
    /// (signal/`rx!`). Set it with [`SelectOption::with_description`].
    pub description: Option<Reactive<String>>,
}

/// Same text (both static) or the same live source (both reactive, one
/// `Rc`). A live source can't be compared by value without reading it.
fn reactive_text_eq(a: &Reactive<String>, b: &Reactive<String>) -> bool {
    match (a, b) {
        (Reactive::Static(a), Reactive::Static(b)) => a == b,
        (Reactive::Dynamic(a), Reactive::Dynamic(b)) => Rc::ptr_eq(a, b),
        _ => false,
    }
}

/// Equal when the ids match and the labels (and descriptions) are the same
/// text (both static) or the same live source (both reactive, one `Rc`).
/// This is what lets a host keep its rows in a `Signal<Vec<SelectOption>>`
/// (signals skip equal writes) and pass that signal as `options`.
impl PartialEq for SelectOption {
    fn eq(&self, other: &Self) -> bool {
        self.id == other.id
            && reactive_text_eq(&self.label, &other.label)
            && match (&self.description, &other.description) {
                (None, None) => true,
                (Some(a), Some(b)) => reactive_text_eq(a, b),
                _ => false,
            }
    }
}

impl SelectOption {
    /// A row with an id and a label, and no description.
    pub fn new(id: impl Into<String>, label: impl Into<Reactive<String>>) -> Self {
        Self { id: id.into(), label: label.into(), description: None }
    }

    /// Add a muted second line under the label in the menu row. Static
    /// (`&str`/`String`) or live (`Signal<String>`/`rx!`).
    pub fn with_description(mut self, description: impl Into<Reactive<String>>) -> Self {
        self.description = Some(description.into());
        self
    }
}

/// The text content of a menu row: the label alone, or — when the option
/// has a description — a column of the label over the muted description.
/// Shared by `Select`'s and `Autocomplete`'s rows so both draw the second
/// line identically. `selected` is read live by both texts' styles: the
/// committed row paints solid primary, so the label and description take
/// the solid text color there. Each text carries its own color because
/// native text widgets don't inherit the row's.
pub(crate) fn option_text(
    label: Reactive<String>,
    description: Option<Reactive<String>>,
    selected: impl Fn() -> bool + 'static,
) -> Element {
    let selected = Rc::new(selected);
    let active = {
        let selected = selected.clone();
        move || if selected() { "on" } else { "off" }.to_string()
    };
    let label_active = active.clone();
    // `label` is a `Reactive<String>` — routed through `IntoTextSource`
    // (Static→Static, Dynamic→Bound) so a live label re-paints its row.
    let label = runtime_core::text(label)
        .with_style(move || {
            StyleApplication::new(SelectOptionLabel::sheet()).with("active", label_active())
        })
        .into_element();
    let Some(description) = description else {
        return label;
    };
    let description = runtime_core::text(description)
        .with_style(move || {
            StyleApplication::new(SelectOptionDescription::sheet()).with("active", active())
        })
        .into_element();
    runtime_core::view(vec![label, description])
        .with_style(SelectOptionBody())
        .into_element()
}

// Reactive-by-default: `#[props]` rewrites each scalar-DATA field `T` →
// `Reactive<T>`. AUTO-SKIPPED: `value` (a `Signal` reactive source) and
// `on_change` (an `Rc` handler). `options` is a `Vec`, which the macro
// skips as a list, so it opts in with `#[prop(reactive)]`: it is DATA (rows
// loaded after mount are the common case), not an element list.
// `size` routes into the reactive `trigger_style` sink; `placeholder` is read
// live inside the trigger's label text source. `icon` is structural (the
// chevron is built once into a bound view) — see the TODO in the body.
#[runtime_core::props]
#[derive(IdealystSchema)]
pub struct SelectProps {
    /// Controlled selected value — the `id` of the chosen
    /// [`SelectOption`]. The host owns the signal; selecting a row sets
    /// it via `on_change`.
    pub value: Signal<String>,
    /// Fires with the chosen option's `id` when the user picks a row.
    pub on_change: Rc<dyn Fn(String)>,
    /// The rows to offer in the dropdown menu. Reactive: pass a
    /// `Signal<Vec<SelectOption>>` (or `rx!`) for rows that change after
    /// mount; a plain `vec![…]` is a fixed list.
    #[prop(reactive)]
    #[schema(constraint = "reactive: static Vec<SelectOption> or Signal/rx!")]
    pub options: Vec<SelectOption>,
    /// Trigger height. Default Md.
    pub size: SelectSize,
    /// Text shown on the trigger when no option matches `value`.
    /// `Reactive<Option<String>>`.
    pub placeholder: Option<String>,
    /// Trailing affordance icon, rotated 180° while the menu is open.
    /// Defaults to a chevron when `None`; pass `Some(icons_lucide::…)` to
    /// customize it.
    pub icon: Option<IconData>,
    /// Make the Select read-only. While `true` the trigger does not open
    /// the menu (mouse, keyboard and programmatic presses alike), is out of
    /// keyboard focus, and dims (the trigger sheet's `state disabled`
    /// overlay); `on_change` therefore never fires. An open menu closes if
    /// the Select becomes disabled. Default `false`. Reactive: pass a
    /// `Signal<bool>`/`rx!` and it enables and disables in place.
    #[schema(constraint = "reactive: static bool or Signal/rx!")]
    pub disabled: bool,
    /// Caps the dropdown's width, in pixels. Longer option labels and
    /// descriptions wrap onto further lines inside the cap. `None` (the
    /// default) sizes the menu to its longest row.
    #[prop(static)]
    #[schema(constraint = "pixels, > 0")]
    pub max_width: Option<f32>,
}

impl Default for SelectProps {
    fn default() -> Self {
        Self {
            value: runtime_core::signal(String::new()),
            on_change: Rc::new(|_| {}),
            options: Reactive::Static(Vec::new()),
            size: Reactive::Static(SelectSize::default()),
            placeholder: Reactive::Static(None),
            icon: Reactive::Static(None),
            disabled: Reactive::Static(false),
            max_width: None,
        }
    }
}

/// Controlled dropdown: a pressable trigger showing the selected
/// option's label (or `placeholder`), opening an anchored menu of
/// [`SelectOption`] rows. Choosing a row fires `on_change` and closes
/// the menu.
#[component]
pub fn Select(props: SelectProps) -> Element {
    let value = props.value;
    let on_change = props.on_change.clone();
    let max_width = props.max_width;
    let size = props.size.clone();
    let placeholder = props.placeholder.clone();
    let options = live_options(props.options.clone());
    // TODO(reactive-sweep): `icon` is read once to build the chevron glyph +
    // bound rotation view (structural). A live `icon` source would need the
    // chevron rebuilt — route it once the trigger affordance is restructured
    // to swap the glyph reactively. Snapshot for now.
    let icon_data = props.icon.get().unwrap_or(CHEVRON_DOWN);

    let open: Signal<bool> = signal(false);
    let trigger_ref: Ref<PressableHandle> = Ref::new();

    let label_placeholder = placeholder.clone();
    // `placeholder` is read LIVE inside the trigger's label source, so a
    // reactive placeholder re-paints the empty-trigger text in place.
    // (Plain closure into `text(...)` — the reactive text-source
    // conversion is implicit; an explicit `into_text_source` spelling
    // would be redundant.)
    let label_source = move || {
        options
            .get()
            .iter()
            .find(|o| o.id == value.get())
            .map(|o| o.label.get())
            .or_else(|| label_placeholder.get())
            .unwrap_or_default()
    };
    let label_child = runtime_core::text(label_source).into_element();

    // Trailing chevron that rotates 180° while the menu is open.
    let chevron = rotating_chevron(icon_data, SELECT_CHEVRON_PX, open);

    // `size` is read LIVE inside the trigger style closure so a reactive
    // `size` re-resolves the trigger height in place (the apply-style Effect
    // subscribes to it). The closure is always reactive here (it reads the
    // `open` signal regardless), so no static fast-path gate is needed.
    let trigger_style = {
        let size = size.clone();
        move || {
            let _ = idea_theme::active_theme_untracked()
                .downcast_ref::<IdeaThemeRef>()
                .expect("idea-ui: no IdeaTheme installed — call install_idea_theme(...) first");
            StyleApplication::new(SelectTrigger::sheet())
                .with("size", size.get().as_variant_str().to_string())
                // Highlight (focus-ring border) while the menu is open.
                .with("open", if open.get() { "on" } else { "off" }.to_string())
        }
    };
    // Open on press; the menu's dismiss backdrop handles close — clicking
    // away OR clicking the trigger again lands on the (invisible) backdrop
    // and dismisses, so an open Select closes on the next click anywhere.
    let on_open: Rc<dyn Fn()> = Rc::new(move || open.set(true));
    let trigger = runtime_core::pressable(vec![label_child, chevron], move || (on_open)())
        .with_style(trigger_style)
        .bind(trigger_ref);
    // `disabled` rides the trigger pressable's own binding: the mount
    // handler's press block (so the menu never opens — mouse, keyboard or
    // programmatic), the host's `set_disabled` (out of keyboard focus), and
    // the DISABLED state bit (the sheet's `state disabled` dim). Attached
    // only when the Select can be disabled (Button / Switch parity).
    let trigger = match props.disabled.clone() {
        Reactive::Static(false) => trigger,
        Reactive::Static(true) => trigger.disabled(true),
        live => {
            // Going disabled while the menu is open closes it: the press
            // block only stops NEW opens, and a read-only Select must not
            // keep offering rows to pick. Scope-adopted like the chevron
            // effect above.
            let close_on_disable = live.clone();
            effect!({
                if close_on_disable.get() {
                    open.set(false);
                }
            });
            trigger.disabled(move || live.get())
        }
    };
    let trigger = trigger.into_element();

    let menu_on_change = on_change.clone();
    let menu_close: Rc<dyn Fn()> = Rc::new(move || open.set(false));
    // The menu exists only while open: each open builds it fresh (rows
    // reflect the current `value`), each close tears it down.
    ui! {
        view {
            trigger
            if open.get() {
                {
                    menu_build(
                        value,
                        options,
                        menu_on_change.clone(),
                        menu_close.clone(),
                        trigger_ref,
                        max_width,
                    )
                }
            }
        }
    }
}

/// One menu row: commits its id and closes on press, paints `active: on`
/// while it is the committed value.
fn menu_row(
    opt_id: String,
    opt_label: Reactive<String>,
    opt_description: Option<Reactive<String>>,
    value: Signal<String>,
    on_change: Rc<dyn Fn(String)>,
    menu_close: Rc<dyn Fn()>,
) -> Element {
    let on_click: Rc<dyn Fn()> = Rc::new({
        let opt_id = opt_id.clone();
        move || {
            (on_change)(opt_id.clone());
            (menu_close)();
        }
    });
    let selected_id = opt_id.clone();
    let row_style = move || {
        let _ = idea_theme::active_theme_untracked()
            .downcast_ref::<IdeaThemeRef>()
            .expect("idea-ui: no IdeaTheme installed — call install_idea_theme(...) first");
        let variant = if value.get() == opt_id { "on" } else { "off" };
        StyleApplication::new(SelectOptionStyle::sheet()).with("active", variant.to_string())
    };
    let text_child =
        option_text(opt_label, opt_description, move || value.get() == selected_id);
    runtime_core::pressable(vec![text_child], move || (on_click)())
        .with_style(row_style)
        .into_element()
}

fn menu_build(
    value: Signal<String>,
    options: runtime_core::ReadSignal<OptionList>,
    on_change: Rc<dyn Fn(String)>,
    menu_close: Rc<dyn Fn()>,
    trigger_ref: Ref<PressableHandle>,
    max_width: Option<f32>,
) -> Element {
    // Keyed by option id so a list that changes while the menu is open
    // (rows loaded after mount) adds and drops rows in place instead of
    // rebuilding the panel.
    let row_close = menu_close.clone();
    let rows = each_keyed(move || {
        options
            .get()
            .iter()
            .map(|option| {
                let opt_id = option.id.clone();
                let opt_label = option.label.clone();
                let opt_description = option.description.clone();
                let on_change = on_change.clone();
                let menu_close = row_close.clone();
                let build: EachRowBuild = Box::new(move || {
                    vec![menu_row(opt_id, opt_label, opt_description, value, on_change, menu_close)]
                });
                (EachKey::new(option.id.clone()), build)
            })
            .collect()
    });

    // Click-away + re-press-to-close: a FULLSCREEN, transparent dismiss
    // backdrop. An anchored overlay's own backdrop only fills its small
    // anchored box (the portal is positioned at the trigger, and there's no
    // `position: fixed` to escape it), so it can't catch a click elsewhere.
    // A fullscreen `overlay()` portal IS viewport-sized, so its transparent
    // Dismiss backdrop catches every outside click — including on the trigger,
    // which it covers while open — and fires `on_dismiss`.
    let catcher_close = menu_close.clone();
    let catcher = overlay(Vec::new())
        // FullScreen so the portal (and its inset-0 backdrop) is viewport-sized
        // — the default Center placement sizes the portal to its (empty)
        // content, collapsing the backdrop to 0×0.
        .placement(ViewportPlacement::FullScreen)
        .backdrop(BackdropMode::Dismiss)
        .backdrop_style(StyleApplication::new(transparent_backdrop_sheet()))
        // Never trap focus: `overlay()` defaults the trap ON (it's there for
        // `Modal`), and a trapping EMPTY portal bounces focus out of every
        // sibling portal — including the anchored menu beside it. See
        // `popover::dismiss_catcher` for the full note.
        .trap_focus(false)
        .on_dismiss(move || (catcher_close)())
        .into_element();

    // The anchored menu sits ABOVE the catcher (same layer, rendered after).
    // Its own backdrop is None — the catcher owns dismissal; Escape still
    // closes via this `on_dismiss`.
    // Cap + scroll the option list so a long list (e.g. every category in a
    // filter) scrolls within a bounded panel instead of running off-screen.
    let panel = crate::components::menu_panel::scrolling_menu_panel(vec![rows], max_width);

    let anchored = runtime_core::anchored_overlay(
        AnchorTarget::from(trigger_ref),
        vec![panel],
    )
    .side(ElementSide::Below)
    .align(ElementAlign::Start)
    .offset(4.0)
    .backdrop(BackdropMode::None)
    .trap_focus(false)
    .on_dismiss(move || (menu_close)())
    .into_element();

    runtime_core::view(vec![catcher, anchored]).into_element()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::{classify, P};
    use idea_theme::testing::with_test_world;
    use idea_theme::theme::{install_idea_theme, light_theme};
    use runtime_core::resolve_style;

    use crate::stylesheets::{SelectMenu, SelectTrigger};

    /// Regression: the open menu's fullscreen catcher must NOT trap focus.
    /// `overlay()` defaults the trap ON, and a trapping EMPTY portal installs
    /// a document-level `focusin` bounce that pulls focus out of every sibling
    /// — including whatever the app has focused behind the Select, and any
    /// focusable content in the anchored menu beside it. See
    /// `popover::dismiss_catcher` for the full note.
    #[test]
    fn regression_select_catcher_does_not_trap_focus() {
        with_test_world(|| {
            install_idea_theme(light_theme());
            let trigger: Ref<PressableHandle> = Ref::new();
            let el = menu_build(
                runtime_core::signal(String::new()),
                live_options(Reactive::Static(vec![SelectOption::new("a", "A")])),
                Rc::new(|_| {}),
                Rc::new(|| {}),
                trigger,
                None,
            );
            let kids = match classify(el) {
                P::View { children, .. } => children,
                _ => panic!("the open menu wraps [catcher, anchored] in a View"),
            };
            for (i, portal) in kids.into_iter().flat_map(crate::test_support::portals).enumerate() {
                match portal {
                    P::Portal { trap_focus, .. } => assert!(
                        !trap_focus,
                        "Select portal {i} traps focus — an empty trapping portal steals focus \
                         from the rest of the page while the menu is open"
                    ),
                    _ => panic!("both of the open menu's children must be Portals"),
                }
            }
    });
    }

    // Regression: on macOS the Select trigger showed AppKit's native blue focus
    // ring (the pressable host opted into `NSFocusRingTypeExterior`) instead of
    // the indigo border a Field paints on focus. The fix suppresses the native
    // ring, drives `StateBits::FOCUSED` from the pressable's first-responder
    // transitions, and adds a `state focused` overlay here. This pins the
    // author-visible half: focus resolves the theme focus-ring border. The
    // native ring suppression + FOCUSED driving live in the macOS backend and
    // need a main-thread NSApp, so they're covered by the robot screenshot pass.
    #[test]
    fn regression_select_trigger_focus_paints_focus_ring_border() {
        with_test_world(|| {
            install_idea_theme(light_theme());

            // The focus-ring color the trigger must resolve to when focused.
            let ring = tokens().color.focus_ring().resolve();

            let focused = resolve_style(
                &StyleApplication::new(SelectTrigger::sheet()).with("__state_focused", "on"),
            );
            let focused_border = focused
                .border_top_color
                .clone()
                .expect("a focused trigger must set a border color")
                .resolve();
            assert_eq!(
                focused_border, ring,
                "a focused Select trigger must paint the theme focus-ring border, not the native ring"
            );

            // Without focus the border is the resting neutral border — proving the
            // ring comes from the `focused` state, not the base.
            let base = resolve_style(&StyleApplication::new(SelectTrigger::sheet()));
            let base_border = base
                .border_top_color
                .clone()
                .expect("the trigger reserves a border in its base")
                .resolve();
            assert_ne!(
                base_border, ring,
                "the resting trigger border must NOT already be the focus ring"
            );
    });
    }

    /// The `[label, description]` texts of a menu row's text body, plus the
    /// description's resolved style — `None` description when the row is the
    /// bare label.
    fn row_texts(row: Element) -> (Option<String>, Option<(Option<String>, Rc<runtime_core::StyleRules>)>) {
        let P::Pressable { mut children, .. } = classify(row) else {
            panic!("a menu row is a pressable");
        };
        assert_eq!(children.len(), 1, "a Select row holds one text body");
        match classify(children.remove(0)) {
            P::Text { text, .. } => (text, None),
            P::View { children, .. } => {
                let mut kids = children.into_iter().map(classify);
                let Some(P::Text { text: label, .. }) = kids.next() else {
                    panic!("the body's first child is the label text");
                };
                let Some(P::Text { text: desc, style: Some(style), .. }) = kids.next() else {
                    panic!("the body's second child is the styled description text");
                };
                assert!(kids.next().is_none(), "the body is exactly [label, description]");
                (label, Some((desc, style.resolve())))
            }
            _ => panic!("a row body is a text or a [label, description] view"),
        }
    }

    /// Feature: a picker row "form_entry.address" can carry the muted note
    /// "as its one-line label" under its label. The description draws as a
    /// second text in the row — muted color, small type — and a row with no
    /// description stays the bare label (no wrapper view).
    #[test]
    fn select_row_draws_muted_description_under_label() {
        with_test_world(|| {
            install_idea_theme(light_theme());
            let value = runtime_core::signal(String::new());
            let row = menu_row(
                "form_entry.address".to_string(),
                Reactive::Static("Address".to_string()),
                Some(Reactive::Static("as its one-line label".to_string())),
                value,
                Rc::new(|_| {}),
                Rc::new(|| {}),
            );
            let (label, desc) = row_texts(row);
            assert_eq!(label.as_deref(), Some("Address"));
            let (desc_text, rules) = desc.expect("a described row draws the description");
            assert_eq!(desc_text.as_deref(), Some("as its one-line label"));
            assert_eq!(
                rules.color.clone().map(|c| c.resolve()),
                Some(tokens().color.text_muted().resolve()),
                "the description is the theme's muted text color"
            );
            assert_eq!(
                rules.font_size.clone().map(|f| f.resolve()),
                Some(tokens().typography.body_sm_size().resolve()),
                "the description is the small (help-text) type size"
            );

            let bare = menu_row(
                "plain".to_string(),
                Reactive::Static("Plain".to_string()),
                None,
                value,
                Rc::new(|_| {}),
                Rc::new(|| {}),
            );
            let (label, desc) = row_texts(bare);
            assert_eq!(label.as_deref(), Some("Plain"));
            assert!(desc.is_none(), "no description → the label alone, no second line");
        });
    }

    /// The resolved style on a menu row's LABEL text (the bare text, or the
    /// first child of the `[label, description]` body).
    fn row_label_style(row: Element) -> Rc<runtime_core::StyleRules> {
        let P::Pressable { mut children, .. } = classify(row) else {
            panic!("a menu row is a pressable");
        };
        let label = match classify(children.remove(0)) {
            P::Text { style, .. } => style,
            P::View { mut children, .. } => match classify(children.remove(0)) {
                P::Text { style, .. } => style,
                _ => panic!("the body's first child is the label text"),
            },
            _ => panic!("a row body is a text or a [label, description] view"),
        };
        label.expect("the label carries its own style").resolve()
    }

    /// Regression: on iOS the committed row's label rendered black on a
    /// black primary fill. The row's solid text color was set only on the
    /// pressable, which reaches the label through the CSS cascade on web
    /// but not on native (UILabel doesn't inherit color). The label must
    /// carry the color itself — committed and resting.
    #[test]
    fn regression_ios_select_committed_label_black_on_primary() {
        with_test_world(|| {
            install_idea_theme(light_theme());
            let value = runtime_core::signal("a".to_string());
            let row = |id: &str, desc: Option<&str>| {
                menu_row(
                    id.to_string(),
                    Reactive::Static(id.to_uppercase()),
                    desc.map(|d| Reactive::Static(d.to_string())),
                    value,
                    Rc::new(|_| {}),
                    Rc::new(|| {}),
                )
            };
            let solid = Some(tokens().intent.primary.solid_text().resolve());
            let plain = Some(tokens().color.text().resolve());
            let color = |r: &Rc<runtime_core::StyleRules>| r.color.clone().map(|c| c.resolve());

            assert_eq!(color(&row_label_style(row("a", None))), solid,
                "the committed bare label takes the solid text color");
            assert_eq!(color(&row_label_style(row("a", Some("note")))), solid,
                "the committed described label takes the solid text color");
            assert_eq!(color(&row_label_style(row("b", None))), plain,
                "an uncommitted label is the plain text color");
            assert_eq!(
                row_label_style(row("b", None)).font_size.clone().map(|f| f.resolve()),
                Some(tokens().typography.body_size().resolve()),
                "the label sets the row's body size itself"
            );
        });
    }

    /// On the committed row (solid primary) the muted color would vanish
    /// into the fill, so the description switches to the solid text color.
    #[test]
    fn select_description_on_committed_row_uses_solid_text() {
        with_test_world(|| {
            install_idea_theme(light_theme());
            let value = runtime_core::signal("a".to_string());
            let row = menu_row(
                "a".to_string(),
                Reactive::Static("A".to_string()),
                Some(Reactive::Static("note".to_string())),
                value,
                Rc::new(|_| {}),
                Rc::new(|| {}),
            );
            let (_, desc) = row_texts(row);
            let (_, rules) = desc.unwrap();
            // The committed row's own text color (`active: on`).
            let row_on = resolve_style(
                &StyleApplication::new(SelectOptionStyle::sheet()).with("active", "on"),
            );
            assert_eq!(
                rules.color.clone().map(|c| c.resolve()),
                row_on.color.clone().map(|c| c.resolve()),
                "the description on the solid committed row matches the row's text color"
            );
            assert_ne!(
                rules.color.clone().map(|c| c.resolve()),
                Some(tokens().color.text_muted().resolve()),
                "muted text on the solid primary fill loses contrast"
            );
        });
    }

    /// `with_description` is additive: `new` keeps the two-field shape
    /// (description `None`), and equality accounts for the description so a
    /// `Signal<Vec<SelectOption>>` republishes when only a note changes.
    #[test]
    fn select_option_description_builder_and_equality() {
        with_test_world(|| {
            let plain = SelectOption::new("a", "A");
            assert!(plain.description.is_none());
            let noted = SelectOption::new("a", "A").with_description("note");
            assert!(matches!(&noted.description, Some(Reactive::Static(d)) if d == "note"));
            assert!(plain != noted, "a description makes the row differ");
            assert!(noted == SelectOption::new("a", "A").with_description("note"));
            assert!(noted != SelectOption::new("a", "A").with_description("other"));
        });
    }

    // Field report: on web the Select dropdown options rendered in the
    // browser's serif default (Times), not the app font. Root cause: the
    // menu renders inside a portal that mounts under `<body>`, escaping the
    // app tree's inherited `font-family` — and the menu sheet pinned no font
    // of its own, so its text fell out of the cascade. The fix stamps the
    // theme font on the `SelectMenu` panel so the whole dropdown subtree
    // inherits it. This test fails before the fix (`font_family` is `None`).
    #[test]
    fn select_menu_sheet_premints_and_rides_the_default_font() {
        with_test_world(|| {
            install_idea_theme(light_theme());

            // The contract INVERTED with the default-font root declaration:
            // the sheet must NOT pin a font (a dynamic `font_family` is the
            // one premint disqualifier — it kept SelectMenu off build-time
            // CSS, and opening any Select panicked under `--premint-only`).
            // The portal-font guarantee now rides the default-font
            // machinery: the theme font is declared on the DOCUMENT ROOT on
            // every build, so a `<body>`-mounted portal inherits it, and
            // the live static path folds it at apply. That machinery is
            // pinned by the runtime-vocabulary font tests and
            // backend-ssr's `regression_reactive_styled_node_inherits_theme_font`.
            let resolved = resolve_style(&StyleApplication::new(SelectMenu::sheet()));
            assert!(
                resolved.font_family.is_none(),
                "the menu sheet must not pin a font — that disqualifies it \
                 from preminting; portals get the theme font from the root"
            );
            assert!(
                StyleApplication::new(SelectMenu::sheet()).preminted_class_list().is_some(),
                "the menu sheet must premint: it first constructs when a \
                 Select OPENS, so only build-time CSS can style it under \
                 --premint-only"
            );
    });
    }
}
