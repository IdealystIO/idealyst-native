//! Media builders: `image()`, `icon()`, `link()`.

use std::rc::Rc;

use runtime_core::accessibility::AccessibilityProps;
use runtime_core::assets::{kinds, Asset};
use runtime_core::primitives::icon::{IconData, IconHandle, StrokeAnimation};
use runtime_core::primitives::image::ImageHandle;
use runtime_core::primitives::link::LinkHandle;
use runtime_core::{Color, ImageErrorHandler, ImageLoadHandler};
use runtime_scene::{item, Element};
use runtime_world::{IntoValue, Value};

use crate::prims::{IconPrim, ImagePrim, LinkPrim, PrimCell};
use crate::style_attach::IntoStyleProp;

use super::SceneChild;

/// Start an `image`. `.src(...)` is required (`build()` panics without
/// it, matching the old constructor's mandatory argument).
pub fn image() -> ImageBuilder {
    ImageBuilder {
        src: None,
        prim: ImagePrim {
            src: Value::Const(String::new()),
            alt: Value::Const(None),
            on_load: None,
            on_error: None,
            asset: None,
            style: None,
            a11y: AccessibilityProps::default(),
            ref_fill: None,
        },
    }
}

pub struct ImageBuilder {
    src: Option<Value<String>>,
    prim: ImagePrim,
}

impl ImageBuilder {
    /// Image URL — static (`&str`/`String`) or reactive
    /// (signal/closure).
    pub fn src(mut self, src: impl IntoValue<String>) -> Self {
        self.src = Some(src.into_value());
        self
    }

    /// Declarative asset source; registered with the backend before
    /// `create_image` and rendered via the `asset://{id}` sentinel.
    pub fn asset(mut self, asset: Asset<kinds::Image>) -> Self {
        let id = asset.id;
        self.prim.asset = Some(asset);
        self.src = Some(Value::Const(format!("asset://{}", id.0)));
        self
    }

    /// Static alt text.
    pub fn alt(mut self, alt: impl Into<String>) -> Self {
        self.prim.alt = Value::Const(Some(alt.into()));
        self
    }

    /// Live alt text — updated in place when the closure's signals
    /// change (`None` clears).
    pub fn alt_dyn(mut self, f: impl Fn() -> Option<String> + 'static) -> Self {
        self.prim.alt = Value::Dyn(Box::new(f));
        self
    }

    pub fn on_load(mut self, handler: ImageLoadHandler) -> Self {
        self.prim.on_load = Some(handler);
        self
    }

    pub fn on_error(mut self, handler: ImageErrorHandler) -> Self {
        self.prim.on_error = Some(handler);
        self
    }

    pub fn style(mut self, style: impl IntoStyleProp) -> Self {
        self.prim.style = Some(style.into_style_prop());
        self
    }

    pub fn a11y(mut self, a11y: AccessibilityProps) -> Self {
        self.prim.a11y = a11y;
        self
    }

    pub fn on_handle(mut self, fill: impl FnOnce(ImageHandle) + 'static) -> Self {
        self.prim.ref_fill = Some(Box::new(fill));
        self
    }

    pub fn build(mut self) -> Element {
        self.prim.src = self
            .src
            .expect("image(): .src(...) (or .asset(...)) is required");
        item(PrimCell::new(self.prim), Vec::new())
    }
}

/// Start an `icon`. `.data(...)` is required (`build()` panics without
/// it, matching the old `icon(data)` constructor).
pub fn icon() -> IconBuilder {
    IconBuilder {
        data: None,
        color: None,
        stroke: None,
        draw_in: None,
        style: None,
        a11y: AccessibilityProps::default(),
        ref_fill: None,
    }
}

pub struct IconBuilder {
    data: Option<Value<IconData>>,
    color: Option<Value<Color>>,
    stroke: Option<Value<f32>>,
    draw_in: Option<StrokeAnimation>,
    style: Option<crate::style_attach::StyleProp>,
    a11y: AccessibilityProps,
    ref_fill: Option<Box<dyn FnOnce(IconHandle)>>,
}

impl IconBuilder {
    /// Static vector data.
    pub fn data(mut self, data: IconData) -> Self {
        self.data = Some(Value::Const(data));
        self
    }

    /// Live vector data — the glyph swaps in place on signal change.
    pub fn data_dyn(mut self, f: impl Fn() -> IconData + 'static) -> Self {
        self.data = Some(Value::Dyn(Box::new(f)));
        self
    }

    /// Static fill/stroke color.
    pub fn color(mut self, color: Color) -> Self {
        self.color = Some(Value::Const(color));
        self
    }

    /// Live color.
    pub fn color_dyn(mut self, f: impl Fn() -> Color + 'static) -> Self {
        self.color = Some(Value::Dyn(Box::new(f)));
        self
    }

    /// Static stroke progress (0.0..=1.0), applied at mount.
    pub fn stroke(mut self, progress: f32) -> Self {
        self.stroke = Some(Value::Const(progress));
        self
    }

    /// Live stroke progress.
    pub fn stroke_dyn(mut self, f: impl Fn() -> f32 + 'static) -> Self {
        self.stroke = Some(Value::Dyn(Box::new(f)));
        self
    }

    /// Mount draw-in animation: snap to `from`, animate on the next
    /// microtask.
    pub fn draw_in(mut self, anim: StrokeAnimation) -> Self {
        self.draw_in = Some(anim);
        self
    }

    pub fn style(mut self, style: impl IntoStyleProp) -> Self {
        self.style = Some(style.into_style_prop());
        self
    }

    pub fn a11y(mut self, a11y: AccessibilityProps) -> Self {
        self.a11y = a11y;
        self
    }

    pub fn on_handle(mut self, fill: impl FnOnce(IconHandle) + 'static) -> Self {
        self.ref_fill = Some(Box::new(fill));
        self
    }

    pub fn build(self) -> Element {
        let prim = IconPrim {
            data: self.data.expect("icon(): .data(...) is required"),
            color: self.color,
            stroke: self.stroke,
            draw_in: self.draw_in,
            style: self.style,
            a11y: self.a11y,
            ref_fill: self.ref_fill,
        };
        item(PrimCell::new(prim), Vec::new())
    }
}

/// Start a `link`. External links (`.external(true)`) default their
/// activation to the platform URL opener; non-external links require
/// `.on_activate(...)` until the navigator layer lands (the handler
/// panics otherwise — a link that silently does nothing is a footgun).
pub fn link() -> LinkBuilder {
    LinkBuilder {
        prim: LinkPrim {
            url: Value::Const(String::new()),
            external: false,
            on_activate: None,
            style: None,
            a11y: AccessibilityProps::default(),
            ref_fill: None,
        },
        children: Vec::new(),
    }
}

pub struct LinkBuilder {
    prim: LinkPrim,
    children: Vec<Element>,
}

impl LinkBuilder {
    pub fn child(mut self, child: impl SceneChild) -> Self {
        self.children.push(child.into_child());
        self
    }

    pub fn children(mut self, children: Vec<Element>) -> Self {
        self.children.extend(children);
        self
    }

    /// Destination URL — static or reactive (the rendered href swaps in
    /// place via `update_link_url`).
    pub fn url(mut self, url: impl IntoValue<String>) -> Self {
        self.prim.url = url.into_value();
        self
    }

    /// `true` = leaves the app (platform external handler).
    pub fn external(mut self, external: bool) -> Self {
        self.prim.external = external;
        self
    }

    /// Activation callback (navigation dispatch for in-app links).
    pub fn on_activate(mut self, f: impl Fn() + 'static) -> Self {
        self.prim.on_activate = Some(Rc::new(f));
        self
    }

    pub fn style(mut self, style: impl IntoStyleProp) -> Self {
        self.prim.style = Some(style.into_style_prop());
        self
    }

    pub fn a11y(mut self, a11y: AccessibilityProps) -> Self {
        self.prim.a11y = a11y;
        self
    }

    pub fn on_handle(mut self, fill: impl FnOnce(LinkHandle) + 'static) -> Self {
        self.prim.ref_fill = Some(Box::new(fill));
        self
    }

    pub fn build(self) -> Element {
        item(PrimCell::new(self.prim), self.children)
    }
}
