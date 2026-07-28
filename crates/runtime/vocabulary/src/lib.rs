//! # runtime-vocabulary — capability traits, legacy bridge, and the
//! built-in primitive vocabulary (P2a + P2b)
//!
//! The capability-trait split of runtime-core's `Backend` mega-trait
//! (idea-lite core migration, design §5). One Ops trait per primitive
//! family lives in [`caps`], with **signatures frozen** — method names,
//! parameter types, return types, and data-only defaults are copied
//! verbatim from `runtime_core::backend::Backend`. [`bridge::LegacyBridge`]
//! wraps any existing `Backend` impl and implements
//! [`runtime_scene::Host`] plus every Ops trait by delegation, proving
//! the freeze compiles and giving every existing backend the full
//! capability surface with zero edits to backend crates.
//!
//! ## P2b — the built-in primitive vocabulary
//!
//! - [`prims`] — one payload struct per primitive (props:
//!   `runtime_world::Value<T>` for reactive-capable props, `Rc<dyn Fn>`
//!   for callbacks, plain fields for inert config).
//! - [`builders`] — the `ui!` contract: `view() -> ViewBuilder`, props
//!   as methods, `.child(...)` with closure children as structural
//!   holes, `.build() -> runtime_scene::Element`.
//! - [`handlers`] — generic mount fns bounded on exactly the caps they
//!   use; the old walker's per-primitive build modules, invariant for
//!   invariant. [`handlers::register_builtins`] installs all 13.
//! - [`style_attach`] — the shared style service: [`style_attach::StyleProp`]
//!   with the two P2 paths (static resolved rules, dynamic rules
//!   closure) and the [`style_attach::on_teardown`] probe.
//!
//! Backend-call-stream parity with the old walker is pinned by
//! `crates/dev/scene-parity`'s full-op golden suite.
//!
//! ## P2b scope — the 13 core primitives
//!
//! `view`, `text`, `button`, `pressable`, `image`, `icon`, `toggle`,
//! `slider`, `activity_indicator`, `link`, `scroll_view`, `text_input`,
//! `text_area`.
//!
//! **Explicitly deferred to later phases** (per the migration plan's
//! phasing — documented here so nothing is silently missing):
//!
//! - `virtualizer`/`flat_list`, `graphics`, `portal`/overlays,
//!   `presence`, navigator primitives (P3–P6, each with its own
//!   contract work);
//! - styled-text runs beyond the basic `create_styled_text` path (theme
//!   re-realization, `JsBinding` fan-out);
//! - hydration behavior (P3 web);
//! - cohort / premint / signal-class / preminted styling and sheet
//!   registration (P3 web style policy) — [`style_attach::StyleProp`]
//!   carries the two P2 paths only;
//! - safe-area *reactive* inset re-application (P3; P2 applies once at
//!   mount) and the native container-query inline-size feedback loop.
//!
//! ## P3a — the macro glue ([`glue`])
//!
//! [`glue`] is the emission surface `ui!` / `#[component]` target when
//! `runtime-macros` is built with its `new-core` feature (the macros
//! retarget `::runtime_core::…` paths in their OUTPUT to
//! `::runtime_vocabulary::glue::…`). Additional deferrals specific to
//! that path — each fails compilation under `new-core` with a message
//! naming its migration phase, never silently:
//!
//! - `overlay` / `anchored_overlay` / `presence` / `graphics` /
//!   `flat_list` tags (their primitives are P3+ deferrals above);
//! - the virtualizer `for i in count_method(sig)` sugar (virtualizer);
//! - in-app `link(route = …)` (navigation, P6) — `external = …` works;
//! - `test_id = …` on primitives (identity/robot registration, P5);
//! - `#[component(lazy)]` / `#[lazy]` (wasm-split chunking, P3b) and
//!   `#[method]` blocks (Ref/robot machinery, P5);
//! - structured generator-backend bindings (`Derived` metadata,
//!   `Element::Switch`/`Repeat` batching): lowered instead to the
//!   equivalent closure forms — same observable reactivity, no wire
//!   metadata (generator backends re-land post-P7).
//!
//! `COVERAGE.md` (crate root) maps every one of the Backend trait's
//! methods to its Ops trait — nothing is silently unaccounted for.
//!
//! ## Node associated type
//!
//! Every Ops trait is a subtrait of [`runtime_scene::Host`], so
//! `Self::Node` is Host's associated node type — declared exactly once,
//! shared by all thirty traits, and structurally identical to the old
//! `Backend::Node`. A generic handler bounds on what it needs
//! (`fn mount_text<B: TextOps + StyleOps>(…) -> B::Node`) and the node
//! types unify through the common supertrait. Host additionally requires
//! `Node: Clone + 'static` and `Self: 'static` (structural regions retain
//! node handles across effect fires), which is why
//! [`bridge::LegacyBridge`]'s impls carry `B: 'static` bounds the plain
//! `Backend` trait never stated — every real backend already satisfies
//! them.
//!
//! ## Cross-trait defaults
//!
//! Backend defaults that call *other* Backend methods are preserved as
//! supertrait bounds rather than moved to the bridge, so a future direct
//! implementor (P2b backends) keeps today's default behavior:
//!
//! - `caps::ExternalOps` is the supertrait of every `create_*` whose
//!   default renders the `missing_primitive_placeholder` (image, icon,
//!   text-input, toggle, slider, activity, virtualizer, graphics,
//!   portal, navigator) — a missing primitive degrades to an External
//!   placeholder, same as today.
//! - `caps::ViewOps` is the supertrait of defaults that lower to
//!   `create_view` (pressable, link, presence placeholder, document
//!   `create_element`).
//! - `caps::ButtonOps: TextOps` because `update_button_label`'s default
//!   lowers to `update_text`.
//! - Defaults that call methods *within the same trait*
//!   (`create_styled_text` → `create_text`, `apply_styled_variants` →
//!   `apply_styled_states` → `apply_style`, `note_virtualizer_binding` →
//!   `note_repeat_binding`, `execute_batch_with_attach` →
//!   `execute_batch` + `Host::insert_many`, `apply_scroll_view_safe_area_inset`
//!   → `apply_safe_area_padding`, `missing_primitive_placeholder` →
//!   `create_external`) keep their bodies in place unchanged.
//!
//! ## Migration-transitional dependency on runtime-core
//!
//! SANCTIONED for P2a–P6: the frozen signatures reference runtime-core's
//! prop/data types (`StyleRules`, `AccessibilityProps`, `TouchHandler`,
//! handler aliases, `primitives::*` payload types, handle types, …).
//! Those types migrate out of runtime-core into vocabulary-owned payload
//! modules at P7 (when the walker, the `Element` enum, and the Backend
//! mega-trait are deleted); the `runtime-core` dependency is removed in
//! the same change. Nothing in this crate depends on the walker, the
//! reactive arena, or `Element` — only on the type vocabulary.

pub mod bridge;
pub mod builders;
pub mod caps;
pub mod glue;
pub mod handlers;
pub mod prims;
pub mod style_attach;

pub use bridge::LegacyBridge;
pub use builders::{
    activity_indicator, button, icon, image, link, pressable, scroll_view, slider, text, text_area,
    text_input, toggle, view, SceneChild, TextContent,
};
pub use caps::AllCaps;
pub use handlers::register_builtins;
pub use style_attach::{attach_style, on_teardown, IntoStyleProp, StyleProp};
