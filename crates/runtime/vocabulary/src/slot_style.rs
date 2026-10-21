//! Applying a component slot to an element: [`SlotStyle`].
//!
//! `component_styles!` generates one accessor per slot
//! (`FieldStyles::input()`) returning a `SlotStyle<FieldStyles>`. The
//! component selects axis values on it — typed, so only this component's
//! axes fit — optionally layers the instance's per-slot override, and hands
//! it to a primitive's `style`:
//!
//! ```ignore
//! view(style = FieldStyles::input()
//!     .set(appearance)            // FieldAppearance — or a Signal / derived(...)
//!     .set(size)
//!     .overridden(props.styles.input.as_ref()))
//! ```
//!
//! The sheet behind it is the slot's COMPOSED sheet — the declaration plus
//! every app restyle (see `runtime_shared::component_styles`) — so a
//! restyle reaches every instance with no change at the call site. The
//! conversion is premint-aware the same way the `stylesheet!` builder is:
//! an all-static application stamps build-time classes, a reactive one
//! whose sheet premints swaps build-time classes from a per-node effect,
//! and only what genuinely needs the engine falls through to it.

use std::marker::PhantomData;
use std::rc::Rc;

use runtime_shared::component_styles::{self, AxisKey, ComponentStyles, SlotOverride};
use runtime_shared::{IntoVariantSource, StyleApplication, StyleRules, StyleSheet};

use crate::style_attach::{IntoStyleProp, StyleProp};

type AxisRead = Rc<dyn Fn() -> &'static str>;

/// A set of axis selections for component `S`, shareable across its slots:
/// build it once from the props and hand it to every slot with
/// [`SlotStyle::values`], so a restyle that adds an arm for an axis to ANY
/// slot sees that axis's value there.
pub struct AxisValues<S: ComponentStyles> {
    axes: Vec<(&'static str, AxisRead)>,
    reactive: bool,
    _s: PhantomData<S>,
}

impl<S: ComponentStyles> Clone for AxisValues<S> {
    fn clone(&self) -> Self {
        Self { axes: self.axes.clone(), reactive: self.reactive, _s: PhantomData }
    }
}

impl<S: ComponentStyles> Default for AxisValues<S> {
    fn default() -> Self {
        Self { axes: Vec::new(), reactive: false, _s: PhantomData }
    }
}

impl<S: ComponentStyles> AxisValues<S> {
    /// No selections — every axis at its default.
    pub fn new() -> Self {
        Self::default()
    }

    fn put(&mut self, axis: &'static str, read: AxisRead, reactive: bool) {
        self.reactive |= reactive;
        match self.axes.iter_mut().find(|(a, _)| *a == axis) {
            Some(slot) => slot.1 = read,
            None => self.axes.push((axis, read)),
        }
    }

    /// Select a value on one of `S`'s axes: the axis type picks the axis.
    /// A plain value, a `Signal`, or `derived(...)`; a reactive source
    /// re-applies the style when it changes.
    pub fn set<A, V>(mut self, value: V) -> Self
    where
        A: AxisKey<Styles = S>,
        V: IntoVariantSource<A>,
    {
        let reactive = V::is_reactive(&value);
        self.put(A::AXIS, Rc::from(value.into_variant_source()), reactive);
        self
    }

    /// Select `value` on an axis the component drives from an open set of
    /// its own rather than a generated axis type (idea-ui's `tone`).
    pub fn raw(mut self, axis: &'static str, value: &'static str) -> Self {
        self.put(axis, Rc::new(move || value), false);
        self
    }

    /// Reactive [`Self::raw`]: `read` runs inside the style effect.
    pub fn raw_with(mut self, axis: &'static str, read: impl Fn() -> &'static str + 'static) -> Self {
        self.put(axis, Rc::new(read), true);
        self
    }

    /// Fold another set in; its selections win on shared axes.
    pub fn merge(mut self, other: &AxisValues<S>) -> Self {
        for (axis, read) in &other.axes {
            self.put(axis, read.clone(), false);
        }
        self.reactive |= other.reactive;
        self
    }

    /// Whether any selection is reactive.
    pub fn is_reactive(&self) -> bool {
        self.reactive
    }

    fn apply(&self, mut app: StyleApplication) -> StyleApplication {
        for (axis, read) in &self.axes {
            let value = read();
            component_styles::check_axis_value::<S>(axis, value);
            app = app.with(*axis, value);
        }
        app
    }
}

/// One slot of component `S`, ready to style an element. See the module
/// docs.
pub struct SlotStyle<S: ComponentStyles> {
    slot: usize,
    values: AxisValues<S>,
    inline: Option<Rc<StyleRules>>,
}

impl<S: ComponentStyles> SlotStyle<S> {
    /// Slot `index` of `S` (its position in the declaration). The
    /// generated per-slot accessors call this.
    pub fn new(index: usize) -> Self {
        Self { slot: index, values: AxisValues::new(), inline: None }
    }

    /// Select a value on one of `S`'s axes — see [`AxisValues::set`].
    pub fn set<A, V>(mut self, value: V) -> Self
    where
        A: AxisKey<Styles = S>,
        V: IntoVariantSource<A>,
    {
        self.values = self.values.set(value);
        self
    }

    /// See [`AxisValues::raw`].
    pub fn raw(mut self, axis: &'static str, value: &'static str) -> Self {
        self.values = self.values.raw(axis, value);
        self
    }

    /// See [`AxisValues::raw_with`].
    pub fn raw_with(mut self, axis: &'static str, read: impl Fn() -> &'static str + 'static) -> Self {
        self.values = self.values.raw_with(axis, read);
        self
    }

    /// Apply a shared selection set (the component's props-level axes).
    pub fn values(mut self, values: &AxisValues<S>) -> Self {
        self.values = self.values.merge(values);
        self
    }

    /// Layer this instance's override for the slot over everything else
    /// (the generated `*SlotStyles` prop field). `None` is a no-op.
    pub fn overridden(mut self, o: Option<&SlotOverride>) -> Self {
        if let Some(o) = o {
            self.inline = Some(match self.inline.take() {
                None => o.0.clone(),
                Some(prev) => Rc::new((*prev).clone().merge(o.rules())),
            });
        }
        self
    }

    /// The slot's composed sheet.
    pub fn sheet(&self) -> Rc<StyleSheet> {
        component_styles::slot_sheet::<S>(self.slot)
    }

    /// Whether any selection is reactive.
    pub fn is_reactive(&self) -> bool {
        self.values.is_reactive()
    }

    /// The live-engine application this describes — for components that
    /// compose or introspect resolved rules (`resolve_style(&app).color` to
    /// tint an icon). Reactive selections are read once, at this call, and
    /// subscribe whatever effect is running: call it inside the closure
    /// that should re-run.
    pub fn into_style_application(self) -> StyleApplication {
        build(&self.sheet(), &self.values, &self.inline)
    }
}

fn build<S: ComponentStyles>(
    sheet: &Rc<StyleSheet>,
    values: &AxisValues<S>,
    inline: &Option<Rc<StyleRules>>,
) -> StyleApplication {
    let mut app = values.apply(StyleApplication::new(sheet.clone()));
    if let Some(rules) = inline {
        app = app.with_inline_shared(rules.clone());
    }
    app
}

impl<S: ComponentStyles> IntoStyleProp for SlotStyle<S> {
    fn into_style_prop(self) -> StyleProp {
        let sheet = self.sheet();
        let SlotStyle { values, inline, .. } = self;
        if !values.is_reactive() {
            // Static: the `StyleApplication` conversion already premints
            // (guard-aware) and carries the inline layer.
            return build(&sheet, &values, &inline).into_style_prop();
        }
        // Reactive and premintable with no inline layer: every arm of every
        // axis already has a build-time rule, so a selection change is a
        // class swap — the same `PremintedDynamic` path the `stylesheet!`
        // builder takes. (An inline layer rides `SheetDynamic`, whose
        // `--premint-only` attach premints it too.)
        #[cfg(idealyst_premint)]
        {
            let eligible = inline.is_none()
                && sheet.premint_class().is_some_and(runtime_shared::minted_class_known);
            if eligible {
                return StyleProp::PremintedDynamic {
                    class_of: Box::new(move || {
                        build(&sheet, &values, &None)
                            .preminted_class_list()
                            .expect("a premint-classed sheet with no runtime layers has a class list")
                    }),
                    overrides: None,
                };
            }
        }
        StyleProp::SheetDynamic(Box::new(move || build(&sheet, &values, &inline)))
    }
}
