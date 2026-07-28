//! Shared style-attachment service for the built-in handlers — the
//! vocabulary-level port of `walker/style.rs` (P2b resolved-rules paths
//! + the P3c sheet engine).
//!
//! # The six [`StyleProp`] paths
//!
//! - **`Static`** — fully resolved [`StyleRules`], applied once at mount
//!   (`StyleOps::apply_style`), `on_node_unstyled` at teardown. The P2
//!   raw-rules path (no sheet, no theme participation).
//! - **`Dynamic`** — a resolved-rules closure re-applied by a binding
//!   effect; per-node `StateBits` signal + `attach_states` re-fire
//!   contract. The P2 raw-rules reactive path.
//! - **`Sheet`** — a static [`StyleApplication`] (the `stylesheet!`
//!   fast path): registered + resolved through the runtime-core sheet
//!   engine, applied once, and enrolled in the per-world **theme
//!   cohort** so a token swap re-applies it without a per-node effect
//!   (`attach_style_static`, walker/style.rs).
//! - **`SheetDynamic`** — a `StyleApplication` closure with a per-node
//!   binding effect: theme-version subscription, sheet pinning,
//!   interaction-state merge, breakpoint/container folds
//!   (`attach_style_reactive`).
//! - **`SignalClass`** — `signal_class!`-style discrete class selection.
//!   Ported on the **fallback** (per-node effect) path on every backend
//!   — see [`signal_class`] for why the old JS fan-out cannot fire for
//!   world signals yet (bench-gate note).
//! - **`Preminted`** — a build-time-minted class stamped via
//!   `attach_html_class`; no `StyleRules` work, state overlays ship as
//!   pseudo-class CSS in the same asset (walker `Preminted` arm).
//!
//! # Ported invariants (each names its old-core source)
//!
//! - **Static-divert** (`attach_style_static`, the state-machine-divert
//!   memory): on an event-driven backend, a STATIC sheet application
//!   whose sheet declares `state …` overlays is diverted to the dynamic
//!   path — the cohort has no state signal, and a no-op setter would
//!   silently lose hover/press/focus on native while web kept them in
//!   CSS.
//! - **Sheet pinning** (`attach_style_reactive`'s `_pinned_sheet`): the
//!   dynamic effect holds the latest `Rc<StyleSheet>` so the
//!   registration table's `Weak` stays upgradeable for the node's
//!   lifetime; without it a per-call inline sheet would be
//!   dead-swept while the node still wears its class.
//! - **Teardown ordering** (`StyleHandle::drop`): cohort unregister
//!   FIRST, `on_node_unstyled` second.
//! - **Registration fast path** (`is_registered`): steady-state
//!   re-fires skip `ensure_registered_with`'s sweep/flush prologue.
//! - **Default-font fill** (`with_default_text_font`): a resolved rule
//!   with no `font_family` inherits the per-world theme font at apply
//!   time on every backend (native has no CSS inheritance).
//!
//! # Deferred (loud, not silent)
//!
//! - **Native container-query feedback**: the old walker's build-time
//!   `CONTAINER_STACK` + inline-size signal is not ported (the new
//!   scene has no container signal yet, P4 with the viewport port).
//!   `mark_container` is emitted and web resolves `@container` in CSS;
//!   on event-driven backends container overlays resolve at width 0
//!   (inert) — same as an old-core node with no container ancestor.
//! - **Native breakpoint re-fire**: overlays merge against
//!   `current_breakpoint()`'s VALUE, but a bucket flip does not re-fire
//!   world effects (old-core signal). Web is complete (`@media` CSS).
//! - **JS class-binding fan-out**: see [`signal_class`].
//! - **State-overlay CSS order**: state overlays resolve in AXIS-NAME
//!   order (`variant_keys()` is a BTreeMap walk) rather than the old
//!   engine's declaration order — visible only to a `handles_states_natively`
//!   backend when two simultaneously-active states set the same
//!   property. Canonical order (focused < hovered < pressed) matches
//!   the common intent; revisit if a golden catches it.

use std::borrow::Cow;
use std::cell::RefCell;
use std::rc::Rc;

use runtime_core::{
    resolve_style, Breakpoint, StateBits, StyleApplication, StyleRules, StyleSheet,
};
use runtime_world::{effect, Signal};

use crate::caps::{AppEnvOps, AssetOps, DocumentOps, StyleOps};
use crate::theme;

/// The capability bundle the style service needs. `StyleOps` for the
/// apply/token surface, `DocumentOps` for preminted class stamping,
/// `AssetOps` for typeface registration riding sheet registration,
/// `AppEnvOps` for the queued host-surface settings the token flush
/// delivers (`set_app_background`, …) — the same call set the old
/// `apply_one` wired through `ensure_registered_with`'s closures.
pub trait StyleServices: StyleOps + DocumentOps + AssetOps + AppEnvOps {}
impl<T: StyleOps + DocumentOps + AssetOps + AppEnvOps> StyleServices for T {}

/// A primitive's style prop. See the module docs for the per-path
/// backend-call contract.
pub enum StyleProp {
    /// Resolved rules applied once at mount.
    Static(Rc<StyleRules>),
    /// A resolved-rules closure; a binding effect re-applies on every
    /// dependency change.
    Dynamic(Box<dyn Fn() -> Rc<StyleRules>>),
    /// A static sheet application (`stylesheet!`'s all-constant
    /// builder): sheet-engine resolution + theme-cohort enrollment.
    Sheet(StyleApplication),
    /// A sheet-application closure (`stylesheet!` builder with a
    /// reactive input): per-node binding effect through the sheet
    /// engine.
    SheetDynamic(Box<dyn Fn() -> StyleApplication>),
    /// Discrete signal→class selection (see [`signal_class`]).
    SignalClass(SignalClassProp),
    /// A build-time-minted class (+ optional runtime slot overrides).
    Preminted {
        class: Cow<'static, str>,
        overrides: Option<Rc<StyleRules>>,
    },
}

/// Spec for a [`StyleProp::SignalClass`] binding — the new-core
/// counterpart of `runtime_core::SignalClassSpec`, built over a
/// `runtime_world` signal by [`signal_class`].
pub struct SignalClassProp {
    /// The discrete values the signal takes (kept for the future JS
    /// fast-path port; the fallback path doesn't consult them).
    pub values: Vec<u32>,
    /// Pre-built application per value — kept alive for the binding's
    /// lifetime so their sheets aren't dead-Weak-swept (the old spec's
    /// `_kept_apps` contract).
    pub apps: Vec<StyleApplication>,
    /// Tracked read producing the application for the CURRENT value;
    /// runs inside the binding effect.
    pub compute: Rc<dyn Fn() -> StyleApplication>,
}

/// Build a [`StyleProp::SignalClass`] from a world `Signal`, its
/// discrete values, and a value→application mapping (the mapping runs
/// once per value at construction, same as the old
/// `runtime_core::signal_class`).
///
/// ## Divergence, documented (bench-gate risk)
///
/// The old core's web fast path installed a **pure-JS dispatcher**
/// (`register_reactive_class_binding` + a notifier fired from the OLD
/// arena's `Signal::set`). World signals never fire that notifier —
/// there is no JS-side write hook in the new kernel yet — so this path
/// always uses the *fallback* shape the old core used on non-JS
/// backends: one per-node binding effect that re-resolves and
/// re-applies on each signal write. Observable behavior (the class
/// swap) is identical; the shared-cohort fan-out optimization returns
/// with the P3 bench work (a world-signal write-notifier channel).
pub fn signal_class<F, V>(signal: Signal<V>, values: &[u32], mapping: F) -> StyleProp
where
    F: Fn(u32) -> StyleApplication + 'static,
    V: Copy + PartialEq + 'static,
    u32: From<V>,
{
    let mapping = Rc::new(mapping);
    let apps: Vec<StyleApplication> = values.iter().map(|&v| mapping(v)).collect();
    let compute: Rc<dyn Fn() -> StyleApplication> = {
        let mapping = mapping.clone();
        Rc::new(move || mapping(u32::from(signal.get())))
    };
    StyleProp::SignalClass(SignalClassProp {
        values: values.to_vec(),
        apps,
        compute,
    })
}

/// Conversions into [`StyleProp`] — what builders' `.style(...)` and the
/// glue's `.with_style(...)` accept: resolved rules (static), a sheet
/// application / bare sheet (sheet engine), a `StyleProp` verbatim
/// (`stylesheet!` builders convert through their generated
/// `IntoStyleProp` impl), or a closure returning any of the value forms
/// (dynamic).
pub trait IntoStyleProp {
    fn into_style_prop(self) -> StyleProp;
}

impl IntoStyleProp for StyleProp {
    fn into_style_prop(self) -> StyleProp {
        self
    }
}

impl IntoStyleProp for StyleRules {
    fn into_style_prop(self) -> StyleProp {
        StyleProp::Static(Rc::new(self))
    }
}

impl IntoStyleProp for Rc<StyleRules> {
    fn into_style_prop(self) -> StyleProp {
        StyleProp::Static(self)
    }
}

impl IntoStyleProp for StyleApplication {
    fn into_style_prop(self) -> StyleProp {
        StyleProp::Sheet(self)
    }
}

/// A bare sheet applies with no variant selection (the old
/// `IntoStyleSource for Rc<StyleSheet>` convenience).
impl IntoStyleProp for Rc<StyleSheet> {
    fn into_style_prop(self) -> StyleProp {
        StyleProp::Sheet(StyleApplication::new(self))
    }
}

/// What a dynamic style closure may return — sealed to the concrete
/// style value shapes so the blanket closure impl stays unambiguous.
/// Each output picks the closure's [`StyleProp`] arm.
pub trait DynStyleResult: sealed::Sealed {
    fn lift(f: Box<dyn Fn() -> Self>) -> StyleProp
    where
        Self: Sized;
}

mod sealed {
    pub trait Sealed {}
    impl Sealed for runtime_core::StyleRules {}
    impl Sealed for std::rc::Rc<runtime_core::StyleRules> {}
    impl Sealed for runtime_core::StyleApplication {}
}

impl DynStyleResult for StyleRules {
    fn lift(f: Box<dyn Fn() -> Self>) -> StyleProp {
        StyleProp::Dynamic(Box::new(move || Rc::new(f())))
    }
}

impl DynStyleResult for Rc<StyleRules> {
    fn lift(f: Box<dyn Fn() -> Self>) -> StyleProp {
        StyleProp::Dynamic(f)
    }
}

impl DynStyleResult for StyleApplication {
    fn lift(f: Box<dyn Fn() -> Self>) -> StyleProp {
        StyleProp::SheetDynamic(f)
    }
}

impl<F, R> IntoStyleProp for F
where
    F: Fn() -> R + 'static,
    R: DynStyleResult + 'static,
{
    fn into_style_prop(self) -> StyleProp {
        R::lift(Box::new(move || self()))
    }
}

/// Register `f` to run when the enclosing realized subtree tears down
/// (its [`Owned`](runtime_world::Owned) drops) — the new-core analogue of
/// the old walker's scope-level `on_cleanup`.
///
/// Mechanism: a probe effect whose body reads no signals (so it never
/// re-fires) and returns `f` as its cleanup; the kernel runs effect
/// cleanups when the collected effect is freed, i.e. exactly at subtree
/// teardown, in effect-creation order. Handlers may call this at mount
/// time (inside a `MountCx` handler there is no running effect, so the
/// kernel's `on_cleanup` free function is unusable directly — it needs a
/// running effect; the probe provides one).
pub fn on_teardown(f: impl FnOnce() + 'static) {
    let mut slot = Some(f);
    let _ = effect(move || {
        let f = slot.take();
        move || {
            if let Some(f) = f {
                f();
            }
        }
    });
}

/// Attach `style` to an already-created `node`. Port of the old walker's
/// `attach_style` dispatch. Returns the node's interaction-state setter
/// (`Rc<dyn Fn(StateBits, bool)>`) — a real signal-flip on the
/// event-driven dynamic paths, a no-op elsewhere — so callers (the
/// `disabled` bindings) can flip prop-driven bits like
/// `StateBits::DISABLED`, exactly as the old `attach_disabled` did.
///
/// Must run inside a `MountCx` handler (or any `collect_owned` scope with
/// the owning world ambient): the binding effect, the state signal, and
/// the teardown probe all register into the ambient collector and die
/// with the subtree.
pub fn attach_style<H: StyleServices>(
    backend: &Rc<RefCell<H>>,
    node: &H::Node,
    style: StyleProp,
) -> Rc<dyn Fn(StateBits, bool)> {
    match style {
        StyleProp::Static(rules) => {
            backend.borrow_mut().apply_style(node, &rules);
            // Teardown notification — the backend frees per-node style
            // state (the old path's `StyleHandle` drop).
            let b = backend.clone();
            let n = node.clone();
            on_teardown(move || {
                b.borrow_mut().on_node_unstyled(&n);
            });
            noop_setter()
        }
        StyleProp::Dynamic(f) => attach_rules_dynamic(backend, node, f),
        StyleProp::Sheet(app) => attach_sheet_static(backend, node, app),
        StyleProp::SheetDynamic(f) => attach_sheet_dynamic(backend, node, f),
        StyleProp::SignalClass(spec) => {
            // Fallback shape (see `signal_class`): the whole spec moves
            // into the closure so `apps` stays alive, pinning the
            // per-value sheets against the dead-Weak sweep — the old
            // `_kept_apps` guard, expressed as capture lifetime.
            attach_sheet_dynamic(
                backend,
                node,
                Box::new(move || {
                    let _pin = &spec.apps;
                    (spec.compute)()
                }),
            )
        }
        StyleProp::Preminted { class, overrides } => {
            debug_assert!(
                backend.borrow().supports_preminted_styles(),
                "StyleProp::Preminted reached a backend with no preminted \
                 support — the stylesheet! macro must keep the full rules \
                 closure on native targets"
            );
            // Preminted rules bypass sheet registration entirely, so the
            // host-state flush (theme tokens → CSS vars, app background,
            // default document font) rides the per-world theme driver
            // instead (old `install_premint_host_driver`).
            theme::mark_premint_used();
            theme::ensure_theme_driver(backend);
            theme::flush_pending_host_state(backend);
            {
                let b = backend.borrow();
                // One stamp per whitespace-separated segment (the delta
                // model: `iy-<hash>` + one `iy-<hash>-<axis>-<value>` per
                // selected axis; `classList.add` rejects spaces).
                for cls in class.split_whitespace() {
                    b.attach_html_class(node, cls);
                }
            }
            if let Some(rules) = overrides {
                // Runtime slot overrides layer a normal static sheet
                // application on top of the preminted class — same
                // shared-empty-sheet trick as the old walker (ONE cached
                // sheet for every override site, not one per node).
                fn empty_sheet() -> Rc<StyleSheet> {
                    static KEY: u8 = 0;
                    runtime_core::cached_stylesheet(&KEY as *const u8 as usize, || {
                        Rc::new(StyleSheet::r#static(StyleRules::default()))
                    })
                }
                return attach_sheet_static(
                    backend,
                    node,
                    StyleApplication::new(empty_sheet()).with_overrides((*rules).clone()),
                );
            }
            noop_setter()
        }
    }
}

fn noop_setter() -> Rc<dyn Fn(StateBits, bool)> {
    Rc::new(|_, _| {})
}

// ===========================================================================
// P2 dynamic resolved-rules path (unchanged behavior, now returns setter)
// ===========================================================================

fn attach_rules_dynamic<H: StyleServices>(
    backend: &Rc<RefCell<H>>,
    node: &H::Node,
    f: Box<dyn Fn() -> Rc<StyleRules>>,
) -> Rc<dyn Fn(StateBits, bool)> {
    // Per-node interaction-state signal. The binding effect subscribes,
    // so a native hover/press/focus flip (via the `attach_states`
    // setter) re-fires and re-applies — the event-driven contract.
    // Raw resolved rules carry no sheet, hence no state overlays to
    // merge; the re-fire is the whole contract here.
    let states = runtime_world::signal(StateBits::NONE);
    let b = backend.clone();
    let n = node.clone();
    let _binding = effect(move || {
        let _ = states.get(); // subscribe: state flips re-apply
        let rules = f();
        b.borrow_mut().apply_style(&n, &rules);
    });
    // Teardown probe AFTER the binding effect, mirroring the old path
    // where `on_node_unstyled` fires from the style effect's own
    // teardown (the captured StyleHandle's drop).
    let b = backend.clone();
    let n = node.clone();
    on_teardown(move || {
        b.borrow_mut().on_node_unstyled(&n);
    });
    let setter: Rc<dyn Fn(StateBits, bool)> = Rc::new(move |bit, on| {
        states.update(move |bits| if on { bits.with(bit) } else { bits.without(bit) });
    });
    backend.borrow_mut().attach_states(node, setter.clone());
    setter
}

// ===========================================================================
// Sheet paths (P3c)
// ===========================================================================

/// Static sheet application — port of `attach_style_static`.
fn attach_sheet_static<H: StyleServices>(
    backend: &Rc<RefCell<H>>,
    node: &H::Node,
    app: StyleApplication,
) -> Rc<dyn Fn(StateBits, bool)> {
    let handles_states_natively = backend.borrow().handles_states_natively();

    // Static-divert (module docs): an event-driven backend drives
    // hover/press/focus through the state machine, which lives on the
    // dynamic path. A static-styled node whose sheet declares `state`
    // overlays must get it — the cohort alone would silently lose
    // hover on native while web keeps it in CSS.
    if !handles_states_natively && !sheet_state_axes(&app.sheet).is_empty() {
        return attach_sheet_dynamic(backend, node, Box::new(move || app.clone()));
    }

    // Make sure the per-world theme driver is alive before registering.
    theme::ensure_theme_driver(backend);

    // Inline first apply (identical work to the dynamic effect's first
    // run, minus the effect). No container ancestor tracking on the new
    // core yet, so container overlays resolve at width 0 (module docs).
    apply_sheet(backend, node, &app, handles_states_natively);

    // Enroll in the per-world cohort: ONE shared driver re-applies on
    // theme change instead of N per-node effects (`theme_cohort`
    // rationale). The application is shared with the reapply closure
    // via Rc — a `StyleApplication` transitively owns ~1 KB.
    let backend_for_cohort = backend.clone();
    let node_for_cohort = node.clone();
    let app = Rc::new(app);
    let app_for_cohort = app.clone();
    let cohort_id = theme::cohort_register(Rc::new(move || {
        apply_sheet(
            &backend_for_cohort,
            &node_for_cohort,
            &app_for_cohort,
            handles_states_natively,
        );
    }));

    // Teardown: cohort unregister FIRST, then on_node_unstyled — the
    // old `StyleHandle::drop` order. The `app` Rc rides in the closure
    // so the sheet stays pinned (registration Weak upgradeable) for the
    // node's lifetime. The ThemeCtx is captured NOW (ambient world
    // available) because the teardown itself runs from a plain drop,
    // outside `World::enter` (see `ThemeCtx::cohort_unregister`).
    let ctx = theme::theme_ctx();
    let b = backend.clone();
    let n = node.clone();
    on_teardown(move || {
        let _pin = &app;
        ctx.cohort_unregister(cohort_id);
        b.borrow_mut().on_node_unstyled(&n);
    });

    // No state signal on the cohort path (the divert above handles
    // state-bearing sheets on event-driven backends; natively-handling
    // backends put states in CSS).
    noop_setter()
}

/// Dynamic sheet application — port of `attach_style_reactive`.
fn attach_sheet_dynamic<H: StyleServices>(
    backend: &Rc<RefCell<H>>,
    node: &H::Node,
    style: Box<dyn Fn() -> StyleApplication>,
) -> Rc<dyn Fn(StateBits, bool)> {
    // The driver is what DELIVERS `update_tokens` to the backend on a
    // theme swap. The old core installed it only from the static/
    // signal-class paths — an all-dynamic tree relied on a static node
    // existing somewhere (latent gap: without one, backends never saw
    // swaps and web `var()` values froze). The new path installs it
    // here too; with nothing pending and an empty cohort its runs are
    // backend-invisible, so the parity goldens are unaffected.
    theme::ensure_theme_driver(backend);
    let handles_states_natively = backend.borrow().handles_states_natively();

    // Per-node active interaction states — event-driven backends only
    // (web pre-emits pseudo-class CSS; skipping the slot is the same
    // saving the old path took).
    let states: Option<Signal<StateBits>> = if handles_states_natively {
        None
    } else {
        Some(runtime_world::signal(StateBits::NONE))
    };

    let backend_for_effect = backend.clone();
    let node_for_effect = node.clone();
    // Sheet pin (module docs): holds the latest Rc<StyleSheet> so its
    // registration Weak stays upgradeable for this effect's lifetime.
    let mut pinned_sheet: Option<Rc<StyleSheet>> = None;
    let _binding = effect(move || {
        // Theme-version subscription — the per-token reads of the old
        // engine go deaf behind the resolution cache; the version
        // signal never does (old `tokens_version_signal` rationale).
        let _ = theme::version_signal().get();

        let app = style();

        // Registration fast path: steady-state re-fires skip the
        // sweep/flush prologue (`is_registered` contract).
        if !theme::sheet_is_registered(&app.sheet) {
            theme::ensure_sheet_registered(&backend_for_effect, &app.sheet);
        }
        pinned_sheet = Some(app.sheet.clone());

        if handles_states_natively {
            // Web: resolve base + every overlay axis; the browser does
            // the state/breakpoint/container switching in CSS. NOT
            // subscribed to the states signal — CSS owns transitions.
            let base = with_default_text_font(resolve_style(&app));
            let state_overlays = resolve_state_overlays(&app);
            let bp_overlays = resolve_breakpoint_overlays(&app);
            let cq_overlays = resolve_container_overlays(&app);
            backend_for_effect.borrow_mut().apply_styled_variants(
                &node_for_effect,
                &base,
                &state_overlays,
                &bp_overlays,
                &cq_overlays,
            );
        } else {
            // Event-driven: merge active state axes into the variant
            // set, then fold breakpoint/container overlays, then apply.
            let bits = states.expect("states signal exists when !handles_states_natively").get();
            let mut app = app;
            for axis in bits.active_axes() {
                app = app.with(axis, "on");
            }
            let base = resolve_style(&app);
            let bp_overlays = resolve_breakpoint_overlays(&app);
            let resolved = merge_active_breakpoints(base, &bp_overlays);
            // Container width 0: no container-signal source on the new
            // core yet (module docs) — overlays stay inert, matching an
            // old-core node with no container ancestor.
            let cq_overlays = resolve_container_overlays(&app);
            let resolved = merge_active_containers(resolved, &cq_overlays, 0.0);
            let resolved = with_default_text_font(resolved);
            backend_for_effect
                .borrow_mut()
                .apply_style(&node_for_effect, &resolved);
        }
    });

    // Teardown probe after the binding effect (old ordering).
    let b = backend.clone();
    let n = node.clone();
    on_teardown(move || {
        b.borrow_mut().on_node_unstyled(&n);
    });

    let setter: Rc<dyn Fn(StateBits, bool)> = match states {
        Some(sig) => Rc::new(move |bit, on| {
            sig.update(move |bits| if on { bits.with(bit) } else { bits.without(bit) });
        }),
        None => noop_setter(),
    };
    backend.borrow_mut().attach_states(node, setter.clone());
    setter
}

/// Resolve + apply one sheet application — port of `apply_one`
/// (walker/style.rs), shared by the mount-time apply and the cohort
/// reapply closure.
fn apply_sheet<H: StyleServices>(
    backend: &Rc<RefCell<H>>,
    node: &H::Node,
    app: &StyleApplication,
    handles_states_natively: bool,
) {
    theme::ensure_sheet_registered(backend, &app.sheet);
    if handles_states_natively {
        let base = with_default_text_font(resolve_style(app));
        let state_overlays = resolve_state_overlays(app);
        let bp_overlays = resolve_breakpoint_overlays(app);
        let cq_overlays = resolve_container_overlays(app);
        backend.borrow_mut().apply_styled_variants(
            node,
            &base,
            &state_overlays,
            &bp_overlays,
            &cq_overlays,
        );
    } else {
        let base = resolve_style(app);
        let bp_overlays = resolve_breakpoint_overlays(app);
        let resolved = merge_active_breakpoints(base, &bp_overlays);
        let cq_overlays = resolve_container_overlays(app);
        let resolved = merge_active_containers(resolved, &cq_overlays, 0.0);
        let resolved = with_default_text_font(resolved);
        backend.borrow_mut().apply_style(node, &resolved);
    }
}

// ===========================================================================
// Overlay resolution — public-API reimplementation of the walker's
// crate-internal `resolve_state_overlays` / breakpoint / container
// helpers (the sheet's cached axis lists are pub(crate) to runtime-core,
// so the vocabulary scans `variant_keys()` by the reserved prefixes).
// ===========================================================================

/// Map a variant axis name to its `StateBits` flag (the `stylesheet!`
/// macro's `__state_<name>` namespace; mirrors runtime-core's private
/// `state_axis_bit`).
fn state_axis_bit(axis: &str) -> Option<StateBits> {
    match axis {
        "__state_hovered" => Some(StateBits::HOVERED),
        "__state_pressed" => Some(StateBits::PRESSED),
        "__state_focused" => Some(StateBits::FOCUSED),
        "__state_disabled" => Some(StateBits::DISABLED),
        _ => None,
    }
}

/// The sheet's declared state-overlay axes (axis-name order — see the
/// module docs' order note).
fn sheet_state_axes(sheet: &Rc<StyleSheet>) -> Vec<(StateBits, String)> {
    sheet
        .variant_keys()
        .into_iter()
        .filter_map(|(axis, _value)| state_axis_bit(&axis).map(|bit| (bit, axis)))
        .collect()
}

/// Resolve each declared state overlay against the application's
/// variants + theme: `(bits, fully resolved rules)` pairs a
/// natively-handling backend emits as pseudo-class CSS.
fn resolve_state_overlays(app: &StyleApplication) -> Vec<(StateBits, Rc<StyleRules>)> {
    let axes = sheet_state_axes(&app.sheet);
    if axes.is_empty() {
        return Vec::new();
    }
    axes.into_iter()
        .map(|(bit, axis)| {
            let state_app = app.clone().with(axis, "on");
            (bit, resolve_style(&state_app))
        })
        .collect()
}

/// Breakpoint analog — `(bucket, fully resolved rules)` sorted
/// ascending by rank (the mobile-first min-width cascade both consumers
/// stack in order).
fn resolve_breakpoint_overlays(app: &StyleApplication) -> Vec<(Breakpoint, Rc<StyleRules>)> {
    let mut out: Vec<(Breakpoint, Rc<StyleRules>)> = app
        .sheet
        .variant_keys()
        .into_iter()
        .filter_map(|(axis, _value)| {
            Breakpoint::from_axis_name(&axis).map(|bp| {
                let bp_app = app.clone().with(axis, "on");
                (bp, resolve_style(&bp_app))
            })
        })
        .collect();
    out.sort_by_key(|(bp, _)| bp.rank());
    out
}

/// Container-query analog — `(min-width threshold px, fully resolved
/// rules)` sorted ascending by threshold.
fn resolve_container_overlays(app: &StyleApplication) -> Vec<(f32, Rc<StyleRules>)> {
    let mut out: Vec<(f32, Rc<StyleRules>)> = app
        .sheet
        .variant_keys()
        .into_iter()
        .filter_map(|(axis, _value)| {
            runtime_core::container_axis_threshold(&axis).map(|threshold| {
                let cq_app = app.clone().with(axis, "on");
                (threshold, resolve_style(&cq_app))
            })
        })
        .collect();
    out.sort_by(|(a, _), (b, _)| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
    out
}

/// Fold the overlays whose bucket is active at the current viewport
/// width onto `base`, lowest first so higher breakpoints win (walker
/// `merge_active_breakpoints`). Reads the old-core breakpoint signal's
/// VALUE only — see the module docs' native-re-fire deferral.
fn merge_active_breakpoints(
    base: Rc<StyleRules>,
    overlays: &[(Breakpoint, Rc<StyleRules>)],
) -> Rc<StyleRules> {
    if overlays.is_empty() {
        return base;
    }
    let current = runtime_core::current_breakpoint().get();
    let mut merged: Option<StyleRules> = None;
    for (bp, overlay) in overlays {
        if bp.rank() <= current.rank() {
            let acc = merged.take().unwrap_or_else(|| (*base).clone());
            merged = Some(acc.merge(overlay));
        }
    }
    match merged {
        Some(rules) => Rc::new(rules),
        None => base,
    }
}

/// Fold the overlays whose threshold is `<=` the container width onto
/// `base` (walker `merge_active_containers`). Width is 0 on the new
/// core until the container-signal port (module docs) — no overlay
/// activates, matching a node with no container ancestor.
fn merge_active_containers(
    base: Rc<StyleRules>,
    overlays: &[(f32, Rc<StyleRules>)],
    container_width: f32,
) -> Rc<StyleRules> {
    if overlays.is_empty() {
        return base;
    }
    let mut merged: Option<StyleRules> = None;
    for (threshold, overlay) in overlays {
        if *threshold <= container_width {
            let acc = merged.take().unwrap_or_else(|| (*base).clone());
            merged = Some(acc.merge(overlay));
        }
    }
    match merged {
        Some(rules) => Rc::new(rules),
        None => base,
    }
}

/// Fill an absent `font_family` with the PER-WORLD theme default (old
/// `with_default_text_font`; clones only when it actually fills).
fn with_default_text_font(rules: Rc<StyleRules>) -> Rc<StyleRules> {
    if rules.font_family.is_none() {
        if let Some(font) = theme::default_text_font() {
            let mut owned = (*rules).clone();
            owned.font_family = Some(font);
            return Rc::new(owned);
        }
    }
    rules
}
