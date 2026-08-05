//! `Tag` — labelled pill with optional close button, built on the
//! extensible Tone + Variant trait surface.
//!
//! ```ignore
//! use std::rc::Rc;
//! use idea_ui::extensible::tag::{tag, TagProps};
//! use idea_theme::extensible::{tone, variant};
//!
//! ui! {
//!     Tag(
//!         label = "Rust",
//!         tone = tone::Primary,
//!         variant = variant::Soft,
//!         on_remove = Some(Rc::new(move || remove("Rust"))),
//!     )
//! }
//! ```
//!
//! Same Tone + Variant axes as [`badge`](super::badge::badge) — the
//! only difference is the optional close affordance. Reuses
//! [`Tag`](crate::stylesheets::Tag) base sheet for the container
//! and [`TagLabel`](crate::stylesheets::TagLabel)/[`TagClose`](crate::stylesheets::TagClose)
//! for the children.

use std::rc::Rc;

use runtime_core::{
    component, ui, Element, IdealystSchema, IntoElement, Reactive, StyleApplication, StyleSheet,
};

use idea_theme::extensible::{
    installed_tag_sheet, installed_tag_text_sheets, tone, variant, ToneRef, VariantRef,
};

use crate::stylesheets::TagClose;

/// Resolves `text_style` and overlays the parent fill's foreground
/// `color` onto its own node.
///
/// Native `UILabel`/`TextView` do NOT inherit text color from a parent
/// (only web's CSS cascade does), so a label colored solely via its
/// wrapping container renders invisible on the colored fill on
/// iOS/Android. Resolving the container's `color` and stamping it on the
/// label node makes every backend match web — the same pattern
/// `Typography` uses (color lives on the text node). The merged
/// `Tokenized` values keep their token references, so theme swaps still
/// re-resolve in bulk via the cohort.
// The label/`×` text color used to be COMPOSED at the call site
// (resolve the container fill, copy its foreground onto an anonymous
// static sheet). Native needs the color on the text node itself, but
// per-instance composition is invisible to the premint dump, so every
// Tag text node dragged the live style engine into `--premint` builds.
// The text sheets now carry their own enumerated `appearance` axis
// (`installed_tag_text_sheets`, built by `TagSheetBuilder` alongside
// the fill), so the component applies the same key it gives the
// container.

// Reactive-by-default: `#[props]` wraps `tone`/`variant` → `Reactive<…>`;
// `label` is already reactive and `on_remove` (an `Rc<dyn Fn()>` handler) is
// auto-skipped. Bare markers (`tone = tone::Primary`) coerce to
// `Reactive<ToneRef>` via the marker's generated `From`. The style-driving
// props route into the container-style sink, read `.get()` INSIDE so the
// apply-style Effect subscribes to whichever are live.
#[runtime_core::props]
#[cfg_attr(feature = "docs", derive(idea_ui::doc_controls::DocControls))]
#[derive(IdealystSchema)]
pub struct TagProps {
    /// Tag text. `Reactive<String>` — static or live (signal/`rx!`).
    pub label: Reactive<String>,
    /// Semantic color palette. Default Neutral.
    pub tone: ToneRef,
    /// Surface treatment (Soft, Filled, Outline, …). Default Soft.
    pub variant: VariantRef,
    /// When `Some`, a close button renders to the right of the label.
    pub on_remove: Option<Rc<dyn Fn()>>,
}

impl Default for TagProps {
    fn default() -> Self {
        Self {
            label: Reactive::Static(String::new()),
            tone: tone::Neutral.into(),
            variant: variant::Soft.into(),
            on_remove: None,
        }
    }
}

/// Renders a tone/variant-styled pill containing `label`, with an
/// optional close button (when `on_remove` is set) to its right.
#[component]
pub fn Tag(props: &TagProps) -> Element {
    let label = props.label.clone();
    let tone = props.tone.clone();
    let variant = props.variant.clone();

    // The container style is REACTIVE when tone/variant is live; else the
    // build-time fast path (no flicker — see Button). The closure reads each
    // prop's `.get()` INSIDE so the apply-style Effect subscribes to whichever
    // are dynamic. The `hug` layer keeps the tag sized to content instead of
    // stretching to a flex parent's row height (see `components::hug_self`).
    let style_is_reactive = !tone.is_static() || !variant.is_static();
    let make_container_style = {
        let tone = tone.clone();
        let variant = variant.clone();
        move || {
            let appearance_key = format!("{}_{}", tone.get().key(), variant.get().key());
            StyleApplication::new(installed_tag_sheet())
                .with("appearance", appearance_key)
        }
    };

    // The label + close glyph apply the SAME appearance key as the
    // container, on their own text sheets (enumerated color-only axis —
    // native doesn't inherit text color, and per-instance composition
    // blocked preminting; see the module comment above). Reactive
    // closure when tone/variant are live, snapshot key otherwise.
    let texts = installed_tag_text_sheets();
    let make_text_app = {
        let tone = tone.clone();
        let variant = variant.clone();
        move |sheet: Rc<StyleSheet>| -> StyleApplication {
            let key = format!("{}_{}", tone.get().key(), variant.get().key());
            StyleApplication::new(sheet).with("appearance", key)
        }
    };

    let container_style = make_container_style();

    let label_el: Element = if style_is_reactive {
        let make_text_app = make_text_app.clone();
        let sheet = texts.label.clone();
        runtime_core::text(label)
            .with_style(move || make_text_app(sheet.clone()))
            .into_element()
    } else {
        let label_style = make_text_app(texts.label.clone());
        ui! { text(style = label_style) { label } }
    };
    let close_style = TagClose();

    let mut children: Vec<Element> = Vec::with_capacity(2);
    children.push(label_el);
    if let Some(on_remove) = props.on_remove.clone() {
        // The `×` is a bare text node inside the pressable; color it on
        // its own node so it's visible on native (TagClose only sizes
        // the affordance and "inherits" foreground — which native won't).
        // Reactive when tone/variant are live (re-resolves the fg in place),
        // else the static snapshot color.
        let close_text = if style_is_reactive {
            let make_text_app = make_text_app.clone();
            let sheet = texts.glyph.clone();
            runtime_core::text("×".to_string())
                .with_style(move || make_text_app(sheet.clone()))
                .into_element()
        } else {
            runtime_core::text("×".to_string())
                .with_style(make_text_app(texts.glyph.clone()))
                .into_element()
        };
        let close = runtime_core::pressable(vec![close_text], move || (on_remove)())
            .with_style(close_style)
            .into_element();
        children.push(close);
    }

    let node = runtime_core::view(children);
    if style_is_reactive {
        node.with_style(make_container_style).into_element()
    } else {
        node.with_style(container_style).into_element()
    }
}

#[cfg(test)]
mod tests {

    /// Mirror of the Alert regression: the label/glyph sheets must
    /// premint — the point of replacing the call-site composition.
    #[test]
    fn regression_tag_text_slots_premint() {
        use idea_theme::extensible::installed_tag_text_sheets;
        with_test_world(|| {
            install_idea_theme(light_theme());
            let texts = installed_tag_text_sheets();
            for sheet in [&texts.label, &texts.glyph] {
                let app = StyleApplication::new(sheet.clone())
                    .with("appearance", "success_soft".to_string());
                assert!(
                    app.preminted_class_list().is_some(),
                    "tag text sheet must premint (was call-site composition)"
                );
            }
        });
    }

    use super::*;
    use crate::test_support::{classify, P, TStyle};
    use idea_theme::testing::with_test_world;
    use idea_theme::theme::{install_idea_theme, light_theme};
    use runtime_core::resolve_style;

    fn theme() {
        install_idea_theme(light_theme());
    }

    fn view_children(el: Element) -> Vec<Element> {
        match classify(el) {
            P::View { children, .. } => children,
            _ => panic!("Tag renders a View"),
        }
    }

    fn text_node_color(el: Element) -> Option<runtime_core::Color> {
        match classify(el) {
            P::Text { style, .. } => {
                let app = match style? {
                    TStyle::App(a) => a,
                    _ => panic!("Tag label uses a static style"),
                };
                resolve_style(&app).color.clone().map(|c| c.resolve())
            }
            _ => None,
        }
    }

    /// The intent foreground the filled container resolves to — the color
    /// the label MUST carry on its own node (native won't inherit it).
    fn container_fg() -> runtime_core::Color {
        let app = StyleApplication::new(installed_tag_sheet())
            .with("appearance", "primary_filled".to_string());
        resolve_style(&app)
            .color
            .clone()
            .expect("the filled container resolves a foreground")
            .resolve()
    }

    // Field report 3.1b: a bare label colored only via the container's
    // appearance vanished on native (no parent-color inheritance). The
    // label node must carry the intent foreground itself. A test that
    // passed against the old bare/uncolored label is not a valid
    // regression — so we assert the label node's OWN resolved color equals
    // the filled container's foreground (white intent-primary-solid-text).
    #[test]
    fn regression_filled_tag_label_carries_intent_text_color() {
        with_test_world(|| {
            theme();
            let props = TagProps {
                label: Reactive::Static("Rust".into()),
                tone: tone::Primary.into(),
                variant: variant::Filled.into(),
                ..Default::default()
            };
            let mut children = view_children(Tag(&props));
            let color = text_node_color(children.remove(0))
                .expect("tag label must carry its own color, not inherit from the container");
            assert_eq!(color, container_fg());
            assert_eq!(color.0.to_ascii_lowercase(), "#ffffff");
    });
    }

    // The close `×` is also a bare text node; it must carry the color too.
    #[test]
    fn regression_filled_tag_close_glyph_carries_intent_text_color() {
        with_test_world(|| {
            theme();
            let props = TagProps {
                label: Reactive::Static("Rust".into()),
                tone: tone::Primary.into(),
                variant: variant::Filled.into(),
                on_remove: Some(std::rc::Rc::new(|| {})),
            };
            let mut children = view_children(Tag(&props));
            // [label, close-pressable]; the close glyph is the pressable's child.
            let close_glyph = match classify(children.remove(1)) {
                P::Pressable { mut children, .. } => children.remove(0),
                _ => panic!("close is a Pressable"),
            };
            let color = text_node_color(close_glyph)
                .expect("close glyph must carry its own color");
            assert_eq!(color, container_fg());
    });
    }
}
