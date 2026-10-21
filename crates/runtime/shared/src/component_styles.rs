//! Component styles: the styling contract a component library builds on.
//!
//! A component declares its styled sections once, as named **slots**
//! (`component_styles!` in the macro crate) — each slot an ordinary
//! [`StyleSheet`] with a base, variant arms, states, breakpoints and
//! compounds. The declaration produces a styles TYPE (`FieldStyles`) that
//! implements [`ComponentStyles`], and that type is the key everything
//! here hangs off:
//!
//! - **Restyle** — an app changes any slot app-wide with
//!   [`restyle`]`::<FieldStyles>(…)` (spelled `FieldStyles::restyle(…)`
//!   through the generated surface): merge properties into the base, into
//!   an arm, into a state, or add whole new arms and compounds.
//! - **Open axes** — every variant axis is a key newtype per component
//!   (`FieldAppearance`), implementing [`AxisKey`]. Built-in values are
//!   associated consts; an app defines more with `FieldAppearance::define`
//!   and gives them rules through a restyle. A value nothing styles warns
//!   in debug builds instead of silently drawing the base.
//! - **Defaults** — [`set_axis_default`] changes what an unset axis means,
//!   for every slot and for the axis type's `Default`.
//!
//! # Composition, not overrides
//!
//! A restyle is DATA merged into the slot sheet when the slot is composed
//! ([`StyleSheet::layer_base`] / [`StyleSheet::layer_variant`]), and the
//! composed sheet is finished with a premint identity derived from the
//! macro's content hash plus every merged rule set's
//! [`content_key`](StyleRules::content_key). The dump build and the shipped
//! bundle run the same `app()`, so they compose the same sheets and agree
//! on the class names: a restyled slot ships as build-time CSS exactly
//! like an untouched one. (A `with_overrides`/`with_computed` layer would
//! have sent every application of the slot to the live engine.)
//!
//! # Keyed by type
//!
//! The registry is keyed by the styles type's [`TypeId`], the way
//! `runtime_scene::Registry` keys payload handlers: two libraries that both
//! ship a `Field` never share an entry, and nothing is keyed by a name
//! string.
//!
//! # When to restyle
//!
//! Restyle at boot — inside `app()` before the tree is built, next to the
//! theme install. Restyles and theme installs are independent registries,
//! so their order does not matter. A restyle issued after a slot of that
//! component was already used still applies to everything built
//! afterwards, but elements built before keep their style, and a premint
//! build has no CSS for the new composition (the minted-class guard falls
//! back to the live engine) — so it logs a warning naming the component.
//! Re-running the SAME restyle (a second SSR request running `app()` again)
//! replaces the registration from that call site and recomposes to the
//! same identity, which is not a change and does not warn.

use std::any::TypeId;
use std::cell::RefCell;
use std::marker::PhantomData;
use std::rc::Rc;

use rustc_hash::{FxHashMap, FxHashSet};

use crate::style::{StyleRules, StyleSheet, VariantEnum, VariantSet};
use crate::{Breakpoint, StateBits};

// ---------------------------------------------------------------------------
// Declarations (what `component_styles!` generates)
// ---------------------------------------------------------------------------

/// A component's style declaration. Implemented by the type
/// `component_styles!` generates; the type is the registry key.
pub trait ComponentStyles: Sized + 'static {
    /// The token namespace restyle closures receive (`|t| StyleRules {
    /// background: Some(t.color.surface()), .. }`) — the declared
    /// vocabulary's [`TokenVocabulary::Tokens`](crate::TokenVocabulary).
    type Tokens: Default;
    /// The per-instance override set — one optional [`SlotOverride`] per
    /// slot (the generated `FieldSlotStyles`). What `#[props(styles =
    /// FieldStyles)]` adds to a props struct as its `styles` field.
    type Overrides: Default + Clone + 'static;
    /// The component name, for diagnostics.
    const NAME: &'static str;
    /// The slots, in declaration order. A slot's index is its position.
    fn slots() -> &'static [SlotDef<Self>];
    /// Every variant axis the component declares (its slots' `variant`
    /// blocks merged with the component-level `axis` declarations).
    fn axes() -> &'static [AxisDef];
}

/// One declared slot.
pub struct SlotDef<S: ComponentStyles> {
    /// The slot name (`input`, `label`, …).
    pub name: &'static str,
    /// Builds the slot's declared sheet, unfinished, so composition can
    /// merge restyles into it before giving it an identity.
    pub build: fn() -> StyleSheet,
    /// The macro's content-hash premint class for the declared sheet, or
    /// `None` when the declaration cannot premint (a non-constant
    /// `font_family`) — such a slot always resolves through the live
    /// engine.
    pub class: Option<&'static str>,
    /// The library's own programmatic contribution (`extend(path)` in the
    /// declaration): arms that are computed rather than written out — one
    /// compound per registered tone, say. Runs before every app restyle,
    /// at every composition, so it sees the registries it reads as they
    /// are when the slot is composed.
    pub extend: Option<fn(&mut SlotRestyle<'_, S>)>,
}

/// One declared variant axis.
pub struct AxisDef {
    /// Axis name as it appears in the `VariantSet` (`appearance`).
    pub name: &'static str,
    /// The declared values, in declaration order.
    pub values: &'static [&'static str],
    /// The `#[default]` value, if one is declared.
    pub default: Option<&'static str>,
}

/// A typed value of one component's variant axis — the key newtype
/// `component_styles!` generates per axis (`FieldAppearance`). The
/// associated [`Styles`](Self::Styles) type is what keeps a value of one
/// component's axis out of another component's restyle and slot API.
pub trait AxisKey: VariantEnum {
    /// The component this axis belongs to.
    type Styles: ComponentStyles;
    /// The axis name.
    const AXIS: &'static str;
}

// ---------------------------------------------------------------------------
// Restyle collection
// ---------------------------------------------------------------------------

#[derive(Clone, Debug, PartialEq)]
enum Target {
    Base,
    Arm(String, String),
    Compound(Vec<(String, String)>),
}

#[derive(Clone)]
struct Item {
    target: Target,
    rules: StyleRules,
}

impl Item {
    fn apply(&self, sheet: StyleSheet) -> StyleSheet {
        match &self.target {
            Target::Base => sheet.layer_base(self.rules.clone()),
            Target::Arm(axis, value) => sheet.layer_variant(axis.clone(), value.clone(), self.rules.clone()),
            Target::Compound(when) => {
                // Every leg's axis must be DECLARED on the sheet: the
                // preminted class list stamps one class per declared axis,
                // and a state is tracked only when the sheet registers its
                // `__state_*` axis — so a compound over an axis this slot
                // never styled would match on the live engine and never in
                // build-time CSS. An empty arm declares it and changes
                // nothing else.
                let mut sheet = sheet;
                for (axis, value) in when {
                    if !sheet.declares_axis(axis) {
                        sheet = sheet.variant(axis.clone(), value.clone(), |_vs: &VariantSet| StyleRules::default());
                    }
                }
                let rules = self.rules.clone();
                sheet.compound(when.clone(), move |_vs: &VariantSet| rules.clone())
            }
        }
    }

    /// The identity contribution: where the rules land plus their
    /// token-name-stable content key (a theme swap never moves it).
    fn descriptor(&self) -> String {
        let target = match &self.target {
            Target::Base => "base".to_string(),
            Target::Arm(a, v) => format!("arm:{a}={v}"),
            Target::Compound(when) => {
                let legs: Vec<String> = when.iter().map(|(a, v)| format!("{a}={v}")).collect();
                format!("compound:{}", legs.join("+"))
            }
        };
        format!("{target}{{{}}}", self.rules.content_key())
    }
}

/// The untyped patch set one composition collects: per slot, the merged
/// layers in application order.
struct Collector {
    slots: Vec<Vec<Item>>,
}

/// The typed handle a restyle closure receives. The generated
/// `FieldRestyle` wraps it with one accessor per slot
/// (`s.input()`, `s.label()`).
pub struct Restyle<'a, S: ComponentStyles> {
    collector: &'a mut Collector,
    _s: PhantomData<S>,
}

impl<'a, S: ComponentStyles> Restyle<'a, S> {
    /// The restyle handle for slot `index` (position in
    /// [`ComponentStyles::slots`]). The generated per-slot accessors call
    /// this; authors use those.
    pub fn slot(&mut self, index: usize) -> SlotRestyle<'_, S> {
        assert!(index < S::slots().len(), "{}: no slot #{index}", S::NAME);
        SlotRestyle { collector: self.collector, slot: index, _s: PhantomData }
    }
}

/// Restyle one slot. Every method MERGES: the properties a closure sets win,
/// everything it leaves unset keeps the declared value. Each closure gets
/// the component's token namespace, so a value is a token reference that a
/// theme swap re-resolves.
pub struct SlotRestyle<'a, S: ComponentStyles> {
    collector: &'a mut Collector,
    slot: usize,
    _s: PhantomData<S>,
}

impl<'a, S: ComponentStyles> SlotRestyle<'a, S> {
    fn push(&mut self, target: Target, rules: StyleRules) -> &mut Self {
        self.collector.slots[self.slot].push(Item { target, rules });
        self
    }

    /// The slot's name.
    pub fn name(&self) -> &'static str {
        S::slots()[self.slot].name
    }

    /// Merge into the slot's base rules.
    pub fn base(&mut self, f: impl FnOnce(&S::Tokens) -> StyleRules) -> &mut Self {
        let rules = f(&S::Tokens::default());
        self.push(Target::Base, rules)
    }

    /// Merge into the arm for `value` — a built-in value or one the app
    /// [`define`d](AxisKey). An arm the slot does not declare is created,
    /// which is how a custom variant value gets its look.
    pub fn arm<A: AxisKey<Styles = S>>(&mut self, value: A, f: impl FnOnce(&S::Tokens) -> StyleRules) -> &mut Self {
        let rules = f(&S::Tokens::default());
        self.push(Target::Arm(A::AXIS.to_string(), value.as_variant_str().to_string()), rules)
    }

    /// Merge into an interaction-state overlay (`StateBits::HOVERED`,
    /// `PRESSED`, `FOCUSED`, `DISABLED` — one bit).
    pub fn state(&mut self, state: StateBits, f: impl FnOnce(&S::Tokens) -> StyleRules) -> &mut Self {
        let axis = state
            .axis_name()
            .unwrap_or_else(|| panic!("{}.{}: `state` takes exactly one StateBits flag", S::NAME, self.name()));
        let rules = f(&S::Tokens::default());
        self.push(Target::Arm(axis.to_string(), "on".to_string()), rules)
    }

    /// Merge into a responsive overlay (`Sm`/`Md`/`Lg`/`Xl`; `Xs` is the
    /// base — use [`Self::base`]).
    pub fn breakpoint(&mut self, bp: Breakpoint, f: impl FnOnce(&S::Tokens) -> StyleRules) -> &mut Self {
        let axis = bp.axis_name().unwrap_or_else(|| {
            panic!("{}.{}: `Xs` is the base layer — restyle it with `base`", S::NAME, self.name())
        });
        let rules = f(&S::Tokens::default());
        self.push(Target::Arm(axis.to_string(), "on".to_string()), rules)
    }

    /// Add rules that apply only when every leg holds — a tuple of axis
    /// values and/or a `StateBits` flag: `(FieldAppearance::Outlined,
    /// FieldSize::Sm)` or `(ButtonVariant::Ghost, StateBits::HOVERED)`.
    /// Compounds layer after every single-axis arm, so this is how a
    /// restyle states which of two axes wins a shared property.
    pub fn compound<C: CompoundKey<S>>(&mut self, legs: C, f: impl FnOnce(&S::Tokens) -> StyleRules) -> &mut Self {
        let rules = f(&S::Tokens::default());
        self.push(Target::Compound(legs.legs()), rules)
    }

    /// Untyped [`Self::arm`]: merge `rules` into `axis = value`. For a
    /// library's own `extend` hook, on axes it drives from an open set it
    /// owns rather than from a generated axis type (idea-ui's `tone`).
    /// App code uses the typed form.
    pub fn raw_arm(&mut self, axis: &str, value: &str, rules: StyleRules) -> &mut Self {
        self.push(Target::Arm(axis.to_string(), value.to_string()), rules)
    }

    /// Untyped [`Self::compound`]; see [`Self::raw_arm`] for when.
    pub fn raw_compound(&mut self, legs: &[(&str, &str)], rules: StyleRules) -> &mut Self {
        let when = legs.iter().map(|(a, v)| (a.to_string(), v.to_string())).collect();
        self.push(Target::Compound(when), rules)
    }
}

/// One leg of a [`SlotRestyle::compound`] condition.
pub trait CompoundLeg<S: ComponentStyles> {
    /// `(axis, value)`.
    fn leg(self) -> (String, String);
}

impl<S: ComponentStyles, A: AxisKey<Styles = S>> CompoundLeg<S> for A {
    fn leg(self) -> (String, String) {
        (A::AXIS.to_string(), self.as_variant_str().to_string())
    }
}

impl<S: ComponentStyles> CompoundLeg<S> for StateBits {
    fn leg(self) -> (String, String) {
        let axis = self.axis_name().expect("a compound state leg takes exactly one StateBits flag");
        (axis.to_string(), "on".to_string())
    }
}

/// A [`SlotRestyle::compound`] condition: a tuple of two or three legs.
pub trait CompoundKey<S: ComponentStyles> {
    /// The `(axis, value)` pairs that must all hold.
    fn legs(self) -> Vec<(String, String)>;
}

impl<S: ComponentStyles, A: CompoundLeg<S>, B: CompoundLeg<S>> CompoundKey<S> for (A, B) {
    fn legs(self) -> Vec<(String, String)> {
        vec![self.0.leg(), self.1.leg()]
    }
}

impl<S: ComponentStyles, A: CompoundLeg<S>, B: CompoundLeg<S>, C: CompoundLeg<S>> CompoundKey<S> for (A, B, C) {
    fn legs(self) -> Vec<(String, String)> {
        vec![self.0.leg(), self.1.leg(), self.2.leg()]
    }
}

// ---------------------------------------------------------------------------
// Registry
// ---------------------------------------------------------------------------

type RestyleFn = Rc<dyn Fn(&mut Collector)>;

/// One composition of a component: its finished slot sheets and what the
/// debug unknown-value check consults.
struct Composed {
    sheets: Vec<Rc<StyleSheet>>,
    identities: Vec<String>,
    /// Every `(axis, value)` some slot styles (an arm or a compound leg),
    /// plus each declared axis's declared values.
    known: FxHashSet<(String, String)>,
}

#[derive(Default)]
struct Entry {
    /// `(call site, restyle)`, in first-registration order. A second call
    /// from the same site replaces its entry, so re-running `app()` does
    /// not stack copies.
    restyles: Vec<(&'static std::panic::Location<'static>, RestyleFn)>,
    /// App-chosen axis defaults (`set_axis_default`).
    defaults: Vec<(&'static str, &'static str)>,
    composed: Option<Rc<Composed>>,
    /// Identities of the last composition — to tell a real change from a
    /// re-registration that composes to the same sheets.
    last_identities: Option<Vec<String>>,
    /// Some slot of this component was handed to an element.
    read: bool,
}

runtime_tls::thread_local! {
    static REGISTRY: RefCell<FxHashMap<TypeId, Entry>> = RefCell::new(FxHashMap::default());
    static WARNED: RefCell<FxHashSet<String>> = RefCell::new(FxHashSet::default());
    static WARN_COUNT: std::cell::Cell<u32> = const { std::cell::Cell::new(0) };
}

fn warn_once(key: String, msg: impl FnOnce() -> String) {
    let fresh = WARNED.with(|w| w.borrow_mut().insert(key));
    if fresh {
        WARN_COUNT.with(|c| c.set(c.get() + 1));
        crate::logging::log(crate::logging::LogLevel::Warn, &msg());
    }
}

/// Test hook: how many component-style warnings this thread has logged.
#[doc(hidden)]
pub fn __warn_count() -> u32 {
    WARN_COUNT.with(|c| c.get())
}

/// Register a restyle of `S`. `f` runs at every composition of `S` (so it
/// must be `Fn`), after the library's own `extend` hooks and after the
/// restyles registered before it. A second registration from the same
/// call site replaces the first.
#[track_caller]
pub fn restyle<S: ComponentStyles>(f: impl Fn(&mut Restyle<'_, S>) + 'static) {
    let site = std::panic::Location::caller();
    let erased: RestyleFn = Rc::new(move |c: &mut Collector| {
        let mut r = Restyle::<S> { collector: c, _s: PhantomData };
        f(&mut r)
    });
    REGISTRY.with(|r| {
        let mut r = r.borrow_mut();
        let entry = r.entry(TypeId::of::<S>()).or_default();
        match entry.restyles.iter_mut().find(|(s, _)| *s == site) {
            Some(slot) => slot.1 = erased,
            None => entry.restyles.push((site, erased)),
        }
        entry.composed = None;
    });
}

/// Make `value` what an unset `A` axis means across every slot of its
/// component, and what `A::default()` returns.
pub fn set_axis_default<A: AxisKey>(value: A) {
    let tid = TypeId::of::<A::Styles>();
    REGISTRY.with(|r| {
        let mut r = r.borrow_mut();
        let entry = r.entry(tid).or_default();
        let v = value.as_variant_str();
        match entry.defaults.iter_mut().find(|(a, _)| *a == A::AXIS) {
            Some(d) => d.1 = v,
            None => entry.defaults.push((A::AXIS, v)),
        }
        entry.composed = None;
    });
}

/// The app-chosen default for `axis` of `S`, if [`set_axis_default`] set
/// one. Generated axis types consult this in their `Default`.
pub fn axis_default<S: ComponentStyles>(axis: &str) -> Option<&'static str> {
    REGISTRY.with(|r| {
        r.borrow()
            .get(&TypeId::of::<S>())
            .and_then(|e| e.defaults.iter().find(|(a, _)| *a == axis).map(|(_, v)| *v))
    })
}

/// Drop every component's composition so the next use recomposes. For a
/// library whose `extend` hooks read a registry of its own (idea-theme's
/// tones): registering into it calls this, so the new value reaches every
/// component that reads it. Warns, like a late restyle, when a recomposed
/// component's sheets differ from what was already in use.
pub fn invalidate_all() {
    REGISTRY.with(|r| {
        for entry in r.borrow_mut().values_mut() {
            entry.composed = None;
        }
    });
}

/// Forget everything registered for `S` — restyles, defaults, the
/// composition and its "already used" mark. Test isolation only.
#[doc(hidden)]
pub fn __reset<S: ComponentStyles>() {
    REGISTRY.with(|r| {
        r.borrow_mut().remove(&TypeId::of::<S>());
    });
}

fn ensure_composed<S: ComponentStyles>(mark_read: bool) -> Rc<Composed> {
    let tid = TypeId::of::<S>();
    let (cached, restyles, defaults) = REGISTRY.with(|r| {
        let mut r = r.borrow_mut();
        let entry = r.entry(tid).or_default();
        if mark_read {
            entry.read = true;
        }
        match &entry.composed {
            Some(c) => (Some(c.clone()), Vec::new(), Vec::new()),
            None => (
                None,
                entry.restyles.iter().map(|(_, f)| f.clone()).collect::<Vec<_>>(),
                entry.defaults.clone(),
            ),
        }
    });
    if let Some(c) = cached {
        return c;
    }
    // No registry borrow is held from here on: extend hooks and restyle
    // closures are author code and may read other components' styles.
    let composed = Rc::new(compose::<S>(&restyles, &defaults));
    REGISTRY.with(|r| {
        let mut r = r.borrow_mut();
        let entry = r.entry(tid).or_default();
        let changed = entry
            .last_identities
            .as_ref()
            .is_some_and(|prev| *prev != composed.identities);
        if entry.read && changed {
            warn_once(format!("late:{}:{}", S::NAME, composed.identities.join("/")), || {
                format!(
                    "[idealyst] {} was restyled after one of its slots was already in use. \
                     Elements built before the restyle keep their old style, and a premint \
                     build has no CSS for the new composition (it falls back to the live \
                     style engine). Register restyles at boot — inside app(), before the \
                     tree is built.",
                    S::NAME
                )
            });
        }
        entry.last_identities = Some(composed.identities.clone());
        entry.composed = Some(composed.clone());
    });
    composed
}

fn compose<S: ComponentStyles>(restyles: &[RestyleFn], defaults: &[(&'static str, &'static str)]) -> Composed {
    let slots = S::slots();
    let mut collector = Collector { slots: vec![Vec::new(); slots.len()] };
    for (i, def) in slots.iter().enumerate() {
        if let Some(extend) = def.extend {
            let mut s = SlotRestyle::<S> { collector: &mut collector, slot: i, _s: PhantomData };
            extend(&mut s);
        }
    }
    for f in restyles {
        f(&mut collector);
    }

    let mut known: FxHashSet<(String, String)> = FxHashSet::default();
    for axis in S::axes() {
        for v in axis.values {
            known.insert((axis.name.to_string(), (*v).to_string()));
        }
    }
    for (a, v) in defaults {
        known.insert(((*a).to_string(), (*v).to_string()));
    }

    let mut sheets = Vec::with_capacity(slots.len());
    let mut identities = Vec::with_capacity(slots.len());
    for (def, items) in slots.iter().zip(collector.slots.iter()) {
        let mut sheet = (def.build)();
        // A component-level axis default reaches every slot that styles
        // the axis without naming a default of its own. Part of the
        // declaration (the macro's content hash covers it), so it does not
        // move the slot off its declared class.
        for axis in S::axes() {
            if let Some(d) = axis.default {
                if sheet.declares_axis(axis.name) && sheet.axis_default(axis.name).is_none() {
                    sheet = sheet.variant_default(axis.name, d);
                }
            }
        }
        for item in items {
            sheet = item.apply(sheet);
        }
        let applied_defaults: Vec<&(&str, &str)> =
            defaults.iter().filter(|(a, _)| sheet.declares_axis(a)).collect();
        for (a, v) in &applied_defaults {
            sheet = sheet.layer_default(a, v);
        }
        for (axis, value) in sheet.variant_keys() {
            known.insert((axis, value));
        }
        for when in sheet.compound_keys() {
            known.extend(when);
        }
        let (rc, identity) = match def.class {
            // The declaration can't premint; neither can a restyle of it.
            None => (Rc::new(sheet), format!("{}.{}#live#{}", S::NAME, def.name, describe(items, &applied_defaults))),
            Some(class) if items.is_empty() && applied_defaults.is_empty() => {
                let rc = sheet.premint_with_class(class);
                #[cfg(feature = "style-dump")]
                crate::premint::register_assembled_sheet(&rc);
                (rc, class.to_string())
            }
            Some(class) => {
                let identity = format!("{class}#{}", describe(items, &applied_defaults));
                (sheet.premint_as(&identity), identity)
            }
        };
        sheets.push(rc);
        identities.push(identity);
    }
    Composed { sheets, identities, known }
}

fn describe(items: &[Item], defaults: &[&(&str, &str)]) -> String {
    let mut parts: Vec<String> = items.iter().map(Item::descriptor).collect();
    for (a, v) in defaults {
        parts.push(format!("default:{a}={v}"));
    }
    parts.join("|")
}

/// The composed sheet for slot `index` of `S` — the declaration plus the
/// library's `extend` hook plus every restyle, finished with its premint
/// identity. Cached until the next restyle of `S` (or [`invalidate_all`]).
pub fn slot_sheet<S: ComponentStyles>(index: usize) -> Rc<StyleSheet> {
    ensure_composed::<S>(true).sheets[index].clone()
}

/// Compose `S` without marking it in use — what the premint dump runs for
/// every linked component after building the app, so a component no
/// crawled screen rendered still ships its (restyled) CSS.
#[doc(hidden)]
pub fn __compose_for_dump<S: ComponentStyles>() {
    let _ = ensure_composed::<S>(false);
}

/// Debug builds: warn once when `value` is selected on `axis` of `S` but no
/// slot styles it and the declaration does not list it — an app-defined
/// value with no restyle behind it, or a tone nothing registered. It would
/// otherwise draw the base silently. Release builds skip the check.
pub fn check_axis_value<S: ComponentStyles>(axis: &str, value: &str) {
    #[cfg(debug_assertions)]
    {
        let composed = ensure_composed::<S>(false);
        let axis_known = composed.known.iter().any(|(a, _)| a == axis);
        if axis_known && !composed.known.contains(&(axis.to_string(), value.to_string())) {
            warn_once(format!("unknown:{}:{axis}={value}", S::NAME), || {
                format!(
                    "[idealyst] {}: `{axis}` value `{value}` has no style — no slot declares an arm \
                     for it and no restyle adds one, so it renders like the base. Give it a look with \
                     `{}::restyle(|s| {{ s.<slot>().arm(<value>, |t| …); }})` at boot.",
                    S::NAME,
                    S::NAME
                )
            });
        }
    }
    #[cfg(not(debug_assertions))]
    {
        let _ = (axis, value);
    }
}

/// The names of every slot of `S`, in declaration order.
pub fn slot_names<S: ComponentStyles>() -> Vec<&'static str> {
    S::slots().iter().map(|s| s.name).collect()
}

// ---------------------------------------------------------------------------
// Per-instance slot overrides
// ---------------------------------------------------------------------------

/// A per-instance override for one slot: rules layered over the slot's
/// resolved style on ONE element. Applied as the application's inline
/// layer ([`StyleApplication::with_inline`](crate::StyleApplication::with_inline)),
/// so it wins over every class layer, keeps the slot premintable, and needs
/// no class of its own.
///
/// Two forms: plain rules, or a sheet resolved against the slot's CURRENT
/// variant selection — so one override can differ per axis value (a sheet
/// with `variant selected { on(_t) { … } }` styles only the selected
/// segment of a segmented control).
#[derive(Clone)]
pub enum SlotOverride {
    /// Fixed rules.
    Rules(Rc<StyleRules>),
    /// A sheet resolved with the slot application's variant set.
    Sheet(Rc<StyleSheet>),
}

impl SlotOverride {
    /// The override's rules for a slot application selecting `variants`.
    pub fn resolve(&self, variants: &VariantSet) -> Rc<StyleRules> {
        match self {
            SlotOverride::Rules(r) => r.clone(),
            SlotOverride::Sheet(s) => Rc::new(s.resolve(variants)),
        }
    }

    /// Whether the override varies with the slot's variant selection.
    pub fn is_sheet(&self) -> bool {
        matches!(self, SlotOverride::Sheet(_))
    }
}

impl std::fmt::Debug for SlotOverride {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            SlotOverride::Rules(r) => f.debug_tuple("Rules").field(r).finish(),
            SlotOverride::Sheet(_) => f.write_str("Sheet(..)"),
        }
    }
}

impl From<StyleRules> for SlotOverride {
    fn from(r: StyleRules) -> Self {
        SlotOverride::Rules(Rc::new(r))
    }
}

impl From<Rc<StyleRules>> for SlotOverride {
    fn from(r: Rc<StyleRules>) -> Self {
        SlotOverride::Rules(r)
    }
}

impl From<Rc<StyleSheet>> for SlotOverride {
    fn from(s: Rc<StyleSheet>) -> Self {
        SlotOverride::Sheet(s)
    }
}

impl From<StyleSheet> for SlotOverride {
    fn from(s: StyleSheet) -> Self {
        SlotOverride::Sheet(Rc::new(s))
    }
}

#[cfg(test)]
mod tests;
