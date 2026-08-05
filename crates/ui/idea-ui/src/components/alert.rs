//! `Alert` — banner with title + optional body, an optional trailing
//! action slot, and a configurable close affordance, built on the
//! extensible Tone + Variant trait surface.
//!
//! ```ignore
//! use std::rc::Rc;
//! use idea_ui::{Alert, AlertClose};
//! use idea_theme::extensible::{tone, variant};
//!
//! ui! {
//!     Alert(
//!         title = "Couldn't save",
//!         body = Some("Server returned 503.".to_string()),
//!         tone = tone::Danger,
//!         variant = variant::Soft,
//!         // Trailing action slot — any element (a Button here).
//!         action = Some(ui! { Button(label = "Retry", on_click = retry) }),
//!         // Close affordance: `None` (default), `Button(handler)` for the
//!         // standard ×, or `Custom(element)` to supply your own.
//!         close = AlertClose::Button(Rc::new(move || hide_alert())),
//!     )
//! }
//! ```
//!
//! Same Tone + Variant axes as [`badge`](super::badge::badge). Alert
//! has its own padding/font/radius in the base stylesheet, so no
//! Size/Shape axis — adding one would imply a continuous range of
//! banner densities which we don't have a use for yet.
//!
//! Layout is a row: a flex-growing title/body column, then the optional
//! `action`, then the optional `close`. The action and close slots take
//! caller-supplied elements verbatim (they carry their own styling and
//! handlers); only the built-in `×` close and the title/body get Alert's
//! native text-color stamping.

use std::rc::Rc;

use runtime_core::{
    component, ui, Element, IdealystSchema, IntoElement, Reactive, StyleApplication, StyleSheet,
};

use idea_theme::extensible::{
    installed_alert_sheet, installed_alert_text_sheets, tone, variant, ToneRef, VariantRef,
};

use crate::stylesheets::{AlertContent, TagClose};

// The title/body/`×` text nodes used to COMPOSE their color at the call
// site: resolve the container fill, copy its foreground onto a fresh
// anonymous `StyleSheet::r#static`. Native needs the color on the text
// node itself (no parent-color inheritance off web), but per-instance
// composition is invisible to the premint dump, so every Alert text
// node dragged the live style engine into `--premint` builds. The text
// sheets now carry their own enumerated `appearance` axis
// (`installed_alert_text_sheets`, built by `AlertSheetBuilder`
// alongside the fill), so the component just applies the same key it
// gives the container.

/// The close affordance shown at an [`Alert`]'s trailing edge.
///
/// One prop expresses all three modes so there's no "show a close?" flag
/// that has to agree with a separate "what does it do?" handler.
pub enum AlertClose {
    /// No close affordance. (Default.)
    None,
    /// The standard `×` glyph; invokes the handler when pressed. Alert
    /// styles and colors it (carrying the intent foreground on native).
    Button(Rc<dyn Fn()>),
    /// A caller-supplied element used in place of the `×`. Taken verbatim
    /// — it carries its own styling and press behaviour.
    Custom(Element),
}

impl Default for AlertClose {
    fn default() -> Self {
        AlertClose::None
    }
}

// Reactive-by-default: `#[props]` wraps the scalar data props (`tone`/`variant`)
// → `Reactive<…>`; `title`/`body` are already `Reactive`. `action` (an
// `Option<Element>`) is skipped (Element isn't wrapped), and `close`
// (`AlertClose`, a custom element-builder enum) is `#[prop(static)]`.
#[runtime_core::props]
#[cfg_attr(feature = "docs", derive(idea_ui::doc_controls::DocControls))]
#[derive(IdealystSchema)]
pub struct AlertProps {
    /// Alert title. `Reactive<String>` — static or live (signal/`rx!`).
    #[schema(constraint = "reactive: static String or Signal/rx!")]
    pub title: Reactive<String>,
    /// Optional second-line detail text, beneath the title.
    /// `Reactive<Option<String>>` — static or live.
    #[schema(constraint = "reactive: static Option<String> or Signal/rx!")]
    pub body: Reactive<Option<String>>,
    /// Semantic color palette (Info, Danger, Warning, Success, …).
    /// Default Info.
    pub tone: ToneRef,
    /// Surface treatment (Soft, Filled, Outline, …). Default Soft.
    pub variant: VariantRef,
    /// Optional trailing action slot — e.g. an "Undo"/"Retry" `Button`,
    /// or any element. Rendered after the text column, before `close`.
    /// Taken verbatim (carries its own styling and handlers).
    pub action: Option<Element>,
    /// Close affordance at the trailing edge. See [`AlertClose`]. Default
    /// [`AlertClose::None`] (no close).
    #[prop(static)]
    pub close: AlertClose,
}

impl Default for AlertProps {
    fn default() -> Self {
        Self {
            title: Reactive::Static(String::new()),
            body: Reactive::Static(None),
            // Info/Soft = the common informational alert. Use Danger/Filled
            // for breaking news, Warning/Soft for cautionary, etc.
            tone: tone::Info.into(),
            variant: variant::Soft.into(),
            action: None,
            close: AlertClose::None,
            // (tone/variant: marker `.into()` → `Reactive<…>`; title/body
            // already `Reactive::Static`; action/close unwrapped.)
        }
    }
}

/// Renders a banner with a bold title, optional body line, an optional
/// trailing action slot, and an optional close affordance, styled by the
/// tone × variant axes.
#[component]
pub fn Alert(props: AlertProps) -> Element {
    let tone = props.tone.clone();
    let variant = props.variant.clone();

    // The container appearance is REACTIVE when tone/variant is live; else the
    // build-time fast path (no first-paint flicker). The closure reads each
    // prop's `.get()` INSIDE so the apply-style Effect subscribes to whichever
    // are dynamic.
    let style_is_reactive = !tone.is_static() || !variant.is_static();
    let make_container_style = {
        let tone = tone.clone();
        let variant = variant.clone();
        move || {
            let appearance_key = format!("{}_{}", tone.get().key(), variant.get().key());
            StyleApplication::new(installed_alert_sheet()).with("appearance", appearance_key)
        }
    };

    // The title/body/`×` text nodes apply the SAME appearance key as the
    // container, on their own text sheets (an enumerated color-only axis —
    // see the module comment above). When tone/variant are live the key is
    // rebuilt inside a reactive style closure; the static fast path applies
    // the snapshot key with no per-node Effect. Either way there is no
    // per-instance sheet and no override, so both paths premint.
    let texts = installed_alert_text_sheets();
    let make_text_app = {
        let tone = tone.clone();
        let variant = variant.clone();
        move |sheet: Rc<StyleSheet>| -> StyleApplication {
            let key = format!("{}_{}", tone.get().key(), variant.get().key());
            StyleApplication::new(sheet).with("appearance", key)
        }
    };

    let title = props.title.clone();
    let title_node: Element = if style_is_reactive {
        let make_text_app = make_text_app.clone();
        let sheet = texts.title.clone();
        runtime_core::text(title)
            .with_style(move || make_text_app(sheet.clone()))
            .into_element()
    } else {
        let title_style = make_text_app(texts.title.clone());
        ui! { text(style = title_style) { title } }
    };

    let body_node: Option<Element> = if style_is_reactive {
        let make_text_app = make_text_app.clone();
        let sheet = texts.body.clone();
        crate::components::optional_reactive_text(
            props.body.clone(),
            move || make_text_app(sheet.clone()),
        )
    } else {
        crate::components::optional_reactive_text(
            props.body.clone(),
            make_text_app(texts.body.clone()),
        )
    };

    // Trailing slots. The action element is used verbatim; the close
    // affordance is built from `AlertClose`.
    let action_node: Option<Element> = props.action;
    let close_node: Option<Element> = match props.close {
        AlertClose::None => None,
        AlertClose::Button(on_press) => {
            // Bare `×` text node — color it directly so it shows on native.
            // Reactive when tone/variant are live; static snapshot otherwise.
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
            Some(
                runtime_core::pressable(vec![close_text], move || (on_press)())
                    .with_style(TagClose())
                    .into_element(),
            )
        }
        AlertClose::Custom(el) => Some(el),
    };

    // Content column: title + optional body.
    let content_style = AlertContent();
    let content_col: Element = ui! {
        view(style = content_style) {
            title_node
            body_node
        }
    };

    // Outer row: content column, then optional action, then optional close.
    // The container style is reactive (closure) when tone/variant are live,
    // else the static snapshot — matching the Tag/Button gate so the static
    // fast path stays a `StyleSource::Static` (no flicker, what the tests read).
    let mut children: Vec<Element> = Vec::with_capacity(3);
    children.push(content_col);
    if let Some(action) = action_node {
        children.push(action);
    }
    if let Some(close) = close_node {
        children.push(close);
    }
    let node = runtime_core::view(children);
    if style_is_reactive {
        node.with_style(make_container_style).into_element()
    } else {
        node.with_style(make_container_style()).into_element()
    }
}

#[cfg(test)]
mod tests {

    /// The text-slot sheets exist to take Alert's title/body/glyph OFF
    /// the live style engine under `--premint`: the former call-site
    /// composition (resolve container → copy color onto an anonymous
    /// sheet) had no premint class by construction. Every text
    /// application must therefore carry a preminted class list for
    /// every appearance the container itself premints.
    #[test]
    fn regression_alert_text_slots_premint() {
        use idea_theme::extensible::installed_alert_text_sheets;
        with_test_world(|| {
            install_idea_theme(light_theme());
            let texts = installed_alert_text_sheets();
            for sheet in [&texts.title, &texts.body, &texts.glyph] {
                let app = StyleApplication::new(sheet.clone())
                    .with("appearance", "danger_filled".to_string());
                assert!(
                    app.preminted_class_list().is_some(),
                    "alert text sheet must premint (was call-site composition)"
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
            _ => panic!("Alert renders a View"),
        }
    }

    fn text_node_color(el: Element) -> Option<runtime_core::Color> {
        match classify(el) {
            P::Text { style, .. } => {
                let app = match style? {
                    TStyle::App(a) => a,
                    _ => panic!("Alert text uses a static style"),
                };
                resolve_style(&app).color.clone().map(|c| c.resolve())
            }
            _ => None,
        }
    }

    fn container_fg() -> runtime_core::Color {
        let app = StyleApplication::new(installed_alert_sheet())
            .with("appearance", "primary_filled".to_string());
        resolve_style(&app)
            .color
            .clone()
            .expect("the filled container resolves a foreground")
            .resolve()
    }

    // Field report 3.1b: the bare title/body text nodes were colored only
    // via the container appearance, so they vanished on native (no
    // parent-color inheritance). Each text node must carry the intent
    // foreground itself. Assert the title, body, AND close glyph carry the
    // filled container's resolved color (white intent-primary-solid-text) —
    // an assertion the old uncolored nodes would have failed.
    #[test]
    fn regression_filled_alert_text_nodes_carry_intent_text_color() {
        with_test_world(|| {
            theme();
            let props = AlertProps {
                title: Reactive::Static("Saved".into()),
                body: Reactive::Static(Some("All changes persisted.".into())),
                tone: tone::Primary.into(),
                variant: variant::Filled.into(),
                close: AlertClose::Button(Rc::new(|| {})),
                ..Default::default()
            };
            let expected = container_fg();

            let mut outer = view_children(Alert(props));
            // [content-view, close-pressable]
            let close = outer.remove(1);
            let mut text_column = match classify(outer.remove(0)) {
                P::View { children, .. } => children,
                _ => panic!("first child is the content view"),
            };
            // title + body
            let body = text_column.remove(1);
            let title_color =
                text_node_color(text_column.remove(0)).expect("title carries its own color");
            assert_eq!(title_color, expected, "title is the intent text color");
            let body_color = text_node_color(body).expect("body carries its own color");
            assert_eq!(body_color, expected, "body is the intent text color");

            // close `×`
            let close_glyph = match classify(close) {
                P::Pressable { mut children, .. } => children.remove(0),
                _ => panic!("close is a Pressable"),
            };
            let close_color = text_node_color(close_glyph).expect("close glyph carries its own color");
            assert_eq!(close_color, expected, "close glyph is the intent text color");

            assert_eq!(expected.0.to_ascii_lowercase(), "#ffffff");
    });
    }

    /// The trailing slots render in order: content column, then the
    /// `action` element (verbatim), then the close affordance.
    #[test]
    fn renders_action_and_close_slots_in_order() {
        with_test_world(|| {
            theme();
            let action = runtime_core::text("Retry".to_string()).into_element();
            let props = AlertProps {
                title: Reactive::Static("Couldn't save".into()),
                tone: tone::Danger.into(),
                variant: variant::Soft.into(),
                action: Some(action),
                close: AlertClose::Button(Rc::new(|| {})),
                ..Default::default()
            };

            let outer = view_children(Alert(props));
            assert_eq!(outer.len(), 3, "content + action + close");
            let kinds: Vec<P> = outer.into_iter().map(classify).collect();
            // The action slot is the bare text node we passed, used verbatim.
            assert!(
                matches!(kinds[1], P::Text { .. }),
                "action slot renders the provided element"
            );
            assert!(matches!(kinds[2], P::Pressable { .. }), "close is a Pressable");
    });
    }

    /// `AlertClose::Custom` uses the supplied element verbatim instead of
    /// building the standard `×` Pressable.
    #[test]
    fn close_custom_renders_provided_element() {
        with_test_world(|| {
            theme();
            let custom = runtime_core::text("done".to_string()).into_element();
            let props = AlertProps {
                title: Reactive::Static("hi".into()),
                close: AlertClose::Custom(custom),
                ..Default::default()
            };
            let mut outer = view_children(Alert(props));
            // [content-view, custom-close-text] — no Pressable wrapper.
            assert_eq!(outer.len(), 2);
            assert!(
                matches!(classify(outer.remove(1)), P::Text { .. }),
                "custom close element is used verbatim"
            );
    });
    }

    /// `AlertClose::None` (the default) emits no close affordance.
    #[test]
    fn close_none_omits_affordance() {
        with_test_world(|| {
            theme();
            let props = AlertProps {
                title: Reactive::Static("hi".into()),
                ..Default::default()
            };
            let outer = view_children(Alert(props));
            assert_eq!(outer.len(), 1, "no action, no close → just the content column");
    });
    }
}
