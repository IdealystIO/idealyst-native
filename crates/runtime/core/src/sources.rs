//! `TextSource` + `StyleSource` and their `Into*Source` constructor
//! traits.
//!
//! These are foundational types — `Element` variants store them
//! directly, and the public-facing primitive constructors (`text`,
//! `button`, `with_style`, …) accept anything implementing the
//! associated `Into*Source` trait so author code can pass static
//! values OR closures uniformly.

use crate::style::{StyleApplication, StyleSheet};
use std::rc::Rc;

// =============================================================================
// TextSource + IntoTextSource
// =============================================================================

/// Source for a text node. `Static` ships the string verbatim;
/// `Bound` carries a `Derived<String>` so backends pick how to
/// realize it — runtime backends call `compute` inside an Effect
/// and re-paint the text; generator backends serialize the
/// `method` + `inputs` and the device-side runtime dispatches the
/// transpiled named function on every signal change.
pub enum TextSource {
    Static(String),
    Bound(crate::derive::Derived<String>),
    /// Pre-decomposed reactive text binding suitable for backends
    /// that can run the per-fire fan-out entirely on their own side
    /// (web backend: JS-side reactive layer; future native backends:
    /// possibly platform-specific binding primitives). Carries the
    /// signal ids + template static parts + initial values directly,
    /// so the walker can hand the structured data straight to the
    /// backend without running a Rust closure per fire.
    ///
    /// `compute_fallback` is the same closure the `Bound` variant
    /// would carry — it gets invoked through the legacy Effect path
    /// on backends that DON'T support JS bindings (default-impl
    /// `Backend::supports_js_text_bindings` returns `false`). The
    /// variant author writes the binding once via a text f-string
    /// (`text { "count: {count}" }`) or by constructing
    /// `JsBindingSpec` directly, and the
    /// framework picks the fast or fallback path based on the
    /// active backend's capabilities.
    JsBinding(JsBindingSpec),
    /// Static styled runs — one paragraph mixing per-range style
    /// deltas (inline code chips, emphasis) realized through the
    /// backend's own attributed-text mechanism so mixed-style text
    /// wraps as a single paragraph. See [`crate::styled_text`] for
    /// the model and scope. `Rc` because the walker hands the runs to
    /// the backend at create time AND captures them in a theme-cohort
    /// closure for native re-realization on theme swaps.
    ///
    /// Only `Element::Text` realizes runs richly. Other `TextSource`
    /// consumers (button labels, the batched view path, robot labels)
    /// lower to the concatenated plain text via
    /// [`crate::styled_text::plain_text_of`].
    Styled(std::rc::Rc<Vec<crate::styled_text::TextRun>>),
}

/// Pre-decomposed reactive text binding shipped via
/// [`TextSource::JsBinding`].
///
/// For `"leaf {}: g={}"` with `id=42` (captured) and `global`
/// (a `Signal<u32>`), the bench's structured form is:
///
/// ```ignore
/// JsBindingSpec {
///     signal_ids: vec![global.id()],
///     template_parts: vec![
///         "leaf 42: g=".into(),  // captured `id` baked in
///         "".into(),
///     ],
///     initial_values: vec!["0".into()],
///     compute_fallback: Rc::new(move || format!("leaf {}: g={}", id, global.get())),
///     stringifiers: vec![Rc::new(move || format!("{}", global.get()))],
/// }
/// ```
///
/// `template_parts` has exactly `signal_ids.len() + 1` entries —
/// the static text surrounding each signal interpolation slot,
/// with any captured (non-signal) values pre-formatted into the
/// adjacent parts.
pub struct JsBindingSpec {
    pub signal_ids: Vec<u64>,
    pub template_parts: Vec<String>,
    pub initial_values: Vec<String>,
    /// Same shape as `Derived<String>::compute` — used by the
    /// fallback Effect path on backends that don't support JS
    /// bindings. Built from the same expression that
    /// `compute_fallback` would otherwise wrap.
    pub compute_fallback: std::rc::Rc<dyn Fn() -> String>,
    /// Per-signal stringifiers — one closure per entry of
    /// `signal_ids`, in the same order. Each reads the current
    /// value of the matching signal and formats it the same way
    /// the JS dispatcher will Display-format it. Web backend uses
    /// these to install per-signal JS notifiers
    /// (`runtime_core::register_signal_js_notifier`) at bind time
    /// so subsequent `signal.set/update` calls ship the new value
    /// across the wasm→JS boundary and the JS-side fan-out paints
    /// the text. Backends without a JS bridge ignore them.
    pub stringifiers: Vec<std::rc::Rc<dyn Fn() -> String>>,
}

/// Allows `text(...)` to accept strings, owned strings, or closures.
/// The `#[component]` macro rewrites reactive call sites into closures;
/// this trait makes the rewrite type-check transparently.
pub trait IntoTextSource {
    fn into_text_source(self) -> TextSource;
}

impl IntoTextSource for &str {
    fn into_text_source(self) -> TextSource {
        TextSource::Static(self.to_string())
    }
}

impl IntoTextSource for String {
    fn into_text_source(self) -> TextSource {
        TextSource::Static(self)
    }
}

impl<F> IntoTextSource for F
where
    F: Fn() -> String + 'static,
{
    fn into_text_source(self) -> TextSource {
        // Opaque closure path: coerce into a `Derived<String>`
        // with no structured metadata. Runtime backends use
        // `compute`; generator backends report a build-time error
        // if they receive one (no method name to dispatch).
        TextSource::Bound(crate::derive::IntoDerived::<String>::into_derived(self))
    }
}

/// Identity passthrough so fully-constructed `TextSource` values (an
/// f-string lowering's `__idealyst_text_from_parts` result, a manual
/// `JsBindingSpec`) can be used at the same call sites that accept
/// `&str` / `String` / closures — the blanket `Fn()` impl above doesn't
/// cover them.
impl IntoTextSource for TextSource {
    fn into_text_source(self) -> TextSource {
        self
    }
}

// =============================================================================
// F-string text: typed interpolation slots
// =============================================================================
//
// `ui!`/`jsx!` lower a text literal containing `{name}` placeholders
// (`text { "count: {count}" }`) to a parts list — literal fragments plus
// one [`TextSlot`] per placeholder — assembled by
// [`__idealyst_text_from_parts`]. What makes a slot live is the interpolated
// value's TYPE, resolved through the `StaticTextSlot` / `ReactiveTextSlot`
// method dispatch (same doctrine and same mechanism as `if is_high` via
// `StaticCond`/`ReactiveCond`): a `Display` value bakes in statically, a
// signal becomes a live slot. The assembly picks the best `TextSource`
// the slots allow — all-static → `Static`; signal slots (with ids) →
// `JsBinding` (web fast path, Effect fallback elsewhere); any opaque
// computed slot (e.g. a `Reactive<T>` prop, which has no signal id) →
// `Bound` closure via the Effect path.

/// One interpolation slot of an f-string text literal.
#[doc(hidden)]
pub enum TextSlot {
    /// Value formatted once at build time.
    Static(String),
    /// Signal-backed slot: carries the signal id so JsBinding-capable
    /// backends can fan out without entering Rust per fire.
    Live {
        id: u64,
        initial: String,
        /// UNTRACKED read+format — used as the per-signal stringifier
        /// (JS-notifier path; reactivity flows through the dispatcher,
        /// not a Rust subscription).
        stringify: Rc<dyn Fn() -> String>,
        /// TRACKED read+format — used inside the Effect fallback
        /// closure, where the read must subscribe.
        read: Rc<dyn Fn() -> String>,
    },
    /// Reactive but with no signal id (`Reactive::Dynamic` props):
    /// forces the whole text down the `Bound`/Effect path. TRACKED.
    Computed(Rc<dyn Fn() -> String>),
}

/// A piece of the f-string: a literal fragment or an interpolation slot.
#[doc(hidden)]
pub enum TextSlotPart {
    Lit(&'static str),
    Slot(TextSlot),
}

/// Assemble f-string pieces into the most capable `TextSource` the
/// slots allow. See the module section comment above for the tiering.
#[doc(hidden)]
pub fn __idealyst_text_from_parts(parts: Vec<TextSlotPart>) -> TextSource {
    let any_computed = parts
        .iter()
        .any(|p| matches!(p, TextSlotPart::Slot(TextSlot::Computed(_))));
    let any_live = parts
        .iter()
        .any(|p| matches!(p, TextSlotPart::Slot(TextSlot::Live { .. })));

    if !any_computed && !any_live {
        let mut s = String::new();
        for p in &parts {
            match p {
                TextSlotPart::Lit(l) => s.push_str(l),
                TextSlotPart::Slot(TextSlot::Static(v)) => s.push_str(v),
                _ => unreachable!(),
            }
        }
        return TextSource::Static(s);
    }

    if any_computed {
        // Opaque slot present — Effect path for the whole text. Tracked
        // reads inside the closure subscribe it to every live input.
        let readers: Vec<Rc<dyn Fn() -> String>> = parts
            .into_iter()
            .map(|p| -> Rc<dyn Fn() -> String> {
                match p {
                    TextSlotPart::Lit(l) => Rc::new(move || l.to_string()),
                    TextSlotPart::Slot(TextSlot::Static(v)) => Rc::new(move || v.clone()),
                    TextSlotPart::Slot(TextSlot::Live { read, .. }) => read,
                    TextSlotPart::Slot(TextSlot::Computed(read)) => read,
                }
            })
            .collect();
        return TextSource::Bound(crate::derive::IntoDerived::<String>::into_derived(
            move || {
                let mut s = String::new();
                for r in &readers {
                    s.push_str(&r());
                }
                s
            },
        ));
    }

    // Live slots only: build the pre-decomposed JsBinding — static text
    // folded into template parts around each signal slot (see
    // [`JsBindingSpec`] for the field contract).
    let mut template_parts: Vec<String> = vec![String::new()];
    let mut signal_ids: Vec<u64> = Vec::new();
    let mut initial_values: Vec<String> = Vec::new();
    let mut stringifiers: Vec<Rc<dyn Fn() -> String>> = Vec::new();
    let mut readers: Vec<Rc<dyn Fn() -> String>> = Vec::new();
    for p in parts {
        match p {
            TextSlotPart::Lit(l) => template_parts.last_mut().unwrap().push_str(l),
            TextSlotPart::Slot(TextSlot::Static(v)) => {
                template_parts.last_mut().unwrap().push_str(&v)
            }
            TextSlotPart::Slot(TextSlot::Live { id, initial, stringify, read }) => {
                signal_ids.push(id);
                initial_values.push(initial);
                stringifiers.push(stringify);
                readers.push(read);
                template_parts.push(String::new());
            }
            TextSlotPart::Slot(TextSlot::Computed(_)) => unreachable!(),
        }
    }
    let fallback_parts = template_parts.clone();
    TextSource::JsBinding(JsBindingSpec {
        signal_ids,
        template_parts,
        initial_values,
        compute_fallback: Rc::new(move || {
            let mut s = String::new();
            for (i, part) in fallback_parts.iter().enumerate() {
                s.push_str(part);
                if let Some(r) = readers.get(i) {
                    s.push_str(&r());
                }
            }
            s
        }),
        stringifiers,
    })
}

/// Static half of the f-string slot dispatch: any `Display` value bakes
/// in at build time. Method-resolution dispatch (both traits in scope,
/// the receiver's type picks the impl) — the same pattern as
/// `StaticCond`/`ReactiveCond` for `if`. Signal types don't implement
/// `Display`, so the blanket never overlaps [`ReactiveTextSlot`].
#[doc(hidden)]
pub trait StaticTextSlot {
    fn __idealyst_text_slot(&self, f: impl Fn(&dyn std::fmt::Display) -> String) -> TextSlot;
}

impl<T: std::fmt::Display> StaticTextSlot for T {
    fn __idealyst_text_slot(&self, f: impl Fn(&dyn std::fmt::Display) -> String) -> TextSlot {
        TextSlot::Static(f(self))
    }
}

/// Reactive half of the f-string slot dispatch — signal-typed values
/// become live slots. `&self` receiver (signal handles are `Copy`;
/// `Reactive` clones) so interpolating never consumes the author's
/// binding.
#[doc(hidden)]
pub trait ReactiveTextSlot {
    fn __idealyst_text_slot(
        &self,
        f: impl Fn(&dyn std::fmt::Display) -> String + 'static,
    ) -> TextSlot;
}

impl<T: std::fmt::Display + Clone + 'static> ReactiveTextSlot for crate::Signal<T> {
    fn __idealyst_text_slot(
        &self,
        f: impl Fn(&dyn std::fmt::Display) -> String + 'static,
    ) -> TextSlot {
        let sig = *self;
        let f = Rc::new(f);
        let fs = f.clone();
        TextSlot::Live {
            id: sig.id(),
            initial: crate::untrack(|| f(&sig.get())),
            stringify: Rc::new(move || crate::untrack(|| fs(&sig.get()))),
            read: Rc::new(move || f(&sig.get())),
        }
    }
}

impl<T: std::fmt::Display + Clone + 'static> ReactiveTextSlot for crate::ReadSignal<T> {
    fn __idealyst_text_slot(
        &self,
        f: impl Fn(&dyn std::fmt::Display) -> String + 'static,
    ) -> TextSlot {
        let sig = *self;
        let f = Rc::new(f);
        let fs = f.clone();
        TextSlot::Live {
            id: sig.id(),
            initial: crate::untrack(|| f(&sig.get())),
            stringify: Rc::new(move || crate::untrack(|| fs(&sig.get()))),
            read: Rc::new(move || f(&sig.get())),
        }
    }
}

/// `Reactive<T>` props interpolate too: a `Static` prop bakes in, a
/// `Dynamic` one (no signal id available) becomes a `Computed` slot,
/// which routes the whole text through the Effect path.
impl<T: std::fmt::Display + Clone + 'static> ReactiveTextSlot for crate::Reactive<T> {
    fn __idealyst_text_slot(
        &self,
        f: impl Fn(&dyn std::fmt::Display) -> String + 'static,
    ) -> TextSlot {
        match self {
            crate::Reactive::Static(v) => TextSlot::Static(f(v)),
            dynamic => {
                let r = dynamic.clone();
                TextSlot::Computed(Rc::new(move || f(&r.get())))
            }
        }
    }
}

// =============================================================================
// StyleSource + IntoStyleSource
// =============================================================================

/// A style source. `Static` is a fixed `StyleApplication` known at
/// build time — no signal subscriptions, no per-node `Effect`. The
/// node is registered with the global theme cohort so a `set_theme`
/// call re-applies it in bulk. `Reactive` is a closure that re-runs
/// inside an `Effect`; signals it reads become deps and changes
/// re-fire the apply path. Most styles are `Static`; the `Reactive`
/// path exists for cases that need per-node reactive overrides.
///
/// The split matters at scale: 10 000 styled rows used to allocate
/// 10 000 `Effect`s + 10 000 `Box<dyn Fn>` closures + 10 000 entries
/// in the active-theme signal's subscriber set. With `Static`, those
/// per-node allocations vanish — the cohort holds a single entry per
/// node and a single `Effect` subscribes to the theme on behalf of
/// the whole set.
pub enum StyleSource {
    Static(StyleApplication),
    Reactive(Box<dyn Fn() -> StyleApplication>),
    /// Pre-resolved signal→class binding. The walker resolves each
    /// declared `(value, app)` to a minted class name at mount, then
    /// hands the (signal_id, values, classes) table to the backend.
    /// Backends that support JS-side bindings (web today) install a
    /// pure-JS dispatcher that fans signal writes out to subscribed
    /// nodes WITHOUT firing a Rust Effect per node — for SHARED
    /// reactive-style cohorts this collapses 50k Effect fires per
    /// fan-out to one signal-changed callback.
    ///
    /// `compute_fallback` mirrors `StyleSource::Reactive`'s closure
    /// shape so backends that don't support JS bindings (native
    /// mobile, in-process renderers) get the same behavior — just
    /// with the per-node Effect path they'd take anyway.
    SignalClass(SignalClassSpec),
}

/// Spec for a `StyleSource::SignalClass` binding. Built via the
/// [`crate::signal_class`] helper.
///
/// `signal_id` is the arena id of a `Signal<u32>`. The walker
/// resolves each `(values[i], apps[i])` pair to a minted class name
/// and ships the resulting table to the backend. On a signal write
/// the backend updates the node's class to `classes[values.index_of(new_value)]`.
pub struct SignalClassSpec {
    pub signal_id: u64,
    pub values: Vec<u32>,
    pub apps: Vec<StyleApplication>,
    /// Fallback closure for backends without JS-binding support.
    /// Built once at construction from the same mapping fn the
    /// caller supplied — runs inside a normal `Effect` and produces
    /// a `StyleApplication` for the signal's current value.
    pub compute_fallback: std::rc::Rc<dyn Fn() -> StyleApplication>,
    /// Reads the signal's current value as `u32`. Used by the
    /// JS-binding backend path to (a) seed the initial class at
    /// mount and (b) provide a value source for the
    /// signal-changed notifier that ships writes across the FFI
    /// boundary. The closure does NOT subscribe — it's expected
    /// to call the signal's untracked accessor.
    pub read_signal: std::rc::Rc<dyn Fn() -> u32>,
}

/// Allows `with_style(...)` to accept any of:
///   - a bare `Rc<StyleSheet>` — applies the stylesheet with no
///     variant selection, no overrides. Best for static one-shot
///     styles like `banner_style()`.
///   - a fixed `StyleApplication` — for the case where you already
///     have a built-up application with variants/overrides.
///   - a closure returning a `StyleApplication` — enables reactive
///     styling: signals read inside the closure become dependencies
///     and changes re-fire the apply-style effect.
///
/// The `Rc<StyleSheet>` impl exists so authors don't have to write
/// `StyleApplication::new(sheet)` for the trivial case — most styles
/// are like that, and the wrapping was pure ceremony.
pub trait IntoStyleSource {
    fn into_style_source(self) -> StyleSource;
}

impl IntoStyleSource for Rc<StyleSheet> {
    fn into_style_source(self) -> StyleSource {
        StyleSource::Static(StyleApplication::new(self))
    }
}

impl IntoStyleSource for StyleApplication {
    fn into_style_source(self) -> StyleSource {
        StyleSource::Static(self)
    }
}

impl<F> IntoStyleSource for F
where
    F: Fn() -> StyleApplication + 'static,
{
    fn into_style_source(self) -> StyleSource {
        StyleSource::Reactive(Box::new(self))
    }
}

impl IntoStyleSource for SignalClassSpec {
    fn into_style_source(self) -> StyleSource {
        StyleSource::SignalClass(self)
    }
}

/// Build a `StyleSource::SignalClass` from a `Signal<u32>`, a list
/// of discrete values it will take, and a mapping from each value
/// to a `StyleApplication`. The mapping closure runs ONCE per
/// value at construction time — the resulting `StyleApplication`s
/// are pre-resolved to minted class names at mount, and signal
/// writes update the node's class via a pure-JS dispatcher.
///
/// ## Example
///
/// ```ignore
/// let active: Signal<u32> = signal(0u32);
///
/// View(style = signal_class(active, &[0, 1], |v| match v {
///     0 => MyRow().tone(Tone::Neutral),
///     1 => MyRow().tone(Tone::Highlighted),
///     _ => unreachable!(),
/// }))
/// ```
///
/// On `active.set(1)`, every row whose style binds to `active`
/// switches to the highlighted class without firing a per-row
/// Rust `Effect`.
///
/// ## When to reach for this vs a reactive closure
///
/// `signal_class` is the right tool when:
/// - The class is a function of exactly one signal (more general
///   bindings exist as a future extension).
/// - The signal takes a small, enumerable set of discrete values.
/// - Many nodes (50+) share the same signal — the JS fan-out wins
///   over Rust Effect dispatch dominate at this scale.
///
/// For arbitrary reactive logic (arithmetic, multi-signal
/// dependencies, dynamically-computed colors), pass a closure
/// directly to `style = ...` instead.
pub fn signal_class<F, V>(
    signal: crate::Signal<V>,
    values: &[u32],
    mapping: F,
) -> SignalClassSpec
where
    F: Fn(u32) -> StyleApplication + 'static,
    V: Copy + 'static,
    u32: From<V>,
{
    let mapping = std::rc::Rc::new(mapping);
    let apps: Vec<StyleApplication> = values.iter().map(|&v| mapping(v)).collect();
    let signal_id = signal.id();
    // Untracked read — the binding's reactivity is wired through
    // the JS-side dispatcher (or `compute_fallback`'s Effect); we
    // don't want the signal_class call site to subscribe.
    let read_signal: std::rc::Rc<dyn Fn() -> u32> = std::rc::Rc::new(move || {
        crate::untrack(|| u32::from(signal.get()))
    });
    let compute_fallback: std::rc::Rc<dyn Fn() -> StyleApplication> = {
        let mapping = mapping.clone();
        std::rc::Rc::new(move || mapping(u32::from(signal.get())))
    };
    SignalClassSpec {
        signal_id,
        values: values.to_vec(),
        apps,
        compute_fallback,
        read_signal,
    }
}
