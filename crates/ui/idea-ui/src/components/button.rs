//! `Button` — the styled clickable, built on the extensible
//! Variant/Tone/Size/Shape trait surface.
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
//! Styling routes through the [installed Button
//! stylesheet][installed_button_sheet]. `install_idea_theme` installs
//! the default sheet at startup; apps with custom modifiers
//! (`Hype` tone, `Elevated` variant) override via
//! `install_button_sheet(ButtonSheetBuilder::new().add_tone(Hype.into()).build())`.
//!
//! Every supported `(tone, variant, size, shape)` combination is
//! pre-generated as a CSS rule at sheet registration time, so
//! apply-style is a className lookup — no FOUC, no dynamic CSS mint.

use std::rc::Rc;

use runtime_core::primitives::activity_indicator::{activity_indicator, ActivityIndicatorSize};
use runtime_core::{
    component, icon, resolve_style, text, AlignSelf, Color, Element, FlexDirection, IconData,
    IdealystSchema, IntoElement, Length, PressableHandle, Reactive, Ref, StyleApplication,
    StyleRules, StyleSheet, Tokenized,
};

use idea_theme::extensible::{
    installed_button_sheet, ButtonSizeRef, ShapeRef, ToneRef, VariantRef,
};

use crate::slot_override::{apply_override, override_rules};

/// Props for the extensible Button. Each modifier axis is a typed
/// handle (`*Ref` newtype) so call sites can write
/// `tone: tone::Primary.into()` instead of `Rc::new(...)`. Built-in
/// defaults route to Filled/Primary/Md/Md.
// Reactive-by-default: `#[props]` wraps each data field. Style axes
// (tone/variant/size/shape) route to the style sink; structural props
// (disabled/loading/block/icons) now route too — icon glyph swaps via the
// primitive's reactive `.data()` (no rebuild), and presence/loading/block/
// disabled via a `switch` over a `PartialEq` tuple that rebuilds the pressable
// subtree on change (the static fast path stays when none is live). See `Button`.
// `on_click` (handler), `bind_to` (Ref), and `test_id` (`&'static str`) are
// auto-skipped (not reactive data).
#[runtime_core::props]
#[cfg_attr(feature = "docs", derive(idea_ui::doc_controls::DocControls))]
#[derive(IdealystSchema)]
pub struct ButtonProps {
    /// Button text. `Reactive<String>` — static for a literal/`String`,
    /// live for a `Signal<String>` or `rx!(…)`.
    #[schema(constraint = "reactive: static String or Signal/rx!")]
    pub label: Reactive<String>,
    /// Fires on press/click.
    pub on_click: Rc<dyn Fn()>,
    /// Semantic color palette (Primary, Neutral, Danger, …). Default Primary.
    pub tone: ToneRef,
    /// Surface treatment (Filled, Soft, Outline, Ghost, …). Default Filled.
    pub variant: VariantRef,
    /// Padding/font scale (Sm, Md, Lg). Default Md.
    pub size: ButtonSizeRef,
    /// Corner-radius scale (Sm, Md, Lg, Pill, …). Default Md.
    pub shape: ShapeRef,
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
    /// Per-slot style overrides layered on top of the theme style. See
    /// [`crate::slot_override`]. `style` overrides the root pressable's box
    /// (background, border, padding, radius, …); `label_style` and `icon_style`
    /// override the label text node and the leading/trailing icon nodes. Use
    /// these for one-off tweaks a theme sheet shouldn't own — e.g. a custom
    /// label colour on a neutral fill (native-safe, unlike the CSS cascade).
    #[prop(static)]
    pub style: Option<Rc<StyleSheet>>,
    /// Override for the label text node. A `color` here wins over the theme
    /// foreground and is stamped on the label's own node, so it renders on
    /// every backend (native text doesn't inherit colour). See `style`.
    #[prop(static)]
    pub label_style: Option<Rc<StyleSheet>>,
    /// Override for the leading/trailing icon nodes. A `color` here wins over
    /// the theme foreground for the icon tint; other fields merge into the icon
    /// box style. See `style`.
    #[prop(static)]
    pub icon_style: Option<Rc<StyleSheet>>,
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
            variant: Reactive::Static(VariantRef::default()),
            size: Reactive::Static(ButtonSizeRef::default()),
            shape: Reactive::Static(ShapeRef::default()),
            disabled: Reactive::Static(false),
            loading: Reactive::Static(false),
            bind_to: None,
            leading_icon: Reactive::Static(None),
            trailing_icon: Reactive::Static(None),
            block: Reactive::Static(false),
            style: None,
            label_style: None,
            icon_style: None,
            test_id: None,
        }
    }
}

/// Pixel size of a button's leading/trailing icon. Matches the label's
/// cap-height closely enough to sit inline without throwing off the
/// centered row.
const BUTTON_ICON_PX: f32 = 16.0;

thread_local! {
    static BUTTON_ICON_SHEET: std::cell::RefCell<Option<Rc<runtime_core::StyleSheet>>> =
        const { std::cell::RefCell::new(None) };
}

/// A cached static sheet that pins the leading/trailing icon to a fixed
/// square and stops it from being squeezed by flex. Icons have no
/// intrinsic content size, so without an explicit width/height they hit
/// a 0×0 box.
fn button_icon_sheet() -> Rc<runtime_core::StyleSheet> {
    BUTTON_ICON_SHEET.with(|s| {
        if s.borrow().is_none() {
            let sheet = runtime_core::StyleSheet::r#static(StyleRules {
                width: Some(Tokenized::Literal(Length::Px(BUTTON_ICON_PX))),
                height: Some(Tokenized::Literal(Length::Px(BUTTON_ICON_PX))),
                flex_shrink: Some(Tokenized::Literal(0.0)),
                ..Default::default()
            });
            *s.borrow_mut() = Some(Rc::new(sheet));
        }
        s.borrow().as_ref().cloned().unwrap()
    })
}

/// Wraps a resolved foreground color in a static, color-only stylesheet
/// for a leaf text/icon node.
///
/// Native `UILabel`/`TextView` (and `UIImageView`/icon shapes) do NOT
/// inherit text color from a parent — only web's CSS cascade does. So a
/// label colored solely via its wrapping pressable renders invisible on
/// the colored fill on iOS/Android. We resolve the fill's foreground and
/// stamp it directly on the label/icon node so every backend matches web
/// (the same pattern `Typography` uses — color lives on the text node).
/// The `Tokenized<Color>` keeps its token reference, so theme swaps still
/// re-resolve the color in bulk via the cohort.
fn label_color_style(color: Tokenized<Color>) -> Rc<StyleSheet> {
    Rc::new(StyleSheet::r#static(StyleRules {
        color: Some(color),
        ..Default::default()
    }))
}

/// Renders a styled, clickable button whose appearance is driven by
/// the tone × variant × size × shape axes of the installed Button sheet.
#[component]
pub fn Button(props: &ButtonProps) -> Element {
    let label = props.label.clone();
    let on_click = props.on_click.clone();
    // Style axes — kept as `Reactive` and read live INSIDE `make_style` so a
    // reactive tone/variant/size/shape re-styles the button in place.
    let tone = props.tone.clone();
    let variant = props.variant.clone();
    let size = props.size.clone();
    let shape = props.shape.clone();
    // reactive-sweep DONE (structure): the structural props drive the children
    // (spinner-vs-icon swap, icon appear/disappear) and the layout/press-block.
    // They're kept as `Reactive` here and routed in TWO layers (see below):
    //   - icon GLYPH swaps (same presence) → the primitive's reactive `.data()`
    //     setter, no rebuild (layer 1);
    //   - presence/loading STRUCTURE (spinner-vs-icon, icon on/off, the layout
    //     layer + press-block) → a `switch` keyed on a `PartialEq` tuple of the
    //     structural booleans, which rebuilds the pressable subtree atomically
    //     on change (layer 2).
    // When NONE of them is live we keep the build-time fast path (no `switch`,
    // no first-paint flicker) — the path the static tests exercise.
    let leading_icon = props.leading_icon.clone();
    let trailing_icon = props.trailing_icon.clone();
    let disabled_prop = props.disabled.clone();
    let loading_prop = props.loading.clone();
    let block_prop = props.block.clone();
    let bind_to = props.bind_to;
    // Per-slot style overrides (see `crate::slot_override`). Static sheets, so
    // clone the `Rc`s into the (reactive-capable) build closures below.
    let style_ovr = props.style.clone();
    let label_ovr = props.label_style.clone();
    let icon_ovr = props.icon_style.clone();

    // STYLE-axis reactivity (tone/variant/size/shape) is independent of
    // STRUCTURE reactivity. `make_style` reads each axis live INSIDE so the
    // apply-style Effect subscribes to the dynamic ones.
    let style_is_reactive =
        !tone.is_static() || !variant.is_static() || !size.is_static() || !shape.is_static();

    // `make_style` is parameterized by the structural booleans (row layout /
    // block / disabled) so the same closure serves the static build and each
    // `switch` arm. The style axes are captured reactively; the layout layer is
    // baked from the per-arm booleans (constant within an arm — a structural
    // change rebuilds the arm, re-baking the layer).
    let make_style = {
        let tone = tone.clone();
        let variant = variant.clone();
        let size = size.clone();
        let shape = shape.clone();
        move |row_layout: bool, block: bool, disabled: bool| {
            let tone = tone.clone();
            let variant = variant.clone();
            let size = size.clone();
            let shape = shape.clone();
            move || {
                let appearance_key = format!("{}_{}", tone.get().key(), variant.get().key());
                let style = StyleApplication::new(installed_button_sheet())
                    .with("appearance", appearance_key)
                    .with("size", size.get().key().to_string())
                    .with("shape", shape.get().key().to_string());
                let layer_key = format!(
                    "layout_{}_{}_{}",
                    row_layout as u8, block as u8, disabled as u8
                );
                style.with_computed(layer_key, move || {
                    let mut rules = StyleRules::default();
                    if row_layout {
                        rules.flex_direction = Some(FlexDirection::Row);
                        rules.align_items = Some(runtime_core::AlignItems::Center);
                        rules.gap = Some(Tokenized::token("spacing-xs", Length::Px(6.0)));
                    }
                    if block {
                        rules.width = Some(Tokenized::Literal(Length::Percent(100.0)));
                        rules.align_self = Some(AlignSelf::Stretch);
                    } else {
                        rules.align_self = Some(AlignSelf::Center);
                    }
                    if disabled {
                        // Deterministic dim so a disabled button reads as off on
                        // every backend.
                        rules.opacity = Some(Tokenized::Literal(0.45));
                    }
                    rules
                })
            }
        }
    };

    // reactive-sweep DONE (label/icon COLOR): the foreground is DERIVED from
    // the resolved fill, so when tone/variant are live it re-resolves IN PLACE
    // — the icon via the primitive's reactive `.color(closure)` and the label
    // via a reactive style closure. The STATIC fast path stamps the snapshot
    // color (no flicker, no per-node Effect). Structure-reactivity (handled by
    // the `switch` below) is orthogonal: each arm rebuilds the children but
    // still threads the same fg machinery.

    // `build_pressable` builds the full pressable (children + style +
    // press-block) for one STRUCTURAL state — the spinner-vs-icon leading slot,
    // the optional trailing icon, the label, and the layout/dim style for these
    // booleans. It captures everything reactively, so both the static path and
    // each `switch` arm call it. Layer-1 (icon `.data()`) routing lives inside
    // its `icon_node`.
    let build_pressable = {
        let label = label.clone();
        let on_click = on_click.clone();
        let make_style = make_style.clone();
        let leading_icon = leading_icon.clone();
        let trailing_icon = trailing_icon.clone();
        let style_ovr = style_ovr.clone();
        let label_ovr = label_ovr.clone();
        let icon_ovr = icon_ovr.clone();
        move |loading: bool, has_lead: bool, has_trail: bool, disabled: bool, block: bool| {
            // A loading spinner occupies the leading slot, so it needs the same
            // centered-row layout an icon does.
            let row_layout = has_lead || has_trail || loading;
            // Both `disabled` and `loading` block the press; only `disabled`
            // dims the surface.
            let inert = disabled || loading;

            let style_closure = make_style(row_layout, block, disabled);
            // Snapshot the fg for this build (the resolved fill's foreground)
            // so the label + icons can carry it on their own nodes (native
            // doesn't inherit text/icon color).
            let fg = resolve_style(&style_closure()).color.clone();
            // Re-resolves the container's foreground from the live tone/variant.
            // Used by the reactive icon `.color`/label-style closures so the
            // tint tracks the container in place when a style axis is live.
            let resolve_fg = {
                let style_closure = style_closure.clone();
                move || resolve_style(&style_closure()).color.clone()
            };

            // Icon-slot override: a `color` in `icon_style` wins over the theme
            // foreground for the icon tint; the whole override sheet also layers
            // onto the icon box style (size/margin/…).
            let icon_color_ovr = override_rules(&icon_ovr).color;

            // Builds one inline icon node from a `Reactive<Option<IconData>>`
            // slot known to be `Some` here. LAYER 1: when the slot is a live
            // `Dynamic`, route the glyph reactively via the primitive's
            // `.data()` setter so a glyph SWAP (same presence) updates in place
            // — no rebuild. (Presence changes are handled by the `switch`
            // scrutinee, which keys on `.is_some()`, not the glyph itself —
            // `IconData` isn't `PartialEq`.) Color tint follows the same
            // static-vs-reactive split the label uses.
            let icon_node = |slot: &Reactive<Option<IconData>>| -> Element {
                let mut el = icon(slot.get().expect("icon slot is Some in this arm"))
                    .with_style(apply_override(
                        StyleApplication::new(button_icon_sheet()),
                        &icon_ovr,
                    ));
                if !slot.is_static() {
                    // Live glyph: the slot is `Some` in this arm (presence is
                    // the switch key); read the inner data reactively. The
                    // `unwrap_or` guards the impossible "arm says Some, closure
                    // now None" — a presence flip rebuilds the arm instead.
                    let slot = slot.clone();
                    el = el.data(move || slot.get().unwrap_or(EMPTY_ICON_DATA));
                }
                // Explicit icon-color override takes precedence over the theme fg.
                if let Some(oc) = icon_color_ovr.clone() {
                    return el.color(move || oc.resolve()).into_element();
                }
                if style_is_reactive {
                    let resolve_fg = resolve_fg.clone();
                    match fg.clone() {
                        Some(_) => el
                            .color(move || {
                                resolve_fg()
                                    .map(|c| c.resolve())
                                    .unwrap_or_else(|| Color("#000000".into()))
                            })
                            .into_element(),
                        None => el.into_element(),
                    }
                } else {
                    match fg.clone() {
                        // Reactive read: `resolve()` re-runs on theme swap, so
                        // the icon tint tracks the token like the label does.
                        Some(c) => el.color(move || c.resolve()).into_element(),
                        None => el.into_element(),
                    }
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
            // A label override applies on top of the theme fg (its `color`, if
            // set, wins). When present it forces the styled path even if there's
            // no theme fg to stamp.
            let has_label_ovr = label_ovr.is_some();
            let label_node = if style_is_reactive {
                // Live tone/variant: re-resolve the fg inside a reactive style
                // closure so the label color tracks the container fill in place,
                // then layer the (static) label override on top.
                if fg.is_some() || has_label_ovr {
                    let resolve_fg = resolve_fg.clone();
                    let label_ovr = label_ovr.clone();
                    text(label.clone())
                        .with_style(move || {
                            let mut app = StyleApplication::new(Rc::new(StyleSheet::r#static(
                                StyleRules::default(),
                            )));
                            if let Some(c) = resolve_fg() {
                                app = app.override_color(c);
                            }
                            apply_override(app, &label_ovr)
                        })
                        .into_element()
                } else {
                    text(label.clone()).into_element()
                }
            } else {
                match (fg.clone(), has_label_ovr) {
                    (Some(c), _) => text(label.clone())
                        .with_style(apply_override(
                            StyleApplication::new(label_color_style(c)),
                            &label_ovr,
                        ))
                        .into_element(),
                    (None, true) => text(label.clone())
                        .with_style(apply_override(
                            StyleApplication::new(Rc::new(StyleSheet::r#static(
                                StyleRules::default(),
                            ))),
                            &label_ovr,
                        ))
                        .into_element(),
                    (None, false) => text(label.clone()).into_element(),
                }
            };
            children.push(label_node);
            if has_trail {
                children.push(icon_node(&trailing_icon));
            }

            let on_click_for_p = on_click.clone();
            let mut bound = runtime_core::pressable(children, move || (on_click_for_p)());
            // Layer the root `style` override on the pressable box (background,
            // border, padding, radius, …) on top of the resolved theme style.
            bound = if style_is_reactive {
                let style_ovr = style_ovr.clone();
                bound.with_style(move || apply_override(style_closure(), &style_ovr))
            } else {
                bound.with_style(apply_override(style_closure(), &style_ovr))
            };
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
/// reactive `.data()` closure never actually observes `None`).
const EMPTY_ICON_DATA: IconData = IconData {
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
    use super::*;
    use idea_theme::theme::{install_idea_theme, light_theme};
    use runtime_core::{resolve_style, FillRule, StyleSource};

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
    fn text_node_color(el: &Element) -> Option<Color> {
        match el {
            Element::Text { style, .. } => {
                let app = match style.as_ref()? {
                    StyleSource::Static(a) => a.clone(),
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
    // The label text node must carry the intent foreground itself. A test
    // that passed against the old (bare, uncolored) text node is not a
    // valid regression test — so we assert the label node's OWN resolved
    // color is the intent-primary-solid-text white.
    #[test]
    fn regression_filled_button_label_carries_intent_text_color() {
        theme();
        let props = ButtonProps {
            label: Reactive::Static("Save".into()),
            tone: Reactive::Static(ToneRef::default()), // Primary
            variant: Reactive::Static(VariantRef::default()), // Filled
            ..Default::default()
        };
        let (children, _) = pressable_parts(Button(&props));
        let label = &children[0];
        let color = text_node_color(label)
            .expect("filled button label must carry its own color, not inherit from the pressable");
        assert_eq!(
            color.0.to_ascii_lowercase(),
            "#ffffff",
            "filled-Primary label is the intent-primary-solid-text white"
        );
    }

    // Same root cause for the leading/trailing icons: native icons don't
    // inherit the button color, so the wrapper must stamp the resolved
    // foreground on each icon's own color closure. Assert the icon carries
    // a color override that resolves to the intent text white.
    #[test]
    fn regression_filled_button_icons_carry_intent_text_color() {
        theme();
        let props = ButtonProps {
            label: Reactive::Static("Save".into()),
            leading_icon: Reactive::Static(Some(PLUS)),
            trailing_icon: Reactive::Static(Some(PLUS)),
            ..Default::default()
        };
        let (children, _) = pressable_parts(Button(&props));
        for (i, slot) in [0usize, 2].iter().zip(["leading", "trailing"]) {
            match &children[*i] {
                Element::Icon { color, .. } => {
                    let c = color
                        .as_ref()
                        .unwrap_or_else(|| panic!("{slot} icon must carry an explicit color"));
                    assert_eq!(
                        c().0.to_ascii_lowercase(),
                        "#ffffff",
                        "{slot} icon tint is the intent text white"
                    );
                }
                _ => panic!("expected an icon at slot {i}"),
            }
        }
    }

    fn pressable_parts(el: Element) -> (Vec<Element>, StyleApplication) {
        match el {
            Element::Pressable {
                children, style, ..
            } => {
                let app = match style.expect("Button always attaches a style") {
                    StyleSource::Static(a) => a,
                    _ => panic!("Button uses a static style source"),
                };
                (children, app)
            }
            _ => panic!("Button renders a Pressable"),
        }
    }

    // D3: the wrapper must pass leading/trailing icons through as icon
    // children (the primitive supported them; the wrapper dropped them).
    #[test]
    fn icons_become_children_around_the_label() {
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
        assert!(
            matches!(children[0], Element::Icon { .. }),
            "first child is the leading icon"
        );
        assert!(
            matches!(children[2], Element::Icon { .. }),
            "last child is the trailing icon"
        );

        // Without icons, the button is just the label — no stray slots.
        let plain = ButtonProps {
            label: Reactive::Static("Save".into()),
            ..Default::default()
        };
        let (children, _) = pressable_parts(Button(&plain));
        assert_eq!(children.len(), 1, "label only when no icons");
        assert!(matches!(children[0], Element::Text { .. }));
    }

    // D3: a leading/trailing icon forces the centered-row layout (the
    // base sheet doesn't pin one) so the icon and label sit inline.
    #[test]
    fn icon_button_lays_out_as_centered_row() {
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
    }

    // D3: `block` stretches the button to its container's width.
    #[test]
    fn block_stretches_to_container_width() {
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
    }

    // Regression: a non-block button must HUG its content — `align_self:
    // Center` (NOT `Stretch`) — so a flex parent's default `align-items:
    // stretch` can't grow it to the row's height (a button row) or the
    // column's width. This is the "buttons flex to the parent height" bug;
    // `Center` (vs `FlexStart`) also lets a centering preview card center it.
    // `block` opts back into Stretch.
    #[test]
    fn regression_non_block_button_hugs_content_not_stretch() {
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
    }

    // `disabled` dims the surface with a deterministic opacity (not a
    // hover-state overlay) AND blocks the press.
    #[test]
    fn disabled_button_dims_and_blocks_press() {
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
        assert!(d(), "disabled reports the button inert");
    }

    // `loading` puts a spinner in the leading slot and blocks the press,
    // without dimming the surface (it reads as busy, not off).
    #[test]
    fn loading_button_shows_spinner_and_blocks_press() {
        theme();
        let mk = || ButtonProps {
            label: Reactive::Static("Saving".into()),
            loading: Reactive::Static(true),
            ..Default::default()
        };
        let (children, app) = pressable_parts(Button(&mk()));
        assert!(
            matches!(children[0], Element::ActivityIndicator { .. }),
            "loading renders a spinner as the leading child"
        );
        assert!(
            resolve_style(&app).opacity.as_ref().map(|t| t.resolve()) != Some(0.45),
            "loading does not dim like disabled"
        );
        let d = pressable_disabled(Button(&mk())).expect("loading blocks the press");
        assert!(d(), "loading reports the button inert");
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
        theme();
        let sheet = installed_button_sheet();

        let base = resolve_style(&StyleApplication::new(sheet.clone()));
        assert_eq!(
            base.opacity.as_ref().map(|t| t.resolve()),
            Some(1.0),
            "resting button is fully opaque so the hover/press dim has a value to animate back to"
        );

        let hovered =
            resolve_style(&StyleApplication::new(sheet.clone()).with("__state_hovered", "on"));
        assert_eq!(
            hovered.opacity.as_ref().map(|t| t.resolve()),
            Some(0.92),
            "hover dims the button"
        );

        let pressed = resolve_style(&StyleApplication::new(sheet).with("__state_pressed", "on"));
        assert_eq!(
            pressed.opacity.as_ref().map(|t| t.resolve()),
            Some(0.85),
            "press dims the button further"
        );
    }

    fn pressable_disabled(el: Element) -> Option<Box<dyn Fn() -> bool>> {
        match el {
            Element::Pressable { disabled, .. } => disabled,
            _ => panic!("Button renders a Pressable"),
        }
    }

    /// Build a one-off static override sheet setting a single color.
    fn color_sheet(hex: &str) -> Rc<StyleSheet> {
        Rc::new(StyleSheet::r#static(StyleRules {
            color: Some(Tokenized::Literal(Color(hex.into()))),
            ..Default::default()
        }))
    }

    // Slot override: `label_style` colour wins over the theme foreground and is
    // stamped on the label's OWN node (native-safe). This is the "dark label on
    // a white/neutral button" case the CSS cascade can't do on native.
    #[test]
    fn label_style_overrides_label_color() {
        theme();
        let props = ButtonProps {
            label: Reactive::Static("Get started".into()),
            label_style: Some(color_sheet("#0b6b3a")),
            ..Default::default()
        };
        let (children, _) = pressable_parts(Button(&props));
        let color = text_node_color(&children[0]).expect("label carries its own color");
        assert_eq!(
            color.0.to_ascii_lowercase(),
            "#0b6b3a",
            "label_style color overrides the theme foreground on the label node",
        );
    }

    // Slot override: the root `style` layers onto the pressable box on top of
    // the theme style (background here) without disturbing untouched fields.
    #[test]
    fn style_overrides_container_box() {
        theme();
        let ovr = Rc::new(StyleSheet::r#static(StyleRules {
            background: Some(Tokenized::Literal(Color("#ffffff".into()))),
            ..Default::default()
        }));
        let props = ButtonProps {
            label: Reactive::Static("Go".into()),
            style: Some(ovr),
            ..Default::default()
        };
        let (_, app) = pressable_parts(Button(&props));
        assert_eq!(
            resolve_style(&app).background.as_ref().map(|c| c.resolve().0.to_ascii_lowercase()),
            Some("#ffffff".to_string()),
            "root style override wins for the container background",
        );
    }

    // Slot override: `icon_style` colour wins for the icon tint.
    #[test]
    fn icon_style_overrides_icon_tint() {
        theme();
        let props = ButtonProps {
            label: Reactive::Static("Go".into()),
            leading_icon: Reactive::Static(Some(PLUS)),
            icon_style: Some(color_sheet("#0b6b3a")),
            ..Default::default()
        };
        let (children, _) = pressable_parts(Button(&props));
        match &children[0] {
            Element::Icon { color, .. } => {
                let c = color.as_ref().expect("icon carries an explicit color");
                assert_eq!(
                    c().0.to_ascii_lowercase(),
                    "#0b6b3a",
                    "icon_style color overrides the theme foreground for the icon tint",
                );
            }
            _ => panic!("expected the leading icon"),
        }
    }

    // D4: `disabled` is a plain `bool` — `disabled = true` (not
    // `Some(Rc::new(|| true))`) compiles and marks the button inert.
    #[test]
    fn disabled_bool_marks_the_button_inert() {
        theme();
        let on = ButtonProps {
            disabled: Reactive::Static(true),
            ..Default::default()
        };
        let d = pressable_disabled(Button(&on)).expect("disabled=true sets a disabled source");
        assert!(d(), "the source reports the button as disabled");

        // Default leaves the press path live (no disabled source attached).
        let off = ButtonProps::default();
        assert!(
            pressable_disabled(Button(&off)).is_none(),
            "a non-disabled button attaches no disabled source"
        );
    }
}
