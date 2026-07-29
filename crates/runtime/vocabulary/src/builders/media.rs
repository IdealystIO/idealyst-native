//! Media builders: `image()`, `icon()`, `link()`.

use std::rc::Rc;

use runtime_shared::accessibility::AccessibilityProps;
use runtime_shared::assets::{kinds, Asset};
use runtime_shared::primitives::icon::{IconData, IconHandle, StrokeAnimation};
use runtime_shared::primitives::image::ImageHandle;
use runtime_shared::primitives::link::LinkHandle;
use runtime_shared::{Color, ImageErrorHandler, ImageLoadHandler};
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
            test_id: None,
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

    /// Robot/automation anchor (`test_id = …`): the mount handler
    /// registers this node under `id` in the vocabulary robot registry
    /// (`robot` feature; the slot is inert otherwise).
    pub fn test_id(mut self, id: &'static str) -> Self {
        self.prim.test_id = Some(id);
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
        test_id: None,
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
    test_id: Option<&'static str>,
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

    /// Robot/automation anchor (`test_id = …`): the mount handler
    /// registers this node under `id` in the vocabulary robot registry
    /// (`robot` feature; the slot is inert otherwise).
    pub fn test_id(mut self, id: &'static str) -> Self {
        self.test_id = Some(id);
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
            test_id: self.test_id,
            a11y: self.a11y,
            ref_fill: self.ref_fill,
        };
        item(PrimCell::new(prim), Vec::new())
    }
}

/// Start a `link`. External links (`.external(true)`) default their
/// activation to the platform URL opener; in-app links declare a
/// destination via `.route(...)` (resolved against the enclosing
/// navigator's ambient `LinkActivator` at mount — P6) or a raw
/// `.on_activate(...)`; a link with none of the three panics at mount —
/// a link that silently does nothing is a footgun.
pub fn link() -> LinkBuilder {
    LinkBuilder {
        prim: LinkPrim {
            url: Value::Const(String::new()),
            external: false,
            on_activate: None,
            route_link: None,
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

    /// Activation callback (raw navigation dispatch for in-app links —
    /// prefer [`route`](Self::route) for typed route destinations).
    pub fn on_activate(mut self, f: impl Fn() + 'static) -> Self {
        self.prim.on_activate = Some(Rc::new(f));
        self
    }

    /// Declare a typed in-app destination: sets the pre-computed URL
    /// (`params.to_path(route.path())` — web `<a href>` + right-click
    /// affordances) and the [`RouteLink`](crate::prims::RouteLink)
    /// payload the mount handler resolves against the enclosing
    /// navigator's ambient `LinkActivator`. `P: Clone` reproduces a
    /// fresh boxed params payload per activation (the old `link()`
    /// contract).
    pub fn route<P>(
        mut self,
        route: &runtime_shared::primitives::navigator::Route<P>,
        params: P,
    ) -> Self
    where
        P: runtime_shared::primitives::navigator::RouteParams + Clone + 'static,
    {
        let url = params.to_path(route.path());
        let params_rc = Rc::new(params);
        self.prim.url = Value::Const(url);
        self.prim.route_link = Some(crate::prims::RouteLink {
            name: route.name(),
            make_params: Rc::new(move || Box::new((*params_rc).clone())),
        });
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
