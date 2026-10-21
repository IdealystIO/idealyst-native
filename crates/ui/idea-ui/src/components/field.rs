//! `Field` — labeled text input with optional helper/error text.
//!
//! ```ignore
//! ui! {
//!     Field(
//!         label = "Email",
//!         value = email,
//!         on_change = move |v: String| email.set(v),
//!         placeholder = "you@example.com",
//!         help = "We'll never share your email.",
//!         keyboard = KeyboardKind::Email,
//!     )
//! }
//! ```
//!
//! `tone` (optional) drives the input border + help-text color. When
//! `error` is `Some(...)`, `tone::Danger` is applied automatically if
//! no explicit tone is given. `size` ([`FieldSize`]) and `variant`
//! ([`FieldAppearance`]) are open axes of [`FieldStyles`]: the built-in
//! values are associated consts, and an app adds its own with
//! `FieldAppearance::define("pill")` plus a restyle arm.
//!
//! ## Styling
//! Every styled section is a slot of [`FieldStyles`] — `group` (the
//! column), `label` / `label_row` / `label_hint` (the label line, shared
//! by every form control that draws one), `input` (the box: size ×
//! appearance × tone arms, the focus `ring`, the adorned shell), `help`
//! (the help / error line) and `adornment` (a tappable adornment). Restyle
//! any slot app-wide at boot with `FieldStyles::restyle`, or one field
//! through its `styles` prop. `Textarea` draws the same slots.
//!
//! The input style is always reactive (it carries the focus ring and
//! reads `tone`/`error` live — see the `INVARIANT (D9)` note in
//! [`Field`]), so a live `error` turns the border Danger in place. The
//! help/error line follows the same tone, statically when `tone` and
//! `error` are both fixed at build.
//!
//! `disabled` (reactive) makes the field read-only and inert: the
//! `text_input` primitive's `disabled` makes the native input non-editable
//! and drops it from keyboard focus, `on_change` never fires, and the input
//! sheet's `state disabled` overlay dims it. An adorned field dims its row
//! shell instead (the `dimmed` axis) and disables its `Adornment::Button`s.
//!
//! `keyboard` (reactive) picks the on-screen keyboard on phones and tablets
//! (`KeyboardKind::Email` / `Phone` / `Number` / `Decimal` / `Url`) through
//! the `text_input` primitive's `keyboard`; desktop platforms ignore it.
//! `Textarea` has no `keyboard`: multi-line text is prose, and the
//! email / phone / digit layouts are single-line ones.

use std::rc::Rc;

use runtime_core::primitives::key::{KeyEvent, KeyOutcome};
use runtime_core::{
    component, component_styles, derived, pressable, recipe, ui, AlignItems, Color, Cursor, Element, FlexDirection,
    FontWeight, IconData, IdealystSchema, IntoElement, JustifyContent, Length, Reactive, Signal, SlotRestyle, StyleRules, Tokenized, VariantEnum,
};

use crate::components::icon::Icon;

/// Opacity of a disabled Field / Textarea (the input sheet's `state
/// disabled` overlay and the adorned shell's `dimmed` axis).
const FIELD_DISABLED_OPACITY: f32 = 0.55;

/// Horizontal inset on the BARE (adorned) input. Just enough that the glyph's
/// left/right bearing doesn't clip against the input edge (macOS draws the cell
/// text flush). The row gap is reduced by this so the icon↔text spacing stays
/// visually equal to the edge↔icon padding.
const FIELD_BARE_H_PAD: f32 = 2.0;

use idea_theme::extensible::{semantic_tones, tone as tones, tone_arms, Tone, ToneRef};
use idea_theme::theme::IdeaThemeRef;
/// The soft-keyboard hint [`FieldProps::keyboard`] takes (re-exported so a
/// form can spell `field::KeyboardKind::Email`).
pub use runtime_core::KeyboardKind;
use idea_theme::tokens;

/// A leading/trailing adornment inside a [`Field`]'s box — an icon or a custom
/// element rendered beside the input (e.g. a search glyph, a unit suffix, a
/// clear button). The shapes:
///
/// - [`Adornment::None`] — nothing (the default).
/// - [`Adornment::Icon`] — a vector icon, rendered in the field's muted text
///   color at a size derived from the field `size` (it "inherits" the field's
///   styling so you just pass the icon).
/// - [`Adornment::Element`] — any element, rendered as-is, for full control
///   (a button, badge, spinner, …).
/// - [`Adornment::Button`] — a tappable icon-sized glyph (clear ✕,
///   visibility toggle, …).
/// - [`Adornment::Group`] — several of the above side by side in one slot.
///
/// Adornments compose into a flex row alongside the input, so any width works.
#[derive(Clone, runtime_core::Remote)]
pub enum Adornment {
    /// No adornment.
    None,
    /// An arbitrary element, built on demand. `Element` isn't `Clone`, so the
    /// variant holds a builder closure (the same shape as the modal's content)
    /// — use [`Adornment::element`] to construct it from a `move || ui! { … }`.
    Element(Rc<dyn Fn() -> Element>),
    /// A vector icon, auto-sized + muted to match the field.
    Icon(IconData),
    /// A TAPPABLE icon — same auto-sized, muted glyph as [`Adornment::Icon`]
    /// but wrapped in a `pressable` with a press handler and a subtle
    /// hover/press dim. Use this for a clear button, a password-visibility
    /// toggle, etc.: unlike dropping an `IconButton` into [`Adornment::Element`]
    /// (which stacks the button's own square padding on top of the field's),
    /// this stays icon-sized so it never inflates the field box.
    Button(IconData, Rc<dyn Fn()>),
    /// Several adornments side by side in ONE slot (e.g. `DateInput`'s
    /// clear ✕ next to its calendar button). Members render as siblings
    /// directly in the field's shell row — the shell's size-derived gap
    /// spaces them like any other row children, so a group needs (and
    /// gets) no wrapper view of its own. Nested groups flatten; an
    /// all-`None`/empty group counts as no adornment.
    Group(Vec<Adornment>),
}

impl Adornment {
    /// Build an [`Adornment::Element`] from a closure: `Adornment::element(move
    /// || ui! { Button(…) })`.
    pub fn element(builder: impl Fn() -> Element + 'static) -> Self {
        Adornment::Element(Rc::new(builder))
    }

    /// Build an icon-sized [`Adornment::Button`]: `Adornment::button(icon, move
    /// || visible.set(!visible.get()))`.
    pub fn button(icon: IconData, on_press: impl Fn() + 'static) -> Self {
        Adornment::Button(icon, Rc::new(on_press))
    }
}

impl Default for Adornment {
    fn default() -> Self {
        Adornment::None
    }
}

/// Icon point size for an adornment at a given field size (an app-defined
/// size takes `md`'s).
fn adornment_icon_px(size: FieldSize) -> f32 {
    adornment_icon_px_for(size.as_variant_str())
}

/// [`adornment_icon_px`] keyed by the `sm`/`md`/`lg` variant string, so
/// controls with their own size enum (Autocomplete's `SelectSize`) size
/// their trailing glyphs identically to a Field's.
pub(crate) fn adornment_icon_px_for(size_variant: &str) -> f32 {
    match size_variant {
        "sm" => 14.0,
        "lg" => 18.0,
        _ => 16.0,
    }
}

/// The muted glyph color shared by `Icon`/`Button` adornments.
pub(crate) fn adornment_icon_color() -> Color {
    tokens().color.text_muted().resolve()
}

/// Resolve an adornment to its renderable elements (empty for `None` /
/// an empty group; one element per leaf, groups flattened in order).
/// `Icon`/`Button` are sized from the field `size` and painted in the
/// theme's muted text color. A `Button` follows the field's `disabled`
/// (press blocked, out of the Tab order) — a read-only field must not be
/// clearable through its ✕. An `Element` adornment is the author's own
/// tree and is left as built.
fn render_adornment(
    adornment: &Adornment,
    size: FieldSize,
    disabled: &Reactive<bool>,
    styles: &FieldSlotStyles,
) -> Vec<Element> {
    match adornment {
        Adornment::None => Vec::new(),
        Adornment::Element(build) => vec![build()],
        Adornment::Icon(data) => {
            let px = adornment_icon_px(size);
            let muted = adornment_icon_color();
            vec![ui! { Icon(data = data.clone(), size = px, color = Some(muted)) }]
        }
        Adornment::Button(data, on_press) => {
            let px = adornment_icon_px(size);
            let muted = adornment_icon_color();
            let glyph = ui! { Icon(data = data.clone(), size = px, color = Some(muted)) };
            let on_press = on_press.clone();
            // An icon-sized pressable — no button chrome/padding, so it never
            // inflates the field box (the whole point of `Button` vs an
            // `IconButton` in an `Element` adornment).
            let button = pressable(vec![glyph], move || on_press())
                .with_style(FieldStyles::adornment().overridden(styles.adornment.as_ref()));
            // Bound only when the field can be disabled — an always-enabled
            // field's button carries no disabled source (Button parity).
            let button = match disabled.clone() {
                Reactive::Static(false) => button,
                Reactive::Static(true) => button.disabled(true),
                live => button.disabled(move || live.get()),
            };
            vec![button.into_element()]
        }
        Adornment::Group(items) => {
            items.iter().flat_map(|a| render_adornment(a, size, disabled, styles)).collect()
        }
    }
}

component_styles! {
    /// The styled sections of [`Field`] (and of `Textarea`, which draws the
    /// same slots, and of every form control's label line).
    pub Field<IdeaThemeRef> {
        /// Input density — padding and font size.
        axis size { sm, #[default] md, lg }
        /// The input shell: `outline` (bordered), `contained` (filled,
        /// borderless) or `bare` (no chrome). Every one keeps a focus ring.
        axis appearance { #[default] outline, contained, bare }

        /// The field's column: label line, input box, help line.
        slot group {
            base(t) {
                flex_direction: FlexDirection::Column,
                gap: t.spacing.xs(),
                // A form field fills its container width by default (web
                // `<input>` is block-level; native matches — §7 parity).
                // Without it a centering parent (the docs `DemoSurface`,
                // `align_items: center`) collapsed the field to its content
                // width. Width stays auto so an explicit `width` prop still
                // constrains it.
                align_self: runtime_core::AlignSelf::Stretch,
            }
        }

        /// A form control's label text.
        slot label {
            base(t) {
                color: t.color.text(),
                font_size: t.typography.body_sm_size(),
                font_weight: FontWeight::Medium,
            }
        }

        /// The label with its `label_hint` beside it ("Full name  Optional").
        slot label_row {
            base(t) {
                flex_direction: FlexDirection::Row,
                align_items: runtime_core::AlignItems::Baseline,
                gap: t.spacing.xs(),
            }
        }

        /// The muted note beside a label — the help-text treatment, so it
        /// reads as a note on the label rather than part of it.
        slot label_hint {
            base(t) {
                color: t.color.text_muted(),
                font_size: t.typography.body_sm_size(),
            }
        }

        /// The input box. `tone` arms (one per registered tone, added by the
        /// `extend` hook) paint the border in the tone's stroke color;
        /// `default` is the neutral border.
        slot input {
            extend(input_tone_arms)
            base(t) {
                background: t.color.surface(),
                color: t.color.text(),
                border_radius: t.radius.md(),
                border_width: 1.0,
                border_color: t.color.border(),
            }
            variant size {
                sm(t) {
                    padding_vertical: t.spacing.xs(),
                    padding_horizontal: t.spacing.sm(),
                    font_size: t.typography.body_sm_size(),
                }
                md(t) {
                    padding_vertical: t.spacing.sm(),
                    padding_horizontal: t.spacing.md(),
                    font_size: t.typography.body_size(),
                }
                lg(t) {
                    padding_vertical: t.spacing.md(),
                    padding_horizontal: t.spacing.lg(),
                    font_size: t.typography.body_lg_size(),
                }
            }
            // Border WIDTH stays 1 (set on the base) in every appearance so
            // the focus ring renders; `contained` / `bare` clear its color.
            variant appearance {
                outline(_t) {}
                contained(t) {
                    background: t.color.surface_alt(),
                    border_color: Color("transparent".into()),
                }
                bare(_t) {
                    background: Color("transparent".into()),
                    border_color: Color("transparent".into()),
                }
            }
            variant tone {
                #[default]
                default(_t) {}
            }
            // Where the input sits: `standalone` (the plain Field/Textarea),
            // or `shell` — nested inside an ADORNED field's row, which owns
            // the chrome, so the input goes flat and fills the row. An arm,
            // not a call-site layer: every rule is constant. It can't ride
            // `bare` either — that is an author-facing value, and these
            // rules would leak onto a standalone bare field.
            variant placement {
                #[default]
                standalone(_t) {}
                shell(_t) {
                    // NB: no `flex_basis: 0` / `min_width: 0` on the shell's
                    // content: the shell fills via `width: 100%`, which
                    // resolves through its CONTENT size on macOS — collapse
                    // the input's contribution to zero and the shell hugs a
                    // lone icon. `flex_grow: 1` then fills the row.
                    flex_grow: 1.0,
                    // KEEP the size-derived VERTICAL padding: it sets the
                    // field's height, so adornments center within it. The
                    // horizontal padding moves to the shell; a 2px inset
                    // stays so the glyph's bearing doesn't clip the edge.
                    // `placement` < `size` alphabetically, so the size arm's
                    // horizontal padding merges over this one: the
                    // (placement: shell, size) compounds below restate it
                    // for every built-in size. This copy covers an
                    // app-defined size whose arm sets no horizontal padding
                    // (one that does adds its own shell compound).
                    padding_horizontal: FIELD_BARE_H_PAD,
                    flex_shrink: 1.0,
                    min_width: Length::Px(0.0),
                    border_width: 0.0,
                    background: Color("transparent".into()),
                }
            }
            // `adorned=on` turns the node into the row shell: horizontal
            // flex, centered, full width, vertical padding zeroed (it lives
            // on the inner input, which drives the box height).
            variant adorned {
                #[default]
                off(_t) {}
                on(_t) {
                    flex_direction: FlexDirection::Row,
                    align_items: AlignItems::Center,
                    width: Length::Percent(100.0),
                    padding_vertical: Length::Px(0.0),
                }
            }
            // The adorned shell's focus ring: a plain view never receives
            // the inner input's FOCUSED state, so the component forwards the
            // input's `on_focus` into this axis. The plain input uses it too
            // — on macOS the state overlay does not repaint a native input's
            // border.
            variant ring {
                #[default]
                off(_t) {}
                on(t) { border_color: t.color.focus_ring() }
            }
            // The disabled look for a node that is NOT itself the disabled
            // input (an adorned field's row shell). Same opacity as the
            // state overlay.
            variant dimmed {
                #[default]
                off(_t) {}
                on(_t) { opacity: FIELD_DISABLED_OPACITY }
            }
            // The size-dependent shell GAP rides (adorned, size) compounds —
            // gap = edge pad − the input's 2px inset, so icon↔text spacing
            // matches edge↔icon spacing. They also re-zero the size axis's
            // vertical padding: `adorned` < `size` alphabetically, so the
            // lone `adorned=on` arm loses to it.
            compound (adorned: on, size: sm)(_t) { gap: 8.0 - FIELD_BARE_H_PAD, padding_vertical: Length::Px(0.0) }
            compound (adorned: on, size: md)(_t) { gap: 12.0 - FIELD_BARE_H_PAD, padding_vertical: Length::Px(0.0) }
            compound (adorned: on, size: lg)(_t) { gap: 16.0 - FIELD_BARE_H_PAD, padding_vertical: Length::Px(0.0) }
            // The input inside the shell keeps the 2px inset at every size:
            // the `size` arm's horizontal padding would otherwise win over
            // the `placement: shell` arm (see that arm).
            compound (placement: shell, size: sm)(_t) { padding_horizontal: FIELD_BARE_H_PAD }
            compound (placement: shell, size: md)(_t) { padding_horizontal: FIELD_BARE_H_PAD }
            compound (placement: shell, size: lg)(_t) { padding_horizontal: FIELD_BARE_H_PAD }
            state focused(t) { border_color: t.color.focus_ring() }
            state disabled(_t) { opacity: FIELD_DISABLED_OPACITY }
            transitions {
                background: 250ms EaseInOut,
                color: 250ms EaseInOut,
                border_color: 150ms EaseOut,
            }
        }

        /// The help / error line under the input: muted, or the tone's soft
        /// foreground (Danger on a validation error).
        slot help {
            extend(help_tone_arms)
            base(t) {
                font_size: t.typography.caption_size(),
            }
            variant tone {
                #[default]
                default(t) { color: t.color.text_muted() }
            }
            transitions {
                color: 250ms EaseInOut,
            }
        }

        /// A tappable [`Adornment::Button`]: icon-sized (no padding), a
        /// pointer cursor, a subtle hover/press dim.
        slot adornment {
            base(_t) {
                cursor: Cursor::Pointer,
                align_items: AlignItems::Center,
                justify_content: JustifyContent::Center,
            }
            state hovered(_t) { opacity: 0.65 }
            state pressed(_t) { opacity: 0.4 }
        }
    }
}

/// The input's tone arms: each registered tone paints the border its stroke
/// color. Plus the shell's disabled fix-up: the input INSIDE a dimmed shell
/// still gets its own DISABLED state bit, and opacity multiplies down the
/// tree (0.55 × 0.55 ≈ 0.3, fainter than the border around it) — this
/// compound (compounds outrank states) cancels the input's own dim; the
/// shell's covers it.
fn input_tone_arms(s: &mut SlotRestyle<'_, FieldStyles>) {
    tone_arms(s, &semantic_tones(), |ctx| StyleRules {
        border_top_color: Some(ctx.tone.stroke_color(ctx.theme)),
        border_right_color: Some(ctx.tone.stroke_color(ctx.theme)),
        border_bottom_color: Some(ctx.tone.stroke_color(ctx.theme)),
        border_left_color: Some(ctx.tone.stroke_color(ctx.theme)),
        ..Default::default()
    });
    s.raw_compound(
        &[("placement", "shell"), ("__state_disabled", "on")],
        StyleRules { opacity: Some(Tokenized::Literal(1.0)), ..Default::default() },
    );
}

/// The help line's tone arms: each registered tone's soft foreground.
fn help_tone_arms(s: &mut SlotRestyle<'_, FieldStyles>) {
    tone_arms(s, &semantic_tones(), |ctx| StyleRules {
        color: Some(ctx.tone.soft_fg(ctx.theme)),
        ..Default::default()
    });
}

/// The `tone` key a field draws with: the explicit tone, else Danger while
/// an error shows, else the neutral `default` arm.
pub(crate) fn field_tone_key(tone: Option<ToneRef>, has_error: bool) -> &'static str {
    match tone {
        Some(t) => t.key(),
        None if has_error => tones::Danger.key(),
        None => "default",
    }
}

// Reactive-by-default: `#[props]` rewrites each scalar-DATA field `T` →
// `Reactive<T>` so a `ui!` call site can pass a `Signal`/`rx!` and have it
// carry through live (a bare value stays a zero-overhead `Static` snapshot).
// EVERY data prop here is reactive. The exclusions are a different category,
// NOT "static": `on_change` (a handler — invoked on events, never rendered to
// a sink), the controlled `value` `Signal` (already a reactive *source*), the
// already-`Reactive` text props, and the `Adornment` slots — which are
// ELEMENT-BUILDERS (`Rc<dyn Fn() -> Element>`), the *children* category, whose
// reactivity is structural/internal (reactive content inside
// `Adornment::element`, e.g. the password recipe), not data-reactive.
// `Reactive<Adornment>` can't even route: `switch` needs `PartialEq` and an
// element-builder isn't comparable — so a bare `Adornment` is the right type.
#[runtime_core::props(styles = FieldStyles)]
#[cfg_attr(feature = "docs", derive(idea_ui::doc_controls::DocControls))]
#[derive(IdealystSchema)]
pub struct FieldProps {
    /// Optional field label. `Reactive<Option<String>>` — static
    /// (`None`/`Some`) or live (signal/`rx!`).
    #[schema(constraint = "reactive: static Option<String> or Signal/rx!")]
    pub label: Reactive<Option<String>>,
    /// A short muted note beside the label — "Optional", "Required",
    /// "Max 20". `Reactive<Option<String>>` — static or live. Default
    /// `None`.
    // Left out of the docs controls: the derive compares its controls as
    // one tuple, and std implements `PartialEq` for tuples of at most 12.
    #[cfg_attr(feature = "docs", doc_control(skip))]
    #[schema(constraint = "reactive: static Option<String> or Signal/rx!")]
    pub label_hint: Option<String>,
    /// The input's current text. The host owns this signal; the Field
    /// reads it to populate the input and writes via `on_change`.
    pub value: Signal<String>,
    /// Fires with the new text on each edit.
    pub on_change: Rc<dyn Fn(String)>,
    /// Placeholder shown when the input is empty.
    pub placeholder: Option<String>,
    /// Helper text below the input. `Reactive<Option<String>>` — static
    /// or live (signal/`rx!`).
    #[schema(constraint = "reactive: static Option<String> or Signal/rx!")]
    pub help: Reactive<Option<String>>,
    /// Error text below the input; takes precedence over `help` when
    /// present. `Reactive<Option<String>>` — typically the live one
    /// (validation result).
    #[schema(constraint = "reactive: static Option<String> or Signal/rx!")]
    pub error: Reactive<Option<String>>,
    /// Optional tone overlay (border + help-text color). A bare marker
    /// coerces (`tone = tone::Warning` in `ui!`, or `tone::Warning.into()`
    /// in a struct literal); `Some(tone::Warning.into())` also works.
    pub tone: Option<ToneRef>,
    /// Input density (`Sm`/`Md`/`Lg`, or an app-defined [`FieldSize`]) —
    /// drives padding + font size. Default `Md`.
    pub size: FieldSize,
    /// Visual shell: `Outline` (bordered, default), `Contained` (filled),
    /// `Bare` (no chrome), or an app-defined [`FieldAppearance`]. All three
    /// built-ins keep a focus ring.
    pub variant: FieldAppearance,
    /// Mask the entered text (password entry). Forwarded to the underlying
    /// `text_input` primitive's `secure` flag. `Reactive<bool>` — a static
    /// `bool` (the common case) or a live `Signal`/`rx!` so the mask can
    /// toggle at runtime (password show/hide) without rebuilding the input.
    #[schema(constraint = "reactive: static bool or Signal/rx!")]
    pub secure: Reactive<bool>,
    /// Which on-screen keyboard a phone or tablet shows for this field —
    /// `KeyboardKind::Email` (the `@` / `.com` layout, no auto-capitalize or
    /// auto-correct), `Phone` (a dial pad), `Number` (digits), `Decimal`
    /// (digits + separator), `Url`, or `Default`. Forwarded to the
    /// underlying `text_input` primitive's `keyboard`. A hint, not
    /// validation: the field still accepts any text (paste, a hardware
    /// keyboard). Composes with `secure` (a masked `Number` field is a PIN
    /// pad). Desktop platforms have no soft keyboard and ignore it.
    /// Reactive: a `Signal`/`rx!` re-applies in place without rebuilding the
    /// input. Default `KeyboardKind::Default`.
    #[schema(constraint = "reactive: static KeyboardKind or Signal/rx!")]
    pub keyboard: KeyboardKind,
    /// Leading adornment (icon/element) rendered before the input, inside the
    /// field box. Default [`Adornment::None`]. See [`Adornment`]. An
    /// element-builder (children category) — kept bare; reactivity is internal
    /// (`Adornment::element(move || …reactive content…)`), not `Reactive<_>`.
    #[prop(static)]
    pub leading: Adornment,
    /// Trailing adornment rendered after the input, inside the field box.
    /// Default [`Adornment::None`]. See [`Adornment`]. Element-builder
    /// (children category) — kept bare; reactivity is internal, not
    /// `Reactive<_>` (an element-builder isn't `PartialEq`, so it can't route).
    #[prop(static)]
    pub trailing: Adornment,
    /// Pin an exact minimum input height in pixels. When set, it layers a
    /// `min-height` on top of the size-derived height so the input can't
    /// shrink below it — use this to match a design's exact px floor (e.g.
    /// `min_height = Some(48.0)`) instead of guessing a `size`. `None`
    /// (the default) leaves the height fully size-derived.
    #[schema(constraint = "pixels; None = size-derived height")]
    pub min_height: Option<f32>,
    /// Pin an exact input width in pixels. `None` (the default) lets the
    /// input fill its column as usual; `Some(px)` fixes the width.
    #[schema(constraint = "pixels; None = fill column")]
    pub width: Option<f32>,
    /// Bind the field's underlying input to a `Ref<TextInputHandle>` for
    /// imperative `focus()` / `blur()` / `select_all()`. `None` (default) binds
    /// nothing. Works for both plain and adorned layouts (the ref tracks the
    /// inner input either way).
    #[prop(static)]
    pub field_ref: Option<runtime_core::Ref<runtime_core::primitives::text_input::TextInputHandle>>,
    /// Observe the input's focus transitions (`true` = gained focus,
    /// `false` = blurred). The Field already bridges `on_focus` internally
    /// for its focus ring; this forwards the same transitions to the host —
    /// e.g. `DateInput` normalizes its text on blur. `None` (default)
    /// forwards nothing.
    pub on_focus_change: Option<Rc<dyn Fn(bool)>>,
    /// Intercept key presses on the inner input before the platform's
    /// default handling (see [`runtime_core::primitives::key`]). Return
    /// [`KeyOutcome::PreventDefault`] to swallow a key — e.g.
    /// `DateInput` turns Tab into "complete the current date segment".
    /// `None` (default) attaches no handler.
    pub on_key_down: Option<Rc<dyn Fn(&KeyEvent) -> KeyOutcome>>,
    /// Focus the input once, right after the Field mounts (HTML
    /// `autofocus`) — e.g. a command palette's search box. Mount-time only
    /// (a plain `bool`, not reactive); when several autofocus fields mount
    /// together the last one wins. Works inside a `Modal`/portal: the
    /// focus waits for the input to be attached, so no timer is needed.
    /// Default `false`.
    #[prop(static)]
    pub autofocus: bool,
    /// Make the field read-only and inert — e.g. a frozen, published
    /// version of a settings form. While `true` the input is not editable
    /// and not keyboard-focusable (it drops focus if it held it),
    /// `on_change` never fires, `autofocus` is skipped, the field dims (the
    /// input sheet's `state disabled` overlay; an adorned field dims its
    /// whole box) and `Adornment::Button`s are disabled with it. Default
    /// `false`. Reactive: pass a `Signal<bool>`/`rx!` and the field enables
    /// and disables in place, without rebuilding the input.
    #[schema(constraint = "reactive: static bool or Signal/rx!")]
    pub disabled: bool,
    /// The input's accessible name — what screen readers announce and what
    /// the robot reports as the input's `label`. Defaults to `label`. Set
    /// it when the visible label is drawn OUTSIDE this component (a custom
    /// label row), so the input is still named. Snapshot at build, like
    /// every a11y name here.
    #[prop(static)]
    pub accessibility_label: Option<String>,
}

impl Default for FieldProps {
    fn default() -> Self {
        Self {
            accessibility_label: None,
            label: Reactive::Static(None),
            label_hint: Reactive::Static(None),
            value: runtime_core::signal(String::new()),
            on_change: Rc::new(|_| {}),
            placeholder: Reactive::Static(None),
            help: Reactive::Static(None),
            error: Reactive::Static(None),
            tone: Reactive::Static(None),
            size: Reactive::Static(FieldSize::default()),
            variant: Reactive::Static(FieldAppearance::default()),
            secure: Reactive::Static(false),
            keyboard: Reactive::Static(KeyboardKind::Default),
            leading: Adornment::None,
            trailing: Adornment::None,
            min_height: Reactive::Static(None),
            width: Reactive::Static(None),
            field_ref: None,
            on_focus_change: None,
            on_key_down: None,
            autofocus: false,
            disabled: Reactive::Static(false),
            styles: Default::default(),
        }
    }
}

/// Renders a labeled text input with optional helper/error text. Composes
/// an optional label, a `text_input` styled by the size × appearance × tone
/// axes of [`FieldStyles::input`], and a helper/error line (error takes
/// precedence) into a column.
#[component]
pub fn Field(props: &FieldProps) -> Element {
    let value = props.value;
    let on_change = props.on_change.clone();
    let styles = props.styles.clone();

    // `size` snapshot, used only for the adornment SIZING below
    // (size-derived icon px). A live `size` re-styles the INPUT through its
    // reactive style.
    let size = props.size.get();

    let error = props.error.clone();

    // The effective tone key, reading `tone`/`error` LIVE: explicit tone
    // wins; otherwise Danger while an error shows. The tone matters
    // reactively only when it's a live prop, or absent-and-derived from a
    // live `error` (D9 — an explicit fixed tone makes `error`
    // style-irrelevant).
    let tone_key_for = {
        let tone = props.tone.clone();
        let error = error.clone();
        move || field_tone_key(tone.get(), error.get().is_some())
    };

    // Size and appearance, shared by the input and the adorned shell.
    let values = FieldStyles::values().set(props.size.clone()).set(props.variant.clone());

    // The input's slot style. The focus ring rides the `ring` AXIS and the
    // per-instance dims (`min_height` / `width`) the INLINE layer —
    // independent layers, so focusing a field with `min_height` keeps the
    // height, and BOTH focus states premint (a computed layer here made
    // the focused evaluation non-premintable: focusing any text input
    // panicked under `--premint-only`).
    let make_input_style = {
        let values = values.clone();
        let styles = styles.clone();
        let min_height = props.min_height.clone();
        let width = props.width.clone();
        let tone_key_for = tone_key_for.clone();
        move |focused: Signal<bool>| -> runtime_core::SlotStyle<FieldStyles> {
            let mut style = FieldStyles::input()
                .values(&values)
                .raw_with("tone", tone_key_for.clone())
                .set(derived(move || FieldRing::from(focused.get())));
            let dims = |mh: Option<f32>, w: Option<f32>| StyleRules {
                min_height: mh.map(|h| Tokenized::Literal(Length::Px(h))),
                width: w.map(|w| Tokenized::Literal(Length::Px(w))),
                ..Default::default()
            };
            if let (Reactive::Static(mh), Reactive::Static(w)) = (&min_height, &width) {
                if mh.is_some() || w.is_some() {
                    style = style.inline(dims(*mh, *w));
                }
            } else {
                // Live dims: an inline layer that tracks them.
                let (mh, w) = (min_height.clone(), width.clone());
                style = style.inline_with(move || dims(mh.get(), w.get()));
            }
            style.overridden(styles.input.as_ref())
        }
    };

    let label_node =
        crate::components::field_label_line(props.label.clone(), props.label_hint.clone(), &styles);

    // error wins over help. Combine into one `Reactive<Option<String>>`,
    // staying `Static` when both inputs are (no Effect, and no empty
    // help slot when both are absent).
    let help_combined = match (props.error.clone(), props.help.clone()) {
        (Reactive::Static(e), Reactive::Static(h)) => Reactive::Static(e.or(h)),
        (e, h) => Reactive::Dynamic(Rc::new(move || e.get().or_else(|| h.get()))),
    };

    // The help line's tone tracks the input tone. When `tone` or `error` is
    // LIVE the help style must be reactive: the tone derives from `error`,
    // so a build-time key snapshots `default` (muted grey) for a Field
    // whose error starts `None`, and the first validation error would then
    // paint grey instead of Danger. With both static it stays a one-shot
    // (premintable, Effect-free) application.
    let help_style = if props.tone.is_static() && error.is_static() {
        FieldStyles::help().values(&values).raw("tone", tone_key_for())
    } else {
        FieldStyles::help().values(&values).raw_with("tone", tone_key_for.clone())
    };
    let help_node = crate::components::optional_reactive_text(
        help_combined,
        help_style.overridden(styles.help.as_ref()),
    );

    let secure = props.secure.clone();
    let leading = render_adornment(&props.leading, size, &props.disabled, &styles);
    let trailing = render_adornment(&props.trailing, size, &props.disabled, &styles);
    let adorned = !leading.is_empty() || !trailing.is_empty();

    // `placeholder` is routed LIVE: a reactive source updates the native
    // placeholder in place (no rebuild); a `Static` one sets it once.
    let mut input = runtime_core::text_input(value, move |v: String| (on_change)(v))
        .secure(secure)
        .keyboard(props.keyboard.clone())
        .placeholder_reactive(props.placeholder.clone())
        .autofocus(props.autofocus);
    // Accessible name: the explicit override, else the visible label. The
    // label is a sibling of the input, so the input can't derive it.
    // Snapshot — the a11y prop bag is plain data (Checkbox does the same).
    if let Some(name) = props.accessibility_label.clone().or_else(|| props.label.get()) {
        input = input.a11y_label(name);
    }
    if let Some(field_ref) = props.field_ref.clone() {
        input = input.bind(field_ref);
    }
    // Attached only when present (a silent no-op handler would still
    // claim every key press on some backends).
    if let Some(on_key) = props.on_key_down.clone() {
        input = input.on_key_down(move |e| (on_key)(e));
    }
    // `disabled` rides the primitive: native inert + out of keyboard focus,
    // the `on_change` gate, and the DISABLED state bit (the slot's `state
    // disabled` dim). Attached only when the field can be disabled — a
    // `Static(false)` field carries no binding (Switch / Button parity).
    let input = match props.disabled.clone() {
        Reactive::Static(false) => input,
        Reactive::Static(true) => input.disabled(true),
        live => input.disabled(move || live.get()),
    };

    let focused = runtime_core::signal(false);
    let notify_focus = props.on_focus_change.clone();
    let input = input.on_focus(move |f| {
        focused.set(f);
        if let Some(cb) = &notify_focus {
            cb(f);
        }
    });

    // The "field box" — either the bare input (no adornments) or a flex-row
    // SHELL wrapping a bare input with leading/trailing adornments.
    //
    // FOCUS RING (both layouts): `on_focus` → `focused` → the `ring` axis —
    // NOT the slot's `__state_focused` overlay. A plain `view` shell can't
    // receive the inner input's FOCUSED state, and on macOS the state
    // overlay does not repaint a native text input's border; `on_focus`
    // fires reliably from the field-editor engage/resign. One mechanism for
    // both layouts keeps the rings identical. INVARIANT (D9): the input
    // style is therefore always reactive.
    let field_box: Element = if adorned {
        // ADORNED: the chrome lives on the row SHELL; the input goes flat
        // and fills the row (the `placement = shell` arm).
        let bare_style = FieldStyles::input()
            .values(&values)
            .set(FieldAppearance::Bare)
            .set(FieldPlacement::Shell)
            .overridden(styles.input.as_ref());
        let input_node = input.with_style(bare_style).into_element();
        // The shell: field chrome + tone border + row layout (`adorned`),
        // the `ring` while the inner input is focused, `dimmed` while
        // disabled — every arm build-time CSS, so a focus flip is a class
        // swap.
        let shell_disabled = props.disabled.clone();
        let shell_style = make_input_style(focused)
            .set(FieldAdorned::On)
            .set(derived(move || FieldDimmed::from(shell_disabled.get())));
        // The adornments are RECEIVED children (props), splatted around
        // the input.
        ui! {
            view(style = shell_style) {
                leading
                input_node
                trailing
            }
        }
    } else {
        input.with_style(make_input_style(focused)).into_element()
    };

    let group_style = FieldStyles::group().values(&values).overridden(styles.group.as_ref());
    // `label_node` / `help_node` are `Option<Element>` — an absent one
    // splats to nothing.
    ui! {
        view(style = group_style) {
            label_node
            field_box
            help_node
        }
    }
}

recipe!(
    Field,
    /// Password field with a show/hide toggle, built from a reactive `secure`
    /// plus a trailing adornment. `secure = rx!(!visible.get())` makes the mask
    /// itself reactive: the Field is NOT wrapped in a `switch`, the underlying
    /// `text_input` is never rebuilt, and the typed `value` is never disturbed
    /// when the mask toggles — the framework flips the native secure-entry mode
    /// in place (on macOS, an in-place `NSSecureTextFieldCell` swap). The eye
    /// glyph is a reactive `text` leaf that flips with the same `visible`
    /// signal, so nothing in the tree is rebuilt on toggle. Swap the emoji for
    /// `icon = Some(icons_lucide::EYE/EYE_OFF)` in an app with an icon pack.
    pub fn field_password_with_visibility_toggle() -> ::runtime_core::Element {
        use crate::components::field::{Adornment, Field};
        use ::runtime_core::{pressable, rx, signal, text, ui, IntoElement};
        use ::std::rc::Rc;

        let value = signal(String::new());
        let visible = signal(false);
        let on_change: Rc<dyn Fn(String)> = Rc::new(move |v: String| value.set(v));
        let toggle: Rc<dyn Fn()> = Rc::new(move || visible.set(!visible.get()));

        ui! {
            Field(
                value = value,
                on_change = on_change,
                placeholder = "Password".to_string(),
                secure = rx!(!visible.get()),
                trailing = Adornment::element(move || {
                    let toggle = toggle.clone();
                    let glyph = text(move || {
                        if visible.get() { "🙈".to_string() } else { "👁".to_string() }
                    })
                    .into_element();
                    pressable(vec![glyph], move || (toggle)()).into_element()
                }),
            )
        }
    }
);

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::{classify, P, TStyle};
    use idea_theme::testing::with_test_world;
    use idea_theme::theme::install_idea_theme;
    use idea_theme::theme::light_theme;
    use runtime_core::resolve_style;

    /// The Field group's first child: its label line.
    fn label_line(field: Element) -> Element {
        let P::View { mut children, .. } = classify(field) else {
            panic!("a Field is a group View");
        };
        children.remove(0)
    }

    fn text_of(el: Element) -> (Option<String>, Option<std::rc::Rc<runtime_core::StyleRules>>) {
        match classify(el) {
            P::Text { text, style, .. } => (text, style.map(|s| s.resolve())),
            other => panic!("expected a text node, got {}", match other {
                P::View { .. } => "view", P::Fragment { .. } => "fragment", P::Other(k) => k, P::TextInput { .. } => "text_input", P::Pressable { .. } => "pressable", P::Icon { .. } => "icon", _ => "other" }),
        }
    }

    /// `label_hint` draws a muted note beside the label ("Full name
    /// Optional"): the label line becomes a row of [label, hint], the hint
    /// in the theme's muted color. Without it the label stays a bare text.
    #[test]
    fn label_hint_draws_a_muted_note_beside_the_label() {
        with_test_world(|| {
            install_idea_theme(light_theme());
            let field = Field(&FieldProps {
                label: Reactive::Static(Some("Full name".into())),
                label_hint: Reactive::Static(Some("Optional".into())),
                ..Default::default()
            });
            let P::View { mut children, .. } = classify(label_line(field)) else {
                panic!("a hinted label is a row View");
            };
            assert_eq!(children.len(), 2, "[label, hint]");
            let (hint, hint_rules) = text_of(children.remove(1));
            let (label, label_rules) = text_of(children.remove(0));
            assert_eq!(label.as_deref(), Some("Full name"));
            assert_eq!(hint.as_deref(), Some("Optional"));
            let color = |r: Option<std::rc::Rc<runtime_core::StyleRules>>| {
                r.and_then(|r| r.color.clone()).map(|c| c.resolve())
            };
            assert_eq!(color(hint_rules), Some(idea_theme::tokens().color.text_muted().resolve()));
            assert_ne!(color(label_rules), Some(idea_theme::tokens().color.text_muted().resolve()));

            let plain = Field(&FieldProps {
                label: Reactive::Static(Some("Meters".into())),
                ..Default::default()
            });
            assert_eq!(text_of(label_line(plain)).0.as_deref(), Some("Meters"), "no hint, bare label");
        });
    }

    /// User-reported `--premint-only` panic: FOCUSING any text input.
    /// The input's style closure re-evaluates with `focused = true`, and
    /// that evaluation carried a `with_computed` layer (ring + dims in
    /// one slot) — non-premintable, so the reactive po arm panicked on
    /// the focus flip. Both evaluations must premint: the ring is the
    /// sheet's `ring` AXIS and per-instance dims ride the INLINE layer.
    /// Fails against the computed spelling (`preminted_class_list()` is
    /// `None` for a computed-carrying application).
    #[test]
    fn regression_focused_input_evaluation_premints() {
        with_test_world(|| {
            install_idea_theme(light_theme());
            let field = Field(&FieldProps {
                label: Reactive::Static(Some("Email".into())),
                min_height: Reactive::Static(Some(120.0)),
                ..Default::default()
            });
            let style = input_style_source(field);
            let app_of = |s: &TStyle| match s {
                TStyle::AppFn(f) => f(),
                _ => panic!("the Field input style is a reactive closure (INVARIANT D9)"),
            };
            // Resting evaluation (not focused).
            let resting = app_of(&style);
            assert!(
                resting.preminted_class_list().is_some(),
                "resting input evaluation must premint (dims ride inline)"
            );
            // The FOCUSED shape: same axes plus ring=on — build it the way
            // the closure does and assert it premints too. (Flipping the
            // component's internal focus signal isn't reachable from the
            // mirror; the ring arm's premintability is the load-bearing
            // half, and the closure's two branches differ only by it.)
            let focused = resting.clone().with("ring", "on");
            assert_ne!(
                focused.preminted_class_list(),
                resting.preminted_class_list(),
                "the ring is a stamped class"
            );
            assert!(
                focused.preminted_class_list().is_some(),
                "focused input evaluation must premint (ring is an enumerated axis)"
            );
        });
    }

    /// Pull the normalized style off the `text_input` node inside a built
    /// `Field` element tree (a `view` wrapping label/input/help).
    fn input_style_source(field: Element) -> TStyle {
        let children = match classify(field) {
            P::View { children, .. } => children,
            _ => panic!("Field renders a view wrapper"),
        };
        for c in children {
            if let P::TextInput { style, .. } = classify(c) {
                return style.expect("Field's text_input always has a style");
            }
        }
        panic!("Field tree has no text_input node");
    }

    /// Resolve a Field input's style to `StyleRules`, whether the source is
    /// static or reactive. Since the D9 focus-ring fix a non-adorned
    /// Field's input is ALWAYS a reactive closure (see the component body),
    /// so behavioral assertions resolve through whichever variant is present.
    fn resolve_input_style(field: Element) -> runtime_core::StyleRules {
        (*input_style_source(field).resolve()).clone()
    }

    fn theme() {
        install_idea_theme(light_theme());
    }

    // D9 regression: a Field whose `error` is a live signal must attach
    // its input style as `StyleSource::Reactive`, and flipping the error
    // signal must change the resolved border color. Before the fix the
    // tone was snapshotted into a `StyleSource::Static` at build, so the
    // border never turned red on live validation (only the error TEXT did).
    #[test]
    fn reactive_error_drives_border_color_live() {
        with_test_world(|| {
            theme();
            let err: Signal<Option<String>> = runtime_core::signal(None);
            let props = FieldProps {
                error: err.into(),
                ..Default::default()
            };
            let src = input_style_source(Field(&props));

            let closure = match src {
                TStyle::AppFn(f) => f,
                _ => panic!(
                    "a Field with a reactive `error` must attach a reactive style \
                     source so the border re-resolves on validation (D9 regression)"
                ),
            };

            // No error → neutral border. The closure reads the signal each call.
            let border_none = resolve_style(&closure()).border_top_color.clone();
            err.set(Some("Required".into()));
            idea_theme::testing::commit();
            let border_err = resolve_style(&closure()).border_top_color.clone();

            assert!(
                border_none.is_some() && border_err.is_some(),
                "border color is set in both states"
            );
            assert_ne!(
                border_none, border_err,
                "flipping the error signal must change the input's border color \
                 (Danger tone vs neutral)"
            );
    });
    }

    // D9 (updated for the always-reactive focus-ring path): an EXPLICIT tone
    // overrides the error-derived tone. The static-fast-path guarantee this
    // test once asserted is gone by design (the non-adorned input now always
    // carries one style Effect for the macOS focus ring — see the component
    // body); what still must hold is the tone-precedence behavior, verified
    // by resolving the reactive closure.
    #[test]
    fn explicit_tone_overrides_error_derived_tone() {
        with_test_world(|| {
            theme();
            // Explicit Warning tone + a reactive error: the explicit tone wins,
            // so the resolved border color must NOT change when the error flips.
            let err: Signal<Option<String>> = runtime_core::signal(None);
            let props = FieldProps {
                error: err.into(),
                tone: Reactive::Static(Some(tones::Warning.into())),
                ..Default::default()
            };
            let border_no_err = resolve_input_style(Field(&props)).border_top_color.clone();
            err.set(Some("bad".into()));
            idea_theme::testing::commit();
            let border_err = resolve_input_style(Field(&props)).border_top_color.clone();
            assert!(border_no_err.is_some(), "explicit tone sets a border color");
            assert_eq!(
                border_no_err, border_err,
                "an explicit tone must override the error-derived tone — the border \
                 color stays the Warning tone regardless of the error signal"
            );
    });
    }

    // D6: a `min_height` prop pins the exact min-height in px.
    #[test]
    fn min_height_prop_sets_min_height_style() {
        with_test_world(|| {
            theme();
            let props = FieldProps {
                min_height: Reactive::Static(Some(48.0)),
                ..Default::default()
            };
            let rules = resolve_input_style(Field(&props));
            assert_eq!(
                rules.min_height,
                Some(Tokenized::Literal(Length::Px(48.0))),
                "min_height prop must pin an exact px min-height"
            );
    });
    }

    // D6: width prop pins an exact px width.
    #[test]
    fn width_prop_sets_width_style() {
        with_test_world(|| {
            theme();
            let props = FieldProps {
                width: Reactive::Static(Some(240.0)),
                ..Default::default()
            };
            let rules = resolve_input_style(Field(&props));
            assert_eq!(rules.width, Some(Tokenized::Literal(Length::Px(240.0))));
    });
    }

    /// The `autofocus` flag on the Field's `text_input`, found anywhere in
    /// the view tree (the adorned layout nests it inside the row shell).
    fn input_autofocus(el: Element) -> Option<bool> {
        match classify(el) {
            P::TextInput { autofocus, .. } => Some(autofocus),
            P::View { children, .. } => children.into_iter().find_map(input_autofocus),
            _ => None,
        }
    }

    /// `autofocus` reaches the primitive in both layouts, and defaults off.
    #[test]
    fn autofocus_forwards_to_the_text_input() {
        with_test_world(|| {
            theme();
            let on = FieldProps { autofocus: true, ..Default::default() };
            assert_eq!(input_autofocus(Field(&on)), Some(true));
            let adorned = FieldProps {
                autofocus: true,
                leading: Adornment::element(|| runtime_core::ui! { view() }),
                ..Default::default()
            };
            assert_eq!(input_autofocus(Field(&adorned)), Some(true), "adorned layout");
            assert_eq!(input_autofocus(Field(&FieldProps::default())), Some(false));
        });
    }

    /// The built Field's `text_input` accessible name.
    fn input_a11y_label(field: Element) -> Option<String> {
        let P::View { children, .. } = classify(field) else { panic!("Field renders a view wrapper") };
        children
            .into_iter()
            .find_map(|c| match classify(c) {
                P::TextInput { a11y_label, .. } => Some(a11y_label),
                _ => None,
            })
            .expect("Field tree has a text_input node")
    }

    /// A Field's input had no accessible name: its visible label is a
    /// sibling Text, so screen readers announced an unnamed field and the
    /// robot reported `label: null` for every form input.
    #[test]
    fn regression_field_label_names_its_input() {
        with_test_world(|| {
            theme();
            let labeled = FieldProps { label: Reactive::Static(Some("Email".into())), ..Default::default() };
            assert_eq!(input_a11y_label(Field(&labeled)).as_deref(), Some("Email"));
            // A label drawn outside the Field names it explicitly.
            let external = FieldProps {
                accessibility_label: Some("Full Name".into()),
                ..Default::default()
            };
            assert_eq!(input_a11y_label(Field(&external)).as_deref(), Some("Full Name"));
            assert_eq!(input_a11y_label(Field(&FieldProps::default())), None);
        });
    }

    /// The built Field's `text_input` secure flag, evaluated NOW.
    fn input_secure(field: Element) -> bool {
        let children = match classify(field) {
            P::View { children, .. } => children,
            _ => panic!("Field renders a view wrapper"),
        };
        for c in children {
            if let P::TextInput { secure, .. } = classify(c) {
                return secure;
            }
        }
        panic!("Field tree has no text_input node");
    }

    // A live `secure` source must thread to the `text_input` reactively —
    // NOT snapshotted at build — so the mask can toggle at runtime without
    // rebuilding the Field (the password show/hide path). Observable form:
    // flip the signal AFTER the Field
    // is built, then evaluate — a build-time snapshot would still report
    // the old value.
    #[test]
    fn reactive_secure_threads_through_not_flattened() {
        with_test_world(|| {
            theme();
            let visible: Signal<bool> = runtime_core::signal(false);
            let props = FieldProps {
                secure: runtime_core::rx!(!visible.get()),
                ..Default::default()
            };
            let field = Field(&props);
            // Built with secure = !false = true; reveal AFTER build.
            visible.set(true);
            idea_theme::testing::commit();
            assert!(
                !input_secure(field),
                "a reactive `secure` must reach the text_input live, not as a build-time snapshot"
            );
    });
    }

    /// The built Field's `text_input` keyboard hint, evaluated NOW. Walks
    /// the whole tree so it finds the input in the plain and the adorned
    /// (shell-wrapped) layouts alike.
    fn input_keyboard(el: Element) -> Option<KeyboardKind> {
        match classify(el) {
            P::TextInput { keyboard, .. } => Some(keyboard),
            P::View { children, .. } => children.into_iter().find_map(input_keyboard),
            _ => None,
        }
    }

    /// REGRESSION: `Field` had no keyboard prop, so an email field opened
    /// the default soft keyboard on iOS/Android (no `@` / `.com` row) and a
    /// phone field got no digit pad. The kind must reach the `text_input`
    /// in both layouts, and a live kind must thread through un-snapshotted.
    #[test]
    fn regression_field_forwards_keyboard_to_its_input() {
        with_test_world(|| {
            theme();
            assert_eq!(
                input_keyboard(Field(&FieldProps::default())),
                Some(KeyboardKind::Default),
                "no hint by default"
            );
            let email = FieldProps { keyboard: Reactive::Static(KeyboardKind::Email), ..Default::default() };
            assert_eq!(input_keyboard(Field(&email)), Some(KeyboardKind::Email));
            let adorned = FieldProps {
                keyboard: Reactive::Static(KeyboardKind::Phone),
                leading: Adornment::element(|| runtime_core::ui! { view() }),
                ..Default::default()
            };
            assert_eq!(input_keyboard(Field(&adorned)), Some(KeyboardKind::Phone));

            let decimal: Signal<bool> = runtime_core::signal(false);
            let live = FieldProps {
                keyboard: runtime_core::rx!(if decimal.get() {
                    KeyboardKind::Decimal
                } else {
                    KeyboardKind::Number
                }),
                ..Default::default()
            };
            let field = Field(&live);
            decimal.set(true);
            idea_theme::testing::commit();
            assert_eq!(
                input_keyboard(field),
                Some(KeyboardKind::Decimal),
                "a reactive `keyboard` must reach the text_input live, not as a build-time snapshot"
            );
        });
    }

    // A live `size` (or any style-driving prop) must attach the input style
    // as `Reactive` so the field re-styles IN PLACE when it changes — props
    // are routed into the style sink, not snapshotted at build.
    #[test]
    fn reactive_size_drives_reactive_input_style() {
        with_test_world(|| {
            theme();
            let big: Signal<bool> = runtime_core::signal(false);
            let props = FieldProps {
                size: runtime_core::rx!(if big.get() { FieldSize::Lg } else { FieldSize::Sm }),
                ..Default::default()
            };
            assert!(
                input_style_source(Field(&props)).is_reactive(),
                "a reactive `size` must attach a reactive input style (routed to the \
                 style sink, not snapshotted)"
            );
    });
    }

    // Regression: a Field with a LIVE error channel (every typed input —
    // DateInput et al. — routes one in) must not mount an EMPTY help line
    // while the error is `None`. The mounted empty text kept a caption
    // line box plus a slot in the group's gap, so typed inputs sat
    // permanently taller than a plain Field. The help line is a guarded
    // hole now — no text node until the source turns `Some`.
    #[test]
    fn regression_live_error_none_mounts_no_empty_help_line() {
        with_test_world(|| {
            theme();
            let err: Signal<Option<String>> = runtime_core::signal(None);
            let props = FieldProps {
                error: err.into(),
                ..Default::default()
            };
            for c in classify(Field(&props)).children() {
                assert!(
                    !matches!(classify(c), P::Text { .. }),
                    "no help text node may be mounted while the live error is None"
                );
            }
        });
    }

    // The help line now mounts ON DEMAND (first validation error), so its
    // evaluation must premint like any open-on-demand surface — otherwise
    // a `--premint-only` app panics on the first error. (The sheet itself
    // is still constructed/installed by every Field build via
    // `help_style`, so the premint dump sees it.)
    #[test]
    fn help_line_evaluation_premints() {
        with_test_world(|| {
            theme();
            let danger: ToneRef = tones::Danger.into();
            let app = FieldStyles::help().raw("tone", danger.key()).into_style_application();
            assert!(
                app.preminted_class_list().is_some(),
                "the on-demand help line must premint (it constructs on first error)"
            );
        });
    }

    // Regression (user-reported): an ADORNED Field sat visibly taller than
    // a bare one. The `adorned=on` arm zeroes the shell's vertical padding,
    // but axes merge ALPHABETICALLY ("adorned" < "size"), so the size
    // axis's vertical padding re-applied OVER it — in the runtime resolver
    // and as equal-specificity stacked classes in the premint cascade
    // alike. The (adorned, size) compounds re-zero it; compounds resolve
    // after every variant arm, so they win in both worlds.
    #[test]
    fn regression_adorned_shell_drops_size_axis_vertical_padding() {
        with_test_world(|| {
            theme();
            for size in [FieldSize::Sm, FieldSize::Md, FieldSize::Lg] {
                let rules = resolve_style(
                    &FieldStyles::input().set(size).set(FieldAdorned::On).into_style_application(),
                );
                assert_eq!(
                    rules.padding_top,
                    Some(Tokenized::Literal(Length::Px(0.0))),
                    "adorned {size:?} shell must drop the size axis's top padding"
                );
                assert_eq!(
                    rules.padding_bottom,
                    Some(Tokenized::Literal(Length::Px(0.0))),
                    "adorned {size:?} shell must drop the size axis's bottom padding"
                );
            }
        });
    }

    /// The resolved style of the `text_input` anywhere in a built Field
    /// (the adorned layout nests it inside the row shell).
    fn nested_input_rules(el: Element) -> Option<runtime_core::StyleRules> {
        match classify(el) {
            P::TextInput { style, .. } => {
                Some((*style.expect("Field's text_input always has a style").resolve()).clone())
            }
            P::View { children, .. } => children.into_iter().find_map(nested_input_rules),
            _ => None,
        }
    }

    // Regression (CrewForge want_f5f5e732): a leading-adorned Field's input
    // got the size axis's horizontal padding (12px at `md`) instead of the
    // 2px shell inset, so the text sat far from the icon. `placement` <
    // `size` alphabetically, so the `size` arm's padding merged OVER the
    // `placement: shell` arm's. The (placement: shell, size) compounds
    // restate the inset; compounds outrank every arm.
    #[test]
    fn regression_adorned_input_keeps_shell_inset_at_every_size() {
        with_test_world(|| {
            theme();
            let inset = Some(Tokenized::Literal(Length::Px(FIELD_BARE_H_PAD)));
            for size in [FieldSize::Sm, FieldSize::Md, FieldSize::Lg] {
                let props = FieldProps {
                    size: Reactive::Static(size),
                    leading: Adornment::Icon(GLYPH_A),
                    ..Default::default()
                };
                let rules = nested_input_rules(Field(&props)).expect("adorned Field has an input");
                assert_eq!(rules.padding_left, inset, "adorned {size:?} input left padding");
                assert_eq!(rules.padding_right, inset, "adorned {size:?} input right padding");
                // The size-derived vertical padding stays: it sets the
                // field's height.
                let plain = resolve_style(&FieldStyles::input().set(size).into_style_application());
                assert_eq!(rules.padding_top, plain.padding_top, "adorned {size:?} keeps vertical padding");
            }
        });
    }

    const GLYPH_A: IconData = IconData {
        view_box: (24, 24),
        paths: &["M0 0h24"],
        fill_rule: runtime_core::FillRule::NonZero,
        filled: false,
    };
    const GLYPH_B: IconData = IconData {
        view_box: (24, 24),
        paths: &["M0 0v24", "M0 24h24"],
        fill_rule: runtime_core::FillRule::NonZero,
        filled: false,
    };

    // `Adornment::Group` flattens its members as ORDERED siblings in the
    // shell row — DateInput's clearable ✕ + calendar pair relies on this.
    #[test]
    fn trailing_group_adornment_renders_members_as_row_siblings() {
        with_test_world(|| {
            theme();
            let props = FieldProps {
                trailing: Adornment::Group(vec![
                    Adornment::button(GLYPH_A, || {}),
                    Adornment::button(GLYPH_B, || {}),
                ]),
                ..Default::default()
            };
            let mut group = classify(Field(&props)).children();
            let shell = classify(group.remove(0)).children();
            let paths: Vec<_> = shell
                .into_iter()
                .map(|c| match classify(c) {
                    P::TextInput { .. } => "input".to_string(),
                    P::Pressable { children, .. } => {
                        match classify(children.into_iter().next().unwrap()) {
                            P::Icon { data, .. } => format!("icon:{}", data.paths.len()),
                            _ => panic!("Button adornment wraps an icon"),
                        }
                    }
                    _ => panic!("unexpected shell child"),
                })
                .collect();
            assert_eq!(
                paths,
                vec!["input", "icon:1", "icon:2"],
                "group members render in order after the input"
            );
        });
    }

    // An empty group is NO adornment: the Field must keep the plain
    // (shell-less) input path.
    #[test]
    fn empty_group_adornment_keeps_the_plain_input_path() {
        with_test_world(|| {
            theme();
            let props = FieldProps {
                trailing: Adornment::Group(Vec::new()),
                ..Default::default()
            };
            let mut group = classify(Field(&props)).children();
            assert!(
                matches!(classify(group.remove(0)), P::TextInput { .. }),
                "an empty group must not create the adorned shell"
            );
        });
    }

    // Since the D9 focus-ring fix, a non-adorned Field's input style is
    // ALWAYS a reactive closure (it reads a `focused` signal to overlay the
    // theme focus ring — the sheet's `__state_focused` overlay doesn't
    // repaint a native input's border on macOS). The former all-static fast
    // path is intentionally gone; this test pins the new invariant so it
    // isn't silently regressed back.
    #[test]
    fn non_adorned_input_is_reactive_for_focus_ring() {
        with_test_world(|| {
            theme();
            let props = FieldProps {
                size: Reactive::Static(FieldSize::Lg),
                ..Default::default()
            };
            assert!(
                input_style_source(Field(&props)).is_reactive(),
                "a non-adorned Field input carries a reactive style for the focus ring"
            );
    });
    }


    fn reset_styles() {
        runtime_core::component_styles::__reset::<FieldStyles>();
    }

    /// An app-defined appearance: `FieldAppearance::define("pill")` plus a
    /// restyle arm on the input slot gives it a look; the field draws it, the
    /// restyled input still premints, and an UNSTYLED app value warns.
    #[test]
    fn custom_appearance_draws_through_a_restyle_arm_and_premints() {
        const PILL: FieldAppearance = FieldAppearance::define("pill");
        with_test_world(|| {
            reset_styles();
            theme();
            FieldStyles::restyle(|s| {
                s.input().arm(PILL, |t| StyleRules {
                    border_top_left_radius: Some(t.radius.pill()),
                    background: Some(t.color.surface_alt()),
                    ..Default::default()
                });
            });
            let props = FieldProps { variant: Reactive::Static(PILL), ..Default::default() };
            let rules = resolve_input_style(Field(&props));
            assert_eq!(rules.border_top_left_radius.as_ref().and_then(|r| r.name()), Some("radius-pill"));
            assert_eq!(rules.background.as_ref().and_then(|c| c.name()), Some("color-surface-alt"));
            let app = FieldStyles::input().set(PILL).into_style_application();
            let classes = app.preminted_class_list().expect("the restyled input premints");
            assert!(classes.contains("-appearance-pill"), "{classes}");
            reset_styles();
        });
    }

    /// A restyle of the input's base reaches every field app-wide, and the
    /// `styles` prop overrides one field on top of it.
    #[test]
    fn restyle_reaches_every_field_and_styles_prop_wins() {
        with_test_world(|| {
            reset_styles();
            theme();
            FieldStyles::restyle(|s| {
                s.input().base(|t| StyleRules { border_top_left_radius: Some(t.radius.lg()), ..Default::default() });
                s.label().base(|t| StyleRules { color: Some(t.color.text_muted()), ..Default::default() });
            });
            let rules = resolve_input_style(Field(&FieldProps::default()));
            assert_eq!(rules.border_top_left_radius.as_ref().and_then(|r| r.name()), Some("radius-lg"));
            let one = FieldProps {
                styles: FieldSlotStyles::default()
                    .input(StyleRules { border_top_left_radius: Some(Length::Px(0.0).into()), ..Default::default() }),
                ..Default::default()
            };
            let rules = resolve_input_style(Field(&one));
            assert_eq!(rules.border_top_left_radius, Some(Tokenized::Literal(Length::Px(0.0))));
            reset_styles();
        });
    }

    /// A tone the app registers reaches the Field's border with no
    /// per-component wiring — registered before OR after the restyle and
    /// the theme install.
    #[test]
    fn a_registered_app_tone_paints_the_field_border() {
        idea_theme::tone! {
            Brand using self, theme {
                key = "brand_field_test",
                fill_bg = theme.colors().text.clone(),
                fill_fg = theme.colors().background.clone(),
                soft_bg = theme.colors().surface_alt.clone(),
                soft_fg = theme.colors().text.clone(),
                stroke_color = idea_theme::color_token!("brand-field-stroke", "#ff00aa"),
                stroke_fg = theme.colors().text.clone(),
                ghost_fg = theme.colors().text.clone(),
                disabled = theme.colors().text_muted.clone(),
                focus_ring = theme.colors().focus_ring.clone(),
            }
        }
        with_test_world(|| {
            reset_styles();
            theme();
            let _ = resolve_input_style(Field(&FieldProps::default()));
            idea_theme::extensible::register_tone(Brand);
            let props = FieldProps { tone: Reactive::Static(Some(Brand.into())), ..Default::default() };
            let rules = resolve_input_style(Field(&props));
            assert_eq!(rules.border_left_color.as_ref().and_then(|c| c.name()), Some("brand-field-stroke"));
            reset_styles();
        });
    }
}
