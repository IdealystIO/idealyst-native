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
    /// When `true`, blocks the press and dims the button. Default `false`.
    /// For reactive disabling, read a signal in the enclosing render scope
    /// (e.g. `disabled = some_state.get()`); the scope re-renders the button
    /// when the value changes.
    pub disabled: bool,
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
            tone: ToneRef::default(),
            variant: VariantRef::default(),
            size: ButtonSizeRef::default(),
            shape: ShapeRef::default(),
            disabled: false,
            bind_to: None,
            leading_icon: None,
            trailing_icon: None,
            block: false,
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
    let tone = props.tone.clone();
    let variant = props.variant.clone();
    let size = props.size.clone();
    let shape = props.shape.clone();
    let disabled = props.disabled;
    let bind_to = props.bind_to;
    let leading_icon = props.leading_icon;
    let trailing_icon = props.trailing_icon;
    let block = props.block;

    // Variant-axis keys map directly to the installed stylesheet's
    // pre-generated arms. For a built-in modifier set the arms exist;
    // for an app-extended set, apps must have installed an extended
    // sheet that includes those arms (else the framework falls back
    // to the default arms).
    let appearance_key = format!("{}_{}", tone.key(), variant.key());
    let size_key = size.key().to_string();
    let shape_key = shape.key().to_string();

    // STATIC style — applied at build time (before first paint) and
    // re-applied in bulk by the theme cohort on `set_theme`. A
    // reactive closure here would defer the apply to a per-node
    // Effect, letting the element paint once with browser-default
    // styles before the themed class lands — which the CSS transition
    // then animates (the on-load / on-navigation flicker). The
    // variant-axis keys are fixed per instance, so nothing here needs
    // to be reactive; theme swaps flow through the CSS-variable tokens.
    let mut style = StyleApplication::new(installed_button_sheet())
        .with("appearance", appearance_key)
        .with("size", size_key)
        .with("shape", shape_key);

    // The Button base sheet doesn't pin a flex direction (it had a single
    // text child). With leading/trailing icons the contents must lay out
    // as a centered row with a small gap; with `block` the button must
    // stretch to its container's width. Both are call-site-varying, so
    // they ride a computed layer (keyed on the variation) rather than the
    // pregenerated variant arms — the framework caches one resolved
    // StyleRules per distinct key.
    let has_icon = leading_icon.is_some() || trailing_icon.is_some();
    if has_icon || block {
        let layer_key = format!("layout_{}_{}", has_icon as u8, block as u8);
        style = style.with_computed(layer_key, move || {
            let mut rules = StyleRules::default();
            if has_icon {
                rules.flex_direction = Some(FlexDirection::Row);
                rules.align_items = Some(runtime_core::AlignItems::Center);
                rules.gap = Some(Tokenized::token("spacing-xs", Length::Px(6.0)));
            }
            if block {
                // 100% width + stretch so the button fills its parent even
                // when the parent is a flex container that would otherwise
                // size it to content.
                rules.width = Some(Tokenized::Literal(Length::Percent(100.0)));
                rules.align_self = Some(AlignSelf::Stretch);
            }
            rules
        });
    }

    // Resolve the fill's foreground so the label + icons can carry it on
    // their own nodes (native doesn't inherit text/icon color — see
    // `label_color_style`). Resolved against the same appearance variant
    // the pressable uses, so the label color always matches the fill.
    let fg = resolve_style(&style).color.clone();

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
    if let Some(d) = leading_icon {
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
    let mut bound = runtime_core::pressable(children, move || (on_click_for_p)())
        .with_style(style);
    if disabled {
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
            tone: ToneRef::default(),     // Primary
            variant: VariantRef::default(), // Filled
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
            leading_icon: Some(PLUS),
            trailing_icon: Some(PLUS),
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
            leading_icon: Some(PLUS),
            trailing_icon: Some(PLUS),
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
            leading_icon: Some(PLUS),
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
            block: true,
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
            disabled: true,
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
