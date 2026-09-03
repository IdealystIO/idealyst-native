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
    installed_button_label_sheet, installed_button_sheet, ButtonSizeRef, ShapeRef, ToneRef,
    VariantRef,
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
            // Constant square → a build-time class. Without an identity this
            // sheet has no preminted CSS, so every Button with an icon fell
            // through to the live engine and kept it linked.
            *s.borrow_mut() = Some(sheet.premint_as("idea-ui.v1.button.icon"));
        }
        s.borrow().as_ref().cloned().unwrap()
    })
}

/// The text-typography subset the label must carry ON ITS OWN NODE.
// (Former `label_typography_style` — a per-instance snapshot of the
// container typography plus a color override — is replaced by the
// installed Button LABEL sheet (`installed_button_label_sheet`), whose
// enumerated `appearance`/`size` axes carry the same values and premint.
// Native text still gets everything on its own node; see
// `ButtonSheetBuilder::build_label`.)

/// Renders a styled, clickable button whose appearance is driven by
/// the tone × variant × size × shape axes of the installed Button sheet.
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
    // callback was silently dropped: the IO completed, the spinner
    // never stopped, and the done-writes vanished. Publishing the
    // body-scope token around the call gives handler-reached spawns
    // the Button's lifetime: alive across arm rebuilds, dead when the
    // Button actually unmounts (same teardown moment as before on the
    // static path, where the pressable IS the button's whole life).
    // Regression: `tests/loading_button_spawn.rs`.
    let on_click: Rc<dyn Fn()> = {
        let anchor = runtime_core::ScopeAlive::current();
        let inner = props.on_click.clone();
        Rc::new(move || anchor.run_within(|| inner()))
    };
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
                // Layout rides three sheet axes rather than one computed
                // layer keyed `layout_{row}_{block}_{disabled}`. They're three
                // independent booleans, so they enumerate — and a computed
                // layer is opaque to premint, which disqualified every Button
                // on the page. See `ButtonSheetBuilder::build`.
                StyleApplication::new(installed_button_sheet())
                    .with("appearance", appearance_key)
                    .with("size", size.get().key().to_string())
                    .with("shape", shape.get().key().to_string())
                    .with("layout", if row_layout { "row" } else { "column" }.to_string())
                    .with("block", if block { "on" } else { "off" }.to_string())
                    .with("dimmed", if disabled { "on" } else { "off" }.to_string())
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
            // Snapshot the resolved container foreground for this build. The
            // icons + spinner carry the fg on their OWN nodes because native
            // doesn't inherit text/icon color. When the container application
            // ATTACHES PREMINTED (a `--premint`/`--premint-only` web build,
            // no runtime override), skip the read-back: the fill's `color` is
            // in the box's build-time CSS, so the web icon/spinner inherit it
            // as `currentColor` — which also tracks `:hover`, something the
            // snapshot never did — and under `--premint-only` a resolve here
            // is the panic the stripped rule closure names. Native builds
            // never premint, so they keep the resolved read.
            let fg = {
                let container_app = style_closure();
                if container_app.attaches_preminted() {
                    None
                } else {
                    resolve_style(&container_app).color.clone()
                }
            };
            // Re-resolves the container's foreground from the live tone/variant.
            // Used by the reactive icon `.color`/label-style closures so the
            // tint tracks the container in place when a style axis is live.
            // Same premint gate, per evaluation.
            let resolve_fg = {
                let style_closure = style_closure.clone();
                move || {
                    let app = style_closure();
                    if app.attaches_preminted() {
                        None
                    } else {
                        resolve_style(&app).color.clone()
                    }
                }
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
            // The label applies the SAME appearance/size keys as the box, on
            // the installed LABEL sheet (color + typography as enumerated
            // arms — no per-instance snapshot, no color override, so it
            // premints). A `label_style` slot override still layers on top
            // and takes the live engine, as every runtime override does.
            let label_app = {
                let tone = tone.clone();
                let variant = variant.clone();
                let size = size.clone();
                move || {
                    StyleApplication::new(installed_button_label_sheet())
                        .with(
                            "appearance",
                            format!("{}_{}", tone.get().key(), variant.get().key()),
                        )
                        .with("size", size.get().key().to_string())
                }
            };
            let label_node = if style_is_reactive {
                let label_ovr = label_ovr.clone();
                let label_app = label_app.clone();
                text(label.clone())
                    .with_style(move || apply_override(label_app(), &label_ovr))
                    .into_element()
            } else {
                text(label.clone())
                    .with_style(apply_override(label_app(), &label_ovr))
                    .into_element()
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

    /// The LABEL sheet must premint for every (appearance, size) the box
    /// itself premints — it replaces the per-instance typography snapshot
    /// + color override, which forced every Button label onto the live
    /// engine under `--premint`.
    #[test]
    fn regression_button_label_sheet_premints() {
        use idea_theme::extensible::installed_button_label_sheet;
        with_test_world(|| {
            install_idea_theme(light_theme());
            let app = StyleApplication::new(installed_button_label_sheet())
                .with("appearance", "primary_filled".to_string())
                .with("size", "lg".to_string());
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
    use runtime_core::{resolve_style, FillRule};

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
    // The label text node must carry the intent foreground itself. A test
    // that passed against the old (bare, uncolored) text node is not a
    // valid regression test — so we assert the label node's OWN resolved
    // color is the intent-primary-solid-text white.
    #[test]
    fn regression_filled_button_label_carries_intent_text_color() {
        with_test_world(|| {
            theme();
            let props = ButtonProps {
                label: Reactive::Static("Save".into()),
                tone: Reactive::Static(ToneRef::default()), // Primary
                variant: Reactive::Static(VariantRef::default()), // Filled
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
    });
    }

    /// The pressable's evaluated `disabled` state.
    fn pressable_disabled(el: Element) -> Option<bool> {
        match classify(el) {
            P::Pressable { disabled, .. } => disabled,
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
        with_test_world(|| {
            theme();
            let props = ButtonProps {
                label: Reactive::Static("Get started".into()),
                label_style: Some(color_sheet("#0b6b3a")),
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
                icon_style: Some(color_sheet("#0b6b3a")),
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
}
