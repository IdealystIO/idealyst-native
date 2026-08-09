//! FULL-OP golden harness (P2b exit gate): a recorder that logs the
//! complete backend-call stream — `create_*` / `update_*` / `apply_style`
//! / handler installs / `release_*` — not just the 7 structural ops, so
//! the vocabulary's generic handlers can be proven to emit the SAME
//! sequences as the old walker.
//!
//! # The recorded alphabet
//!
//! [`FullRecorder`] overrides exactly the capability methods the mount
//! path models; everything else stays at the trait default
//! (unrecorded). The P3c style-engine gate added `install_tokens` /
//! `update_tokens` (theme delivery + cohort-swap ordering) and
//! `attach_html_class` (preminted class stamping) — none fire in the
//! pre-P3c scenarios, so their goldens are unchanged. Deliberately
//! OUTSIDE the alphabet, with rationale:
//!
//! - `register_stylesheet`/`unregister_stylesheet` — the fixtures mint
//!   a fresh `Rc<StyleSheet>` per closure fire (the common inline
//!   shape), so this stream pins Rc-lifetime churn (dead-Weak sweep
//!   timing) rather than style semantics; and the two sides register
//!   through different engines (runtime-core's thread-local table vs
//!   the vocabulary's per-world table) whose sweep POINTS legitimately
//!   differ. Registration behavior on the new path is pinned by
//!   `runtime-vocabulary/tests/vocab.rs`'s sheet-path suite instead.
//! - `apply_default_text_font` — document-level premint plumbing with
//!   no per-node observable; pinned by the vocabulary suite.
//! - `WireBindingOps` notes (`note_text_binding`, …) — declarative
//!   wire/generator backends only; scenarios use opaque closures, which
//!   skip them on the old side too.
//! - `finish`/`run_layout`/`set_page_metadata`/robot/introspection —
//!   host-lifecycle plumbing outside the mount contract (P3 drivers).
//!
//! Args render compactly and deterministically: closures as `<fn>`,
//! `StyleRules` as a one-line non-`None`-fields digest
//! ([`rules_digest`]), `AccessibilityProps` omitted when default.
//!
//! Each op has exactly ONE body: an inherent `<method>_impl` on
//! [`FullRecorder`], reached through `impl runtime_scene::Host` + the 30
//! `runtime_vocabulary::caps::*Ops` impls, directly on the recorder. The
//! `*_impl` indirection is a fossil of the old-vs-new parity era, when
//! the same bodies were ALSO reached through a now-deleted
//! `impl runtime_core::Backend`; it is kept because the golden files were
//! frozen against exactly these bodies.
//!
//! Methods the recorder does NOT implement fall to the **caps** defaults.
//! Those bodies were audited against the old `Backend` defaults the
//! goldens were frozen under and are byte-identical for every method
//! reachable here (`apply_styled_states` → `apply_style`,
//! `update_styled_text` → `update_text(plain_text_of(runs))`,
//! `execute_batch_with_attach` → `execute_batch` + `insert_many`,
//! `create_element` → `create_view`, all the `false`/`None`/no-op
//! capability flags).

use std::path::PathBuf;
use std::rc::Rc;

use runtime_scene::Host;
use runtime_shared::accessibility::AccessibilityProps;
use runtime_shared::primitives;
use runtime_shared::primitives::icon::IconData;
use runtime_shared::{Color, Easing, StateBits, StyleRules};
use runtime_vocabulary::caps;

use crate::{Mode, PNode, Recorder};

// ===========================================================================
// Queuing microtask scheduler (navigator scenarios)
// ===========================================================================
//
// The old swap/stack handlers defer their author-layout build to a
// microtask (it re-borrows the backend, so it can't run inside the
// `create_navigator` borrow). Without an installed scheduler,
// `schedule_microtask` runs INLINE -> double-borrow panic. So the full-op
// harness installs a QUEUING scheduler (the SSR backend's pattern) and
// `FullCx::mount`/`step` drain the queue after each closure - deferred
// chrome ops land inside the same golden step, deterministically. The
// queue is thread-local (tests run in parallel threads; the OnceLock
// scheduler is process-global); non-navigator scenarios queue nothing,
// so their goldens are unaffected.

mod parity_scheduler {
    use runtime_shared::scheduling::{ScheduleHandle, Scheduler};
    use std::cell::RefCell;
    use std::collections::VecDeque;

    thread_local! {
        static QUEUE: RefCell<VecDeque<Box<dyn FnOnce() + 'static>>> =
            const { RefCell::new(VecDeque::new()) };
    }

    struct NoopHandle;
    impl ScheduleHandle for NoopHandle {
        fn cancel(&mut self) {}
    }

    // ---- Cancellable one-shot queues (presence scenarios) ----
    //
    // `after_animation_frame` / `after_ms` queue their callbacks in
    // per-kind thread-local queues instead of dropping them, and the
    // returned handle's cancel-on-drop VACATES the slot — real
    // `ScheduledTask` semantics, which presence relies on (a cancelled
    // pending-enter must not fire over a fresh exit state). Scenarios
    // pump them at explicit step boundaries via [`pump_frames`] /
    // [`pump_timers`]; scenarios that never pump behave exactly as
    // before (callback held instead of dropped, zero ops).

    type Slot = std::rc::Rc<RefCell<Option<Box<dyn FnOnce() + 'static>>>>;

    thread_local! {
        static FRAMES: RefCell<Vec<Slot>> = const { RefCell::new(Vec::new()) };
        static TIMERS: RefCell<Vec<Slot>> = const { RefCell::new(Vec::new()) };
    }

    struct SlotHandle(Slot);
    impl ScheduleHandle for SlotHandle {
        fn cancel(&mut self) {
            self.0.borrow_mut().take();
        }
    }
    impl Drop for SlotHandle {
        fn drop(&mut self) {
            self.cancel();
        }
    }

    fn queue_slot(
        queue: &'static std::thread::LocalKey<RefCell<Vec<Slot>>>,
        f: Box<dyn FnOnce() + 'static>,
    ) -> Box<dyn ScheduleHandle> {
        let slot: Slot = std::rc::Rc::new(RefCell::new(Some(f)));
        queue.with(|q| q.borrow_mut().push(slot.clone()));
        Box::new(SlotHandle(slot))
    }

    fn pump_queue(queue: &'static std::thread::LocalKey<RefCell<Vec<Slot>>>) {
        let slots: Vec<Slot> = queue.with(|q| std::mem::take(&mut *q.borrow_mut()));
        for slot in slots {
            // Release the slot borrow BEFORE running: a callback may
            // drop its own spent ScheduledTask, whose cancel re-borrows.
            let f = slot.borrow_mut().take();
            if let Some(f) = f {
                f();
            }
        }
    }

    /// Fire every pending animation-frame callback (one frame elapses).
    pub(crate) fn pump_frames() {
        pump_queue(&FRAMES);
    }

    /// Fire every pending `after_ms` callback (all timers elapse).
    pub(crate) fn pump_timers() {
        pump_queue(&TIMERS);
    }

    struct ParityScheduler;
    impl Scheduler for ParityScheduler {
        fn schedule_microtask(&self, f: Box<dyn FnOnce() + 'static>) {
            QUEUE.with(|q| q.borrow_mut().push_back(f));
        }
        fn after_animation_frame(
            &self,
            f: Box<dyn FnOnce() + 'static>,
        ) -> Box<dyn ScheduleHandle> {
            queue_slot(&FRAMES, f)
        }
        fn after_ms(
            &self,
            _delay_ms: i32,
            f: Box<dyn FnOnce() + 'static>,
        ) -> Box<dyn ScheduleHandle> {
            queue_slot(&TIMERS, f)
        }
        fn raf_loop(&self, _f: Box<dyn FnMut() + 'static>) -> Box<dyn ScheduleHandle> {
            Box::new(NoopHandle)
        }
    }

    pub(crate) fn ensure_installed() {
        // First install wins process-wide; queueing is per-thread.
        runtime_shared::scheduling::install_scheduler(Box::new(ParityScheduler));
    }

    /// Run every queued microtask (and any they enqueue) to completion.
    pub(crate) fn drain() {
        loop {
            let next = QUEUE.with(|q| q.borrow_mut().pop_front());
            match next {
                Some(task) => task(),
                None => break,
            }
        }
    }
}

/// Elapse one animation frame / every pending `after_ms` timer inside a
/// scenario step (presence: the enter's animate-to-rest fires on the
/// next frame; the exit's detach-and-teardown fires when its duration
/// elapses). Exposed for BOTH scenario files — the two sides must pump
/// at identical step boundaries for the goldens to line up.
pub fn pump_frames() {
    parity_scheduler::pump_frames();
}

pub fn pump_timers() {
    parity_scheduler::pump_timers();
}

/// Install the queueing parity scheduler (idempotent; first install wins
/// process-wide). The NEW-side driver calls this too — both sides must
/// schedule through the same queues or the pump steps can't line up.
pub(crate) fn ensure_parity_scheduler() {
    parity_scheduler::ensure_installed();
}

// ===========================================================================
// Arg rendering
// ===========================================================================

/// One-line digest of resolved `StyleRules`: the derived Debug output
/// with every `field: None` entry dropped (depth-aware split, so nested
/// `Some(..)` payloads render fully). Deterministic: derived Debug
/// follows declaration order, and both sides digest equal structs.
pub fn rules_digest(rules: &StyleRules) -> String {
    compact_struct_debug(&format!("{rules:?}"))
}

fn compact_struct_debug(dbg: &str) -> String {
    let Some(open) = dbg.find('{') else {
        return dbg.to_string();
    };
    let close = dbg.rfind('}').unwrap_or(dbg.len());
    let inner = &dbg[open + 1..close];
    let mut segs: Vec<&str> = Vec::new();
    let mut level = 0i32;
    let mut in_str = false;
    let mut prev = ' ';
    let mut seg_start = 0usize;
    for (i, ch) in inner.char_indices() {
        match ch {
            '"' if prev != '\\' => in_str = !in_str,
            '{' | '(' | '[' if !in_str => level += 1,
            '}' | ')' | ']' if !in_str => level -= 1,
            ',' if !in_str && level == 0 => {
                segs.push(inner[seg_start..i].trim());
                seg_start = i + 1;
            }
            _ => {}
        }
        prev = ch;
    }
    let tail = inner[seg_start..].trim();
    if !tail.is_empty() {
        segs.push(tail);
    }
    let kept: Vec<&str> = segs.into_iter().filter(|s| !s.ends_with(": None")).collect();
    format!("{{{}}}", kept.join(", "))
}

/// ` a11y={..}` suffix, or empty for the default props (the common
/// case, keeping golden lines short).
fn a11y_suffix(a11y: &AccessibilityProps) -> String {
    let dbg = format!("{a11y:?}");
    if dbg == format!("{:?}", AccessibilityProps::default()) {
        String::new()
    } else {
        format!(" a11y={}", compact_struct_debug(&dbg))
    }
}

fn icon_digest(data: &IconData) -> String {
    format!(
        "icon(vb={:?} paths={} filled={})",
        data.view_box,
        data.paths.len(),
        data.filled
    )
}

fn opt_str(v: Option<&str>) -> String {
    match v {
        Some(s) => format!("{s:?}"),
        None => "none".to_string(),
    }
}

/// Positioning-intent digest of a [`primitives::portal::PortalTarget`].
/// The `Anchor` arm renders only the side/align/offset intent — the
/// `AnchorTarget` itself is an opaque ref (unfilled in scenarios;
/// `rect()` is `None`), so it carries nothing deterministic to pin.
fn target_digest(target: &primitives::portal::PortalTarget) -> String {
    use primitives::portal::PortalTarget as T;
    match target {
        T::Viewport(placement) => format!("viewport({placement:?})"),
        T::Anchor {
            side,
            align,
            offset,
            ..
        } => format!("anchor(side={side:?} align={align:?} offset={offset})"),
        T::Named(name) => format!("named({name})"),
    }
}

/// One-line digest of a `PresenceState`: `rest` for the all-`None`
/// resting state, else the non-`None` fields (derived Debug order).
fn presence_state_digest(state: &primitives::presence::PresenceState) -> String {
    if *state == primitives::presence::PresenceState::rest() {
        "rest".to_string()
    } else {
        compact_struct_debug(&format!("{state:?}"))
    }
}

// ===========================================================================
// FullRecorder
// ===========================================================================

/// The full-op recording backend. See the module docs for the alphabet
/// contract.
pub struct FullRecorder {
    pub(crate) rec: Recorder,
    pub(crate) splice: bool,
    /// Captured button press callbacks, in creation order — lets an
    /// integration test fire an authored `on_click` exactly as a real
    /// backend would (proving the macro → builders → handlers → backend
    /// wiring end to end). See [`FullRecorder::button_action`].
    actions: Vec<(String, Rc<dyn Fn()>)>,
    /// Captured `attach_states` setters, in attach order — lets a style
    /// scenario flip interaction bits exactly as a native event source
    /// would (the P3c state-overlay gate). See
    /// [`FullRecorder::state_setter`].
    state_setters: Vec<Rc<dyn Fn(StateBits, bool)>>,
    /// Captured virtualizer callback bundles, in creation order — the
    /// scenario drives the visible window through [`VirtSim`] exactly
    /// as a native recycler would. See [`FullRecorder::virt_sim`].
    virt_sims: Vec<Rc<VirtSim>>,
    /// Captured graphics lifecycle closures, in creation order — the
    /// scenario fires ready/resize/lost as the platform surface would.
    /// See [`FullRecorder::gfx_sim`].
    gfx_sims: Vec<Rc<GfxSim>>,
}

impl FullRecorder {
    pub fn new(rec: Recorder, mode: Mode) -> Self {
        FullRecorder {
            rec,
            splice: matches!(mode, Mode::Spliced),
            actions: Vec::new(),
            state_setters: Vec::new(),
            virt_sims: Vec::new(),
            gfx_sims: Vec::new(),
        }
    }

    /// The `nth` created virtualizer's platform sim (creation order).
    pub fn virt_sim(&self, nth: usize) -> Rc<VirtSim> {
        self.virt_sims
            .get(nth)
            .cloned()
            .unwrap_or_else(|| {
                panic!("no virtualizer #{nth}; {} were created", self.virt_sims.len())
            })
    }

    /// The `nth` created graphics surface's platform sim.
    pub fn gfx_sim(&self, nth: usize) -> Rc<GfxSim> {
        self.gfx_sims
            .get(nth)
            .cloned()
            .unwrap_or_else(|| panic!("no graphics #{nth}; {} were created", self.gfx_sims.len()))
    }

    /// The `nth` captured interaction-state setter (attach order).
    /// Panics loudly when out of range.
    pub fn state_setter(&self, nth: usize) -> Rc<dyn Fn(StateBits, bool)> {
        self.state_setters
            .get(nth)
            .cloned()
            .unwrap_or_else(|| {
                panic!(
                    "no state setter #{nth}; {} were captured",
                    self.state_setters.len()
                )
            })
    }

    /// The press callback of the most recently created button whose
    /// label matched at creation time. Panics (with the known labels)
    /// when nothing matched — tests want that loud.
    pub fn button_action(&self, label: &str) -> Rc<dyn Fn()> {
        self.actions
            .iter()
            .rev()
            .find(|(l, _)| l == label)
            .map(|(_, f)| f.clone())
            .unwrap_or_else(|| {
                panic!(
                    "no recorded button labeled {label:?}; known: {:?}",
                    self.actions.iter().map(|(l, _)| l).collect::<Vec<_>>()
                )
            })
    }
}

// ===========================================================================
// Inherent op bodies — the ONE implementation of every recorded op
// ===========================================================================
//
// Realization routes here through `impl runtime_scene::Host` + the 30
// `runtime_vocabulary::caps::*Ops` impls (below), each a mechanical
// one-line delegation.
//
// The bodies live HERE rather than inline in the caps impls because they
// used to be shared with a second, now-deleted `impl runtime_core::Backend`
// that the golden corpus was frozen through. Keeping the split keeps the
// recorded alphabet textually identical to what produced the goldens.
// Method names are therefore `<old_backend_method>_impl`.
impl FullRecorder {
    // --- structural (same line format as ParityBackend/SceneHost) ---

    fn insert_impl(&mut self, parent: &mut PNode, child: PNode) {
        self.rec.push(format!("insert {parent} <- {child}"));
    }

    fn insert_many_impl(&mut self, parent: &mut PNode, children: Vec<PNode>) {
        let list = children
            .iter()
            .map(|c| c.to_string())
            .collect::<Vec<_>>()
            .join(", ");
        self.rec.push(format!("insert_many {parent} <- [{list}]"));
    }

    fn insert_at_impl(&mut self, parent: &mut PNode, child: PNode, index: usize) {
        self.rec
            .push(format!("insert_at {parent} <- {child} @ {index}"));
    }

    fn remove_child_impl(&mut self, parent: &PNode, child: &PNode) {
        self.rec.push(format!("remove_child {parent} -x {child}"));
    }

    fn clear_children_impl(&mut self, node: &PNode) {
        self.rec.push(format!("clear_children {node}"));
    }

    fn create_reactive_anchor_impl(&mut self) -> PNode {
        let n = self.rec.mint();
        self.rec.push(format!("create {n} anchor"));
        n
    }

    fn supports_child_splice_impl(&self) -> bool {
        self.splice
    }

    // --- view / containers ---

    fn create_view_impl(&mut self, a11y: &AccessibilityProps) -> PNode {
        let n = self.rec.mint();
        self.rec.push(format!("create {n} view{}", a11y_suffix(a11y)));
        n
    }

    fn mark_container_impl(&mut self, node: &PNode) {
        self.rec.push(format!("mark_container {node}"));
    }

    fn create_pressable_impl(
        &mut self,
        _on_click: Rc<dyn Fn()>,
        a11y: &AccessibilityProps,
    ) -> PNode {
        let n = self.rec.mint();
        self.rec
            .push(format!("create {n} pressable on_click=<fn>{}", a11y_suffix(a11y)));
        n
    }

    fn create_scroll_view_impl(
        &mut self,
        horizontal: bool,
        on_scroll: Option<Rc<dyn Fn(f32, f32)>>,
        a11y: &AccessibilityProps,
    ) -> PNode {
        let n = self.rec.mint();
        self.rec.push(format!(
            "create {n} scroll_view horizontal={horizontal} on_scroll={}{}",
            if on_scroll.is_some() { "<fn>" } else { "none" },
            a11y_suffix(a11y)
        ));
        n
    }

    // --- input channels ---

    fn install_touch_handler_impl(&mut self, node: &PNode, _handler: runtime_shared::TouchHandler) {
        self.rec.push(format!("install_touch_handler {node}"));
    }

    fn install_wheel_handler_impl(&mut self, node: &PNode, _handler: runtime_shared::WheelHandler) {
        self.rec.push(format!("install_wheel_handler {node}"));
    }

    fn install_hover_handler_impl(&mut self, node: &PNode, _handler: runtime_shared::HoverHandler) {
        self.rec.push(format!("install_hover_handler {node}"));
    }

    fn install_file_drop_handler_impl(
        &mut self,
        node: &PNode,
        _handler: runtime_shared::FileDropHandler,
    ) {
        self.rec.push(format!("install_file_drop_handler {node}"));
    }

    fn mark_preserves_focus_impl(&mut self, node: &PNode) {
        self.rec.push(format!("mark_preserves_focus {node}"));
    }

    // --- text ---

    fn create_text_impl(&mut self, content: &str, a11y: &AccessibilityProps) -> PNode {
        let n = self.rec.mint();
        self.rec
            .push(format!("create {n} text {content:?}{}", a11y_suffix(a11y)));
        n
    }

    fn create_styled_text_impl(
        &mut self,
        runs: &[runtime_shared::styled_text::TextRun],
        a11y: &AccessibilityProps,
    ) -> PNode {
        let n = self.rec.mint();
        let plain = runtime_shared::styled_text::plain_text_of(runs);
        self.rec.push(format!(
            "create {n} styled_text {plain:?} runs={}{}",
            runs.len(),
            a11y_suffix(a11y)
        ));
        n
    }

    /// Opted-in: bound texts take the batched-id fast path so the
    /// goldens pin `update_text_by_id`/`release_text_id`. The id is the
    /// node's creation index.
    fn create_text_with_id_impl(
        &mut self,
        content: &str,
        a11y: &AccessibilityProps,
    ) -> Option<(PNode, u32)> {
        let n = self.rec.mint();
        self.rec.push(format!(
            "create {n} text+id{} {content:?}{}",
            n.0,
            a11y_suffix(a11y)
        ));
        Some((n, n.0))
    }

    fn update_text_impl(&mut self, node: &PNode, content: &str) {
        self.rec.push(format!("update_text {node} {content:?}"));
    }

    fn update_text_by_id_impl(&mut self, id: u32, content: String) {
        self.rec.push(format!("update_text_by_id id{id} {content:?}"));
    }

    fn release_text_id_impl(&mut self, id: u32) {
        self.rec.push(format!("release_text_id id{id}"));
    }

    // --- button ---

    fn create_button_impl(
        &mut self,
        label: &str,
        on_click: &runtime_shared::Action,
        leading_icon: Option<&IconData>,
        trailing_icon: Option<&IconData>,
        a11y: &AccessibilityProps,
    ) -> PNode {
        self.actions.push((label.to_string(), on_click.fire.clone()));
        let n = self.rec.mint();
        let mut line = format!("create {n} button {label:?} on_click=<fn>");
        if let Some(icon) = leading_icon {
            line.push_str(&format!(" leading={}", icon_digest(icon)));
        }
        if let Some(icon) = trailing_icon {
            line.push_str(&format!(" trailing={}", icon_digest(icon)));
        }
        line.push_str(&a11y_suffix(a11y));
        self.rec.push(line);
        n
    }

    fn update_button_label_impl(&mut self, node: &PNode, label: &str) {
        self.rec.push(format!("update_button_label {node} {label:?}"));
    }

    // --- image ---

    fn create_image_impl(
        &mut self,
        src: &str,
        alt: Option<&str>,
        a11y: &AccessibilityProps,
    ) -> PNode {
        let n = self.rec.mint();
        self.rec.push(format!(
            "create {n} image src={src:?} alt={}{}",
            opt_str(alt),
            a11y_suffix(a11y)
        ));
        n
    }

    fn update_image_src_impl(&mut self, node: &PNode, src: &str) {
        self.rec.push(format!("update_image_src {node} {src:?}"));
    }

    fn update_image_alt_impl(&mut self, node: &PNode, alt: Option<&str>) {
        self.rec
            .push(format!("update_image_alt {node} {}", opt_str(alt)));
    }

    fn install_image_load_handler_impl(
        &mut self,
        node: &PNode,
        _handler: runtime_shared::ImageLoadHandler,
    ) {
        self.rec.push(format!("install_image_load_handler {node}"));
    }

    fn install_image_error_handler_impl(
        &mut self,
        node: &PNode,
        _handler: runtime_shared::ImageErrorHandler,
    ) {
        self.rec.push(format!("install_image_error_handler {node}"));
    }

    fn register_asset_impl(
        &mut self,
        id: runtime_shared::assets::AssetId,
        kind: runtime_shared::assets::AssetTag,
        _source: &runtime_shared::assets::AssetSource,
    ) {
        self.rec
            .push(format!("register_asset id={} kind={kind:?}", id.0));
    }

    // --- icon ---

    fn create_icon_impl(
        &mut self,
        data: &IconData,
        color: Option<&Color>,
        a11y: &AccessibilityProps,
    ) -> PNode {
        let n = self.rec.mint();
        self.rec.push(format!(
            "create {n} {} color={}{}",
            icon_digest(data),
            color.map(|c| c.0.clone()).unwrap_or_else(|| "none".into()),
            a11y_suffix(a11y)
        ));
        n
    }

    fn update_icon_color_impl(&mut self, node: &PNode, color: &Color) {
        self.rec.push(format!("update_icon_color {node} {}", color.0));
    }

    fn update_icon_data_impl(&mut self, node: &PNode, data: &IconData) {
        self.rec
            .push(format!("update_icon_data {node} {}", icon_digest(data)));
    }

    fn update_icon_stroke_impl(&mut self, node: &PNode, progress: f32) {
        self.rec
            .push(format!("update_icon_stroke {node} {progress}"));
    }

    #[allow(clippy::too_many_arguments)]
    fn animate_icon_stroke_impl(
        &mut self,
        node: &PNode,
        from: f32,
        to: f32,
        duration_ms: u32,
        easing: Easing,
        infinite: bool,
        autoreverses: bool,
    ) {
        self.rec.push(format!(
            "animate_icon_stroke {node} {from}->{to} {duration_ms}ms {easing:?} infinite={infinite} autoreverses={autoreverses}"
        ));
    }

    // --- toggle / slider / activity ---

    fn create_toggle_impl(
        &mut self,
        initial_value: bool,
        _on_change: Rc<dyn Fn(bool)>,
        a11y: &AccessibilityProps,
    ) -> PNode {
        let n = self.rec.mint();
        self.rec.push(format!(
            "create {n} toggle {initial_value} on_change=<fn>{}",
            a11y_suffix(a11y)
        ));
        n
    }

    fn update_toggle_value_impl(&mut self, node: &PNode, value: bool) {
        self.rec.push(format!("update_toggle_value {node} {value}"));
    }

    fn create_slider_impl(
        &mut self,
        initial_value: f32,
        min: f32,
        max: f32,
        step: Option<f32>,
        _on_change: Rc<dyn Fn(f32)>,
        a11y: &AccessibilityProps,
    ) -> PNode {
        let n = self.rec.mint();
        self.rec.push(format!(
            "create {n} slider {initial_value} [{min},{max}] step={} on_change=<fn>{}",
            step.map(|s| s.to_string()).unwrap_or_else(|| "none".into()),
            a11y_suffix(a11y)
        ));
        n
    }

    fn update_slider_value_impl(&mut self, node: &PNode, value: f32) {
        self.rec.push(format!("update_slider_value {node} {value}"));
    }

    fn create_activity_indicator_impl(
        &mut self,
        size: primitives::activity_indicator::ActivityIndicatorSize,
        color: Option<&Color>,
        a11y: &AccessibilityProps,
    ) -> PNode {
        let n = self.rec.mint();
        self.rec.push(format!(
            "create {n} activity_indicator {size:?} color={}{}",
            color.map(|c| c.0.clone()).unwrap_or_else(|| "none".into()),
            a11y_suffix(a11y)
        ));
        n
    }

    fn update_activity_indicator_size_impl(
        &mut self,
        node: &PNode,
        size: primitives::activity_indicator::ActivityIndicatorSize,
    ) {
        self.rec
            .push(format!("update_activity_indicator_size {node} {size:?}"));
    }

    // --- link ---

    fn create_link_impl(
        &mut self,
        config: primitives::link::LinkConfig,
        a11y: &AccessibilityProps,
    ) -> PNode {
        let n = self.rec.mint();
        self.rec.push(format!(
            "create {n} link route={:?} url={:?} external={} on_activate=<fn>{}",
            config.route,
            config.url,
            config.external,
            a11y_suffix(a11y)
        ));
        n
    }

    fn update_link_url_impl(&mut self, node: &PNode, url: &str) {
        self.rec.push(format!("update_link_url {node} {url:?}"));
    }

    // --- text input / area ---

    #[allow(clippy::too_many_arguments)]
    fn create_text_input_impl(
        &mut self,
        initial_value: &str,
        placeholder: Option<&str>,
        _on_change: Rc<dyn Fn(String)>,
        on_key_down: Option<primitives::key::KeyDownHandler>,
        on_blur: Option<primitives::text_input::BlurHandler>,
        secure: bool,
        a11y: &AccessibilityProps,
    ) -> PNode {
        let n = self.rec.mint();
        self.rec.push(format!(
            "create {n} text_input {initial_value:?} placeholder={} secure={secure} on_change=<fn> on_key_down={} on_blur={}{}",
            opt_str(placeholder),
            if on_key_down.is_some() { "<fn>" } else { "none" },
            if on_blur.is_some() { "<fn>" } else { "none" },
            a11y_suffix(a11y)
        ));
        n
    }

    fn update_text_input_value_impl(&mut self, node: &PNode, value: &str) {
        self.rec
            .push(format!("update_text_input_value {node} {value:?}"));
    }

    fn update_text_input_secure_impl(&mut self, node: &PNode, secure: bool) {
        self.rec
            .push(format!("update_text_input_secure {node} {secure}"));
    }

    fn update_text_input_placeholder_impl(&mut self, node: &PNode, placeholder: Option<&str>) {
        self.rec.push(format!(
            "update_text_input_placeholder {node} {}",
            opt_str(placeholder)
        ));
    }

    fn set_text_input_focus_handler_impl(&mut self, node: &PNode, _handler: Rc<dyn Fn(bool)>) {
        self.rec
            .push(format!("set_text_input_focus_handler {node}"));
    }

    #[allow(clippy::too_many_arguments)]
    fn create_text_area_impl(
        &mut self,
        initial_value: &str,
        placeholder: Option<&str>,
        wrap: bool,
        min_rows: Option<u32>,
        max_rows: Option<u32>,
        _on_change: Rc<dyn Fn(String)>,
        on_key_down: Option<primitives::key::KeyDownHandler>,
        a11y: &AccessibilityProps,
    ) -> PNode {
        let n = self.rec.mint();
        self.rec.push(format!(
            "create {n} text_area {initial_value:?} placeholder={} wrap={wrap} rows=[{:?},{:?}] on_change=<fn> on_key_down={}{}",
            opt_str(placeholder),
            min_rows,
            max_rows,
            if on_key_down.is_some() { "<fn>" } else { "none" },
            a11y_suffix(a11y)
        ));
        n
    }

    fn update_text_area_value_impl(&mut self, node: &PNode, value: &str) {
        self.rec
            .push(format!("update_text_area_value {node} {value:?}"));
    }

    // --- style ---

    fn apply_style_impl(&mut self, node: &PNode, style: &Rc<StyleRules>) {
        self.rec
            .push(format!("apply_style {node} {}", rules_digest(style)));
    }

    fn on_node_unstyled_impl(&mut self, node: &PNode) {
        self.rec.push(format!("on_node_unstyled {node}"));
    }

    fn attach_states_impl(&mut self, node: &PNode, setter: Rc<dyn Fn(StateBits, bool)>) {
        self.state_setters.push(setter);
        self.rec.push(format!("attach_states {node}"));
    }

    // --- P3c style-engine alphabet: theme-token delivery + preminted
    //     class stamping. None of these fire in the pre-P3c scenarios
    //     (no tokens installed, no Preminted styles), so adding them
    //     left the existing goldens untouched. ---

    fn install_tokens_impl(&mut self, tokens: &[runtime_shared::TokenEntry]) {
        let names: Vec<&str> = tokens.iter().map(|t| t.name).collect();
        self.rec.push(format!("install_tokens {names:?}"));
    }

    fn update_tokens_impl(&mut self, tokens: &[runtime_shared::TokenEntry]) {
        let names: Vec<&str> = tokens.iter().map(|t| t.name).collect();
        self.rec.push(format!("update_tokens {names:?}"));
    }

    fn supports_preminted_styles_impl(&self) -> bool {
        true
    }

    fn attach_html_class_impl(&self, node: &PNode, class: &str) {
        self.rec.push(format!("attach_html_class {node} {class}"));
    }

    fn set_disabled_impl(&mut self, node: &PNode, disabled: bool) {
        self.rec.push(format!("set_disabled {node} {disabled}"));
    }

    fn apply_safe_area_padding_impl(&mut self, node: &PNode, sides: runtime_shared::SafeAreaSides) {
        self.rec
            .push(format!("apply_safe_area_padding {node} {sides:?}"));
    }

    fn apply_scroll_view_safe_area_inset_impl(
        &mut self,
        node: &PNode,
        sides: runtime_shared::SafeAreaSides,
    ) {
        self.rec
            .push(format!("apply_scroll_view_safe_area_inset {node} {sides:?}"));
    }

    // --- virtualizer (the P3-set port) ---

    /// Captures the callback bundle WITHOUT invoking it: the backend is
    /// mutably borrowed during `create_*`, so a synchronous window fill
    /// here would double-borrow on BOTH cores (real backends defer the
    /// initial fill for the same reason). The scenario drives the
    /// window through [`VirtSim`] afterwards.
    fn create_virtualizer_impl(
        &mut self,
        callbacks: runtime_shared::VirtualizerCallbacks<PNode>,
        overscan: f32,
        layout: primitives::virtualizer::VirtualLayout,
        a11y: &AccessibilityProps,
    ) -> PNode {
        let n = self.rec.mint();
        self.rec.push(format!(
            "create {n} virtualizer overscan={overscan} layout={layout:?}{}",
            a11y_suffix(a11y)
        ));
        self.virt_sims.push(Rc::new(VirtSim {
            rec: self.rec.clone(),
            callbacks,
            mounted: std::cell::RefCell::new(Vec::new()),
        }));
        n
    }

    fn virtualizer_data_changed_impl(&mut self, node: &PNode) {
        self.rec.push(format!("virtualizer_data_changed {node}"));
    }

    fn release_virtualizer_impl(&mut self, node: &PNode) {
        self.rec.push(format!("release_virtualizer {node}"));
    }

    // --- graphics (the P3-set port) ---

    fn create_graphics_impl(
        &mut self,
        on_ready: primitives::graphics::OnReady,
        on_resize: primitives::graphics::OnResize,
        on_lost: primitives::graphics::OnLost,
        a11y: &AccessibilityProps,
    ) -> PNode {
        let n = self.rec.mint();
        self.rec.push(format!(
            "create {n} graphics on_ready=<fn> on_resize=<fn> on_lost=<fn>{}",
            a11y_suffix(a11y)
        ));
        self.gfx_sims.push(Rc::new(GfxSim {
            on_ready: std::cell::RefCell::new(on_ready),
            on_resize: std::cell::RefCell::new(on_resize),
            on_lost: std::cell::RefCell::new(on_lost),
        }));
        n
    }

    fn release_graphics_impl(&mut self, node: &PNode) {
        self.rec.push(format!("release_graphics {node}"));
    }

    // --- portal + presence (the P3-set port) ---

    fn create_portal_impl(
        &mut self,
        target: primitives::portal::PortalTarget,
        on_dismiss: Option<Rc<dyn Fn()>>,
        trap_focus: bool,
        a11y: &AccessibilityProps,
    ) -> PNode {
        let n = self.rec.mint();
        self.rec.push(format!(
            "create {n} portal target={} on_dismiss={} trap_focus={trap_focus}{}",
            target_digest(&target),
            if on_dismiss.is_some() { "<fn>" } else { "none" },
            a11y_suffix(a11y)
        ));
        n
    }

    fn release_portal_impl(&mut self, node: &PNode) {
        self.rec.push(format!("release_portal {node}"));
    }

    fn set_portal_hidden_impl(&mut self, node: &PNode, hidden: bool) {
        self.rec
            .push(format!("set_portal_hidden {node} {hidden}"));
    }

    fn create_presence_placeholder_impl(&mut self, a11y: &AccessibilityProps) -> PNode {
        let n = self.rec.mint();
        self.rec.push(format!(
            "create {n} presence_placeholder{}",
            a11y_suffix(a11y)
        ));
        n
    }

    fn apply_presence_impl(
        &mut self,
        node: &PNode,
        state: primitives::presence::PresenceState,
        transition: Option<(u32, Easing)>,
    ) {
        let t = match transition {
            Some((ms, easing)) => format!("{ms}ms {easing:?}"),
            None => "snap".to_string(),
        };
        self.rec.push(format!(
            "apply_presence {node} {} {t}",
            presence_state_digest(&state)
        ));
    }

    // --- lifecycle: required but out of alphabet ---

    fn finish_impl(&mut self, _root: PNode) {}
}

// ===========================================================================
// Realization: `runtime_scene::Host` + the 30 capability traits
// ===========================================================================
//
// The harness realizes against the bare `FullRecorder` (see
// `full_new.rs`). Every method here delegates to an inherent `*_impl`
// body — the same bodies the goldens were frozen against.
//
// Traits with an EMPTY impl block take the caps defaults for every
// method — the same behavior the frozen corpus recorded from the
// corresponding `Backend` defaults (audited method-by-method; the
// bodies were byte-identical, e.g. `apply_styled_states` →
// `apply_style`, `update_styled_text` →
// `update_text(plain_text_of(runs))`, `execute_batch_with_attach` →
// `execute_batch` + `insert_many`). `caps::NavigatorOps` is deliberately
// in that group: navigators mount through
// `runtime_vocabulary::handlers::navigator` (SwapNav/StackNav over the
// Lifecycle/View caps), so the harness never needs a navigator cap.

impl Host for FullRecorder {
    type Node = PNode;

    fn insert(&mut self, parent: &mut PNode, child: PNode) {
        self.insert_impl(parent, child)
    }

    fn insert_many(&mut self, parent: &mut PNode, children: Vec<PNode>) {
        self.insert_many_impl(parent, children)
    }

    fn insert_at(&mut self, parent: &mut PNode, child: PNode, index: usize) {
        self.insert_at_impl(parent, child, index)
    }

    fn remove_child(&mut self, parent: &PNode, child: &PNode) {
        self.remove_child_impl(parent, child)
    }

    fn clear_children(&mut self, node: &PNode) {
        self.clear_children_impl(node)
    }

    fn create_anchor(&mut self) -> PNode {
        self.create_reactive_anchor_impl()
    }

    fn supports_splice(&self) -> bool {
        self.supports_child_splice_impl()
    }
}

impl caps::AppEnvOps for FullRecorder {}

impl caps::LifecycleOps for FullRecorder {
    fn finish(&mut self, root: PNode) {
        self.finish_impl(root)
    }
}

impl caps::ViewOps for FullRecorder {
    fn create_view(&mut self, a11y: &AccessibilityProps) -> PNode {
        self.create_view_impl(a11y)
    }
}

impl caps::InputOps for FullRecorder {
    fn install_touch_handler(&mut self, node: &PNode, handler: runtime_shared::TouchHandler) {
        self.install_touch_handler_impl(node, handler)
    }

    fn install_wheel_handler(&mut self, node: &PNode, handler: runtime_shared::WheelHandler) {
        self.install_wheel_handler_impl(node, handler)
    }

    fn install_hover_handler(&mut self, node: &PNode, handler: runtime_shared::HoverHandler) {
        self.install_hover_handler_impl(node, handler)
    }

    fn install_file_drop_handler(
        &mut self,
        node: &PNode,
        handler: runtime_shared::FileDropHandler,
    ) {
        self.install_file_drop_handler_impl(node, handler)
    }

    fn mark_preserves_focus(&mut self, node: &PNode) {
        self.mark_preserves_focus_impl(node)
    }
}

impl caps::PressableOps for FullRecorder {
    fn create_pressable(&mut self, on_click: Rc<dyn Fn()>, a11y: &AccessibilityProps) -> PNode {
        self.create_pressable_impl(on_click, a11y)
    }
}

impl caps::TextOps for FullRecorder {
    fn create_text(&mut self, content: &str, a11y: &AccessibilityProps) -> PNode {
        self.create_text_impl(content, a11y)
    }

    fn create_styled_text(
        &mut self,
        runs: &[runtime_shared::styled_text::TextRun],
        a11y: &AccessibilityProps,
    ) -> PNode {
        self.create_styled_text_impl(runs, a11y)
    }

    fn create_text_with_id(
        &mut self,
        content: &str,
        a11y: &AccessibilityProps,
    ) -> Option<(PNode, u32)> {
        self.create_text_with_id_impl(content, a11y)
    }

    fn update_text(&mut self, node: &PNode, content: &str) {
        self.update_text_impl(node, content)
    }

    fn update_text_by_id(&mut self, id: u32, content: String) {
        self.update_text_by_id_impl(id, content)
    }

    fn release_text_id(&mut self, id: u32) {
        self.release_text_id_impl(id)
    }
}

impl caps::ButtonOps for FullRecorder {
    fn create_button(
        &mut self,
        label: &str,
        on_click: &runtime_shared::Action,
        leading_icon: Option<&IconData>,
        trailing_icon: Option<&IconData>,
        a11y: &AccessibilityProps,
    ) -> PNode {
        self.create_button_impl(label, on_click, leading_icon, trailing_icon, a11y)
    }

    fn update_button_label(&mut self, node: &PNode, label: &str) {
        self.update_button_label_impl(node, label)
    }
}

impl caps::ImageOps for FullRecorder {
    fn create_image(&mut self, src: &str, alt: Option<&str>, a11y: &AccessibilityProps) -> PNode {
        self.create_image_impl(src, alt, a11y)
    }

    fn update_image_src(&mut self, node: &PNode, src: &str) {
        self.update_image_src_impl(node, src)
    }

    fn update_image_alt(&mut self, node: &PNode, alt: Option<&str>) {
        self.update_image_alt_impl(node, alt)
    }

    fn install_image_load_handler(
        &mut self,
        node: &PNode,
        handler: runtime_shared::ImageLoadHandler,
    ) {
        self.install_image_load_handler_impl(node, handler)
    }

    fn install_image_error_handler(
        &mut self,
        node: &PNode,
        handler: runtime_shared::ImageErrorHandler,
    ) {
        self.install_image_error_handler_impl(node, handler)
    }
}

impl caps::IconOps for FullRecorder {
    fn create_icon(
        &mut self,
        data: &IconData,
        color: Option<&Color>,
        a11y: &AccessibilityProps,
    ) -> PNode {
        self.create_icon_impl(data, color, a11y)
    }

    fn update_icon_color(&mut self, node: &PNode, color: &Color) {
        self.update_icon_color_impl(node, color)
    }

    fn update_icon_data(&mut self, node: &PNode, data: &IconData) {
        self.update_icon_data_impl(node, data)
    }

    fn update_icon_stroke(&mut self, node: &PNode, progress: f32) {
        self.update_icon_stroke_impl(node, progress)
    }

    fn animate_icon_stroke(
        &mut self,
        node: &PNode,
        from: f32,
        to: f32,
        duration_ms: u32,
        easing: Easing,
        infinite: bool,
        autoreverses: bool,
    ) {
        self.animate_icon_stroke_impl(
            node,
            from,
            to,
            duration_ms,
            easing,
            infinite,
            autoreverses,
        )
    }
}

impl caps::LinkOps for FullRecorder {
    fn create_link(
        &mut self,
        config: primitives::link::LinkConfig,
        a11y: &AccessibilityProps,
    ) -> PNode {
        self.create_link_impl(config, a11y)
    }

    fn update_link_url(&mut self, node: &PNode, url: &str) {
        self.update_link_url_impl(node, url)
    }
}

impl caps::TextInputOps for FullRecorder {
    fn create_text_input(
        &mut self,
        initial_value: &str,
        placeholder: Option<&str>,
        on_change: Rc<dyn Fn(String)>,
        on_key_down: Option<primitives::key::KeyDownHandler>,
        on_blur: Option<primitives::text_input::BlurHandler>,
        secure: bool,
        a11y: &AccessibilityProps,
    ) -> PNode {
        self.create_text_input_impl(
            initial_value,
            placeholder,
            on_change,
            on_key_down,
            on_blur,
            secure,
            a11y,
        )
    }

    fn update_text_input_value(&mut self, node: &PNode, value: &str) {
        self.update_text_input_value_impl(node, value)
    }

    fn update_text_input_secure(&mut self, node: &PNode, secure: bool) {
        self.update_text_input_secure_impl(node, secure)
    }

    fn update_text_input_placeholder(&mut self, node: &PNode, placeholder: Option<&str>) {
        self.update_text_input_placeholder_impl(node, placeholder)
    }

    fn set_text_input_focus_handler(&mut self, node: &PNode, handler: Rc<dyn Fn(bool)>) {
        self.set_text_input_focus_handler_impl(node, handler)
    }

    fn create_text_area(
        &mut self,
        initial_value: &str,
        placeholder: Option<&str>,
        wrap: bool,
        min_rows: Option<u32>,
        max_rows: Option<u32>,
        on_change: Rc<dyn Fn(String)>,
        on_key_down: Option<primitives::key::KeyDownHandler>,
        a11y: &AccessibilityProps,
    ) -> PNode {
        self.create_text_area_impl(
            initial_value,
            placeholder,
            wrap,
            min_rows,
            max_rows,
            on_change,
            on_key_down,
            a11y,
        )
    }

    fn update_text_area_value(&mut self, node: &PNode, value: &str) {
        self.update_text_area_value_impl(node, value)
    }
}

impl caps::ToggleOps for FullRecorder {
    fn create_toggle(
        &mut self,
        initial_value: bool,
        on_change: Rc<dyn Fn(bool)>,
        a11y: &AccessibilityProps,
    ) -> PNode {
        self.create_toggle_impl(initial_value, on_change, a11y)
    }

    fn update_toggle_value(&mut self, node: &PNode, value: bool) {
        self.update_toggle_value_impl(node, value)
    }
}

impl caps::SliderOps for FullRecorder {
    fn create_slider(
        &mut self,
        initial_value: f32,
        min: f32,
        max: f32,
        step: Option<f32>,
        on_change: Rc<dyn Fn(f32)>,
        a11y: &AccessibilityProps,
    ) -> PNode {
        self.create_slider_impl(initial_value, min, max, step, on_change, a11y)
    }

    fn update_slider_value(&mut self, node: &PNode, value: f32) {
        self.update_slider_value_impl(node, value)
    }
}

impl caps::ActivityIndicatorOps for FullRecorder {
    fn create_activity_indicator(
        &mut self,
        size: primitives::activity_indicator::ActivityIndicatorSize,
        color: Option<&Color>,
        a11y: &AccessibilityProps,
    ) -> PNode {
        self.create_activity_indicator_impl(size, color, a11y)
    }

    fn update_activity_indicator_size(
        &mut self,
        node: &PNode,
        size: primitives::activity_indicator::ActivityIndicatorSize,
    ) {
        self.update_activity_indicator_size_impl(node, size)
    }
}

impl caps::ScrollOps for FullRecorder {
    fn create_scroll_view(
        &mut self,
        horizontal: bool,
        on_scroll: Option<Rc<dyn Fn(f32, f32)>>,
        a11y: &AccessibilityProps,
    ) -> PNode {
        self.create_scroll_view_impl(horizontal, on_scroll, a11y)
    }
}

impl caps::SafeAreaOps for FullRecorder {
    fn apply_safe_area_padding(&mut self, node: &PNode, sides: runtime_shared::SafeAreaSides) {
        self.apply_safe_area_padding_impl(node, sides)
    }

    fn apply_scroll_view_safe_area_inset(
        &mut self,
        node: &PNode,
        sides: runtime_shared::SafeAreaSides,
    ) {
        self.apply_scroll_view_safe_area_inset_impl(node, sides)
    }
}

// No two-axis grid engine on this backend yet; every `GridOps`
// method defaults, so `virtual_grid` reports itself as an
// unsupported primitive instead of silently rendering nothing.
impl caps::GridOps for FullRecorder {}

impl caps::VirtualizerOps for FullRecorder {
    fn create_virtualizer(
        &mut self,
        callbacks: runtime_shared::VirtualizerCallbacks<PNode>,
        overscan: f32,
        layout: primitives::virtualizer::VirtualLayout,
        a11y: &AccessibilityProps,
    ) -> PNode {
        self.create_virtualizer_impl(callbacks, overscan, layout, a11y)
    }

    fn virtualizer_data_changed(&mut self, node: &PNode) {
        self.virtualizer_data_changed_impl(node)
    }

    fn release_virtualizer(&mut self, node: &PNode) {
        self.release_virtualizer_impl(node)
    }
}

impl caps::GraphicsOps for FullRecorder {
    fn create_graphics(
        &mut self,
        on_ready: primitives::graphics::OnReady,
        on_resize: primitives::graphics::OnResize,
        on_lost: primitives::graphics::OnLost,
        a11y: &AccessibilityProps,
    ) -> PNode {
        self.create_graphics_impl(on_ready, on_resize, on_lost, a11y)
    }

    fn release_graphics(&mut self, node: &PNode) {
        self.release_graphics_impl(node)
    }
}

impl caps::PortalOps for FullRecorder {
    fn create_portal(
        &mut self,
        target: primitives::portal::PortalTarget,
        on_dismiss: Option<Rc<dyn Fn()>>,
        trap_focus: bool,
        a11y: &AccessibilityProps,
    ) -> PNode {
        self.create_portal_impl(target, on_dismiss, trap_focus, a11y)
    }

    fn release_portal(&mut self, node: &PNode) {
        self.release_portal_impl(node)
    }

    fn set_portal_hidden(&mut self, node: &PNode, hidden: bool) {
        self.set_portal_hidden_impl(node, hidden)
    }
}

impl caps::PresenceOps for FullRecorder {
    fn create_presence_placeholder(&mut self, a11y: &AccessibilityProps) -> PNode {
        self.create_presence_placeholder_impl(a11y)
    }

    fn apply_presence(
        &mut self,
        node: &PNode,
        state: primitives::presence::PresenceState,
        transition: Option<(u32, Easing)>,
    ) {
        self.apply_presence_impl(node, state, transition)
    }
}

// Defaults only — see the block comment above (`create_navigator` was
// deleted with the old core's `NavigatorHost`; navigators mount through
// the vocabulary's own handlers).
impl caps::NavigatorOps for FullRecorder {}

impl caps::ExternalOps for FullRecorder {}

impl caps::DocumentOps for FullRecorder {
    fn attach_html_class(&self, node: &PNode, class: &str) {
        self.attach_html_class_impl(node, class)
    }
}

impl caps::StyleOps for FullRecorder {
    fn apply_style(&mut self, node: &PNode, style: &Rc<StyleRules>) {
        self.apply_style_impl(node, style)
    }

    fn mark_container(&mut self, node: &PNode) {
        self.mark_container_impl(node)
    }

    fn install_tokens(&mut self, tokens: &[runtime_shared::TokenEntry]) {
        self.install_tokens_impl(tokens)
    }

    fn update_tokens(&mut self, tokens: &[runtime_shared::TokenEntry]) {
        self.update_tokens_impl(tokens)
    }

    fn on_node_unstyled(&mut self, node: &PNode) {
        self.on_node_unstyled_impl(node)
    }

    fn attach_states(&mut self, node: &PNode, setter: Rc<dyn Fn(StateBits, bool)>) {
        self.attach_states_impl(node, setter)
    }

    fn set_disabled(&mut self, node: &PNode, disabled: bool) {
        self.set_disabled_impl(node, disabled)
    }

    fn supports_preminted_styles(&self) -> bool {
        self.supports_preminted_styles_impl()
    }
}

impl caps::AssetOps for FullRecorder {
    fn register_asset(
        &mut self,
        id: runtime_shared::assets::AssetId,
        kind: runtime_shared::assets::AssetTag,
        source: &runtime_shared::assets::AssetSource,
    ) {
        self.register_asset_impl(id, kind, source)
    }
}

impl caps::A11yOps for FullRecorder {}

impl caps::AnimationOps for FullRecorder {}

impl caps::IntrospectionOps for FullRecorder {}

impl caps::BatchOps for FullRecorder {}

impl caps::WireBindingOps for FullRecorder {}

// ===========================================================================
// Platform sims: the recorder-side stand-ins for the native machinery
// that DRIVES the two lazy primitives (a recycler's window math, a
// surface's lifecycle events). Shared by both sides — determinism of
// the sim IS the parity property for callback-driven op streams.
// ===========================================================================

/// One mounted virtualizer row, as the recorder-backend tracks it.
/// (The row's node is logged at mount; the sim itself never re-touches
/// it — placement is outside the recorded contract.)
struct VMounted {
    key: u64,
    scope: u64,
}

/// A deterministic visible-window simulator over a captured
/// [`runtime_shared::VirtualizerCallbacks`] bundle — the golden suite's
/// stand-in for the JS scroll handler / UICollectionView / RecyclerView.
/// Keyed like a real recycler: rows whose key survives a window change
/// keep their mounted subtree (and therefore their reactive state).
///
/// All verbs run OUTSIDE any backend borrow (mount/release re-enter the
/// backend), and must be invoked with the owning world ambient — the
/// same contract real backends carry.
pub struct VirtSim {
    rec: Recorder,
    callbacks: runtime_shared::VirtualizerCallbacks<PNode>,
    mounted: std::cell::RefCell<Vec<VMounted>>,
}

impl VirtSim {
    /// Set the visible window to `range` (clamped to the current item
    /// count): release mounted rows whose key left the window (mounted
    /// order), then mount missing indices (index order). `vsim` lines
    /// pin the callback traffic — count/key/size queries and the
    /// mount/release decisions — alongside the real backend ops the
    /// callbacks emit.
    pub fn set_window(&self, range: std::ops::Range<usize>) {
        let count = (self.callbacks.item_count)();
        let hi = range.end.min(count);
        let lo = range.start.min(hi);
        let want: Vec<(usize, u64)> = (lo..hi)
            .map(|i| (i, (self.callbacks.item_key)(i)))
            .collect();
        self.rec.push(format!(
            "vsim window {lo}..{hi} count={count} keys={:?}",
            want.iter().map(|(_, k)| *k).collect::<Vec<_>>()
        ));
        let mut kept: Vec<VMounted> = Vec::new();
        for row in self.mounted.borrow_mut().drain(..) {
            if want.iter().any(|(_, k)| *k == row.key) {
                kept.push(row);
            } else {
                self.rec
                    .push(format!("vsim release key={} scope={}", row.key, row.scope));
                (self.callbacks.release_item)(row.scope);
            }
        }
        for (idx, key) in want {
            if kept.iter().any(|r| r.key == key) {
                continue;
            }
            let size = (self.callbacks.item_size)(idx);
            let (node, scope) = (self.callbacks.mount_item)(idx);
            self.rec.push(format!(
                "vsim mounted idx={idx} key={key} size={size} -> {node} scope={scope}"
            ));
            kept.push(VMounted { key, scope });
        }
        *self.mounted.borrow_mut() = kept;
    }

    /// Report a measured size for the `nth` currently-mounted row (in
    /// mounted order) — Measured mode's backend half.
    pub fn report_measured(&self, nth: usize, size: f32) {
        let scope = {
            let mounted = self.mounted.borrow();
            let row = mounted
                .get(nth)
                .unwrap_or_else(|| panic!("no mounted row #{nth}"));
            self.rec
                .push(format!("vsim measured key={} scope={} {size}", row.key, row.scope));
            row.scope
        };
        (self.callbacks.set_measured_size)(scope, size);
    }
}

/// A captured graphics surface's lifecycle, fired like the platform
/// would (SurfaceHolder callbacks, canvas events).
pub struct GfxSim {
    on_ready: std::cell::RefCell<primitives::graphics::OnReady>,
    on_resize: std::cell::RefCell<primitives::graphics::OnResize>,
    on_lost: std::cell::RefCell<primitives::graphics::OnLost>,
}

impl GfxSim {
    /// Deliver `on_ready` with a handle-less dummy surface (author code
    /// in these scenarios only reads size/scale — real handle plumbing
    /// is backend territory, outside the mount contract).
    pub fn fire_ready(&self, size: (u32, u32), scale: f32) {
        // `target` is an enum since the GL work: a backend either lends a raw
        // window handle or a GL context. This harness fakes the window arm.
        (self.on_ready.borrow_mut())(primitives::graphics::OnReadyEvent {
            target: primitives::graphics::GraphicsTarget::RawWindow(
                primitives::graphics::GraphicsSurface::new(std::sync::Arc::new(DummySurface)),
            ),
            size,
            scale,
        });
    }

    pub fn fire_resize(&self, size: (u32, u32), scale: f32) {
        (self.on_resize.borrow_mut())(primitives::graphics::OnResizeEvent { size, scale });
    }

    pub fn fire_lost(&self) {
        (self.on_lost.borrow_mut())();
    }
}

/// Surface stand-in: reports `Unavailable` for both raw handles —
/// enough for lifecycle parity (authors here never create a GPU
/// context).
struct DummySurface;

impl raw_window_handle::HasWindowHandle for DummySurface {
    fn window_handle(
        &self,
    ) -> Result<raw_window_handle::WindowHandle<'_>, raw_window_handle::HandleError> {
        Err(raw_window_handle::HandleError::Unavailable)
    }
}

impl raw_window_handle::HasDisplayHandle for DummySurface {
    fn display_handle(
        &self,
    ) -> Result<raw_window_handle::DisplayHandle<'_>, raw_window_handle::HandleError> {
        Err(raw_window_handle::HandleError::Unavailable)
    }
}

// ===========================================================================
// Shared fixtures — used by BOTH scenario files so the two sides build
// identical prop values (digest equality depends on it).
// ===========================================================================

/// Test rules: `width: <w>px` + a literal background color. The old
/// side wraps these in a static `StyleSheet`; resolution returns the
/// same struct (literals resolve to themselves), so both sides digest
/// identically.
pub fn test_rules(width: f32, background: &str) -> StyleRules {
    StyleRules {
        width: Some(runtime_shared::Tokenized::Literal(runtime_shared::Length::Px(width))),
        background: Some(runtime_shared::Tokenized::Literal(Color(background.to_string()))),
        ..Default::default()
    }
}

/// A fixed icon for the scenarios.
pub const TEST_ICON: IconData = IconData {
    view_box: (24, 24),
    paths: &["M0 0h24v24H0z"],
    fill_rule: primitives::icon::FillRule::NonZero,
    filled: false,
};

/// An anchor target for the anchored-overlay scenarios: an UNFILLED
/// `Ref<ViewHandle>` (no primitive ever binds it, `rect()` stays
/// `None`). `runtime_shared::Ref` is plain shared-slot data, so both
/// sides construct it identically; only the side/align/offset intent is
/// digested (see `target_digest`).
pub fn test_anchor_target() -> primitives::portal::AnchorTarget {
    primitives::portal::AnchorTarget::from(runtime_shared::Ref::<runtime_shared::ViewHandle>::new())
}

/// The presence enter/exit fixtures (fade + rise, distinct durations so
/// the golden lines tell the two halves apart).
pub fn presence_enter() -> primitives::presence::PresenceAnim {
    primitives::presence::PresenceAnim::new(
        primitives::presence::PresenceState::rest()
            .opacity(0.0)
            .translate_y(8.0),
        200,
        Easing::EaseOut,
    )
}

pub fn presence_exit() -> primitives::presence::PresenceAnim {
    primitives::presence::PresenceAnim::new(
        primitives::presence::PresenceState::rest()
            .opacity(0.0)
            .translate_y(8.0),
        150,
        Easing::EaseIn,
    )
}

// --- P3c style-engine fixtures ---

use runtime_shared::{StyleApplication, StyleSheet, TokenEntry, TokenValue, Tokenized};

/// The theme token both style scenarios swap.
pub fn surface_token(value: &str) -> TokenEntry {
    TokenEntry {
        name: "color-surface",
        value: TokenValue::Color(Color(value.to_string())),
    }
}

/// A `stylesheet!`-shaped sheet: token-referencing base (background via
/// `color-surface`) + a `size` variant with a `large` arm and a
/// defaulted `medium` arm.
pub fn themed_sheet() -> Rc<StyleSheet> {
    fn base() -> StyleRules {
        StyleRules {
            width: Some(Tokenized::Literal(runtime_shared::Length::Px(100.0))),
            background: Some(Tokenized::token("color-surface", Color("#000".into()))),
            ..Default::default()
        }
    }
    Rc::new(
        StyleSheet::new(|_vs| base())
            .variant("size", "large", |_vs| StyleRules {
                width: Some(Tokenized::Literal(runtime_shared::Length::Px(400.0))),
                ..Default::default()
            })
            .variant("size", "medium", |_vs| StyleRules::default())
            .variant_default("size", "medium"),
    )
}

/// A sheet with a `state hovered { … }` overlay (the macro's reserved
/// `__state_hovered` axis) — the static-divert + overlay-flip fixture.
pub fn hover_sheet() -> Rc<StyleSheet> {
    Rc::new(
        StyleSheet::new(|_vs| StyleRules {
            width: Some(Tokenized::Literal(runtime_shared::Length::Px(100.0))),
            ..Default::default()
        })
        .variant("__state_hovered", "on", |_vs| StyleRules {
            width: Some(Tokenized::Literal(runtime_shared::Length::Px(999.0))),
            ..Default::default()
        }),
    )
}

/// The signal-class mapping: value 0 → a 60px sheet, value 1 → 200px.
/// A fresh static sheet per call, matching the common inline shape.
pub fn class_app(v: u32) -> StyleApplication {
    let width = if v == 0 { 60.0 } else { 200.0 };
    StyleApplication::new(Rc::new(StyleSheet::r#static(test_rules(width, "#606060"))))
}

// ===========================================================================
// Golden paths
// ===========================================================================

/// Absolute path of a full-op scenario's shared golden (owned by the
/// OLD side, like `goldens/`).
pub fn full_golden_path(name: &str, mode: Mode) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("goldens_full")
        .join(format!("{name}.{}.golden", mode.suffix()))
}

/// Absolute path of a full-op NEW-core override golden (sanctioned
/// divergences only — see `full_new::FULL_NEWCORE_OVERRIDES`).
pub fn full_newcore_golden_path(name: &str, mode: Mode) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("goldens_full_newcore")
        .join(format!("{name}.{}.golden", mode.suffix()))
}
