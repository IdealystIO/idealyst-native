//! `Button` — the styled clickable.
//!
//! ```ignore
//! ui! {
//!     Button(
//!         label = "Save",
//!         on_click = on_save,
//!         tone = tone::Primary,
//!         variant = variant::Filled,
//!         size = size::Md,
//!         shape = shape::Md,
//!     )
//! }
//! ```
//!
//! Styling is [`ButtonStyles`]: a `root` slot (the pressable box), a
//! `label` slot and an `icon` slot. The box and the label draw one
//! compound per `(tone, variant)` pair — every registered tone over every
//! built-in variant recipe — plus `size`, `shape`, `layout`, `block` and
//! `dimmed` arms. Every combination ships as build-time CSS, so applying a
//! style is a class lookup.
//!
//! Extend it at boot: `ButtonStyles::restyle(|s| …)` changes any slot,
//! `ButtonVariant::define("raised")` adds a variant (give it a look with
//! a restyle arm, or a tone-aware one with
//! [`tone_variant_arms`](idea_theme::extensible::tone_variant_arms)), and
//! [`register_tone`](idea_theme::extensible::register_tone) (or a theme's
//! `extension_tones`) adds a tone every tone-aware component draws.

use crate::components::DISABLED_CONTROL_OPACITY;
use std::rc::Rc;

use runtime_core::primitives::activity_indicator::{activity_indicator, ActivityIndicatorSize};
use runtime_core::{
    component, component_styles, icon, resolve_style, text, AlignItems, AlignSelf, Color, Cursor,
    Element, FlexDirection, FontWeight, IconData, IdealystSchema, IntoElement, JustifyContent, Length,
    PressableHandle, Reactive, Ref, SlotRestyle, TextAlign, UserSelect, VariantSet,
};

use idea_theme::extensible::{
    semantic_tones, tone_variant_arms, tone_variant_state_overlays, ToneRef,
};
use idea_theme::theme::IdeaThemeRef;

use crate::components::tone_slots::{foreground_arms, foreground_compounds};
use crate::modifiers::{marker_into_axis, shape, size, variant};

/// Pixel size of a button's leading/trailing icon. Matches the label's
/// cap-height closely enough to sit inline without throwing off the
/// centered row.
const BUTTON_ICON_PX: f32 = 16.0;

component_styles! {
    /// The styled sections of [`Button`].
    pub Button<IdeaThemeRef> {
        /// The surface treatment. Each built-in value is a tone-aware
        /// recipe ([`variant`]) drawn for every registered tone.
        axis variant { #[default] filled, soft, outlined, ghost }

        /// The pressable box.
        slot root {
            extend(root_tone_variants)
            base(_t) {
                font_weight: FontWeight::SemiBold,
                letter_spacing: 0.2,
                text_align: TextAlign::Center,
                // Center the content on BOTH axes: web centers the label via
                // the box's `text-align` + inline flow, native flex needs it
                // explicit or the label lands at the top-left.
                align_items: AlignItems::Center,
                justify_content: JustifyContent::Center,
                // A component library opts in to the interaction
                // affordances; the bare `pressable` imposes neither. Touch
                // backends no-op both.
                cursor: Cursor::Pointer,
                user_select: UserSelect::None,
                // Explicit resting opacity so the hover/press dim has a
                // value to animate back TO: native applies the state
                // overlay by re-resolving, and leaves an unset opacity
                // untouched.
                opacity: 1.0,
            }
            variant size {
                sm(t) {
                    padding_vertical: t.spacing.xs(),
                    padding_horizontal: t.spacing.md(),
                    font_size: t.typography.body_sm_size(),
                }
                #[default]
                md(t) {
                    padding_vertical: t.spacing.sm(),
                    padding_horizontal: t.spacing.lg(),
                    font_size: t.typography.body_size(),
                }
                lg(t) {
                    padding_vertical: t.spacing.md(),
                    padding_horizontal: t.spacing.xl(),
                    font_size: t.typography.body_lg_size(),
                }
            }
            variant shape {
                sm(t) { border_radius: t.radius.sm() }
                #[default]
                md(t) { border_radius: t.radius.md() }
                lg(t) { border_radius: t.radius.lg() }
                pill(t) { border_radius: t.radius.pill() }
            }
            // Icon + label lay out in a row with a gap; a plain label stays
            // a column.
            variant layout {
                #[default]
                column(_t) {}
                row(t) {
                    flex_direction: FlexDirection::Row,
                    gap: t.spacing.xs(),
                }
            }
            // `block` fills the container; otherwise the button hugs its
            // content — `Center`, not `Stretch`, so a flex parent's
            // `align-items: stretch` can't grow it.
            variant block {
                #[default]
                off(_t) { align_self: AlignSelf::Center }
                on(_t) {
                    width: Length::Percent(100.0),
                    align_self: AlignSelf::Stretch,
                }
            }
            // The author's `disabled` prop: a deterministic dim on every
            // backend, whether or not the host marks the node disabled.
            variant dimmed {
                #[default]
                off(_t) {}
                on(_t) { opacity: DISABLED_CONTROL_OPACITY }
            }
            // Registered so hover/press are tracked; the feedback itself is
            // per-(tone, variant) — see `root_tone_variants`.
            state hovered(_t) {}
            state pressed(_t) {}
            transitions {
                background: 120ms EaseOut,
                color: 200ms EaseOut,
                opacity: 200ms EaseOut,
                border_color: 150ms EaseOut,
                // `idea_theme::extensible::variant::GLOW_TRANSITION_MS`.
                shadow: 180ms EaseOut,
            }
        }

        /// The label text. It carries the box's typography and its
        /// `(tone, variant)` foreground on its own node: native text
        /// inherits neither color nor weight/size/alignment from the box.
        slot label {
            extend(label_tone_variants)
            base(_t) {
                font_weight: FontWeight::SemiBold,
                letter_spacing: 0.2,
                text_align: TextAlign::Center,
            }
            variant size {
                sm(t) { font_size: t.typography.body_sm_size() }
                #[default]
                md(t) { font_size: t.typography.body_size() }
                lg(t) { font_size: t.typography.body_lg_size() }
            }
        }

        /// The leading / trailing icon: a fixed square that flex can't
        /// squeeze (icons have no intrinsic content size).
        slot icon {
            base(_t) {
                width: BUTTON_ICON_PX,
                height: BUTTON_ICON_PX,
                flex_shrink: 0.0,
            }
        }
    }
}

/// The box's `(tone, variant)` fills and their hover/press feedback, for
/// every registered tone.
fn root_tone_variants(s: &mut SlotRestyle<'_, ButtonStyles>) {
    let tones = semantic_tones();
    for (key, recipe) in variant::builtin_recipes() {
        tone_variant_arms(s, &tones, key, recipe);
    }
    for (key, _) in variant::builtin_recipes() {
        tone_variant_state_overlays(s, &tones, key);
    }
}

/// The label's `(tone, variant)` foreground — the recipe's `color` alone
/// (a text node takes no surface).
fn label_tone_variants(s: &mut SlotRestyle<'_, ButtonStyles>) {
    foreground_arms(s, &semantic_tones());
}

impl ButtonRestyle<'_, '_> {
    /// Give a variant value a tone-aware look from a surface recipe, the
    /// way the built-in variants get theirs: the box's fill per tone, its
    /// hover / press feedback, and the label's foreground.
    ///
    /// ```ignore
    /// // An app variant…
    /// const RAISED: ButtonVariant = ButtonVariant::define("raised");
    /// ButtonStyles::restyle(|s| s.tone_variant(RAISED, &Raised));
    /// // …or the opt-in gradient.
    /// ButtonStyles::restyle(|s| s.tone_variant(ButtonVariant::Gradient, &variant::Gradient));
    /// ```
    pub fn tone_variant(&mut self, value: ButtonVariant, recipe: &dyn idea_theme::extensible::Variant) {
        let tones = semantic_tones();
        let mut root = self.root();
        tone_variant_arms(&mut root, &tones, value.key(), recipe);
        tone_variant_state_overlays(&mut root, &tones, value.key());
        foreground_compounds(&mut self.label(), &tones, value.key(), recipe);
    }
}

marker_into_axis!(ButtonVariant {
    variant::Filled => Filled,
    variant::Soft => Soft,
    variant::Outlined => Outlined,
    variant::Ghost => Ghost,
    variant::Gradient => Gradient,
});
marker_into_axis!(ButtonSize { size::Sm => Sm, size::Md => Md, size::Lg => Lg });
marker_into_axis!(ButtonShape { shape::Sm => Sm, shape::Md => Md, shape::Lg => Lg, shape::Pill => Pill });

runtime_core::axis_values! {
    impl ButtonVariant {
        /// The opt-in accent-gradient variant ([`variant::Gradient`]). Draws
        /// once an app adds its arms at boot — see the recipe's docs.
        Gradient,
    }
}

/// Props for [`Button`]. `variant`, `size` and `shape` are Button's own
/// axes — a built-in marker (`variant::Ghost`, `size::Sm`, `shape::Pill`)
/// converts, as does an app value (`axis_values!`, or `ButtonVariant::define`).
/// `tone` is the shared palette.
// Reactive-by-default: `#[props]` wraps each data field. Style axes
// (tone/variant/size/shape) route to the style sink; structural props
// (disabled/loading/block/icons) route too — icon glyph swaps via the
// primitive's reactive `.data()` (no rebuild), and presence/loading/block/
// disabled via a `switch` over a `PartialEq` tuple that rebuilds the pressable
// subtree on change (the static fast path stays when none is live). See `Button`.
// `on_click` (handler), `bind_to` (Ref), and `test_id` (`&'static str`) are
// auto-skipped (not reactive data). `styles` (from `styles = ButtonStyles`) is
// the per-instance override for the root, label and icon slots.
#[runtime_core::props(styles = ButtonStyles)]
#[cfg_attr(feature = "docs", derive(idea_ui::doc_controls::DocControls))]
#[derive(IdealystSchema)]
pub struct ButtonProps {
    /// Button text. `Reactive<String>` — static for a literal/`String`,
    /// live for a `Signal<String>` or `rx!(…)`.
    #[schema(constraint = "reactive: static String or Signal/rx!")]
    pub label: Reactive<String>,
    /// Fires on press/click.
    pub on_click: Rc<dyn Fn()>,
    /// Semantic color palette (Primary, Neutral, Danger, …, or a registered
    /// app tone). Default Primary.
    pub tone: ToneRef,
    /// Surface treatment (Filled, Soft, Outlined, Ghost, or an app value).
    /// Default Filled.
    pub variant: ButtonVariant,
    /// Padding/font scale (Sm, Md, Lg). Default Md.
    pub size: ButtonSize,
    /// Corner-radius scale (Sm, Md, Lg, Pill). Default Md.
    pub shape: ButtonShape,
    /// When `true`, blocks the press and dims the button (opacity drop).
    /// Default `false`. Reactive: pass a `Signal<bool>` (`disabled =
    /// some_state.into()`) and the button rebuilds its pressable in place
    /// when the value changes — no enclosing-scope re-render needed. (A live
    /// structural prop is mutually exclusive with `bind_to`; see `bind_to`.)
    pub disabled: bool,
    /// When `true`, swaps the leading slot for a spinner (tinted to the
    /// button's text color) and blocks the press while the action runs.
    /// Default `false`. Unlike `disabled` it does not dim the surface — the
    /// button reads as "busy", not "off".
    ///
    /// Safe to drive from this button's own `on_click` (`busy.set(true)`
    /// → `spawn_then(io, done)` → `busy.set(false)`): the flip rebuilds
    /// the pressable, but the handler is re-anchored to the Button's own
    /// scope, so work spawned from it survives that rebuild and dies
    /// only when the Button unmounts. A control you build out of
    /// primitives does NOT get this for free — see the trap and the
    /// one-line remedy in `runtime_core::spawn_then`'s module docs.
    /// Regression: `tests/loading_button_spawn.rs`.
    pub loading: bool,
    /// When `Some`, fills the given `Ref<PressableHandle>` on mount.
    /// Useful for anchoring an `Overlay` to this button. A `Ref` fills
    /// exactly once, so this is incompatible with the structural `switch`
    /// rebuild: if you pass `bind_to` AND a *live* structural prop
    /// (`disabled`/`loading`/`leading_icon`/`trailing_icon`/`block` as a
    /// signal), the structure is snapshotted (static build) to keep the bind
    /// correct. Bind-and-also-reactive-structure isn't supported on one button.
    pub bind_to: Option<Ref<PressableHandle>>,
    /// Vector icon rendered before the label (the leading slot). Pass an
    /// `IconData` constant from an icon pack (e.g. `icons_lucide::PLUS`).
    /// Inherits the button's text color.
    pub leading_icon: Option<IconData>,
    /// Vector icon rendered after the label (the trailing slot). Inherits
    /// the button's text color.
    pub trailing_icon: Option<IconData>,
    /// When `true`, the button stretches to fill its container's width
    /// (a full-bleed CTA). Default `false` — the button hugs its content.
    pub block: bool,
    /// Optional robot/E2E test id, forwarded to the root pressable. Only
    /// honored when idea-ui's `robot` feature is on; ignored otherwise.
    pub test_id: Option<&'static str>,
}

impl Default for ButtonProps {
    fn default() -> Self {
        Self {
            label: Reactive::Static(String::new()),
            on_click: Rc::new(|| {}),
            tone: Reactive::Static(ToneRef::default()),
            variant: Reactive::Static(ButtonVariant::default()),
            size: Reactive::Static(ButtonSize::default()),
            shape: Reactive::Static(ButtonShape::default()),
            disabled: Reactive::Static(false),
            loading: Reactive::Static(false),
            bind_to: None,
            leading_icon: Reactive::Static(None),
            trailing_icon: Reactive::Static(None),
            block: Reactive::Static(false),
            test_id: None,
            styles: Default::default(),
        }
    }
}

/// Renders a styled, clickable button whose appearance is driven by
/// the tone × variant × size × shape axes of [`ButtonStyles`].
#[component]
pub fn Button(props: &ButtonProps) -> Element {
    let label = props.label.clone();
    // The handler is re-anchored to the BUTTON's own scope, not the
    // pressable node's. The structural `switch` below rebuilds the
    // pressable when a live structural prop flips — including a
    // `loading` signal the handler itself sets ("busy button": press →
    // `busy.set(true)` → `spawn_then(io, done)`). A task spawned from
    // the handler anchors, via `ScopeAlive::current`, to the node that
    // mounted the handler — the very arm that flip tears down — so its
    // callback was silently dropped. Publishing the body-scope token
    // around the call gives handler-reached spawns the Button's
    // lifetime: alive across arm rebuilds, dead when the Button actually
    // unmounts. Regression: `tests/loading_button_spawn.rs`.
    let on_click: Rc<dyn Fn()> = {
        let anchor = runtime_core::ScopeAlive::current();
        let inner = props.on_click.clone();
        Rc::new(move || anchor.run_within(|| inner()))
    };
    let tone = props.tone.clone();
    // Structural props drive the children (spinner-vs-icon swap, icon
    // appear/disappear) and the layout/press-block, routed in TWO layers:
    //   - icon GLYPH swaps (same presence) → the primitive's reactive
    //     `.data()` setter, no rebuild (layer 1);
    //   - presence/loading STRUCTURE (spinner-vs-icon, icon on/off, the
    //     layout layer + press-block) → a `switch` keyed on a `PartialEq`
    //     tuple of the structural booleans, which rebuilds the pressable
    //     subtree atomically on change (layer 2).
    // When NONE of them is live the build-time fast path stays (no
    // `switch`, no first-paint flicker).
    let leading_icon = props.leading_icon.clone();
    let trailing_icon = props.trailing_icon.clone();
    let disabled_prop = props.disabled.clone();
    let loading_prop = props.loading.clone();
    let block_prop = props.block.clone();
    let bind_to = props.bind_to;
    let styles = props.styles.clone();

    // The style axes every slot sees: tone (the shared palette, an open
    // set of registered keys), variant, size, shape.
    let values = {
        let tone = tone.clone();
        let v = ButtonStyles::values()
            .set(props.variant.clone())
            .set(props.size.clone())
            .set(props.shape.clone());
        match tone {
            Reactive::Static(t) => v.raw("tone", t.key()),
            live => v.raw_with("tone", move || live.get().key()),
        }
    };
    let style_is_reactive = values.is_reactive();

    // The box style for one STRUCTURAL state: the axes above plus the
    // layout booleans, baked per arm (a structural change rebuilds the arm).
    let make_style = {
        let values = values.clone();
        move |row_layout: bool, block: bool, disabled: bool| {
            ButtonStyles::root()
                .values(&values)
                .set(if row_layout { ButtonLayout::Row } else { ButtonLayout::Column })
                .set(ButtonBlock::from(block))
                .set(ButtonDimmed::from(disabled))
        }
    };

    // `build_pressable` builds the full pressable (children + style +
    // press-block) for one STRUCTURAL state — the spinner-vs-icon leading
    // slot, the optional trailing icon, the label, and the layout/dim style
    // for these booleans. Both the static path and each `switch` arm call it.
    let build_pressable = {
        let label = label.clone();
        let on_click = on_click.clone();
        let make_style = make_style.clone();
        let leading_icon = leading_icon.clone();
        let trailing_icon = trailing_icon.clone();
        let values = values.clone();
        let styles = styles.clone();
        move |loading: bool, has_lead: bool, has_trail: bool, disabled: bool, block: bool| {
            // A loading spinner occupies the leading slot, so it needs the same
            // centered-row layout an icon does.
            let row_layout = has_lead || has_trail || loading;
            // Both `disabled` and `loading` block the press; only `disabled`
            // dims the surface.
            let inert = disabled || loading;

            // The icons and the spinner carry the foreground on their OWN
            // nodes: native doesn't inherit text/icon color. When the box
            // attaches PREMINTED (a `--premint` web build), skip the
            // read-back — the fill's `color` is in the box's build-time CSS,
            // so the web icon/spinner inherit it as `currentColor` (which
            // also tracks `:hover`), and under `--premint-only` a resolve
            // here is the panic the stripped rule closure names. Native
            // builds never premint, so they keep the resolved read.
            let resolve_fg = {
                let make_style = make_style.clone();
                let styles = styles.clone();
                move || {
                    let app = make_style(row_layout, block, disabled)
                        .overridden(styles.root.as_ref())
                        .into_style_application();
                    if app.attaches_preminted() {
                        None
                    } else {
                        resolve_style(&app).color.clone()
                    }
                }
            };
            let fg = runtime_core::untrack(&resolve_fg);

            // An explicit color in the instance's `icon` override wins over
            // the theme foreground for the icon tint.
            let icon_color_ovr =
                styles.icon.as_ref().and_then(|o| o.resolve(&VariantSet::default()).color.clone());

            // Builds one inline icon node from a `Reactive<Option<IconData>>`
            // slot known to be `Some` here. LAYER 1: a live slot routes the
            // glyph through the primitive's `.data()` setter, so a glyph SWAP
            // (same presence) updates in place. Presence changes are the
            // `switch` scrutinee's job (`IconData` isn't `PartialEq`).
            let icon_node = |slot: &Reactive<Option<IconData>>| -> Element {
                let mut el = icon(slot.get().expect("icon slot is Some in this arm"))
                    .with_style(ButtonStyles::icon().values(&values).overridden(styles.icon.as_ref()));
                if !slot.is_static() {
                    // The `unwrap_or` guards the impossible "arm says Some,
                    // closure now None" — a presence flip rebuilds the arm.
                    let slot = slot.clone();
                    el = el.data(move || slot.get().unwrap_or(EMPTY_ICON_DATA));
                }
                if let Some(oc) = icon_color_ovr.clone() {
                    return el.color(move || oc.resolve()).into_element();
                }
                match fg.clone() {
                    // A live style axis re-resolves the foreground in place.
                    Some(_) if style_is_reactive => {
                        let resolve_fg = resolve_fg.clone();
                        el.color(move || {
                            resolve_fg().map(|c| c.resolve()).unwrap_or_else(|| Color("#000000".into()))
                        })
                        .into_element()
                    }
                    // `resolve()` re-runs on a theme swap, so the tint
                    // tracks the token like the label does.
                    Some(c) => el.color(move || c.resolve()).into_element(),
                    None => el.into_element(),
                }
            };

            let mut children: Vec<Element> = Vec::with_capacity(3);
            // Loading takes the leading slot (a spinner) in place of the icon.
            if loading {
                let ai = activity_indicator().size(ActivityIndicatorSize::Small);
                children.push(match fg.clone() {
                    Some(c) => ai.color(c.resolve()).into_element(),
                    None => ai.into_element(),
                });
            } else if has_lead {
                children.push(icon_node(&leading_icon));
            }
            // The label draws the LABEL slot with the same axes as the box:
            // its foreground and typography are enumerated arms, so it
            // premints, and it carries them on its own node for native.
            let label_style = ButtonStyles::label().values(&values).overridden(styles.label.as_ref());
            children.push(text(label.clone()).with_style(label_style).into_element());
            if has_trail {
                children.push(icon_node(&trailing_icon));
            }

            let on_click_for_p = on_click.clone();
            let mut bound = runtime_core::pressable(children, move || (on_click_for_p)())
                .with_style(make_style(row_layout, block, disabled).overridden(styles.root.as_ref()));
            if inert {
                bound = bound.disabled(true);
            }
            bound
        }
    };

    // Decide between the static fast path and the structure `switch`.
    // STRUCTURE is reactive when any structural prop is live.
    let structure_is_reactive = !loading_prop.is_static()
        || !disabled_prop.is_static()
        || !leading_icon.is_static()
        || !trailing_icon.is_static()
        || !block_prop.is_static();
    // `bind_to` fills its `Ref` exactly once; a `switch` that rebuilds the
    // pressable would re-fill it each rebuild. When a caller both binds AND
    // passes a live structural prop, prefer the static build (snapshot the
    // structure) so the bind stays correct — a documented limitation, not a
    // double-fill. Without `bind_to`, the `switch` is safe.
    let use_switch = structure_is_reactive && bind_to.is_none();

    if use_switch {
        // LAYER 2: rebuild the pressable subtree atomically when a structural
        // boolean changes. The scrutinee reads each `.get()` so the Effect
        // subscribes; it keys on `.is_some()` (a `bool`) — `IconData` is not
        // `PartialEq`, and glyph swaps within a present slot are handled by
        // layer 1's `.data()`, not here. Clone the structural props into the
        // scrutinee closure (the originals stay valid for the static-path
        // expressions the borrow-checker still sees below the `return`).
        let loading_s = loading_prop.clone();
        let leading_s = leading_icon.clone();
        let trailing_s = trailing_icon.clone();
        let disabled_s = disabled_prop.clone();
        let block_s = block_prop.clone();
        // idealyst-lint-disable-next-line prefer-ui-control-flow -- keyed rebuild of one shape (see `components/mod.rs`)
        let switch_el = runtime_core::switch(
            move || {
                (
                    loading_s.get(),
                    leading_s.get().is_some(),
                    trailing_s.get().is_some(),
                    disabled_s.get(),
                    block_s.get(),
                )
            },
            {
                let build_pressable = build_pressable.clone();
                move |&(loading, has_lead, has_trail, disabled, block)| {
                    build_pressable(loading, has_lead, has_trail, disabled, block).into_element()
                }
            },
        );
        // `switch` returns a bare `Element`. No `bind_to` here — `use_switch`
        // requires `bind_to.is_none()`; the test id (when requested) rides a
        // transparent wrapper view since the pressable is rebuilt per arm.
        return finalize_switch(switch_el, props);
    }

    // STATIC fast path (or a binding caller): build once from the structural
    // snapshots.
    let mut bound = build_pressable(
        loading_prop.get(),
        leading_icon.get().is_some(),
        trailing_icon.get().is_some(),
        disabled_prop.get(),
        block_prop.get(),
    );
    if let Some(r) = bind_to {
        bound = bound.bind(r);
    }
    // Forward the test id to the root pressable for robot/E2E location.
    // Gated: `.test_id()` only exists under `runtime-core/robot`.
    #[cfg(feature = "robot")]
    if let Some(tid) = props.test_id {
        bound = bound.test_id(tid);
    }
    bound.into_element()
}

/// A zero-path placeholder glyph. Only reachable in the impossible
/// "arm says the icon slot is `Some`, but the live closure now yields
/// `None`" case (a presence flip rebuilds the `switch` arm instead, so the
/// reactive `.data()` closure never actually observes `None`). Shared with
/// `IconButton`, which follows a live `icon` the same way.
pub(crate) const EMPTY_ICON_DATA: IconData = IconData {
    view_box: (24, 24),
    paths: &[],
    fill_rule: runtime_core::FillRule::NonZero,
    filled: false,
};

/// Finishes the structure-`switch` path: forwards the test id (when the
/// `robot` feature is on) and coerces to `Element`. Split out so the
/// `switch` early-return and the static path share the same test-id wiring
/// shape. The `switch` root is a synthetic node, so the test id rides on a
/// thin wrapper view only when actually requested.
fn finalize_switch(switch_el: Element, _props: &ButtonProps) -> Element {
    #[cfg(feature = "robot")]
    if let Some(tid) = _props.test_id {
        // The pressable is rebuilt inside the switch arms, so a robot test id
        // can't live on it across rebuilds. Attach it to a transparent wrapper
        // `view` around the switch so location stays stable.
        return runtime_core::view(vec![switch_el])
            .test_id(tid)
            .into_element();
    }
    switch_el
}

#[cfg(test)]
mod tests {

    /// The LABEL slot must premint for every (tone, variant, size) the box
    /// itself premints — it replaces the per-instance typography snapshot
    /// + color override, which forced every Button label onto the live
    /// engine under `--premint`.
    #[test]
    fn regression_button_label_sheet_premints() {
        with_test_world(|| {
            install_idea_theme(light_theme());
            let app = ButtonStyles::label()
                .raw("tone", "primary")
                .set(ButtonVariant::Filled)
                .set(ButtonSize::Lg)
                .into_style_application();
            assert!(
                app.preminted_class_list().is_some(),
                "button label sheet must premint (was snapshot + color override)"
            );
        });
    }

    use super::*;
    use crate::test_support::{classify, P, TStyle};
    use idea_theme::testing::with_test_world;
    use idea_theme::theme::{install_idea_theme, light_theme};
    use runtime_core::{resolve_style, FillRule, StyleApplication, StyleRules, Tokenized};

    fn theme() {
        install_idea_theme(light_theme());
    }

    const PLUS: IconData = IconData {
        view_box: (24, 24),
        paths: &["M12 5v14M5 12h14"],
        fill_rule: FillRule::NonZero,
        filled: false,
    };

    /// Resolves the `color` on a Text node's OWN style. Returns `None`
    /// when the node carries no style (the buggy state — color relied on
    /// container inheritance) or its style sets no color.
    fn text_node_color(el: Element) -> Option<Color> {
        match classify(el) {
            P::Text { style, .. } => {
                let app = match style? {
                    TStyle::App(a) => a,
                    _ => panic!("button label uses a static style"),
                };
                resolve_style(&app).color.clone().map(|c| c.resolve())
            }
            _ => None,
        }
    }

    // Field report 3.1b (HIGH): a filled Primary button's label rendered
    // INVISIBLE on Android/iOS because the white label color lived only on
    // the wrapping pressable and native text doesn't inherit parent color.
    // The label text node must carry the intent foreground itself, so we
    // assert the label node's OWN resolved color is the
    // intent-primary-solid-text white.
    #[test]
    fn regression_filled_button_label_carries_intent_text_color() {
        with_test_world(|| {
            theme();
            let props = ButtonProps {
                label: Reactive::Static("Save".into()),
                tone: Reactive::Static(ToneRef::default()), // Primary
                variant: Reactive::Static(ButtonVariant::default()), // Filled
                ..Default::default()
            };
            let (mut children, _) = pressable_parts(Button(&props));
            let color = text_node_color(children.remove(0))
                .expect("filled button label must carry its own color, not inherit from the pressable");
            assert_eq!(
                color.0.to_ascii_lowercase(),
                "#ffffff",
                "filled-Primary label is the intent-primary-solid-text white"
            );
    });
    }

    // macOS "not bold / not centered": the Button sheet sets font_weight/
    // text_align on the pressable BOX, but native text leaves inherit NOTHING
    // from the box (the same reason the label's color is stamped on its own
    // node). The label must carry the typography itself, and the box must center
    // its content on both axes — web centers via the box's text-align + inline
    // flow, native flex needs it explicit. A test that passed against the old
    // bare label is not a valid regression test, so we assert the label node's
    // OWN resolved typography AND the box's centering.
    #[test]
    fn regression_button_label_carries_weight_alignment_and_box_centers() {
        with_test_world(|| {
            theme();
            let props = ButtonProps {
                label: Reactive::Static("Create account".into()),
                ..Default::default()
            };
            let (mut children, box_app) = pressable_parts(Button(&props));
            let label_rules = match classify(children.remove(0)) {
                P::Text { style, .. } => match style.expect("label carries a style") {
                    TStyle::App(a) => resolve_style(&a),
                    _ => panic!("button label uses a static style"),
                },
                _ => panic!("expected a label text node at slot 0"),
            };
            assert_eq!(
                label_rules.font_weight,
                Some(runtime_core::FontWeight::SemiBold),
                "label must carry the button's SemiBold weight on its own node (native doesn't inherit)"
            );
            assert_eq!(
                label_rules.text_align,
                Some(runtime_core::TextAlign::Center),
                "label must carry the button's center text-align"
            );
            assert!(
                label_rules.font_size.is_some(),
                "label must carry the size axis's font_size, not fall back to the backend default"
            );
            // The box centers content on both axes so the label sits centered in the
            // button, not packed at the top-left.
            let box_rules = resolve_style(&box_app);
            assert_eq!(
                box_rules.align_items,
                Some(runtime_core::AlignItems::Center),
                "button box centers content on the cross axis"
            );
            assert_eq!(
                box_rules.justify_content,
                Some(runtime_core::JustifyContent::Center),
                "button box centers content on the main axis"
            );
    });
    }

    // Same root cause for the leading/trailing icons: native icons don't
    // inherit the button color, so the wrapper must stamp the resolved
    // foreground on each icon's own color closure. Assert the icon carries
    // a color override that resolves to the intent text white.
    #[test]
    fn regression_filled_button_icons_carry_intent_text_color() {
        with_test_world(|| {
            theme();
            let props = ButtonProps {
                label: Reactive::Static("Save".into()),
                leading_icon: Reactive::Static(Some(PLUS)),
                trailing_icon: Reactive::Static(Some(PLUS)),
                ..Default::default()
            };
            let (children, _) = pressable_parts(Button(&props));
            let kinds: Vec<P> = children.into_iter().map(classify).collect();
            for (i, slot) in [0usize, 2].iter().zip(["leading", "trailing"]) {
                match &kinds[*i] {
                    P::Icon { color, .. } => {
                        let c = color
                            .as_ref()
                            .unwrap_or_else(|| panic!("{slot} icon must carry an explicit color"));
                        assert_eq!(
                            c.0.to_ascii_lowercase(),
                            "#ffffff",
                            "{slot} icon tint is the intent text white"
                        );
                    }
                    _ => panic!("expected an icon at slot {i}"),
                }
            }
    });
    }

    fn pressable_parts(el: Element) -> (Vec<Element>, StyleApplication) {
        match classify(el) {
            P::Pressable { children, style, .. } => {
                let app = match style.expect("Button always attaches a style") {
                    TStyle::App(a) => a,
                    _ => panic!("Button uses a static style source"),
                };
                (children, app)
            }
            _ => panic!("Button renders a Pressable"),
        }
    }

    // THE `--premint-only` read-back (the stripped-closure panic in
    // runtime-shared names this exact pattern): Button resolved the
    // container's fill color in Rust to tint its leading icon and loading
    // spinner. On a premint build the container attaches a preminted class
    // whose CSS carries the fill's `color`, so the icon/spinner must ship
    // with NO explicit color and inherit `currentColor` — stamping one
    // requires the resolve-read that `--premint-only` panics on. On live/
    // native builds the resolved snapshot must still be stamped, because
    // native nodes don't inherit color.
    #[test]
    fn regression_premint_icon_and_spinner_tint_inherit_current_color() {
        with_test_world(|| {
            theme();
            let icon_props = ButtonProps {
                label: Reactive::Static("Save".into()),
                leading_icon: Reactive::Static(Some(PLUS)),
                ..Default::default()
            };
            let mut children = classify(Button(&icon_props)).children();
            let icon_color = match classify(children.remove(0)) {
                P::Icon { color, .. } => color,
                _ => panic!("expected the leading icon at slot 0"),
            };

            let spinner_props = ButtonProps {
                label: Reactive::Static("Save".into()),
                loading: Reactive::Static(true),
                ..Default::default()
            };
            let mut children = classify(Button(&spinner_props)).children();
            let spinner_color = match classify(children.remove(0)) {
                P::ActivityIndicator { color } => color,
                _ => panic!("expected the loading spinner at slot 0"),
            };

            #[cfg(idealyst_premint)]
            {
                assert!(
                    icon_color.is_none(),
                    "premint build: the icon inherits the box class's color as \
                     `currentColor`; a stamped color is the --premint-only panic"
                );
                assert!(
                    spinner_color.is_none(),
                    "premint build: the spinner inherits `currentColor` the same way"
                );
            }
            #[cfg(not(idealyst_premint))]
            {
                assert_eq!(
                    icon_color.map(|c| c.0.to_ascii_lowercase()),
                    Some("#ffffff".into()),
                    "live/native build: the icon carries the resolved primary-filled \
                     foreground snapshot (native doesn't inherit)"
                );
                assert_eq!(
                    spinner_color.map(|c| c.0.to_ascii_lowercase()),
                    Some("#ffffff".into()),
                    "live/native build: the spinner carries the same resolved foreground"
                );
            }
        });
    }

    // D3: the wrapper must pass leading/trailing icons through as icon
    // children (the primitive supported them; the wrapper dropped them).
    #[test]
    fn icons_become_children_around_the_label() {
        with_test_world(|| {
            theme();
            let props = ButtonProps {
                label: Reactive::Static("Save".into()),
                leading_icon: Reactive::Static(Some(PLUS)),
                trailing_icon: Reactive::Static(Some(PLUS)),
                ..Default::default()
            };
            let (children, _) = pressable_parts(Button(&props));
            // leading icon + label text + trailing icon = 3 children.
            assert_eq!(children.len(), 3, "leading + label + trailing");
            let kinds: Vec<P> = children.into_iter().map(classify).collect();
            assert!(
                matches!(kinds[0], P::Icon { .. }),
                "first child is the leading icon"
            );
            assert!(
                matches!(kinds[2], P::Icon { .. }),
                "last child is the trailing icon"
            );

            // Without icons, the button is just the label — no stray slots.
            let plain = ButtonProps {
                label: Reactive::Static("Save".into()),
                ..Default::default()
            };
            let (mut children, _) = pressable_parts(Button(&plain));
            assert_eq!(children.len(), 1, "label only when no icons");
            assert!(matches!(classify(children.remove(0)), P::Text { .. }));
    });
    }

    // D3: a leading/trailing icon forces the centered-row layout (the
    // base sheet doesn't pin one) so the icon and label sit inline.
    #[test]
    fn icon_button_lays_out_as_centered_row() {
        with_test_world(|| {
            theme();
            let props = ButtonProps {
                leading_icon: Reactive::Static(Some(PLUS)),
                ..Default::default()
            };
            let (_, app) = pressable_parts(Button(&props));
            let rules = resolve_style(&app);
            assert_eq!(
                rules.flex_direction,
                Some(FlexDirection::Row),
                "icons must compose into a row"
            );
            assert!(rules.gap.is_some(), "row gap between icon and label");
    });
    }

    // D3: `block` stretches the button to its container's width.
    #[test]
    fn block_stretches_to_container_width() {
        with_test_world(|| {
            theme();
            let props = ButtonProps {
                block: Reactive::Static(true),
                ..Default::default()
            };
            let (_, app) = pressable_parts(Button(&props));
            let rules = resolve_style(&app);
            assert_eq!(
                rules.width,
                Some(Tokenized::Literal(Length::Percent(100.0))),
                "block button is full-width"
            );
            assert_eq!(rules.align_self, Some(AlignSelf::Stretch));

            // Default (non-block) leaves width unset → hugs content.
            let plain = ButtonProps::default();
            let (_, app) = pressable_parts(Button(&plain));
            assert!(
                resolve_style(&app).width.is_none(),
                "a non-block button doesn't pin a width"
            );
    });
    }

    // Regression: a non-block button must HUG its content — `align_self:
    // Center` (NOT `Stretch`) — so a flex parent's default `align-items:
    // stretch` can't grow it to the row's height (a button row) or the
    // column's width. This is the "buttons flex to the parent height" bug;
    // `Center` (vs `FlexStart`) also lets a centering preview card center it.
    // `block` opts back into Stretch.
    #[test]
    fn regression_non_block_button_hugs_content_not_stretch() {
        with_test_world(|| {
            theme();
            let (_, app) = pressable_parts(Button(&ButtonProps::default()));
            assert_eq!(
                resolve_style(&app).align_self,
                Some(AlignSelf::Center),
                "a non-block button sizes to content (centered, not stretched to the parent cross axis)"
            );

            let block = ButtonProps {
                block: Reactive::Static(true),
                ..Default::default()
            };
            let (_, app) = pressable_parts(Button(&block));
            assert_eq!(
                resolve_style(&app).align_self,
                Some(AlignSelf::Stretch),
                "a block button stretches"
            );
    });
    }

    // `disabled` dims the surface with a deterministic opacity (not a
    // hover-state overlay) AND blocks the press.
    #[test]
    fn disabled_button_dims_and_blocks_press() {
        with_test_world(|| {
            theme();
            let mk = || ButtonProps {
                disabled: Reactive::Static(true),
                ..Default::default()
            };
            let (_, app) = pressable_parts(Button(&mk()));
            assert_eq!(
                resolve_style(&app).opacity.as_ref().map(|t| t.resolve()),
                Some(0.45),
                "a disabled button is dimmed so it reads as off"
            );
            let d = pressable_disabled(Button(&mk())).expect("disabled blocks the press");
            assert!(d, "disabled reports the button inert");
    });
    }

    // `loading` puts a spinner in the leading slot and blocks the press,
    // without dimming the surface (it reads as busy, not off).
    #[test]
    fn loading_button_shows_spinner_and_blocks_press() {
        with_test_world(|| {
            theme();
            let mk = || ButtonProps {
                label: Reactive::Static("Saving".into()),
                loading: Reactive::Static(true),
                ..Default::default()
            };
            let (mut children, app) = pressable_parts(Button(&mk()));
            assert!(
                matches!(classify(children.remove(0)), P::ActivityIndicator { .. }),
                "loading renders a spinner as the leading child"
            );
            assert!(
                resolve_style(&app).opacity.as_ref().map(|t| t.resolve()) != Some(0.45),
                "loading does not dim like disabled"
            );
            let d = pressable_disabled(Button(&mk())).expect("loading blocks the press");
            assert!(d, "loading reports the button inert");
    });
    }

    // The framework imposes NO cursor/selection default on the bare
    // `pressable` primitive (a raw pressable inherits the platform default).
    // idea-ui's Button opts in via its sheet, so the rendered button resolves
    // to a pointer cursor and non-selectable label text — the cross-platform
    // realization of "buttons use the right pointer, and text in buttons isn't
    // selectable" (web `cursor`/`user-select`, macOS `NSCursor`/`isSelectable`,
    // touch backends no-op).
    #[test]
    fn button_opts_into_pointer_cursor_and_non_selectable_text() {
        with_test_world(|| {
            theme();
            let (_, app) = pressable_parts(Button(&ButtonProps::default()));
            let rules = resolve_style(&app);
            assert_eq!(
                rules.cursor,
                Some(runtime_core::Cursor::Pointer),
                "a button shows the pointer affordance"
            );
            assert_eq!(
                rules.user_select,
                Some(runtime_core::UserSelect::None),
                "a button's label text can't be drag-selected"
            );
    });
    }

    // Hover + press feedback: the installed Button sheet carries
    // `__state_hovered` / `__state_pressed` overlays that dim opacity, plus an
    // explicit resting `opacity: 1.0` so the dim animates back cleanly on
    // native (where the overlay is applied by re-resolving the style — see the
    // base-opacity rationale in idea-theme). Web realizes these as
    // `:hover`/`:active`; macOS via `attach_states`. Disabled is deliberately
    // NOT a state overlay here.
    #[test]
    fn button_has_hover_and_pressed_opacity_overlays() {
        with_test_world(|| {
            theme();
            let app = ButtonStyles::root().raw("tone", "primary").into_style_application();

            let base = resolve_style(&app);
            assert_eq!(
                base.opacity.as_ref().map(|t| t.resolve()),
                Some(1.0),
                "resting button is fully opaque so the hover/press dim has a value to animate back to"
            );

            let hovered = resolve_style(&app.clone().with("__state_hovered", "on"));
            assert_eq!(
                hovered.opacity.as_ref().map(|t| t.resolve()),
                Some(0.92),
                "hover dims the button"
            );

            let pressed = resolve_style(&app.with("__state_pressed", "on"));
            assert_eq!(
                pressed.opacity.as_ref().map(|t| t.resolve()),
                Some(0.85),
                "press dims the button further"
            );
    });
    }

    /// The pressable's evaluated `disabled` state.
    fn pressable_disabled(el: Element) -> Option<bool> {
        match classify(el) {
            P::Pressable { disabled, .. } => disabled,
            _ => panic!("Button renders a Pressable"),
        }
    }

    /// A one-off override setting a single color.
    fn color_sheet(hex: &str) -> StyleRules {
        StyleRules {
            color: Some(Tokenized::Literal(Color(hex.into()))),
            ..Default::default()
        }
    }

    // Slot override: the `label` slot's colour wins over the theme foreground and is
    // stamped on the label's OWN node (native-safe). This is the "dark label on
    // a white/neutral button" case the CSS cascade can't do on native.
    #[test]
    fn label_style_overrides_label_color() {
        with_test_world(|| {
            theme();
            let props = ButtonProps {
                label: Reactive::Static("Get started".into()),
                styles: ButtonSlotStyles::default().label(color_sheet("#0b6b3a")),
                ..Default::default()
            };
            let (mut children, _) = pressable_parts(Button(&props));
            let color = text_node_color(children.remove(0)).expect("label carries its own color");
            assert_eq!(
                color.0.to_ascii_lowercase(),
                "#0b6b3a",
                "label_style color overrides the theme foreground on the label node",
            );
    });
    }

    // Slot override: the root `style` layers onto the pressable box on top of
    // the theme style (background here) without disturbing untouched fields.
    #[test]
    fn style_overrides_container_box() {
        with_test_world(|| {
            theme();
            let ovr = StyleRules {
                background: Some(Tokenized::Literal(Color("#ffffff".into()))),
                ..Default::default()
            };
            let props = ButtonProps {
                label: Reactive::Static("Go".into()),
                styles: ButtonSlotStyles::default().root(ovr),
                ..Default::default()
            };
            let (_, app) = pressable_parts(Button(&props));
            assert_eq!(
                resolve_style(&app).background.as_ref().map(|c| c.resolve().0.to_ascii_lowercase()),
                Some("#ffffff".to_string()),
                "root style override wins for the container background",
            );
    });
    }

    // Slot override: `icon_style` colour wins for the icon tint.
    #[test]
    fn icon_style_overrides_icon_tint() {
        with_test_world(|| {
            theme();
            let props = ButtonProps {
                label: Reactive::Static("Go".into()),
                leading_icon: Reactive::Static(Some(PLUS)),
                styles: ButtonSlotStyles::default().icon(color_sheet("#0b6b3a")),
                ..Default::default()
            };
            let (mut children, _) = pressable_parts(Button(&props));
            match classify(children.remove(0)) {
                P::Icon { color, .. } => {
                    let c = color.expect("icon carries an explicit color");
                    assert_eq!(
                        c.0.to_ascii_lowercase(),
                        "#0b6b3a",
                        "icon_style color overrides the theme foreground for the icon tint",
                    );
                }
                _ => panic!("expected the leading icon"),
            }
    });
    }

    // D4: `disabled` is a plain `bool` — `disabled = true` (not
    // `Some(Rc::new(|| true))`) compiles and marks the button inert.
    #[test]
    fn disabled_bool_marks_the_button_inert() {
        with_test_world(|| {
            theme();
            let on = ButtonProps {
                disabled: Reactive::Static(true),
                ..Default::default()
            };
            let d = pressable_disabled(Button(&on)).expect("disabled=true sets a disabled source");
            assert!(d, "the source reports the button as disabled");

            // Default leaves the press path live (no disabled source attached).
            let off = ButtonProps::default();
            assert!(
                pressable_disabled(Button(&off)).is_none(),
                "a non-disabled button attaches no disabled source"
            );
    });
    }

    fn box_rules(props: ButtonProps) -> Rc<StyleRules> {
        let (_, app) = pressable_parts(Button(&props));
        resolve_style(&app)
    }

    fn reset_styles() {
        runtime_core::component_styles::__reset::<ButtonStyles>();
    }

    /// `tone_variant` opts a variant value into a tone-aware look: the
    /// gradient recipe paints the box (fill + gradient) and the label's
    /// foreground — on the LABEL node, never a surface on the text.
    #[test]
    fn tone_variant_adds_the_gradient_to_box_and_label() {
        with_test_world(|| {
            reset_styles();
            theme();
            ButtonStyles::restyle(|s| s.tone_variant(ButtonVariant::Gradient, &variant::Gradient));
            let props = ButtonProps {
                label: Reactive::Static("Go".to_string()),
                variant: Reactive::Static(ButtonVariant::Gradient),
                ..Default::default()
            };
            let (children, app) = pressable_parts(Button(&props));
            let rules = resolve_style(&app);
            assert!(rules.background_gradient.is_some(), "the box paints the accent gradient");
            let label = children.into_iter().last().expect("a label");
            let label_rules = match classify(label) {
                P::Text { style: Some(TStyle::App(a)), .. } => resolve_style(&a),
                _ => panic!("the label is a styled text node"),
            };
            assert_eq!(label_rules.color.as_ref().and_then(|c| c.name()), Some("effect-gradient-text"));
            assert!(label_rules.background_gradient.is_none(), "the label takes no surface");
            reset_styles();
        });
    }

    /// A custom variant: `ButtonVariant::define` plus a restyle arm. The
    /// button draws it, and its classes premint.
    #[test]
    fn custom_variant_draws_through_a_restyle_arm() {
        const RAISED: ButtonVariant = ButtonVariant::define("raised");
        with_test_world(|| {
            reset_styles();
            theme();
            ButtonStyles::restyle(|s| {
                s.root().arm(RAISED, |t| StyleRules {
                    background: Some(t.color.surface()),
                    shadow: Some(runtime_core::Shadow {
                        x: 0.0,
                        y: 2.0,
                        blur: 8.0,
                        color: Tokenized::Literal(Color("rgba(0,0,0,0.2)".into())),
                    }),
                    ..Default::default()
                });
            });
            let rules = box_rules(ButtonProps { variant: Reactive::Static(RAISED), ..Default::default() });
            assert_eq!(rules.background.as_ref().and_then(|c| c.name()), Some("color-surface"));
            assert!(rules.shadow.is_some());
            let classes = ButtonStyles::root()
                .raw("tone", "primary")
                .set(RAISED)
                .into_style_application()
                .preminted_class_list()
                .expect("the extended root premints");
            assert!(classes.contains("-variant-raised"), "{classes}");
            reset_styles();
        });
    }

    /// A tone-aware custom variant: the app's own recipe rendered over every
    /// registered tone with `tone_variant_arms`.
    #[test]
    fn tone_aware_custom_variant_uses_each_tones_colors() {
        const TINTED: ButtonVariant = ButtonVariant::define("tinted");
        with_test_world(|| {
            reset_styles();
            theme();
            ButtonStyles::restyle(|s| {
                tone_variant_arms(&mut s.root(), &semantic_tones(), TINTED.key(), &variant::Soft);
            });
            let danger = box_rules(ButtonProps {
                variant: Reactive::Static(TINTED),
                tone: Reactive::Static(idea_theme::extensible::tone::Danger.into()),
                ..Default::default()
            });
            assert_eq!(danger.background.as_ref().and_then(|c| c.name()), Some("intent-danger-soft-bg"));
            reset_styles();
        });
    }

    /// The `app_theme! { …, tones: [Brand] }` path: installing the theme
    /// registers its tones, so `Button(tone = Brand)` draws with no other
    /// wiring.
    #[test]
    fn a_theme_extension_tone_draws_on_button() {
        idea_theme::tone! {
            Brand using self, theme {
                key = "brand_button_test",
                fill_bg = idea_theme::color_token!("brand-button-fill", "#5b21b6"),
                fill_fg = theme.colors().background.clone(),
                soft_bg = theme.colors().surface_alt.clone(),
                soft_fg = theme.colors().text.clone(),
                stroke_color = theme.colors().border.clone(),
                stroke_fg = theme.colors().text.clone(),
                ghost_fg = theme.colors().text.clone(),
                disabled = theme.colors().text_muted.clone(),
                focus_ring = theme.colors().focus_ring.clone(),
                tokens = ["brand-button-fill" => "#5b21b6"],
            }
        }
        idea_theme::app_theme! {
            BrandTheme { idea: idea_theme::theme::IdeaThemeDefaults, tones: [Brand] }
        }
        with_test_world(|| {
            reset_styles();
            install_idea_theme(BrandTheme { idea: light_theme() });
            let rules = box_rules(ButtonProps { tone: Reactive::Static(Brand.into()), ..Default::default() });
            assert_eq!(rules.background.as_ref().and_then(|c| c.name()), Some("brand-button-fill"));
            reset_styles();
        });
    }
}
