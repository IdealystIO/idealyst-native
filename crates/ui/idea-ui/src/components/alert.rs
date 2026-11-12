//! `Alert` — banner with a tone icon, title + optional body, an optional
//! trailing action slot, and a configurable close affordance, built on the
//! extensible Tone + Variant trait surface.
//!
//! ```ignore
//! use std::rc::Rc;
//! use idea_ui::{Alert, AlertClose, AlertIcon};
//! use idea_ui::{tone, variant};
//!
//! ui! {
//!     Alert(
//!         title = "Couldn't save",
//!         body = Some("Server returned 503.".to_string()),
//!         tone = tone::Danger,
//!         variant = variant::Soft,
//!         // Leading glyph: `Auto` (default) picks one from the tone,
//!         // `None` drops it, `Custom(data)` draws your own.
//!         icon = AlertIcon::Auto,
//!         // Trailing action slot — any element (a Button here).
//!         action = Some(ui! { Button(label = "Retry", on_click = retry) }),
//!         // Close affordance: `None` (default), `Button(handler)` for the
//!         // standard ×, or `Custom(element)` to supply your own.
//!         close = AlertClose::Button(Rc::new(move || hide_alert())),
//!     )
//! }
//! ```
//!
//! Same tone × variant axes as [`Tag`](super::tag::Tag). Every variant draws
//! a 1px border (Soft in the tone's border color, Filled in its own fill), so
//! a soft banner separates from a white page and banners of mixed variants
//! line up. Styling is [`AlertStyles`]: `root` (the banner), `main` (the
//! icon + text row), `icon` (the leading glyph), `content` (the title/body
//! column), `title`, `body`, `close` (the built-in close button) and `glyph`
//! (its ✕). The text and glyph slots carry the fill's foreground on their own
//! nodes — native text and icons inherit no color from their container.
//!
//! Layout is a row that stretches to its column's width: the `main` row
//! (icon level with the title, then the title/body column) grows to fill,
//! then the optional `action`, then the optional `close`. The action centers
//! vertically in the banner; the icon, title and built-in ✕ keep to its top
//! line.

use std::rc::Rc;

use runtime_core::{
    component, component_styles, ui, AlignItems, AlignSelf, Color, Cursor, Element, FlexDirection, FontWeight,
    IconData, IdealystSchema, IntoElement, Length, Reactive, SlotRestyle, SlotStyle,
};

use idea_theme::extensible::{semantic_tones, tone, ToneRef};

use crate::components::date_input::CLEAR_GLYPH;
use crate::components::tone_slots::{bordered_surface_arms, foreground_arms, pill_icon, tinted_icon, with_tone};
use crate::modifiers::{marker_into_axis, variant};
use crate::theme::IdeaThemeRef;

/// Line height of the title — the line the leading icon and the built-in ✕
/// center on.
const TITLE_LINE_PX: f32 = 20.0;
/// Size of the leading tone icon and of the ✕ glyph.
const GLYPH_PX: f32 = 16.0;

component_styles! {
    /// The styled sections of [`Alert`].
    pub Alert<IdeaThemeRef> {
        /// The surface treatment — a tone-aware recipe per value.
        axis variant { filled, #[default] soft, outlined, ghost }

        /// The banner. Stretches across a column parent: a banner that
        /// hugs its title reads as a button, not a message.
        slot root {
            extend(root_surfaces)
            base(t) {
                flex_direction: FlexDirection::Row,
                align_self: AlignSelf::Stretch,
                align_items: AlignItems::Center,
                gap: t.spacing.md(),
                padding_vertical: t.spacing.md(),
                padding_horizontal: t.spacing.lg(),
                border_radius: t.radius.md(),
            }
        }

        /// The icon + title/body row. `flex_grow: 1` + `min_width: 0` lets
        /// it take the available width and shrink (wrapping text), so the
        /// trailing `action` and `close` align to the banner's far edge.
        /// Stretched to the banner's height, so in a banner taller than its
        /// text (a tall action) the title stays level with the built-in ✕.
        slot main {
            base(t) {
                flex_direction: FlexDirection::Row,
                align_self: AlignSelf::Stretch,
                align_items: AlignItems::FlexStart,
                gap: t.spacing.sm(),
                flex_grow: 1.0,
                flex_shrink: 1.0,
                min_width: 0.0,
            }
        }

        /// The leading glyph, centered on the title's line, in the fill's
        /// foreground.
        slot icon {
            extend(text_foregrounds)
            base(_t) {
                width: GLYPH_PX,
                height: GLYPH_PX,
                flex_shrink: 0.0,
                margin_top: (TITLE_LINE_PX - GLYPH_PX) / 2.0,
            }
        }

        /// The title/body column.
        slot content {
            base(_t) {
                flex_direction: FlexDirection::Column,
                flex_grow: 1.0,
                flex_shrink: 1.0,
                min_width: 0.0,
                // Below the theme's smallest spacing step (`xs` = 4px).
                gap: Length::Px(2.0),
            }
        }

        /// The title, in the fill's foreground.
        slot title {
            extend(text_foregrounds)
            base(t) {
                font_size: t.typography.body_size(),
                font_weight: FontWeight::SemiBold,
                line_height: TITLE_LINE_PX,
            }
        }

        /// The body line, in the fill's foreground.
        slot body {
            extend(text_foregrounds)
            base(t) {
                font_size: t.typography.body_sm_size(),
                font_weight: FontWeight::Normal,
                line_height: 18.0,
            }
        }

        /// The built-in close button: a title-line-high square with no fill,
        /// at the banner's top corner, dimmed until hovered.
        slot close {
            base(_t) {
                background: Color("transparent".into()),
                padding: 0.0,
                align_self: AlignSelf::FlexStart,
                align_items: AlignItems::Center,
                justify_content: runtime_core::JustifyContent::Center,
                cursor: Cursor::Pointer,
                width: TITLE_LINE_PX,
                height: TITLE_LINE_PX,
                flex_shrink: 0.0,
                opacity: 0.7,
            }
            state hovered(_t) { opacity: 1.0 }
            transitions {
                opacity: 150ms EaseOut,
            }
        }

        /// The close button's ✕, in the fill's foreground.
        slot glyph {
            extend(text_foregrounds)
            base(_t) {
                width: GLYPH_PX,
                height: GLYPH_PX,
            }
        }
    }
}

fn root_surfaces(s: &mut SlotRestyle<'_, AlertStyles>) {
    bordered_surface_arms(s, &semantic_tones());
}

fn text_foregrounds(s: &mut SlotRestyle<'_, AlertStyles>) {
    foreground_arms(s, &semantic_tones());
}

marker_into_axis!(AlertVariant {
    variant::Filled => Filled,
    variant::Soft => Soft,
    variant::Outlined => Outlined,
    variant::Ghost => Ghost,
});

/// The leading glyph of an [`Alert`].
#[derive(Clone, Default, runtime_core::Remote)]
pub enum AlertIcon {
    /// A glyph picked from the tone (the default): a check for Success, a
    /// triangle for Warning, an exclamation circle for Danger, an `i` circle
    /// for every other tone.
    #[default]
    Auto,
    /// No leading glyph.
    None,
    /// The given glyph, tinted in the fill's foreground like the auto one.
    Custom(IconData),
}

/// Lucide `info`.
pub(crate) const INFO_GLYPH: IconData = IconData {
    view_box: (24, 24),
    paths: &["M12 2a10 10 0 1 0 0 20a10 10 0 1 0 0-20z", "M12 16v-4", "M12 8h.01"],
    fill_rule: runtime_core::FillRule::NonZero,
    filled: false,
};
/// Lucide `circle-check`.
pub(crate) const SUCCESS_GLYPH: IconData = IconData {
    view_box: (24, 24),
    paths: &["M12 2a10 10 0 1 0 0 20a10 10 0 1 0 0-20z", "m9 12 2 2 4-4"],
    fill_rule: runtime_core::FillRule::NonZero,
    filled: false,
};
/// Lucide `triangle-alert`.
pub(crate) const WARNING_GLYPH: IconData = IconData {
    view_box: (24, 24),
    paths: &[
        "m21.73 18-8-14a2 2 0 0 0-3.48 0l-8 14A2 2 0 0 0 4 21h16a2 2 0 0 0 1.73-3",
        "M12 9v4",
        "M12 17h.01",
    ],
    fill_rule: runtime_core::FillRule::NonZero,
    filled: false,
};
/// Lucide `circle-alert`.
pub(crate) const DANGER_GLYPH: IconData = IconData {
    view_box: (24, 24),
    paths: &["M12 2a10 10 0 1 0 0 20a10 10 0 1 0 0-20z", "M12 8v4", "M12 16h.01"],
    fill_rule: runtime_core::FillRule::NonZero,
    filled: false,
};

/// The [`AlertIcon::Auto`] glyph for a tone key.
pub(crate) fn auto_glyph(tone_key: &str) -> IconData {
    match tone_key {
        "success" => SUCCESS_GLYPH,
        "warning" => WARNING_GLYPH,
        "danger" => DANGER_GLYPH,
        _ => INFO_GLYPH,
    }
}

/// The close affordance shown at an [`Alert`]'s trailing edge.
///
/// One prop expresses all three modes so there's no "show a close?" flag
/// that has to agree with a separate "what does it do?" handler.
#[derive(runtime_core::Remote)]
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

/// `idealyst docs` previews edit `close` as "attach a handler": on is the
/// standard `×` with a no-op handler, off is no close affordance.
/// (`Custom` takes an element the preview can't make up.) Compiled only in
/// a docs build (`#[prop_control]`).
#[runtime_core::prop_control]
impl runtime_core::previews::PropControl for AlertClose {
    fn control() -> runtime_core::previews::Control {
        runtime_core::previews::Control::Optional {
            inner: Box::new(runtime_core::previews::Control::Handler),
            example: Some(Box::new(runtime_core::previews::ControlValue::Handler)),
        }
    }
    fn to_value(&self) -> runtime_core::previews::ControlValue {
        let attached = !matches!(self, AlertClose::None);
        runtime_core::previews::ControlValue::Optional(
            attached.then(|| Box::new(runtime_core::previews::ControlValue::Handler)),
        )
    }
    fn from_value(value: &runtime_core::previews::ControlValue) -> Option<Self> {
        match value {
            runtime_core::previews::ControlValue::Optional(None) => Some(AlertClose::None),
            runtime_core::previews::ControlValue::Optional(Some(_)) => Some(AlertClose::Button(Rc::new(|| {}))),
            _ => None,
        }
    }
    fn code(&self) -> Option<String> {
        Some(match self {
            AlertClose::None => "AlertClose::None".to_string(),
            _ => "AlertClose::Button(Rc::new(|| {}))".to_string(),
        })
    }
    fn imports(value: &runtime_core::previews::ControlValue) -> Vec<String> {
        let mut paths = vec![concat!(module_path!(), "::AlertClose").to_string()];
        if matches!(value, runtime_core::previews::ControlValue::Optional(Some(_))) {
            paths.push("std::rc::Rc".to_string());
        }
        paths
    }
}

// Reactive-by-default: `#[props]` wraps the scalar data props (`tone`/`variant`)
// → `Reactive<…>`; `title`/`body` are already `Reactive`. `action` (an
// `Option<Element>`) is skipped (Element isn't wrapped), and `icon` / `close`
// (custom element-builder enums) are `#[prop(static)]`.
#[runtime_core::props(styles = AlertStyles)]
#[derive(IdealystSchema)]
pub struct AlertProps {
    /// Alert title. `Reactive<String>` — static or live (signal/`rx!`).
    #[schema(constraint = "reactive: static String or Signal/rx!")]
    #[prop(example = "Changes saved")]
    pub title: Reactive<String>,
    /// Optional second-line detail text, beneath the title.
    /// `Reactive<Option<String>>` — static or live.
    #[schema(constraint = "reactive: static Option<String> or Signal/rx!")]
    #[prop(example = "Your profile is up to date.")]
    pub body: Reactive<Option<String>>,
    /// Semantic color palette (Info, Danger, Warning, Success, …).
    /// Default Info.
    pub tone: ToneRef,
    /// Surface treatment (Soft, Filled, Outlined, Ghost, or an app value).
    /// Default Soft.
    pub variant: AlertVariant,
    /// Leading glyph. See [`AlertIcon`]. Default [`AlertIcon::Auto`] (one
    /// picked from the tone).
    #[prop(static)]
    pub icon: AlertIcon,
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
            variant: Reactive::Static(AlertVariant::default()),
            icon: AlertIcon::Auto,
            action: None,
            close: AlertClose::None,
            styles: Default::default(),
        }
    }
}

/// The leading glyph node, or `None` for [`AlertIcon::None`].
fn leading_icon(
    icon: AlertIcon,
    tone: &Reactive<ToneRef>,
    slot: Rc<dyn Fn() -> SlotStyle<AlertStyles>>,
) -> Option<Element> {
    match icon {
        AlertIcon::None => None,
        AlertIcon::Custom(data) => Some(pill_icon(data, &slot)),
        AlertIcon::Auto => match tone.clone() {
            Reactive::Static(t) => Some(pill_icon(auto_glyph(t.key()), &slot)),
            // A live tone swaps the glyph's paths in place.
            live => {
                let initial = auto_glyph(live.get_untracked().key());
                let el = runtime_core::icon(initial).data(move || auto_glyph(live.get().key()));
                Some(tinted_icon(el, &slot))
            }
        },
    }
}

/// Renders a banner with a tone icon, a bold title, an optional body line,
/// an optional trailing action slot, and an optional close affordance,
/// styled by the tone × variant axes.
#[component]
pub fn Alert(props: AlertProps) -> Element {
    let styles = props.styles.clone();
    let values = with_tone(AlertStyles::values().set(props.variant.clone()), &props.tone);

    let icon_node: Option<Element> = {
        let values = values.clone();
        let styles = styles.clone();
        leading_icon(
            props.icon,
            &props.tone,
            Rc::new(move || AlertStyles::icon().values(&values).overridden(styles.icon.as_ref())),
        )
    };

    let title = props.title.clone();
    let title_style = AlertStyles::title().values(&values).overridden(styles.title.as_ref());
    let body_node: Option<Element> = crate::components::optional_reactive_text(
        props.body.clone(),
        AlertStyles::body().values(&values).overridden(styles.body.as_ref()),
    );

    // Trailing slots. The action element is used verbatim; the close
    // affordance is built from `AlertClose`.
    let action_node: Option<Element> = props.action;
    let close_node: Option<Element> = match props.close {
        AlertClose::None => None,
        AlertClose::Button(on_press) => {
            let glyph_values = values.clone();
            let glyph_styles = styles.clone();
            let glyph = pill_icon(
                CLEAR_GLYPH,
                &(Rc::new(move || AlertStyles::glyph().values(&glyph_values).overridden(glyph_styles.glyph.as_ref()))
                    as Rc<dyn Fn() -> _>),
            );
            Some(
                runtime_core::pressable(vec![glyph], move || (on_press)())
                    .with_style(AlertStyles::close().values(&values).overridden(styles.close.as_ref()))
                    .into_element(),
            )
        }
        AlertClose::Custom(el) => Some(el),
    };

    let main_style = AlertStyles::main().values(&values).overridden(styles.main.as_ref());
    let content_style = AlertStyles::content().values(&values).overridden(styles.content.as_ref());
    let root_style = AlertStyles::root().values(&values).overridden(styles.root.as_ref());
    ui! {
        view(style = root_style) {
            view(style = main_style) {
                icon_node
                view(style = content_style) {
                    text(style = title_style) { title }
                    body_node
                }
            }
            action_node
            close_node
        }
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
        with_test_world(|| {
            install_idea_theme(light_theme());
            for slot in [AlertStyles::title(), AlertStyles::body(), AlertStyles::glyph()] {
                let app = slot.raw("tone", "danger").set(AlertVariant::Filled).into_style_application();
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
        let app = AlertStyles::root().raw("tone", "primary").set(AlertVariant::Filled).into_style_application();
        resolve_style(&app)
            .color
            .clone()
            .expect("the filled container resolves a foreground")
            .resolve()
    }

    /// `[icon?, content]` of the banner's `main` row, plus the trailing
    /// siblings after it.
    fn split(el: Element) -> (Vec<Element>, Vec<Element>) {
        let mut outer = view_children(el);
        let main = view_children(outer.remove(0));
        (main, outer)
    }

    fn icon_paths(el: Element) -> (&'static [&'static str], Option<runtime_core::Color>) {
        match classify(el) {
            P::Icon { data, color, .. } => (data.paths, color),
            _ => panic!("expected an icon"),
        }
    }

    // Field report 3.1b: the bare title/body text nodes were colored only
    // via the container appearance, so they vanished on native (no
    // parent-color inheritance). Each text node must carry the intent
    // foreground itself. Assert the title, body, leading icon AND close
    // glyph carry the filled container's resolved color (white
    // intent-primary-solid-text).
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

            let (mut main, mut trailing) = split(Alert(props));
            // main = [icon, content-column]; trailing = [close-pressable]
            let mut text_column = view_children(main.remove(1));
            let (_, icon_color) = icon_paths(main.remove(0));
            assert_eq!(icon_color.expect("the icon is tinted"), expected, "icon is the intent text color");

            let body = text_column.remove(1);
            let title_color =
                text_node_color(text_column.remove(0)).expect("title carries its own color");
            assert_eq!(title_color, expected, "title is the intent text color");
            let body_color = text_node_color(body).expect("body carries its own color");
            assert_eq!(body_color, expected, "body is the intent text color");

            let close_glyph = match classify(trailing.remove(0)) {
                P::Pressable { mut children, .. } => children.remove(0),
                _ => panic!("close is a Pressable"),
            };
            let (paths, close_color) = icon_paths(close_glyph);
            assert_eq!(paths, CLEAR_GLYPH.paths, "the close glyph is the vector ✕");
            assert_eq!(close_color.expect("close glyph carries its own color"), expected);

            assert_eq!(expected.0.to_ascii_lowercase(), "#ffffff");
        });
    }

    /// The trailing slots render in order: the icon/text row, then the
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
            assert_eq!(outer.len(), 3, "main + action + close");
            let kinds: Vec<P> = outer.into_iter().map(classify).collect();
            assert!(matches!(kinds[1], P::Text { .. }), "action slot renders the provided element");
            assert!(matches!(kinds[2], P::Pressable { .. }), "close is a Pressable");
        });
    }

    /// `AlertClose::Custom` uses the supplied element verbatim instead of
    /// building the standard ✕ Pressable.
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
            assert_eq!(outer.len(), 2, "main + custom close, no Pressable wrapper");
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
            let props = AlertProps { title: Reactive::Static("hi".into()), ..Default::default() };
            let outer = view_children(Alert(props));
            assert_eq!(outer.len(), 1, "no action, no close → just the main row");
        });
    }

    /// `AlertIcon::Auto` picks the glyph from the tone; `None` drops the
    /// node; `Custom` draws the given data.
    #[test]
    fn auto_icon_follows_the_tone() {
        with_test_world(|| {
            theme();
            let glyph_for = |tone: ToneRef, icon: AlertIcon| {
                let (mut main, _) =
                    split(Alert(AlertProps { title: Reactive::Static("t".into()), tone: Reactive::Static(tone), icon, ..Default::default() }));
                (main.len(), icon_paths(main.remove(0)).0)
            };
            assert_eq!(glyph_for(tone::Success.into(), AlertIcon::Auto), (2, SUCCESS_GLYPH.paths));
            assert_eq!(glyph_for(tone::Warning.into(), AlertIcon::Auto), (2, WARNING_GLYPH.paths));
            assert_eq!(glyph_for(tone::Danger.into(), AlertIcon::Auto), (2, DANGER_GLYPH.paths));
            assert_eq!(glyph_for(tone::Info.into(), AlertIcon::Auto), (2, INFO_GLYPH.paths));
            assert_eq!(glyph_for(tone::Primary.into(), AlertIcon::Auto), (2, INFO_GLYPH.paths));
            assert_eq!(glyph_for(tone::Info.into(), AlertIcon::Custom(CLEAR_GLYPH)), (2, CLEAR_GLYPH.paths));

            let (main, _) = split(Alert(AlertProps {
                title: Reactive::Static("t".into()),
                icon: AlertIcon::None,
                ..Default::default()
            }));
            assert_eq!(main.len(), 1, "AlertIcon::None → the text column alone");
            assert!(matches!(classify(main.into_iter().next().unwrap()), P::View { .. }));
        });
    }

    /// Regression (Soft alerts vanished on a white page): the soft tint is a
    /// 50-shade wash with no edge. Every variant now draws a 1px border —
    /// Soft in the tone's border color, Filled in its own fill — so a soft
    /// banner has an edge and mixed-variant banners are the same height.
    #[test]
    fn regression_every_alert_variant_draws_a_hairline_border() {
        with_test_world(|| {
            theme();
            let root = |tone: &'static str, v: AlertVariant| {
                resolve_style(&AlertStyles::root().raw("tone", tone).set(v).into_style_application())
            };
            for (name, v) in [
                ("filled", AlertVariant::Filled),
                ("soft", AlertVariant::Soft),
                ("outlined", AlertVariant::Outlined),
                ("ghost", AlertVariant::Ghost),
            ] {
                let r = root("success", v);
                assert_eq!(r.border_top_width.as_ref().map(|w| w.resolve()), Some(1.0), "{name} has a 1px border");
            }
            let soft = root("success", AlertVariant::Soft);
            let soft_border = soft.border_top_color.clone().expect("soft border colored").resolve();
            assert_eq!(soft_border.0.to_ascii_lowercase(), "#bbf7d0", "soft borders in the tone's border color");
            let filled = root("success", AlertVariant::Filled);
            assert_eq!(
                filled.border_top_color.clone().map(|c| c.resolve()),
                filled.background.clone().map(|c| c.resolve()),
                "filled borders in its own fill"
            );
        });
    }
}
