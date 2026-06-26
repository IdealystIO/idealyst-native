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

use idea_theme::extensible::{installed_button_sheet, ButtonSizeRef, ShapeRef, ToneRef, VariantRef};

/// Props for the extensible Button. Each modifier axis is a typed
/// handle (`*Ref` newtype) so call sites can write
/// `tone: tone::Primary.into()` instead of `Rc::new(...)`. Built-in
/// defaults route to Filled/Primary/Md/Md.
// Reactive-by-default: `#[props]` wraps each data field. Style axes
// (tone/variant/size/shape) route to the style sink; structural props
// (disabled/loading/block/icons) are read once (see the TODO in `Button`).
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
    /// Default `false`. For reactive disabling, read a signal in the
    /// enclosing render scope (e.g. `disabled = some_state.get()`); the scope
    /// re-renders the button when the value changes.
    pub disabled: bool,
    /// When `true`, swaps the leading slot for a spinner (tinted to the
    /// button's text color) and blocks the press while the action runs.
    /// Default `false`. Unlike `disabled` it does not dim the surface — the
    /// button reads as "busy", not "off".
    pub loading: bool,
    /// When `Some`, fills the given `Ref<PressableHandle>` on mount.
    /// Useful for anchoring an `Overlay` to this button.
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
            variant: Reactive::Static(VariantRef::default()),
            size: Reactive::Static(ButtonSizeRef::default()),
            shape: Reactive::Static(ShapeRef::default()),
            disabled: Reactive::Static(false),
            loading: Reactive::Static(false),
            bind_to: None,
            leading_icon: Reactive::Static(None),
            trailing_icon: Reactive::Static(None),
            block: Reactive::Static(false),
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
    // TODO(reactive-sweep): these drive STRUCTURE (press-block, spinner-vs-icon
    // children, the layout layer, and the resolved fg color stamped on the
    // label/icons), so they are snapshotted here. A live one needs a
    // `switch`/`when` rebuild (children) or a reactive fg-color sink, not a
    // style closure — flagged. The tone/variant/size/shape STYLE axes below
    // ARE routed reactively.
    let disabled = props.disabled.get();
    let loading = props.loading.get();
    let bind_to = props.bind_to;
    let leading_icon = props.leading_icon.get();
    let trailing_icon = props.trailing_icon.get();
    let block = props.block.get();
    // Both `disabled` and `loading` make the button inert (block the press);
    // only `disabled` dims the surface.
    let inert = disabled || loading;

    // Variant-axis keys map directly to the installed stylesheet's
    // pre-generated arms. For a built-in modifier set the arms exist;
    // for an app-extended set, apps must have installed an extended
    // sheet that includes those arms (else the framework falls back
    // to the default arms).
    // A loading spinner occupies the leading slot, so it needs the same
    // centered-row layout an icon does. (`row_layout`/`block`/`disabled` are
    // structural snapshots — see the TODO above.)
    let row_layout = leading_icon.is_some() || trailing_icon.is_some() || loading;

    // The button style is REACTIVE when any style axis (tone/variant/size/
    // shape) is live; otherwise the build-time fast path (no first-paint
    // flicker). `make_style` reads each axis live INSIDE so the apply-style
    // Effect subscribes to the dynamic ones. The layout layer keys on the
    // structural snapshots (icons/block/disabled).
    let style_is_reactive = !tone.is_static()
        || !variant.is_static()
        || !size.is_static()
        || !shape.is_static();
    let make_style = {
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
            let layer_key =
                format!("layout_{}_{}_{}", row_layout as u8, block as u8, disabled as u8);
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
    };

    // Resolve the fill's foreground so the label + icons can carry it on
    // their own nodes (native doesn't inherit text/icon color). Snapshot — a
    // live tone won't re-tint the label/icons (coupled fg sink, see TODO).
    let fg = resolve_style(&make_style()).color.clone();

    // Fixed-size sheet for inline icons — they need explicit dimensions
    // (an SVG/CAShapeLayer has no intrinsic content size to flex against).
    // Icons inherit the button's text color on web; stamp it explicitly so
    // they're correct on native too (an uncolored icon renders in the
    // widget-default color and vanishes on a colored fill).
    let icon_node = |data: IconData| -> Element {
        let el = icon(data).with_style(button_icon_sheet());
        match fg.clone() {
            // Reactive read: `resolve()` re-runs on theme swap, so the
            // icon tint tracks the token like the label's color does.
            Some(c) => el.color(move || c.resolve()).into_element(),
            None => el.into_element(),
        }
    };

    let mut children: Vec<Element> = Vec::with_capacity(3);
    // Loading takes the leading slot (a spinner) in place of the leading icon.
    if loading {
        let ai = activity_indicator().size(ActivityIndicatorSize::Small);
        children.push(match fg.clone() {
            Some(c) => ai.color(c.resolve()).into_element(),
            None => ai.into_element(),
        });
    } else if let Some(d) = leading_icon {
        children.push(icon_node(d));
    }
    let label_node = match fg.clone() {
        Some(c) => text(label).with_style(label_color_style(c)).into_element(),
        None => text(label).into_element(),
    };
    children.push(label_node);
    if let Some(d) = trailing_icon {
        children.push(icon_node(d));
    }
    let on_click_for_p = on_click.clone();
    let mut bound = runtime_core::pressable(children, move || (on_click_for_p)());
    bound = if style_is_reactive {
        bound.with_style(make_style)
    } else {
        bound.with_style(make_style())
    };
    if inert {
        bound = bound.disabled(true);
    }
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
            tone: Reactive::Static(ToneRef::default()),     // Primary
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
            Element::Pressable { children, style, .. } => {
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

        let block = ButtonProps { block: Reactive::Static(true), ..Default::default() };
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
        let mk = || ButtonProps { disabled: Reactive::Static(true), ..Default::default() };
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

        let hovered = resolve_style(
            &StyleApplication::new(sheet.clone()).with("__state_hovered", "on"),
        );
        assert_eq!(
            hovered.opacity.as_ref().map(|t| t.resolve()),
            Some(0.92),
            "hover dims the button"
        );

        let pressed =
            resolve_style(&StyleApplication::new(sheet).with("__state_pressed", "on"));
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
