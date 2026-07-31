//! Shared style-attachment service for the built-in handlers — the
//! vocabulary-level port of `walker/style.rs` (P2b resolved-rules paths
//! + the P3c sheet engine).
//!
//! # The seven [`StyleProp`] paths
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
//!   JS fast path on `supports_js_class_bindings` backends (per-value
//!   minted classes + one per-signal notifier effect — see
//!   [`signal_class`]); per-node-effect fallback elsewhere and for
//!   post-construction-wrapped specs.
//! - **`Preminted`** — a build-time-minted class stamped via
//!   `attach_html_class`; no `StyleRules` work, state overlays ship as
//!   pseudo-class CSS in the same asset (walker `Preminted` arm).
//! - **`PremintedDynamic`** — the reactive peer of `Preminted`: a
//!   closure returning the class LIST, re-stamped by a per-node effect
//!   when its axis signals change. Still no engine — every arm's CSS is
//!   already in the shipped asset, so a discrete axis flip is a class
//!   swap.
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
//! - **Default-font fill** (`fill_default_text_font`): a STATIC sheet
//!   application whose resolved rules set no `font_family` inherits the
//!   per-world theme font at apply time (`apply_sheet`, the old
//!   `apply_one`). The DYNAMIC path deliberately does NOT fill — the
//!   old `attach_style_reactive` never did on either branch (reactive
//!   nodes ride the `apply_default_text_font` document channel), and
//!   filling there minted class hashes old-core SSR never mints,
//!   breaking SSG byte-parity.
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
//! - **JS TEXT-binding fan-out** (`register_reactive_text_binding`):
//!   still deferred with the macro's structured f-string lowering; the
//!   CLASS-binding fan-out is ported (see [`signal_class`]).
//! - **State-overlay CSS order**: resolved from the sheet's CACHED
//!   axis slice in DECLARATION order — the old engine's order (this
//!   was briefly axis-name order via a `variant_keys()` scan; the scan
//!   also allocated the full key list per node per fire and was
//!   replaced by the cached slice for the bench gate).

use std::borrow::Cow;
use std::cell::RefCell;
use std::rc::Rc;

use runtime_shared::{
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
    ///
    /// Boxed on purpose: `StyleApplication` inlines its `overrides:
    /// StyleRules` (~2.3 KB), and an unboxed variant makes EVERY prim
    /// payload ~2.5 KB — each builder-chain move then memcpys the whole
    /// struct, which was the create-rows bench residual (+23% vs the
    /// old core, ~0.33 µs/row of pure payload copying at 10k rows).
    /// One box per styled node keeps the moving parts pointer-sized.
    Sheet(Box<StyleApplication>),
    /// A sheet-application closure (`stylesheet!` builder with a
    /// reactive input): per-node binding effect through the sheet
    /// engine.
    SheetDynamic(Box<dyn Fn() -> StyleApplication>),
    /// Discrete signal→class selection (see [`signal_class`]). Boxed
    /// for the same size reason as `Sheet` (the spec carries Vecs +
    /// four Rcs — the largest remaining variant otherwise).
    SignalClass(Box<SignalClassProp>),
    /// A build-time-minted class (+ optional runtime slot overrides).
    Preminted {
        class: Cow<'static, str>,
        overrides: Option<Rc<StyleRules>>,
        /// Per-instance rules applied as an INLINE style alongside the
        /// class list — see `StyleApplication::with_inline`. Unlike
        /// `overrides` this does NOT drag the engine back in: it is
        /// applied directly to the node, and it wins over the classes in
        /// the cascade the same way the merge order says it does.
        inline: Option<Rc<StyleRules>>,
    },
    /// A build-time-minted class list that CHANGES — the reactive
    /// counterpart of [`StyleProp::Preminted`].
    ///
    /// The closure returns the whole space-separated class list and
    /// subscribes to whatever it reads, so a per-node effect re-stamps on
    /// change. Emitted by the `stylesheet!` builder when a variant axis
    /// got a reactive source (`Signal<E>` / `derived`) but nothing else
    /// forces the live engine.
    ///
    /// This is the shape that makes a selection UI premintable. Every arm
    /// of a discrete axis already has CSS in the shipped asset — the dump
    /// emits `-active-on` AND `-active-off` — so switching between them is
    /// a class swap, not a rule mint. Before this existed those styles
    /// fell through to `SheetDynamic` and dragged the whole style engine
    /// in: 46 of 68 fall-throughs on the component catalog were one
    /// nav-item sheet whose only reactivity was `active`.
    ///
    /// Unlike [`StyleProp::SignalClass`] this needs no single driving
    /// signal id, so it covers `derived(...)` closures reading several
    /// signals — which is what author code actually writes.
    ///
    /// `overrides` mirrors [`StyleProp::Preminted`]'s slot and exists for
    /// the same reason: a navigator screen-style fold layers runtime
    /// rules onto whatever the screen's style is, and no class list can
    /// carry those. The `stylesheet!` builder never emits it — its
    /// premint branch bails to the live path the moment any override is
    /// set — so in generated code this is always `None`.
    PremintedDynamic {
        class_of: Box<dyn Fn() -> String>,
        overrides: Option<Rc<StyleRules>>,
    },
}

/// Spec for a [`StyleProp::SignalClass`] binding — the new-core
/// counterpart of `runtime_shared::SignalClassSpec`, built over a
/// `runtime_world` signal by [`signal_class`].
pub struct SignalClassProp {
    /// The discrete values the signal takes (the JS fast path ships
    /// them to the backend's binding registry; the fallback path
    /// doesn't consult them).
    pub values: Vec<u32>,
    /// Pre-built application per value (parallel to `values`) — kept
    /// alive for the binding's lifetime so their sheets aren't
    /// dead-Weak-swept (the old spec's `_kept_apps` contract), and
    /// minted into per-value classes on the JS fast path.
    pub apps: Vec<StyleApplication>,
    /// Tracked read producing the application for the CURRENT value;
    /// runs inside the binding effect (fallback path).
    pub compute: Rc<dyn Fn() -> StyleApplication>,
    /// The Rc `compute` was CONSTRUCTED as. The JS fast path requires
    /// `Rc::ptr_eq(compute, pristine_compute)`: a wrapper layered onto
    /// `compute` afterwards (the navigator's `fold_style_overrides`
    /// wraps it with screen style overrides) invalidates the
    /// pre-built `apps` table, and minting stale apps would silently
    /// drop the overrides — such specs take the (correct, per-node
    /// effect) fallback instead.
    pub pristine_compute: Rc<dyn Fn() -> StyleApplication>,
    /// The driving signal's [`runtime_world::Signal::raw_id`] — keys
    /// the backend's JS binding registry and the per-world notifier
    /// dedup table.
    pub signal_id: u64,
    /// Untracked-safe read of the signal's current `u32` value (the
    /// backend's `value_reader`; also the notifier effect's tracked
    /// read).
    pub read_value: Rc<dyn Fn() -> u32>,
}

/// Build a [`StyleProp::SignalClass`] from a world `Signal`, its
/// discrete values, and a value→application mapping (the mapping runs
/// once per value at construction, same as the old
/// `runtime_shared::signal_class`).
///
/// On a backend with `supports_js_class_bindings` (web), the attach
/// takes the ported **JS fast path**: per-value classes are minted at
/// mount, ONE per-signal notifier effect ships commits across the FFI,
/// and the JS dispatcher swaps classes on every subscribed node —
/// zero per-node Rust work per fire (the old walker's
/// `attach_style_signal_class`, rebuilt on a world-effect notifier
/// because world signals have no `Signal::set` JS write hook). Other
/// backends — and specs whose `compute` was wrapped after
/// construction (see [`SignalClassProp::pristine_compute`]) — use the
/// per-node binding-effect fallback, the shape the old core used on
/// non-JS backends.
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
    StyleProp::SignalClass(Box::new(SignalClassProp {
        values: values.to_vec(),
        apps,
        compute: compute.clone(),
        pristine_compute: compute,
        signal_id: signal.raw_id(),
        read_value: Rc::new(move || u32::from(signal.get())),
    }))
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
        // Preminted fast path for a RUNTIME-ASSEMBLED sheet (idea-theme's
        // component sheets — see `StyleSheet::premint_as`). The macro
        // plays the same trick on its generated builder, but these sheets
        // have no expansion site, so without this every idea-ui component
        // built on one fell through to `Sheet` and kept the live engine
        // linked.
        //
        // Both disqualifiers are runtime-valued layers the dump could not
        // have seen: `overrides` are per-call-site rules, and `computed`
        // is an arbitrary closure. Either one means this application's
        // resolved rules are not the ones any build-time class names.
        #[cfg(idealyst_premint)]
        {
            if let Some(class) = self.preminted_class_list() {
                return StyleProp::Preminted {
                    class: Cow::Owned(class),
                    overrides: None,
                    inline: self.inline().cloned(),
                };
            }
        }
        StyleProp::Sheet(Box::new(self))
    }
}

/// A bare sheet applies with no variant selection (the old
/// `IntoStyleSource for Rc<StyleSheet>` convenience).
///
/// Routed through the `StyleApplication` impl rather than constructing
/// `Sheet` directly so a premintable sheet premints here too — the raw
/// sheet is one of the fall-through shapes the `--premint-only` panic
/// names, and it does not have to be.
impl IntoStyleProp for Rc<StyleSheet> {
    fn into_style_prop(self) -> StyleProp {
        StyleApplication::new(self).into_style_prop()
    }
}

/// The boxed form verbatim — lets a caller that already has the box
/// (the repeat handler's style-attachment carry, a navigator override
/// fold) avoid an unbox/rebox round-trip.
impl IntoStyleProp for Box<StyleApplication> {
    fn into_style_prop(self) -> StyleProp {
        // Premint check first (same conditions as the unboxed impl); the
        // box is kept, not round-tripped, on the live-engine fall-through.
        #[cfg(idealyst_premint)]
        {
            if let Some(class) = self.preminted_class_list() {
                return StyleProp::Preminted {
                    class: Cow::Owned(class),
                    overrides: None,
                    inline: self.inline().cloned(),
                };
            }
        }
        StyleProp::Sheet(self)
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
    impl Sealed for runtime_shared::StyleRules {}
    impl Sealed for std::rc::Rc<runtime_shared::StyleRules> {}
    impl Sealed for runtime_shared::StyleApplication {}
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

/// ONE cached empty sheet shared by every preminted-with-overrides site
/// (the old walker's trick — not one sheet per node). Only reachable from
/// the override branches, which `--premint-only` compiles out.
#[cfg(not(idealyst_premint_only))]
fn preminted_override_sheet() -> Rc<StyleSheet> {
    static KEY: u8 = 0;
    runtime_shared::cached_stylesheet(&KEY as *const u8 as usize, || {
        Rc::new(StyleSheet::r#static(StyleRules::default()))
    })
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
/// What a `--premint-only` build says when it meets a style it cannot
/// render.
///
/// Deliberately verbose: the flag is a promise the author made at build time
/// ("every style in this app is preminted"), and the failure surfaces far
/// from the call that broke it, so the message carries the whole diagnosis
/// and both exits.
#[cfg(idealyst_premint_only)]
const PREMINT_ONLY_VIOLATION: &str = "\
this bundle was built with --premint-only, which compiles OUT the runtime \
style engine, but a style reached `attach_style` that needs it.\n\n\
--premint mints a class at BUILD time only for an all-constant, \
override-free stylesheet BUILDER application — `style = Card()`. Passing the \
raw sheet instead (`style = card_style()`), a reactive input, a runtime slot \
override, `signal_class`, or a raw `StyleRules` closure all fall through to \
the live engine, which this build does not contain.\n\n\
Either move the offending style onto the builder form so its whole variant \
space premints, or drop --premint-only and keep the engine. --premint on its \
own is always safe.";


// ---------------------------------------------------------------------------
// --premint-report
// ---------------------------------------------------------------------------

/// Build-time diagnostic: report every style that reaches the LIVE ENGINE.
///
/// `--premint-only` pays off only when an app's every style premints, and
/// one straggler forfeits the whole saving with a boot panic that names
/// the variant but not the source. Finding the stragglers used to mean
/// hand-patching this file to log instead of panic — done four separate
/// times while building this feature, twice from stale results. This is
/// that patch, made a real flag.
///
/// It rides `--premint` rather than `--premint-only`, so the engine is
/// still present and **the app renders normally** — you get a complete
/// list from one working page load instead of a panic at the first
/// offender.
///
/// Each distinct fall-through logs ONCE (keyed on its own report line), so
/// 46 nav items produce one entry rather than 46. The line carries what
/// you need to find it in source: which `StyleProp` shape, whether the
/// sheet has build-time CSS at all, the runtime-valued layers that
/// disqualified it, and a content fingerprint of the resolved rules.
#[cfg(idealyst_premint_report)]
pub(crate) mod report {
    use super::{resolve_style, StyleApplication, StyleProp};
    use std::cell::RefCell;

    thread_local! {
        static SEEN: RefCell<std::collections::BTreeSet<String>> =
            RefCell::new(std::collections::BTreeSet::new());
    }

    fn describe_app(kind: &str, app: &StyleApplication) -> String {
        let axes = app
            .variants
            .0
            .iter()
            .map(|(a, v)| format!("{a}={v}"))
            .collect::<Vec<_>>()
            .join(",");
        // WHERE the sheet was constructed. This is the field that makes the
        // report actionable: everything else identifies a sheet only to
        // someone who already knows the codebase, and locating the three
        // framework-owned fall-throughs on the catalog took a
        // property-by-property decode of the rules dump below.
        let origin = app
            .sheet
            .origin()
            .map(|l| format!("{}:{}:{}", l.file(), l.line(), l.column()))
            .unwrap_or_else(|| "origin-unknown".to_string());
        // The resolved-rules content key disambiguates two sheets built at
        // one site (a per-size cache, a `match` over variants).
        let fingerprint = resolve_style(app).content_key();
        let fingerprint = fingerprint.chars().take(72).collect::<String>();
        format!(
            "{kind} at {origin} css={} overrides={} computed={} axes=[{}] rules={}",
            match app.sheet.premint_class() {
                Some(c) => c,
                None => "NONE-no-build-time-css",
            },
            app.has_overrides(),
            app.computed().map(|c| c.key.as_str()).unwrap_or("-"),
            axes,
            fingerprint,
        )
    }

    /// Called for every `attach_style`, before the arm runs.
    pub(crate) fn note(style: &StyleProp) {
        let line = match style {
            // These two ARE preminted — the whole point. Nothing to report.
            StyleProp::Preminted { .. } | StyleProp::PremintedDynamic { .. } => return,
            StyleProp::Static(rules) => format!(
                "Static (raw StyleRules — no stylesheet, so nothing to premint) rules={}",
                rules.content_key().chars().take(72).collect::<String>(),
            ),
            StyleProp::Dynamic(_) => {
                "Dynamic (raw StyleRules closure — no stylesheet to premint)".to_string()
            }
            StyleProp::Sheet(app) => describe_app("Sheet", app),
            StyleProp::SheetDynamic(f) => describe_app("SheetDynamic", &f()),
            StyleProp::SignalClass(spec) => {
                describe_app("SignalClass", &(spec.pristine_compute)())
            }
        };
        let fresh = SEEN.with(|s| s.borrow_mut().insert(line.clone()));
        if !fresh {
            return;
        }
        let n = SEEN.with(|s| s.borrow().len());
        runtime_shared::log_warn!("[premint-report] #{n} {line}");
    }
}

pub fn attach_style<H: StyleServices>(
    backend: &Rc<RefCell<H>>,
    node: &H::Node,
    style: StyleProp,
) -> Rc<dyn Fn(StateBits, bool)> {
    // Diagnostic only (`--premint-report`); compiled out otherwise. Sits
    // ahead of the match so it sees every shape without touching an arm.
    #[cfg(idealyst_premint_report)]
    report::note(&style);
    match style {
        StyleProp::Static(rules) => {
            // Old-core parity: every STATIC style there is a
            // `StyleSource::Static` application riding `apply_one`,
            // which fills the theme's default text font into rules that
            // set none (the reactive path doesn't — module docs). The
            // `is_none` guard skips the world-context lookup for rules
            // that carry their own font. Without this fill, e.g. the
            // empty-`if`-branch absolute placeholder minted a
            // font-less class old-core SSR never mints (SSG
            // byte-parity).
            let rules = if rules.font_family.is_none() {
                fill_default_text_font(rules, theme::theme_ctx().default_text_font())
            } else {
                rules
            };
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
        // ---- live-engine arms ----
        //
        // Under `--cfg idealyst_premint_only` these are the only paths that
        // reach the runtime style engine (sheet registration, the token
        // cohort, `StyleRules` → CSS), so compiling them out is what drops
        // it — ~76 KB raw / ~23 KB brotli on a web build.
        //
        // `--premint` alone cannot: the `stylesheet!` macro's preminted fast
        // path FALLS THROUGH to `Sheet`/`SheetDynamic` for any reactive or
        // override-carrying application, so the engine stays named and
        // therefore linked even in an app whose every class preminted. That
        // fallthrough is why this has to be a separate, explicit promise
        // rather than something `--premint` implies.
        #[cfg(not(idealyst_premint_only))]
        StyleProp::Dynamic(f) => attach_rules_dynamic(backend, node, f),
        #[cfg(not(idealyst_premint_only))]
        StyleProp::Sheet(app) => attach_sheet_static(backend, node, *app),
        #[cfg(not(idealyst_premint_only))]
        StyleProp::SheetDynamic(f) => attach_sheet_dynamic(backend, node, f),
        // The promise was wrong: this build has no engine to fall back to.
        // Panic loudly rather than render the node unstyled — a silently
        // unstyled subtree is far harder to diagnose than a stack trace, and
        // it matches the loud-failure policy an unregistered payload gets at
        // realize.
        #[cfg(idealyst_premint_only)]
        StyleProp::Dynamic(_) | StyleProp::Sheet(_) => {
            panic!("{}", PREMINT_ONLY_VIOLATION)
        }
        // NOT a blanket panic like its two neighbours: a reactive
        // application whose sheet premints needs no engine, and this arm
        // is where every one of idea-theme's runtime-assembled component
        // sheets lands (the blanket `Fn() -> StyleApplication` impl has no
        // expansion site to premint at). The panic moves inside, to the
        // evaluations that genuinely can't premint.
        #[cfg(idealyst_premint_only)]
        StyleProp::SheetDynamic(f) => attach_sheet_dynamic_preminted(backend, node, f),
        #[cfg(not(idealyst_premint_only))]
        StyleProp::SignalClass(spec) => {
            let spec = *spec;
            // JS fast path (web): pre-minted per-value classes + JS-side
            // fan-out — only when the spec is PRISTINE (a wrapped
            // `compute` means the apps table no longer reflects the
            // rendered style; minting it would drop the wrapper's
            // overrides — navigator screen-style overlays).
            if backend.borrow().supports_js_class_bindings()
                && Rc::ptr_eq(&spec.compute, &spec.pristine_compute)
            {
                return attach_signal_class_js(backend, node, spec);
            }
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
        #[cfg(idealyst_premint_only)]
        StyleProp::SignalClass(_) => panic!("{}", PREMINT_ONLY_VIOLATION),
        StyleProp::PremintedDynamic { class_of, overrides } => {
            debug_assert!(
                backend.borrow().supports_preminted_styles(),
                "StyleProp::PremintedDynamic reached a backend with no \
                 preminted support"
            );
            attach_preminted_dynamic(backend, node, class_of);
            // Overrides reach the engine, same as on the static arm.
            #[cfg(idealyst_premint_only)]
            if overrides.is_some() {
                panic!("{}", PREMINT_ONLY_VIOLATION);
            }
            #[cfg(not(idealyst_premint_only))]
            if let Some(rules) = overrides {
                return attach_sheet_static(
                    backend,
                    node,
                    StyleApplication::new(preminted_override_sheet())
                        .with_overrides((*rules).clone()),
                );
            }
            // No state setter: interaction states ride the preminted
            // pseudo-class CSS, exactly as on the static preminted path.
            noop_setter()
        }
        StyleProp::Preminted { class, overrides, inline } => {
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
            // The inline layer rides ON TOP of the stamped classes, applied
            // straight to the node. It carries only per-instance values the
            // build-time CSS could not name (see
            // `StyleApplication::with_inline`), so it needs no sheet, no
            // registration, and — the point — no engine.
            if let Some(rules) = &inline {
                backend.borrow_mut().apply_inline_style(node, rules);
            }
            // A preminted class carrying RUNTIME slot overrides layers a
            // real sheet application on top, which reaches the engine. The
            // `stylesheet!` macro never emits this shape — its preminted
            // fast path bails to the live path whenever any override is
            // set, so `overrides` is always `None` in generated code (only
            // hand-built `StyleProp`s in tests construct `Some`). Compiling
            // it out is therefore not a behavior change for real apps, and
            // it is what finally unanchors `attach_sheet_static` →
            // `ensure_sheet_registered`.
            #[cfg(idealyst_premint_only)]
            if overrides.is_some() {
                panic!("{}", PREMINT_ONLY_VIOLATION);
            }
            #[cfg(not(idealyst_premint_only))]
            if let Some(rules) = overrides {
                // Runtime slot overrides layer a normal static sheet
                // application on top of the preminted class — same
                // shared-empty-sheet trick as the old walker (ONE cached
                // sheet for every override site, not one per node).
                return attach_sheet_static(
                    backend,
                    node,
                    StyleApplication::new(preminted_override_sheet())
                        .with_overrides((*rules).clone()),
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
// SignalClass JS fast path (port of walker/style.rs
// `attach_style_signal_class`, world-notifier edition)
// ===========================================================================

/// Per-world dedup table for signal→JS notifier effects (world context,
/// like `ThemeCtx`). One entry per signal id; entries live for the
/// world's lifetime — the notifier effect is world-root-owned
/// (`unscoped`), because it serves EVERY node ever bound to the signal,
/// not the first node's subtree (a collected effect would die with that
/// subtree while later-bound nodes lived on — the same regression class
/// the theme driver documents).
///
/// SHARED between the class-binding path and the text-binding path: the
/// old core allowed at most ONE JS notifier per signal (the first
/// registrant's stringifier wins; both dispatchers tap the same
/// `__idealystOnSignalChanged` value cache), and two notifier effects
/// for one signal would double-ship every commit.
#[derive(Clone, Default)]
struct SignalNotifiers(Rc<RefCell<std::collections::HashSet<u64>>>);

/// First-registrant-wins install seam for the per-signal JS notifier
/// effect: if `signal_id` has no notifier in this world yet, run
/// `install` (which must create the world-root effect); otherwise do
/// nothing. The effect's FIRST run happens synchronously at creation —
/// i.e. BEFORE the caller registers its binding — which seeds the
/// JS-side signal-value cache so the binding's registration-time
/// initial paint resolves (the old core relied on a prior
/// `register_signal_for_js` write for that seed).
pub(crate) fn ensure_signal_notifier_installed(signal_id: u64, install: impl FnOnce()) {
    let registry = match runtime_world::inject::<SignalNotifiers>() {
        Some(r) => r,
        None => {
            let r = SignalNotifiers::default();
            runtime_world::provide(r.clone());
            r
        }
    };
    if !registry.0.borrow_mut().insert(signal_id) {
        return;
    }
    install();
}

#[cfg(not(idealyst_premint_only))]
/// Ensure ONE world-root effect exists for `signal_id` that ships the
/// signal's committed value to the backend's JS CLASS dispatcher
/// (`StyleOps::notify_signal_value_js`).
fn ensure_signal_notifier<H: StyleServices>(
    backend: &Rc<RefCell<H>>,
    signal_id: u64,
    read_value: Rc<dyn Fn() -> u32>,
) {
    ensure_signal_notifier_installed(signal_id, || {
        let b = backend.clone();
        runtime_world::unscoped(|| {
            let _notifier = effect(move || {
                // Tracked read: re-fires on every committed change of the
                // signal; one FFI hop fans out to every JS-side subscriber.
                let value = read_value();
                b.borrow_mut().notify_signal_value_js(signal_id, value);
            });
        });
    });
}

#[cfg(not(idealyst_premint_only))]
/// The JS fast path: mint one class per declared value, register the
/// (signal → value → class) table with the backend's JS dispatcher,
/// ensure the per-signal notifier, release on teardown. Zero per-node
/// Rust work per signal fire.
fn attach_signal_class_js<H: StyleServices>(
    backend: &Rc<RefCell<H>>,
    node: &H::Node,
    spec: SignalClassProp,
) -> Rc<dyn Fn(StateBits, bool)> {
    theme::ensure_theme_driver(backend);

    // Mint per-value classes. Registration first — mint_class_for_app
    // resolves against registered sheets (same ordering as the old
    // walker's ensure_registered_with → mint sequence).
    let mut class_names: Vec<String> = Vec::with_capacity(spec.apps.len());
    for app in &spec.apps {
        theme::ensure_sheet_registered(backend, &app.sheet);
        let class = backend.borrow_mut().mint_class_for_app(app).expect(
            "mint_class_for_app returned None for a SignalClass app — backends that \
             support JS class bindings must mint fresh classes for dynamic override content",
        );
        class_names.push(class);
    }

    // Notifier BEFORE binding registration (seeds the JS value cache —
    // see ensure_signal_notifier).
    ensure_signal_notifier(backend, spec.signal_id, spec.read_value.clone());

    let class_refs: Vec<&str> = class_names.iter().map(|s| s.as_str()).collect();
    let binding_id = backend.borrow_mut().register_reactive_class_binding(
        node,
        spec.signal_id,
        &spec.values,
        &class_refs,
        spec.read_value.clone(),
    );

    // Teardown: release the JS-side binding; the moved-in `apps` keep
    // the per-value sheets registration-pinned for the node's lifetime
    // (the old guard's `_kept_apps`).
    let b = backend.clone();
    let apps = spec.apps;
    on_teardown(move || {
        let _pin = &apps;
        b.borrow_mut().release_reactive_class_binding(binding_id);
    });

    // Same no-op state setter the static path returns — state overlays
    // aren't part of the SignalClass abstraction (old walker parity).
    noop_setter()
}

// ===========================================================================
// P2 dynamic resolved-rules path (unchanged behavior, now returns setter)
// ===========================================================================

#[cfg(not(idealyst_premint_only))]
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

#[cfg(not(idealyst_premint_only))]
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

    // Capture the ctx ONCE per node: the apply below, the cohort
    // reapply closure, and the teardown all use it — the per-call
    // `inject` world-context lookup is measurable at bulk-create scale
    // (10k rows × registration check per apply, the bench-gate work).
    let ctx = theme::theme_ctx();

    // Inline first apply (identical work to the dynamic effect's first
    // run, minus the effect). No container ancestor tracking on the new
    // core yet, so container overlays resolve at width 0 (module docs).
    apply_sheet(backend, node, &app, handles_states_natively, &ctx);

    // Enroll in the per-world cohort: ONE shared driver re-applies on
    // theme change instead of N per-node effects (`theme_cohort`
    // rationale). The application is shared with the reapply closure
    // via Rc — a `StyleApplication` transitively owns ~1 KB.
    let backend_for_cohort = backend.clone();
    let node_for_cohort = node.clone();
    let app = Rc::new(app);
    let app_for_cohort = app.clone();
    let ctx_for_cohort = ctx.clone();
    let cohort_id = theme::cohort_register(Rc::new(move || {
        apply_sheet(
            &backend_for_cohort,
            &node_for_cohort,
            &app_for_cohort,
            handles_states_natively,
            &ctx_for_cohort,
        );
    }));

    // Teardown: cohort unregister FIRST, then on_node_unstyled — the
    // old `StyleHandle::drop` order. The `app` Rc rides in the closure
    // so the sheet stays pinned (registration Weak upgradeable) for the
    // node's lifetime. The ThemeCtx was captured above (ambient world
    // available) because the teardown itself runs from a plain drop,
    // outside `World::enter` (see `ThemeCtx::cohort_unregister`).
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

/// Stamp a reactive preminted class list onto `node`, swapping the
/// outgoing classes for the incoming ones on every re-evaluation.
///
/// Shared by [`StyleProp::PremintedDynamic`] (the macro's reactive
/// preminted form) and, under `--premint-only`, by the `SheetDynamic`
/// arm — a reactive application over a runtime-assembled sheet
/// (idea-theme's component sheets) lands there, and it needs the exact
/// same treatment.
fn attach_preminted_dynamic<H: StyleServices>(
    backend: &Rc<RefCell<H>>,
    node: &H::Node,
    class_of: Box<dyn Fn() -> String>,
) {
    // Same host-state wiring as the static preminted arm: a preminted
    // class bypasses sheet registration, so tokens / app background /
    // default font ride the theme driver.
    theme::mark_premint_used();
    theme::ensure_theme_driver(backend);
    theme::flush_pending_host_state(backend);

    let b = backend.clone();
    let n = node.clone();
    // Classes stamped by the previous run, so the next one can take them
    // off. Held OUTSIDE the backend borrow — swapping needs both the list
    // and the backend, and nesting a RefCell borrow inside a backend
    // borrow is how this file's other paths have deadlocked before.
    let stamped: RefCell<Vec<String>> = RefCell::new(Vec::new());
    let _binding = effect(move || {
        // Read FIRST: this is the tracked call, and it must run before any
        // borrow so a panic inside author code can't leave a borrow
        // outstanding.
        let class = class_of();

        let previous: Vec<String> = stamped.borrow_mut().drain(..).collect();
        let next: Vec<String> = class.split_whitespace().map(|c| c.to_string()).collect();
        if previous != next {
            let bb = b.borrow();
            for old in &previous {
                if !next.iter().any(|c| c == old) {
                    bb.detach_html_class(&n, old);
                }
            }
            for c in &next {
                if !previous.iter().any(|p| p == c) {
                    bb.attach_html_class(&n, c);
                }
            }
        }
        *stamped.borrow_mut() = next;
    });
}

/// `SheetDynamic` under `--premint-only`: a reactive application whose
/// sheet DOES premint, re-derived per evaluation.
///
/// The blanket "SheetDynamic → panic" this replaces was too coarse. The
/// arm is where every reactive application over a runtime-assembled
/// sheet lands — idea-theme's component sheets reach it through the
/// blanket `Fn() -> StyleApplication` impl, which has no expansion site
/// to premint at and so cannot decide statically. But a *reactive*
/// selection over a preminted sheet is exactly the delta model's home
/// case: each evaluation picks variant arms, and each arm is already its
/// own CSS rule. Stamping the class list per evaluation needs no engine.
///
/// Deciding per evaluation rather than once is the load-bearing part: a
/// closure may legally return a premintable application on one run and an
/// override-carrying one on the next, so a probe at attach time would be
/// unsound. Non-premintable evaluations panic with the same violation
/// message the static arm uses — loud, and only when actually reached.
#[cfg(idealyst_premint_only)]
fn attach_sheet_dynamic_preminted<H: StyleServices>(
    backend: &Rc<RefCell<H>>,
    node: &H::Node,
    style: Box<dyn Fn() -> StyleApplication>,
) -> Rc<dyn Fn(StateBits, bool)> {
    attach_preminted_dynamic(
        backend,
        node,
        Box::new(move || {
            style().preminted_class_list().unwrap_or_else(|| panic!("{}", PREMINT_ONLY_VIOLATION))
        }),
    );
    // No state setter: interaction states ride the preminted pseudo-class
    // CSS, exactly as on the static preminted path.
    noop_setter()
}

#[cfg(not(idealyst_premint_only))]
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

    // Captured ONCE per node; the binding effect below re-fires per
    // dependency change and must not pay a world-context `inject` per
    // version read / registration check / font fill (3 lookups per
    // node per fire — measured on the js-framework-bench shared-signal
    // style fan-out).
    let ctx = theme::theme_ctx();

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
    let mut _pinned_sheet: Option<Rc<StyleSheet>> = None;
    let _binding = effect(move || {
        #[cfg(feature = "debug-stats")]
        let _t_effect = runtime_shared::debug::now_micros();
        // Theme-version subscription — the per-token reads of the old
        // engine go deaf behind the resolution cache; the version
        // signal never does (old `tokens_version_signal` rationale).
        let _ = ctx.version().get();

        let app = style();

        // Registration fast path: steady-state re-fires skip the
        // sweep/flush prologue (`is_registered` contract).
        if !ctx.sheet_is_registered(&app.sheet) {
            theme::ensure_sheet_registered(&backend_for_effect, &app.sheet);
        }
        _pinned_sheet = Some(app.sheet.clone());

        if handles_states_natively {
            // Web: resolve base + every overlay axis; the browser does
            // the state/breakpoint/container switching in CSS. NOT
            // subscribed to the states signal — CSS owns transitions.
            //
            // NO default-font fill on this path (unlike `apply_sheet`):
            // the old core's `attach_style_reactive` resolves the base
            // WITHOUT `with_default_text_font` on both branches — only
            // the static `apply_one` fills. Reactive nodes inherit the
            // document default through the `apply_default_text_font`
            // channel instead. Filling here minted DIFFERENT class
            // hashes than old-core SSR for every reactive-styled node
            // (visually inert, but it broke SSG byte-parity — pinned by
            // `dynamic_sheet_path_does_not_fold_default_font`).
            #[cfg(feature = "debug-stats")]
            let _t_resolve = runtime_shared::debug::now_micros();
            let base = resolve_style(&app);
            let state_overlays = resolve_state_overlays(&app);
            let bp_overlays = resolve_breakpoint_overlays(&app);
            let cq_overlays = resolve_container_overlays(&app);
            #[cfg(feature = "debug-stats")]
            runtime_shared::debug::record_apply_phase(
                "nc_sheet_dyn_resolve",
                runtime_shared::debug::now_micros().saturating_sub(_t_resolve),
            );
            #[cfg(feature = "debug-stats")]
            let _t_apply = runtime_shared::debug::now_micros();
            backend_for_effect.borrow_mut().apply_styled_variants(
                &node_for_effect,
                &base,
                &state_overlays,
                &bp_overlays,
                &cq_overlays,
            );
            #[cfg(feature = "debug-stats")]
            runtime_shared::debug::record_apply_phase(
                "nc_sheet_dyn_apply",
                runtime_shared::debug::now_micros().saturating_sub(_t_apply),
            );
            #[cfg(feature = "debug-stats")]
            runtime_shared::debug::record_apply_phase(
                "nc_sheet_dyn_effect_total",
                runtime_shared::debug::now_micros().saturating_sub(_t_effect),
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
            // No default-font fill — old-core `attach_style_reactive`
            // parity (see the natively-handled branch above).
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
pub(crate) fn apply_sheet<H: StyleServices>(
    backend: &Rc<RefCell<H>>,
    node: &H::Node,
    app: &StyleApplication,
    handles_states_natively: bool,
    ctx: &theme::ThemeCtx,
) {
    // Registration fast path (bulk-create hot spot): only pay the
    // sweep/flush prologue when the sheet is genuinely unregistered —
    // 10k identical rows hit this once, not 10k times.
    if !ctx.sheet_is_registered(&app.sheet) {
        theme::ensure_sheet_registered(backend, &app.sheet);
    }
    if handles_states_natively {
        let base = fill_default_text_font(resolve_style(app), ctx.default_text_font());
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
        let resolved = fill_default_text_font(resolved, ctx.default_text_font());
        backend.borrow_mut().apply_style(node, &resolved);
    }
}

// ===========================================================================
// Overlay resolution — public-API reimplementation of the walker's
// crate-internal `resolve_state_overlays` / breakpoint / container
// helpers (the sheet's cached axis lists are pub(crate) to runtime-core,
// so the vocabulary scans `variant_keys()` by the reserved prefixes).
// ===========================================================================

#[cfg(not(idealyst_premint_only))]
/// The sheet's declared state-overlay axes — the CACHED per-sheet
/// slice (empty for the common no-`state`-block case). Scanning
/// `variant_keys()` here allocated the full key list per styled node
/// per fire — measured in the repeat enqueue loop at bulk-create
/// scale (the old walker always read the cached slice).
fn sheet_state_axes(sheet: &Rc<StyleSheet>) -> Vec<(StateBits, String)> {
    let cached = sheet.state_axes();
    if cached.is_empty() {
        return Vec::new();
    }
    cached
        .iter()
        .map(|(bit, axis)| (*bit, axis.clone()))
        .collect()
}

/// Resolve each declared state overlay against the application's
/// variants + theme: `(bits, fully resolved rules)` pairs a
/// natively-handling backend emits as pseudo-class CSS. Reads the
/// sheet's cached axis slice — allocation-free when no `state` blocks
/// are declared.
pub(crate) fn resolve_state_overlays(app: &StyleApplication) -> Vec<(StateBits, Rc<StyleRules>)> {
    let axes = app.sheet.state_axes();
    if axes.is_empty() {
        return Vec::new();
    }
    axes.to_vec()
        .into_iter()
        .map(|(bit, axis)| {
            let state_app = app.clone().with(axis, "on");
            (bit, resolve_style(&state_app))
        })
        .collect()
}

/// Breakpoint analog — `(bucket, fully resolved rules)`; the cached
/// axis slice is already in declaration order and the walker sorted by
/// rank, preserved here.
fn resolve_breakpoint_overlays(app: &StyleApplication) -> Vec<(Breakpoint, Rc<StyleRules>)> {
    let axes = app.sheet.breakpoint_axes();
    if axes.is_empty() {
        return Vec::new();
    }
    let mut out: Vec<(Breakpoint, Rc<StyleRules>)> = axes
        .to_vec()
        .into_iter()
        .map(|(bp, axis)| {
            let bp_app = app.clone().with(axis, "on");
            (bp, resolve_style(&bp_app))
        })
        .collect();
    out.sort_by_key(|(bp, _)| bp.rank());
    out
}

/// Container-query analog — `(min-width threshold px, fully resolved
/// rules)` sorted ascending by threshold.
fn resolve_container_overlays(app: &StyleApplication) -> Vec<(f32, Rc<StyleRules>)> {
    let axes = app.sheet.container_axes();
    if axes.is_empty() {
        return Vec::new();
    }
    let mut out: Vec<(f32, Rc<StyleRules>)> = axes
        .to_vec()
        .into_iter()
        .map(|(threshold, axis)| {
            let cq_app = app.clone().with(axis, "on");
            (threshold, resolve_style(&cq_app))
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
    let current = runtime_shared::current_breakpoint().get();
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
/// Takes the font by VALUE from the caller's captured `ThemeCtx`
/// (`ctx.default_text_font()`) so hot paths don't pay a world-context
/// `inject` per call.
fn fill_default_text_font(
    rules: Rc<StyleRules>,
    default_font: Option<runtime_shared::FontFamily>,
) -> Rc<StyleRules> {
    if rules.font_family.is_none() {
        if let Some(font) = default_font {
            let mut owned = (*rules).clone();
            owned.font_family = Some(font);
            return Rc::new(owned);
        }
    }
    rules
}

// ===========================================================================
// Tests — breakpoint + container overlay folding.
//
// Ports of the old walker's inline `walker/style.rs::{breakpoint_tests,
// container_tests}` (8). `runtime-shared` tests the style-engine
// PRIMITIVES (sheet resolution, variant axes, `StyleRules::merge`); what
// dies with the walker is the *fold policy* built on top of them —
// sort-by-rank-not-declaration-order, mobile-first cumulative layering,
// the same-`Rc` fast path when nothing is active, zero-width ⇒ base, and
// the convergence property the native container-query feedback loop
// depends on. Nothing else covers those: the scene-parity goldens record
// only the FINAL applied rules for the widths their fixtures happen to
// run at.
// ===========================================================================

#[cfg(test)]
mod overlay_merge_tests {
    use super::*;
    use runtime_shared::container_query::container_axis_name;
    use runtime_shared::{set_viewport_size, Breakpoint, Length, StyleSheet, Tokenized, ViewportSize};

    fn px(p: f32) -> Option<Tokenized<Length>> {
        Some(Tokenized::Literal(Length::Px(p)))
    }

    fn width_of(rules: &StyleRules) -> Length {
        *rules
            .width
            .as_ref()
            .expect("width is set in these fixtures")
            .value()
    }

    /// Base `width: 100`, `breakpoint md { width: 500 }`,
    /// `breakpoint lg { width: 900 }` — `lg` declared FIRST on purpose,
    /// so a resolver that merely preserved declaration order would fail.
    fn responsive_app() -> StyleApplication {
        let sheet = Rc::new(
            StyleSheet::new(|_vs| StyleRules {
                width: px(100.0),
                ..Default::default()
            })
            .variant("__bp_lg", "on", |_vs| StyleRules {
                width: px(900.0),
                ..Default::default()
            })
            .variant("__bp_md", "on", |_vs| StyleRules {
                width: px(500.0),
                ..Default::default()
            }),
        );
        StyleApplication::new(sheet)
    }

    #[test]
    fn resolve_breakpoint_overlays_sorts_ascending_and_resolves_each() {
        let app = responsive_app();
        let overlays = resolve_breakpoint_overlays(&app);
        assert_eq!(overlays.len(), 2, "two breakpoint overlays declared");
        assert_eq!(overlays[0].0, Breakpoint::Md, "sorted by rank, not declaration");
        assert_eq!(overlays[1].0, Breakpoint::Lg);
        // Each entry is the FULLY resolved rules for that bucket (base
        // merged with the overlay), so consumers can stack them.
        assert_eq!(width_of(&overlays[0].1), Length::Px(500.0));
        assert_eq!(width_of(&overlays[1].1), Length::Px(900.0));
    }

    #[test]
    fn resolve_breakpoint_overlays_empty_without_breakpoint_blocks() {
        let sheet = Rc::new(StyleSheet::new(|_vs| StyleRules {
            width: px(100.0),
            ..Default::default()
        }));
        let app = StyleApplication::new(sheet);
        assert!(resolve_breakpoint_overlays(&app).is_empty());
    }

    #[test]
    fn merge_active_breakpoints_layers_mobile_first_by_viewport_width() {
        let app = responsive_app();
        let base = resolve_style(&app);
        let overlays = resolve_breakpoint_overlays(&app);

        // Below sm: nothing active → base width, and the SAME Rc back
        // (no allocation on the common mobile path).
        set_viewport_size(ViewportSize::new(390.0, 800.0));
        let merged = merge_active_breakpoints(base.clone(), &overlays);
        assert_eq!(width_of(&merged), Length::Px(100.0));
        assert!(
            Rc::ptr_eq(&merged, &base),
            "no active overlay must reuse the base Rc"
        );

        // Md bucket: only md is active (lg is above).
        set_viewport_size(ViewportSize::new(800.0, 800.0));
        let merged = merge_active_breakpoints(base.clone(), &overlays);
        assert_eq!(width_of(&merged), Length::Px(500.0));

        // Lg bucket: md AND lg both active (min-width is cumulative);
        // lg wins the conflicting `width`.
        set_viewport_size(ViewportSize::new(1100.0, 800.0));
        let merged = merge_active_breakpoints(base.clone(), &overlays);
        assert_eq!(width_of(&merged), Length::Px(900.0));
    }

    /// Base `width: 100`, `container (min_width: 600) { width: 900 }`,
    /// `container (min_width: 300) { width: 500 }` — larger threshold
    /// declared FIRST, again to prove sorting.
    fn container_app() -> StyleApplication {
        let sheet = Rc::new(
            StyleSheet::new(|_vs| StyleRules {
                width: px(100.0),
                ..Default::default()
            })
            .variant(container_axis_name(600.0), "on", |_vs| StyleRules {
                width: px(900.0),
                ..Default::default()
            })
            .variant(container_axis_name(300.0), "on", |_vs| StyleRules {
                width: px(500.0),
                ..Default::default()
            }),
        );
        StyleApplication::new(sheet)
    }

    #[test]
    fn resolve_container_overlays_sorts_ascending_and_resolves_each() {
        let app = container_app();
        let overlays = resolve_container_overlays(&app);
        assert_eq!(overlays.len(), 2, "two container overlays declared");
        assert_eq!(overlays[0].0, 300.0, "ascending by threshold");
        assert_eq!(overlays[1].0, 600.0);
        assert_eq!(width_of(&overlays[0].1), Length::Px(500.0));
        assert_eq!(width_of(&overlays[1].1), Length::Px(900.0));
    }

    #[test]
    fn resolve_container_overlays_empty_without_container_blocks() {
        let sheet = Rc::new(StyleSheet::new(|_vs| StyleRules {
            width: px(100.0),
            ..Default::default()
        }));
        let app = StyleApplication::new(sheet);
        assert!(resolve_container_overlays(&app).is_empty());
    }

    #[test]
    fn merge_active_containers_layers_mobile_first_by_container_width() {
        let app = container_app();
        let base = resolve_style(&app);
        let overlays = resolve_container_overlays(&app);

        let merged = merge_active_containers(base.clone(), &overlays, 200.0);
        assert_eq!(width_of(&merged), Length::Px(100.0));
        assert!(
            Rc::ptr_eq(&merged, &base),
            "no active overlay must reuse the base Rc"
        );

        let merged = merge_active_containers(base.clone(), &overlays, 450.0);
        assert_eq!(width_of(&merged), Length::Px(500.0));

        let merged = merge_active_containers(base.clone(), &overlays, 700.0);
        assert_eq!(width_of(&merged), Length::Px(900.0));

        // Exactly at a threshold is inclusive (min-width semantics).
        let merged = merge_active_containers(base.clone(), &overlays, 300.0);
        assert_eq!(width_of(&merged), Length::Px(500.0));
    }

    /// Container width 0 — which is what every non-web backend reports on
    /// the new core until the container-signal port (module docs) —
    /// activates nothing, so the node renders at its mobile-first base.
    /// This is the assertion that makes the documented deferral SAFE
    /// rather than merely stated.
    #[test]
    fn merge_active_containers_zero_width_is_base() {
        let app = container_app();
        let base = resolve_style(&app);
        let overlays = resolve_container_overlays(&app);
        let merged = merge_active_containers(base.clone(), &overlays, 0.0);
        assert!(Rc::ptr_eq(&merged, &base));
    }

    /// Convergence: merging at the SAME width twice yields identical
    /// rules. The native container-query feedback loop depends on it —
    /// after a restyle the container's width is unchanged (inline-size
    /// containment), so re-resolving must produce the same result and the
    /// change-guarded signal must not re-fire. Without this the loop
    /// oscillates forever.
    #[test]
    fn merge_active_containers_is_idempotent_at_fixed_width() {
        let app = container_app();
        let base = resolve_style(&app);
        let overlays = resolve_container_overlays(&app);
        let a = merge_active_containers(base.clone(), &overlays, 700.0);
        let b = merge_active_containers(base.clone(), &overlays, 700.0);
        assert_eq!(width_of(&a), width_of(&b));
    }
}

// ===========================================================================
// Tests — the SignalClass JS fast path (the fallback path is covered by
// the scene-parity goldens; recorders report no JS-binding support).
// ===========================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use runtime_shared::accessibility::AccessibilityProps;
    use runtime_scene::Host;
    use runtime_world::{collect_owned, World};

    /// Minimal StyleServices host recording the JS-binding surface.
    #[derive(Default)]
    struct JsHost {
        registered: Vec<(u64, Vec<u32>, Vec<String>)>,
        released: Vec<u32>,
        notified: Vec<(u64, u32)>,
        minted: usize,
        next_binding: u32,
    }

    impl Host for JsHost {
        type Node = u32;
        fn insert(&mut self, _p: &mut u32, _c: u32) {}
        fn insert_at(&mut self, _p: &mut u32, _c: u32, _i: usize) {}
        fn remove_child(&mut self, _p: &u32, _c: &u32) {}
        fn clear_children(&mut self, _n: &u32) {}
        fn create_anchor(&mut self) -> u32 {
            0
        }
        fn supports_splice(&self) -> bool {
            true
        }
    }
    impl crate::caps::ViewOps for JsHost {
        fn create_view(&mut self, _a11y: &AccessibilityProps) -> u32 {
            0
        }
    }
    impl crate::caps::DocumentOps for JsHost {}
    impl crate::caps::AssetOps for JsHost {}
    impl crate::caps::AppEnvOps for JsHost {}
    impl crate::caps::StyleOps for JsHost {
        fn apply_style(&mut self, _node: &u32, _style: &Rc<StyleRules>) {}
        fn supports_js_class_bindings(&self) -> bool {
            true
        }
        fn mint_class_for_app(&mut self, _app: &StyleApplication) -> Option<String> {
            self.minted += 1;
            Some(format!("iy-test-{}", self.minted))
        }
        fn register_reactive_class_binding(
            &mut self,
            _node: &u32,
            signal_id: u64,
            values: &[u32],
            classes: &[&str],
            _value_reader: Rc<dyn Fn() -> u32>,
        ) -> u32 {
            self.registered.push((
                signal_id,
                values.to_vec(),
                classes.iter().map(|s| s.to_string()).collect(),
            ));
            self.next_binding += 1;
            self.next_binding
        }
        fn release_reactive_class_binding(&mut self, binding_id: u32) {
            self.released.push(binding_id);
        }
        fn notify_signal_value_js(&mut self, signal_id: u64, value: u32) {
            self.notified.push((signal_id, value));
        }
    }

    fn test_app(v: u32) -> StyleApplication {
        // One cached sheet for the whole test module (mirrors
        // `stylesheet!` output); per-value apps differ by override.
        fn sheet() -> Rc<StyleSheet> {
            static KEY: u8 = 0;
            runtime_shared::cached_stylesheet(&KEY as *const u8 as usize, || {
                Rc::new(StyleSheet::r#static(StyleRules::default()))
            })
        }
        let mut rules = StyleRules::default();
        rules.opacity = Some(runtime_shared::Tokenized::Literal(if v == 0 { 0.25 } else { 0.75 }));
        StyleApplication::new(sheet()).with_overrides(rules)
    }

    /// The named bench-gate risk: a shared signal driving N
    /// signal-class nodes must fan out through ONE per-signal notifier
    /// (one `notify_signal_value_js` per commit), not N per-node
    /// effects. Also proves: per-value classes minted per node,
    /// binding registered with the signal's raw id, seeding first-run
    /// notify BEFORE registration, release on teardown, and notifier
    /// dedup across nodes.
    #[test]
    fn signal_class_js_fast_path_single_notifier_fan_out() {
        let world = World::new();
        let backend = Rc::new(RefCell::new(JsHost::default()));
        let (sig, owned) = world.enter(|| {
            let sig = runtime_world::signal(0u32);
            let ((), owned) = collect_owned(|| {
                for node in 0..3u32 {
                    let StyleProp::SignalClass(spec) =
                        signal_class(sig, &[0, 1], test_app)
                    else {
                        panic!("signal_class builds a SignalClass prop")
                    };
                    let _setter = attach_style(&backend, &node, StyleProp::SignalClass(spec));
                }
            });
            (sig, owned)
        });

        {
            let b = backend.borrow();
            assert_eq!(b.registered.len(), 3, "one binding per node");
            assert_eq!(b.minted, 6, "two classes minted per node");
            for (sid, values, classes) in &b.registered {
                assert_eq!(*sid, sig.raw_id());
                assert_eq!(values, &vec![0, 1]);
                assert_eq!(classes.len(), 2);
            }
            // Seeding: the notifier's creation run shipped the initial
            // value ONCE (deduped across the three nodes), before any
            // registration could rely on it.
            assert_eq!(b.notified, vec![(sig.raw_id(), 0)]);
        }

        // One commit → exactly ONE notify, regardless of node count.
        world.enter(|| sig.set(1));
        world.flush();
        assert_eq!(
            backend.borrow().notified.last(),
            Some(&(sig.raw_id(), 1)),
            "commit shipped the new value"
        );
        assert_eq!(
            backend.borrow().notified.len(),
            2,
            "shared-signal fan-out is ONE notify per commit, not per node"
        );

        // Teardown releases every binding.
        drop(owned);
        let released = backend.borrow().released.clone();
        assert_eq!(released.len(), 3, "each node's binding released");
    }

    /// A spec whose `compute` was wrapped after construction (the
    /// navigator's `fold_style_overrides` shape) must NOT take the JS
    /// fast path — the pre-built apps table no longer reflects the
    /// rendered style, and minting it would drop the folded overrides.
    #[test]
    fn regression_folded_signal_class_skips_js_fast_path() {
        let world = World::new();
        let backend = Rc::new(RefCell::new(JsHost::default()));
        world.enter(|| {
            let sig = runtime_world::signal(0u32);
            let StyleProp::SignalClass(mut spec) = signal_class(sig, &[0, 1], test_app)
            else {
                panic!("signal_class builds a SignalClass prop")
            };
            // Wrap compute the way handlers/navigator.rs does.
            let inner = spec.compute.clone();
            spec.compute = Rc::new(move || inner());
            let ((), _owned) = collect_owned(|| {
                let node = 7u32;
                let _setter = attach_style(&backend, &node, StyleProp::SignalClass(spec));
            });
            assert!(
                backend.borrow().registered.is_empty(),
                "folded spec must take the per-node-effect fallback, not the JS binding"
            );
        });
    }
}
