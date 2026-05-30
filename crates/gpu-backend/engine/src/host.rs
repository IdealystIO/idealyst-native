//! Platform-agnostic interaction host.
//!
//! Owns the live `WgpuBackend`, the shared text + font-system
//! stores, and the per-interaction state (focused input, slider
//! drag, last pointer position). Exposes a small method surface
//! that platform shells (winit on desktop, browser, UIKit, …)
//! drive from their native event stream.
//!
//! Nothing in this module depends on a platform. The winit shim in
//! [`crate::app`] is one client; a future web or iOS shell would
//! consume the same `Host` API by translating its own events to
//! [`crate::input::PointerEvent`] / [`crate::input::KeyEvent`].
//!
//! # Threading
//!
//! Single-threaded. Same as the framework itself — `Signal`s,
//! `Effect`s, and the `Rc`-based node tree all live on the host
//! thread. Cross-thread input (background networking) would
//! need to post into this thread before calling `pointer_*` /
//! `key`.

use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;
// `web-time` mirrors `std::time` on native and uses `performance.now()`
// on wasm32 (where `std::time::Instant::now()` panics). The `EventSink`
// trait in `render-api` re-exports the same `Instant`, so this stays
// one type across the call boundary.
use web_time::{Duration, Instant};

// `render` stays in scope because the #[cfg(test)] pointer-routing
// tests at the bottom of this file build throw-away trees through
// it. The non-test `Host::mount` routes through `fw_mount` so
// author-side `effect!`s land inside the framework's reactive scope
// (see the comment on `Host::mount`).
#[allow(unused_imports)]
use runtime_core::{
    mount as fw_mount, render, ColorScheme, Easing, Owner, Element, StateBits, TouchEvent,
    TouchHandler, TouchId, TouchPhase, TouchPoint,
};
use glyphon::FontSystem;
use runtime_layout::LayoutNode;

use render_api::EventSink;
use crate::backend_impl::WgpuBackend;
use render_api::{Key, KeyEvent, PointerButton, PointerEvent, ScrollEvent};
use crate::keyboard;
use crate::node::{
    NodeKind, WgpuNode, HIT_SLOP, IOS_MIN_HIT_TARGET, KEYBOARD_ANIM_MS, KEYBOARD_HEIGHT,
    KEYBOARD_INPUT_MARGIN, PAN_THRESHOLD, SCROLL_MOMENTUM_DECAY_PER_SEC,
    SCROLL_MOMENTUM_MIN_VELOCITY, SCROLL_MOMENTUM_STALE_MS, SCROLL_RUBBERBAND_RESISTANCE,
    SCROLL_SPRINGBACK_EPSILON, SCROLL_SPRINGBACK_RATE_PER_SEC, SCROLL_VELOCITY_SMOOTHING,
    SLIDER_THUMB_SIZE,
};
use crate::painter::Painter;
use crate::text::TextStore;

/// Bundled default font (Inter Regular, SIL Open Font License 1.1 —
/// see `assets/fonts/LICENSE-Inter.txt`). Registered in the
/// `FontSystem` by `Host::new` so wasm32 builds (where
/// `cosmic-text` finds no system fonts) have a guaranteed baseline
/// for shaping. Native builds get it too — system fonts are still
/// available via fontconfig/CoreText, but the bundled font means
/// "no default font" is never the failure mode on any target.
///
/// 398 KB at the regular weight. If we ever ship multiple weights
/// or want to swap to a variable font, this slot is the only place
/// to touch.
pub const DEFAULT_FONT_BYTES: &[u8] =
    include_bytes!("../assets/fonts/Inter-Regular.ttf");

pub struct Host {
    backend: Rc<RefCell<WgpuBackend>>,
    text: Rc<RefCell<TextStore>>,
    font_system: Rc<RefCell<FontSystem>>,
    /// Active platform skin. Plumbed through to the renderer for
    /// widget + keyboard paint, and to the keyboard hit-test path
    /// so taps on synthesized keys land in the right `KeySpec`s.
    skin: Rc<dyn Painter>,
    /// Currently keyboard-focused TextInput, if any. Cleared on
    /// click outside any input or on Esc.
    focused_input: Option<WgpuNode>,
    /// Active pointer-down interaction, if any. iOS-style: a press
    /// captures a node and a release action; whether the action
    /// fires depends on whether the pointer is still inside the
    /// node when the pointer comes up. Cleared on pointer-up /
    /// pointer-cancel.
    active_press: Option<ActivePress>,
    /// Live post-pan scroll motion (momentum-coast or
    /// rubber-band spring-back). Ticked by
    /// [`Host::tick_animations`] every frame until it settles
    /// (velocity below the threshold for Coast, distance below
    /// epsilon for SpringBack). A fresh pointer-down clears it
    /// (tap-to-catch).
    momentum: Option<ScrollMotion>,
    /// Most recent pointer position in logical px. Updated by
    /// every pointer event so slider drag has fresh coordinates
    /// when the platform only delivers a `down` (no `move`).
    pointer: (f32, f32),
    /// Logical viewport size in CSS pixels. Set by the shell via
    /// [`Host::set_viewport`] on startup and on resize; used to
    /// position the on-screen keyboard against the bottom edge.
    pub(crate) viewport: (f32, f32),
    /// Pre-built glyphon buffers for every on-screen keyboard
    /// label. Constructed once in [`Host::new`] so the renderer's
    /// read-only walk can borrow them straight out — no
    /// per-frame text allocation.
    pub(crate) keyboard_glyphs: HashMap<&'static str, glyphon::Buffer>,
    /// Pre-built glyphon buffers for the device chrome (clock,
    /// status digits, etc.). RefCell so the host can refresh
    /// the clock label once a minute without invalidating the
    /// surrounding immutable borrows the renderer holds during
    /// its read-only walk.
    pub(crate) chrome_glyphs:
        std::cell::RefCell<HashMap<&'static str, glyphon::Buffer>>,
    /// Last wall-clock minute we re-shaped the chrome clock
    /// glyph for. The host's tick compares `current_clock_minute()`
    /// against this and re-shapes when the minute rolls over.
    pub(crate) chrome_clock_minute: std::cell::Cell<i64>,
    /// Current rest visibility of the on-screen keyboard:
    /// `0.0` = fully hidden, `1.0` = fully visible. Updated by
    /// [`Host::sync_keyboard`] whenever focus changes; the
    /// transition itself is interpolated by `keyboard_anim`.
    keyboard_value: f32,
    /// In-flight slide animation, if the keyboard is currently
    /// moving between hidden and visible. Cleared automatically
    /// when [`tick_animations`] notices the duration has elapsed.
    keyboard_anim: Option<KeyboardAnim>,
    /// Label of the keyboard key currently shown as pressed, if
    /// any. Set on tap (virtual keyboard) or on physical
    /// keystroke (resolved via [`label_for_key_event`]) and
    /// cleared a fixed duration later — gives every keystroke a
    /// brief visual press-feedback flash regardless of source.
    pub(crate) keyboard_pressed_label: std::cell::Cell<Option<&'static str>>,
    /// Wall-clock at which the press flash was set. Used by
    /// `tick` to clear the label once the flash duration has
    /// elapsed.
    keyboard_pressed_at: std::cell::Cell<Option<Instant>>,
    /// Framework reactive scopes. Held so they outlive the host;
    /// cleared on `unmount` or drop.
    _owner: Option<Owner>,
    /// Live raw-touch interactions tracked by [`TouchId`]. Populated
    /// on `Began` when a `touch_handler` along the hit-test path
    /// returns `consumed: true`; cleared on `Ended` / `Cancelled`.
    /// While present, the host routes subsequent events for the same
    /// id straight to the handler and short-circuits the existing
    /// widget / scroll-pan paths.
    active_touches: HashMap<TouchId, ActiveTouch>,
    /// Navigator-header hit regions collected during the last
    /// render. Each entry binds a screen-rect inside a header
    /// strip to a [`HeaderHitAction`] keyed on the owning
    /// navigator's `WgpuNode`. The renderer writes this every
    /// frame; the pointer-down dispatch consults it before
    /// running the normal hit-test walk so a tap on the back
    /// chevron or a `header_left`/`header_right` icon dispatches
    /// the right navigator command without ever reaching the
    /// screen subtree.
    pub(crate) header_hits: std::cell::RefCell<Vec<HeaderHit>>,
}

/// One tappable header-bar slot. Pushed during render by
/// `paint_navigator_header`; resolved by pointer dispatch.
///
/// `navigator` is `Weak` so the per-frame hit registry can't
/// pin a navigator alive past its real lifetime. The renderer
/// rewrites `host.header_hits` every frame, and replacing the
/// vec drops the previous entries' Rcs synchronously — if those
/// Rcs were strong and the old vec held the last reference to a
/// popped navigator, the cascade would walk into
/// `NavigatorControl` → `scopes` HashMap → inner `Scope::drop`
/// → `StyleHandle::drop` → `backend.borrow_mut()` panic, because
/// the renderer is currently holding `backend.borrow_mut()`.
pub(crate) struct HeaderHit {
    pub rect: (f32, f32, f32, f32),
    pub navigator: std::rc::Weak<std::cell::RefCell<crate::node::NodeData>>,
    pub action: HeaderHitAction,
}

/// What to do when a header hit is resolved on pointer-up.
#[derive(Clone)]
pub(crate) enum HeaderHitAction {
    /// Pop the owning navigator's stack.
    Back,
    /// Close the owning drawer navigator. Used by the scrim's
    /// tap-outside-to-close region — distinct from `Back` so
    /// the dispatcher doesn't try to pop a drawer (which would
    /// panic; drawers don't handle Pop).
    CloseDrawer,
    /// Fire the screen's `header_left` button's `on_press`.
    /// Cached at hit-collect time so the dispatch doesn't have
    /// to walk the screen tree to find the active one.
    HeaderLeft(Rc<dyn Fn()>),
    /// Fire the screen's `header_right` button's `on_press`.
    HeaderRight(Rc<dyn Fn()>),
}

/// One in-flight raw-touch interaction. Cached so subsequent
/// `Moved` / `Ended` / `Cancelled` events for the same [`TouchId`]
/// dispatch directly to the owning handler without re-running the
/// responder-chain walk.
struct ActiveTouch {
    /// The owning node — the one whose `touch_handler` returned
    /// `consumed: true` at `Began`. Kept so we can recompute the
    /// origin per event (layout can shift between events).
    node: WgpuNode,
    /// Cached handler closure. Equivalent to
    /// `node.borrow().touch_handler.clone().unwrap()` but kept here
    /// to avoid the borrow on every move.
    handler: TouchHandler,
    /// `true` once the handler returned `claim: true` on any event
    /// in this interaction. Suppresses competing dispatch (currently
    /// a no-op on wgpu since we already short-circuit on consume;
    /// future scroll-coexistence logic will read this).
    claimed: bool,
}

#[derive(Copy, Clone, Debug)]
struct KeyboardAnim {
    from: f32,
    to: f32,
    started: Instant,
    duration: Duration,
    easing: Easing,
}

/// In-flight scroll-motion driving a scrollview's offset over
/// time. Two flavors:
/// - `Coast` — exponential-decay momentum after a fling. Drives
///   `offset += -velocity * dt`, decays `velocity *= exp(-k*dt)`.
/// - `SpringBack` — exponential approach to a target offset
///   after a rubber-band overshoot. Drives
///   `offset += (target - offset) * (1 - exp(-k*dt))`.
struct ScrollMotion {
    scrollview: WgpuNode,
    kind: ScrollMotionKind,
    last_tick: Instant,
}

enum ScrollMotionKind {
    Coast { velocity: (f32, f32) },
    SpringBack { target: (f32, f32) },
}

/// What a pointer-down created. Lives only while the pointer is
/// held; cleared on up/cancel.
enum ActivePress {
    /// Pressable / Button / Toggle: press → maybe-fire on release.
    /// If `scrollview` is set, dragging past [`PAN_THRESHOLD`]
    /// converts this into a `Pan` (cancelling the click).
    Click {
        node: WgpuNode,
        action: ReleaseAction,
        /// `true` while the pointer is over the captured node.
        /// Toggles as the pointer moves in/out of the bounds; the
        /// release action fires only if `over` is true at up.
        over: bool,
        /// Innermost scrollview at the press origin, if any.
        /// Lets a deliberate drag upgrade the press into a pan.
        scrollview: Option<WgpuNode>,
        /// Pointer position at press-down. Used to measure the
        /// drag distance against [`PAN_THRESHOLD`].
        start: (f32, f32),
    },
    /// Phone-style pan scrolling: pointer-drag inside (or starting
    /// from empty space within) a scrollview translates the
    /// content with the cursor. `last` is updated on every move so
    /// the next delta is relative; `last_time` is the timestamp of
    /// that update, used to derive the smoothed `velocity` (px/sec
    /// in screen space) that we hand to the momentum kick-off on
    /// release. `start` is the pointer position at the moment the
    /// pan began (either an empty-area press or the upgrade point
    /// from a `Click`) — `pointer_up` measures total motion
    /// against it to decide whether the gesture was a real pan
    /// or a tap on empty space.
    Pan {
        scrollview: WgpuNode,
        last: (f32, f32),
        last_time: Instant,
        velocity: (f32, f32),
        start: (f32, f32),
    },
    /// Slider: press fires the initial value, every move updates,
    /// release just ends the drag.
    SliderDrag { node: WgpuNode },
}

enum ReleaseAction {
    /// Pressable / Button.
    Fire(Rc<dyn Fn()>),
    /// Toggle: read current value at fire time and pass !current to
    /// `on_change`. Reading at release rather than press keeps us
    /// honest if the signal flipped via another path during the
    /// press.
    FlipToggle(Rc<dyn Fn(bool)>),
}

impl Host {
    pub fn new(skin: Rc<dyn Painter>, color_scheme: ColorScheme) -> Self {
        let text = Rc::new(RefCell::new(TextStore::new()));
        let font_system = Rc::new(RefCell::new(FontSystem::new()));
        // Register the bundled default font BEFORE anything reads from
        // the font system. On wasm32 there are no system fonts at all
        // — `FontSystem::new()` returns an empty DB and `cosmic-text`
        // would panic with "no default font found" inside the very
        // first text shape (the keyboard glyph cache below). On native
        // the system DB is populated via fontconfig/CoreText, but we
        // still register Inter so every host has a guaranteed
        // baseline font regardless of platform.
        font_system
            .borrow_mut()
            .db_mut()
            .load_font_data(DEFAULT_FONT_BYTES.to_vec());
        // Pre-build keyboard glyph buffers for the active skin
        // while we have exclusive access to the font system. Cheap
        // (~30 small Buffers); the alternative is mutating the
        // font system from the read-only render walk, which would
        // require a `RefCell` borrow dance on the hot path.
        let keyboard_glyphs = keyboard::build_glyph_cache(
            &mut font_system.borrow_mut(),
            skin.as_ref(),
        );
        // Pre-build the device-chrome glyph buffers (clock,
        // battery percent, etc.) the same way. The clock label
        // gets refreshed every minute from `refresh_clock_glyph`
        // — same Buffer key, new shape on each minute boundary,
        // so the renderer keeps reading from this cache without
        // reshaping per-frame.
        let chrome_glyphs = build_chrome_glyph_cache(
            &mut font_system.borrow_mut(),
            skin.as_ref(),
        );
        // Push the skin's safe-area metrics into the framework's
        // reactive signal so any `.safe_area(TOP | BOTTOM)`
        // container in the app subtree inset itself off the
        // device chrome. Idempotent — `set_safe_area_insets`
        // is signal-compare-and-set.
        runtime_core::set_safe_area_insets(skin.safe_area_insets());
        let backend = Rc::new(RefCell::new(WgpuBackend::new(
            text.clone(),
            font_system.clone(),
            color_scheme,
            skin.clone(),
        )));
        // Plumb a weak self-reference into the backend so its
        // navigator / tab / drawer dispatchers can re-acquire a
        // mutable borrow when the user calls `handle.push(...)`
        // etc. The framework releases its own borrow before
        // user callbacks fire, so this upgrade-then-borrow_mut
        // is sound — see `install_navigator_dispatcher`.
        let _ = backend
            .borrow()
            .self_weak
            .set(std::rc::Rc::downgrade(&backend));
        // ALSO publish the weak via a thread-local handle so the
        // framework's animation subscribers (which reach the backend
        // from outside any current `Backend` borrow) can route
        // per-frame writes through `set_animated_f32` /
        // `set_animated_color`. Same shape as iOS's
        // `install_global_self`.
        crate::backend_impl::install_global_self(std::rc::Rc::downgrade(&backend));
        Self {
            backend,
            text,
            font_system,
            skin,
            focused_input: None,
            active_press: None,
            momentum: None,
            pointer: (0.0, 0.0),
            viewport: (0.0, 0.0),
            keyboard_glyphs,
            chrome_glyphs: std::cell::RefCell::new(chrome_glyphs),
            chrome_clock_minute: std::cell::Cell::new(current_clock_minute()),
            keyboard_value: 0.0,
            keyboard_anim: None,
            keyboard_pressed_label: std::cell::Cell::new(None),
            keyboard_pressed_at: std::cell::Cell::new(None),
            _owner: None,
            active_touches: HashMap::new(),
            header_hits: std::cell::RefCell::new(Vec::new()),
        }
    }

    /// Tell the host the current logical viewport size. The
    /// shell calls this on window creation and on resize so the
    /// on-screen keyboard knows where the bottom of the screen
    /// is.
    pub fn set_viewport(&mut self, w: f32, h: f32) {
        self.viewport = (w, h);
    }

    /// Is the simulator's on-screen keyboard currently visible
    /// (or animating in)? `true` means the renderer should paint
    /// it. The actual position is interpolated via
    /// [`Host::sample_keyboard`]; this method is a coarse
    /// "should I paint anything at all" check.
    pub fn keyboard_visible(&self) -> bool {
        self.focused_input.is_some() || self.keyboard_anim.is_some()
    }

    /// Current keyboard slide value, in `[0.0, 1.0]`:
    /// - `0.0` = fully hidden (off-screen below the viewport)
    /// - `1.0` = fully visible (resting at the bottom)
    /// The renderer translates the keyboard's y-position by
    /// `(1.0 - value) * KEYBOARD_HEIGHT` so the slide is
    /// continuous across frames.
    pub fn sample_keyboard(&self, now: Instant) -> f32 {
        let Some(a) = &self.keyboard_anim else {
            return self.keyboard_value;
        };
        let dur = a.duration.as_secs_f32();
        if dur <= 0.0 {
            return a.to;
        }
        let elapsed = now.saturating_duration_since(a.started).as_secs_f32();
        if elapsed >= dur {
            return a.to;
        }
        let t = elapsed / dur;
        let eased = ease(a.easing, t);
        a.from + (a.to - a.from) * eased
    }

    /// Reconcile the keyboard's rest value with the current focus
    /// state. Call after any event that may have changed focus
    /// (pointer-down on a non-input, key Escape, …). If the rest
    /// value differs from the focus state, kick off a slide
    /// animation between them — and, on focus-in, push the
    /// scrollview up so the input sits above the keyboard's
    /// final rest position (iOS auto-scroll).
    fn sync_keyboard(&mut self) {
        let desired: f32 = if self.focused_input.is_some() { 1.0 } else { 0.0 };
        if (desired - self.keyboard_value).abs() < f32::EPSILON {
            return;
        }
        let now = Instant::now();
        let from = self.sample_keyboard(now);
        self.keyboard_value = desired;
        self.keyboard_anim = Some(KeyboardAnim {
            from,
            to: desired,
            started: now,
            duration: Duration::from_millis(KEYBOARD_ANIM_MS as u64),
            easing: Easing::EaseOut,
        });
        crate::scheduler::request_redraw();

        // On focus-in, scroll the input into view above the
        // keyboard's final position. iOS does this automatically
        // via `UIScrollView`'s content-inset; we emulate by
        // tweening the scrollview's offset.
        if desired > 0.5 {
            self.auto_scroll_focused_into_view(now);
        }
    }

    /// If the currently focused input sits below the keyboard's
    /// final top edge, scroll its enclosing `ScrollView` up by
    /// enough to put the input's bottom edge above the keyboard
    /// (plus a small margin). Re-uses the SpringBack scroll
    /// motion for a smooth eased transition.
    fn auto_scroll_focused_into_view(&mut self, now: Instant) {
        let Some(input) = self.focused_input.clone() else { return };
        let target_layout = input.borrow().layout;
        let (vw, vh) = self.viewport;
        if vw <= 0.0 || vh <= 0.0 {
            return;
        }
        let kb_top = vh - KEYBOARD_HEIGHT.min(vh);

        let backend = self.backend.borrow();
        let Some(root) = backend.roots.first() else { return };
        let root = root.clone();

        // Locate the input's absolute frame *and* its enclosing
        // scrollview (innermost) in a single pre-order walk.
        let Some((input_y, input_h, sv)) =
            find_input_in_scrollview(&backend, &root, target_layout, 0.0, 0.0, None)
        else {
            return;
        };
        let input_bottom = input_y + input_h;
        let want_visible_bottom = kb_top - KEYBOARD_INPUT_MARGIN;
        if input_bottom <= want_visible_bottom {
            // Already above the keyboard's projected position.
            return;
        }

        // Push the scrollview's offset_y down by the overlap so
        // the input rides just above the keyboard. Clamp to the
        // scrollview's max so we never request an impossible
        // offset.
        let frame = backend.layout.frame_of(sv.borrow().layout);
        let extent = scrollview_content_extent(&backend, &sv);
        let max_y = (extent.1 - frame.height).max(0.0);
        let (cur_x, cur_y) = match &sv.borrow().kind {
            NodeKind::ScrollView { offset_x, offset_y, .. } => (*offset_x, *offset_y),
            _ => return,
        };
        drop(backend);
        let delta = input_bottom - want_visible_bottom;
        let target_y = (cur_y + delta).clamp(0.0, max_y);
        if (target_y - cur_y).abs() < f32::EPSILON {
            return;
        }
        self.momentum = Some(ScrollMotion {
            scrollview: sv,
            kind: ScrollMotionKind::SpringBack {
                target: (cur_x, target_y),
            },
            last_tick: now,
        });
        crate::scheduler::request_redraw();
    }

    /// Build and mount the framework tree against the backend. The
    /// returned `Owner` is held internally so reactive scopes live
    /// until the host is dropped.
    ///
    /// We route through `runtime_core::mount`, not `render(backend,
    /// tree)`, because the author's `build_ui` closure typically
    /// declares `effect!` blocks + animation timelines whose
    /// `on_cleanup` callbacks own scheduled tasks. `mount` runs the
    /// closure INSIDE the framework's root reactive scope, so those
    /// effects register with the scope and survive past the closure's
    /// return. The `render(backend, tree)` shortcut builds the tree
    /// before opening the scope; effects created inside the closure
    /// are then owned by the local `_effect` binding, drop at the
    /// end of `build_ui`, and cancel every scheduled task with them —
    /// which is exactly what was masking the welcome example's
    /// animations on the wgpu sim.
    pub fn mount<F>(&mut self, build_ui: F)
    where
        F: FnOnce() -> Element + 'static,
    {
        if self._owner.is_some() {
            return;
        }
        self._owner = Some(fw_mount(self.backend.clone(), build_ui));
    }

    /// Drop the mounted reactive scope. All effects, AnimatedValue
    /// subscribers, and scheduled tasks created inside `build_ui`
    /// unregister via their `on_cleanup` callbacks, which is what
    /// stops the global animation clock from ticking this host's
    /// animations every frame.
    ///
    /// Idempotent (no-op if already unmounted). After unmount, the
    /// host's wgpu surface / renderer / device stay alive, so a
    /// subsequent `mount(...)` re-establishes the scene without
    /// paying the wgpu init cost.
    ///
    /// **Does NOT clear `session::REGISTRY`.** Embedded apps that
    /// use `session::animated(key, …)` keep their AVs in the
    /// thread-global registry by design — that's the whole point
    /// (hot-patch state survival). If you want a fresh-restart
    /// semantics where unmount followed by a future mount resets
    /// those AVs to their initial values, call
    /// [`runtime_core::session::clear`] yourself after `unmount`.
    /// The platform host handles (e.g. `IosHostHandle::pause`)
    /// document their chosen semantics.
    pub fn unmount(&mut self) {
        self._owner = None;
    }

    /// True iff a build_ui has been mounted (and not since unmounted).
    pub fn is_mounted(&self) -> bool {
        self._owner.is_some()
    }

    // ---------------- Read-only accessors used by the renderer ---

    pub fn backend(&self) -> &Rc<RefCell<WgpuBackend>> { &self.backend }
    pub fn text_store(&self) -> &Rc<RefCell<TextStore>> { &self.text }
    pub fn font_system(&self) -> &Rc<RefCell<FontSystem>> { &self.font_system }

    // ---------------- Async font loading (web host) -------------
    //
    // On native, `face!` fonts are baked into the binary and loaded
    // synchronously at `register_asset`. On web, `embed-font-bytes`
    // is off, so fonts are served files; the host shell fetches them
    // and feeds the bytes back through these three calls (drain URLs
    // → load each fetched buffer → invalidate so text re-shapes).

    /// Take the served-font URLs the app registered without inline
    /// bytes (web path). Empty on native. The host shell fetches each
    /// and calls [`Host::load_font_bytes`].
    pub fn take_pending_font_urls(&self) -> Vec<String> {
        self.backend.borrow_mut().drain_pending_font_urls()
    }

    /// Feed fetched font bytes into the text shaper's font database.
    /// cosmic-text's db is append-only, so this is safe any time; pair
    /// with [`Host::invalidate_text_layout`] when fonts arrive after
    /// text was already shaped.
    pub fn load_font_bytes(&self, bytes: Vec<u8>) {
        self.font_system.borrow_mut().db_mut().load_font_data(bytes);
    }

    /// Mark every shaped text node dirty so the next layout pass
    /// re-measures (and thus re-shapes) it against the current font
    /// database. Called once after late-arriving fonts load so text
    /// that fell back to the embedded default re-shapes to its real
    /// face. No-op visually if nothing was shaped yet.
    pub fn invalidate_text_layout(&self) {
        let nodes: Vec<LayoutNode> =
            self.text.borrow().buffers.keys().copied().collect();
        if nodes.is_empty() {
            return;
        }
        let mut backend = self.backend.borrow_mut();
        for node in nodes {
            backend.layout.mark_dirty(node);
        }
        drop(backend);
        crate::scheduler::request_redraw();
    }

    /// The active skin. Renderer borrows this every frame to
    /// paint widget chrome + the on-screen keyboard.
    pub fn skin(&self) -> &Rc<dyn Painter> { &self.skin }
    pub fn focused_input_layout(&self) -> Option<LayoutNode> {
        self.focused_input.as_ref().map(|n| n.borrow().layout)
    }

    /// Advance the per-frame state that the renderer samples:
    /// - the tween engine (toggle slide, theme crossfade, …)
    /// - momentum scroll (post-pan deceleration)
    /// - keyboard slide animation
    /// - caret blink (drives a continuous redraw while a
    ///   TextInput is focused so the blink stays in motion)
    ///
    /// Returns `true` if anything still needs animation — the
    /// shell's render loop should `request_redraw` so the next
    /// frame catches the next step.
    pub fn tick(&mut self) -> bool {
        // Sample the clock once per tick so every animation reads
        // the same "now." Shells no longer thread an `Instant`
        // across the API boundary — clock choice is local to the
        // render backend (here, `web_time` so wasm32 doesn't panic
        // on `std::time::Instant::now()`).
        let now = Instant::now();
        let any_anim = self.backend.borrow_mut().animator.tick(now);
        let any_presence = crate::backend_impl::tick_presence_tweens(&self.backend, now);
        let any_momentum = self.tick_momentum(now);
        // Advance navigator push/pop slides. Returns true while a
        // transition is still in flight; on completion of a Pop
        // it also fires `release_screen` + `drop_subtree` for the
        // popping subtree (deferred from the dispatcher so the
        // outgoing screen stayed mounted during the slide).
        let any_nav = crate::backend_impl::tick_nav_transitions(&self.backend, now);
        // Drawer slide-in / slide-out — runs ~250ms, drives a
        // redraw cycle while in flight so the renderer's
        // `sample_drawer_progress` can step the position.
        let any_drawer = crate::backend_impl::drawer_anim_alive(&self.backend);
        // Device-chrome clock — re-shape on minute boundaries
        // so the status bar's "H:MM" stays accurate. The
        // current-minute lookup is O(1); cheap enough to do
        // every tick. No `any_*` signal here because the
        // refresh itself produces the redraw via the standard
        // request path — the tick loop only needs to keep
        // running for in-flight animations, and we don't want
        // the clock alone holding the loop open.
        if self.chrome_clock_minute.get() != current_clock_minute() {
            self.refresh_clock_glyph();
        }
        // Tick the keyboard slide. Once duration elapses, drop
        // the anim so we stop firing redraws.
        let mut kb_alive = false;
        if let Some(a) = self.keyboard_anim {
            if now.saturating_duration_since(a.started) >= a.duration {
                self.keyboard_anim = None;
            } else {
                kb_alive = true;
            }
        }
        // Press-flash decay: a tapped key holds its highlight
        // for KEY_PRESS_FLASH_MS and is then cleared so a fresh
        // redraw repaints without the highlight.
        let mut press_alive = false;
        if let Some(started) = self.keyboard_pressed_at.get() {
            if now.saturating_duration_since(started).as_millis()
                >= crate::node::KEY_PRESS_FLASH_MS as u128
            {
                self.keyboard_pressed_label.set(None);
                self.keyboard_pressed_at.set(None);
            } else {
                press_alive = true;
            }
        }
        let caret_alive = self.focused_input.is_some();
        // Any visible spinner needs the next frame to advance its
        // rotation phase.
        let spinner_alive = self.backend.borrow().active_spinner_count > 0;
        any_anim
            || any_presence
            || any_momentum
            || kb_alive
            || caret_alive
            || spinner_alive
            || press_alive
            || any_nav
            || any_drawer
    }

    /// Re-shape the status-bar clock glyph against the current
    /// minute. Called from `tick` when the wall clock rolls over.
    /// Also pings the redraw scheduler so the on-screen value
    /// catches up on the next frame.
    fn refresh_clock_glyph(&self) {
        let label = format_clock_label();
        let mut fs = self.font_system.borrow_mut();
        let mut glyphs = self.chrome_glyphs.borrow_mut();
        if let Some(buf) = glyphs.get_mut("clock") {
            use glyphon::{Attrs, Family, Shaping};
            buf.set_text(
                &mut fs,
                &label,
                &Attrs::new().family(Family::SansSerif),
                Shaping::Advanced,
                None,
            );
            buf.shape_until_scroll(&mut fs, false);
        }
        self.chrome_clock_minute.set(current_clock_minute());
        crate::scheduler::request_redraw();
    }

    /// Flash the given keyboard key's chrome as "pressed" for
    /// the next [`crate::node::KEY_PRESS_FLASH_MS`] ms. Called
    /// from both the virtual-keyboard tap path and the physical
    /// key path (via [`label_for_key_event`]) so the visual
    /// feedback is consistent regardless of input source.
    pub(crate) fn flash_key_press(&self, label: &'static str) {
        self.keyboard_pressed_label.set(Some(label));
        self.keyboard_pressed_at.set(Some(Instant::now()));
        crate::scheduler::request_redraw();
    }

    /// Single tick of the active scroll motion. Handles both:
    /// - `Coast` — momentum-fling deceleration. Hard-clamps at
    ///   the bounds; on a boundary hit, zeros that axis's
    ///   velocity. If a clamp happened, fold a `SpringBack`
    ///   onto the very same scrollview's bound if rubber-band
    ///   overshoot is in play (typically none at coast — the
    ///   pan was the rubber-band path; coast hard-clamps).
    /// - `SpringBack` — exponential approach to the target
    ///   offset. Returns false when the gap drops below
    ///   `SCROLL_SPRINGBACK_EPSILON`.
    fn tick_momentum(&mut self, now: Instant) -> bool {
        let Some(mut m) = self.momentum.take() else { return false };
        // Cap `dt` so a paused tab or long blocking call doesn't
        // teleport the offset across the entire content.
        let dt = now.duration_since(m.last_tick).as_secs_f32().min(0.1);
        m.last_tick = now;

        match &mut m.kind {
            ScrollMotionKind::Coast { velocity } => {
                let before = match &m.scrollview.borrow().kind {
                    NodeKind::ScrollView { offset_x, offset_y, .. } => {
                        (*offset_x, *offset_y)
                    }
                    _ => return false,
                };
                self.apply_scroll(
                    &m.scrollview,
                    -velocity.0 * dt,
                    -velocity.1 * dt,
                    false,
                );
                let after = match &m.scrollview.borrow().kind {
                    NodeKind::ScrollView { offset_x, offset_y, .. } => {
                        (*offset_x, *offset_y)
                    }
                    _ => return false,
                };
                // Boundary hit → zero that axis. Without this
                // the coast keeps trying to push past a clamped
                // offset and only ends on the decay timer.
                if (after.0 - before.0).abs() < f32::EPSILON {
                    velocity.0 = 0.0;
                }
                if (after.1 - before.1).abs() < f32::EPSILON {
                    velocity.1 = 0.0;
                }
                let decay = (-SCROLL_MOMENTUM_DECAY_PER_SEC * dt).exp();
                velocity.0 *= decay;
                velocity.1 *= decay;
                let speed_sq = velocity.0 * velocity.0 + velocity.1 * velocity.1;
                let min_sq = SCROLL_MOMENTUM_MIN_VELOCITY * SCROLL_MOMENTUM_MIN_VELOCITY;
                if speed_sq < min_sq {
                    return false;
                }
            }
            ScrollMotionKind::SpringBack { target } => {
                // Exponential approach: position += (target -
                // position) * (1 - exp(-k*dt)). Framerate-stable
                // (same wall-clock rate at any tick frequency).
                let factor = 1.0 - (-SCROLL_SPRINGBACK_RATE_PER_SEC * dt).exp();
                let (cur_x, cur_y) = match &m.scrollview.borrow().kind {
                    NodeKind::ScrollView { offset_x, offset_y, .. } => {
                        (*offset_x, *offset_y)
                    }
                    _ => return false,
                };
                let dx = (target.0 - cur_x) * factor;
                let dy = (target.1 - cur_y) * factor;
                if let NodeKind::ScrollView {
                    offset_x, offset_y, ..
                } = &mut m.scrollview.borrow_mut().kind
                {
                    *offset_x = cur_x + dx;
                    *offset_y = cur_y + dy;
                }
                crate::node::fire_on_scroll(&m.scrollview);
                crate::scheduler::request_redraw();
                let gap = ((target.0 - cur_x - dx).powi(2)
                    + (target.1 - cur_y - dy).powi(2))
                .sqrt();
                if gap < SCROLL_SPRINGBACK_EPSILON {
                    // Snap to target so the final position is
                    // exactly the bound (no sub-pixel drift).
                    if let NodeKind::ScrollView {
                        offset_x, offset_y, ..
                    } = &mut m.scrollview.borrow_mut().kind
                    {
                        *offset_x = target.0;
                        *offset_y = target.1;
                    }
                    crate::node::fire_on_scroll(&m.scrollview);
                    return false;
                }
            }
        }
        self.momentum = Some(m);
        true
    }

    // ---------------- Event entry points -------------------------

    /// Pointer moved. Drives:
    /// - Slider drag → value update.
    /// - Click press whose `scrollview` is set → if cumulative
    ///   drag exceeds [`PAN_THRESHOLD`], cancel the press (clear
    ///   PRESSED state) and convert to a `Pan`. Otherwise update
    ///   the `over` flag for visual press feedback.
    /// - Pan → translate the scrollview's content by the delta
    ///   from `last`.
    /// Raw-touch dispatch — `Began`. Walks the responder chain from
    /// the deepest hit upward, invoking handlers in order until one
    /// returns `consumed: true`. The consumer is recorded in
    /// `active_touches` so subsequent `Moved` / `Ended` events for
    /// the same [`TouchId`] route directly to it (no re-walk).
    ///
    /// Returns `true` if a handler consumed the event. Callers use
    /// this to short-circuit the legacy widget-action / scroll-pan
    /// paths — touch-claimed interactions own the pointer until
    /// release.
    fn dispatch_touch_began(&mut self, ev: &PointerEvent) -> bool {
        let touch_id = TouchId(ev.id.0);
        let mut path: Vec<TouchPathEntry> = Vec::new();
        {
            let backend = self.backend.borrow();
            let Some(root) = backend.root() else { return false };
            collect_touch_path(&backend, &root, 0.0, 0.0, ev.position, &mut path);
        }
        if path.is_empty() {
            return false;
        }
        let ts = monotonic_ns();
        // Deepest-first bubble.
        for entry in path.into_iter().rev() {
            let local = (ev.position.0 - entry.origin.0, ev.position.1 - entry.origin.1);
            let te = TouchEvent {
                id: touch_id,
                phase: TouchPhase::Began,
                position: TouchPoint::new(local.0, local.1),
                window_position: TouchPoint::new(ev.position.0, ev.position.1),
                timestamp_ns: ts,
                force: None,
            };
            let response = (entry.handler)(&te);
            if response.consumed {
                self.active_touches.insert(
                    touch_id,
                    ActiveTouch {
                        node: entry.node,
                        handler: entry.handler,
                        claimed: response.claim,
                    },
                );
                return true;
            }
        }
        false
    }

    /// Raw-touch dispatch — `Moved`. Routes the event to the handler
    /// that consumed `Began` for this [`TouchId`], if any. Returns
    /// `true` when dispatched (caller skips the legacy move path).
    fn dispatch_touch_moved(&mut self, ev: &PointerEvent) -> bool {
        let touch_id = TouchId(ev.id.0);
        let Some(active) = self.active_touches.get(&touch_id) else { return false };
        // Recompute origin every event — layout can shift between
        // events (animations, scroll, dynamic content) and stale
        // origins would give the handler wrong-coordinate moves.
        let origin = absolute_origin(&self.backend.borrow(), &active.node);
        let handler = active.handler.clone();
        let te = TouchEvent {
            id: touch_id,
            phase: TouchPhase::Moved,
            position: TouchPoint::new(ev.position.0 - origin.0, ev.position.1 - origin.1),
            window_position: TouchPoint::new(ev.position.0, ev.position.1),
            timestamp_ns: monotonic_ns(),
            force: None,
        };
        let response = handler(&te);
        if response.claim {
            if let Some(active) = self.active_touches.get_mut(&touch_id) {
                active.claimed = true;
            }
        }
        true
    }

    /// Raw-touch dispatch — `Ended` / `Cancelled`. Routes the
    /// terminal event and clears the [`TouchId`] from
    /// `active_touches`. `cancelled` distinguishes a system / claim
    /// interrupt from a clean user release; recognizers branch on
    /// this to suppress fire-on-release semantics for cancelled
    /// gestures (a Cancelled tap is not a click). Returns `true`
    /// when dispatched.
    fn dispatch_touch_ended(&mut self, ev: &PointerEvent, cancelled: bool) -> bool {
        let touch_id = TouchId(ev.id.0);
        let Some(active) = self.active_touches.remove(&touch_id) else { return false };
        let origin = absolute_origin(&self.backend.borrow(), &active.node);
        let te = TouchEvent {
            id: touch_id,
            phase: if cancelled { TouchPhase::Cancelled } else { TouchPhase::Ended },
            position: TouchPoint::new(ev.position.0 - origin.0, ev.position.1 - origin.1),
            window_position: TouchPoint::new(ev.position.0, ev.position.1),
            timestamp_ns: monotonic_ns(),
            force: None,
        };
        let _ = (active.handler)(&te);
        true
    }

    pub fn pointer_move(&mut self, ev: PointerEvent) {
        self.pointer = ev.position;
        if self.dispatch_touch_moved(&ev) {
            return;
        }
        let Some(press) = self.active_press.take() else { return };
        match press {
            ActivePress::SliderDrag { node } => {
                self.update_slider_drag(&node);
                self.active_press = Some(ActivePress::SliderDrag { node });
            }
            ActivePress::Pan {
                scrollview,
                last,
                last_time,
                velocity,
                start,
            } => {
                let dx = ev.position.0 - last.0;
                let dy = ev.position.1 - last.1;
                // Drag-and-stick semantics: a finger moving down
                // (positive dy) drags the content down. Content
                // moving down = scroll offset *decreases* (we're
                // revealing what's above). So we feed `-dy`.
                // `rubber_band: true` so the user can drag past
                // the edges with damped overshoot — released
                // past-bound, the spring-back kicks in.
                self.apply_scroll(&scrollview, -dx, -dy, true);

                // Update smoothed velocity (px/sec, screen-space).
                // Clamp dt to a tiny floor so two same-frame moves
                // don't produce explosive raw velocities.
                let now = Instant::now();
                let dt = now
                    .duration_since(last_time)
                    .as_secs_f32()
                    .max(0.001);
                let raw = (dx / dt, dy / dt);
                let a = SCROLL_VELOCITY_SMOOTHING;
                let new_velocity = (
                    velocity.0 * (1.0 - a) + raw.0 * a,
                    velocity.1 * (1.0 - a) + raw.1 * a,
                );

                self.active_press = Some(ActivePress::Pan {
                    scrollview,
                    last: ev.position,
                    last_time: now,
                    velocity: new_velocity,
                    start,
                });
            }
            ActivePress::Click {
                node,
                action,
                over,
                scrollview,
                start,
            } => {
                // If the press is over a scrollview and the
                // drag has gone past the slop, upgrade to a pan
                // and cancel the click. Preserve the original
                // press position as the pan's `start` so the
                // total-motion check at `pointer_up` reflects
                // the whole gesture, not just post-upgrade
                // motion.
                if let Some(sv) = scrollview.as_ref() {
                    let dx = ev.position.0 - start.0;
                    let dy = ev.position.1 - start.1;
                    if (dx * dx + dy * dy).sqrt() > PAN_THRESHOLD {
                        set_state(&self.backend, &node, StateBits::PRESSED, false);
                        self.active_press = Some(ActivePress::Pan {
                            scrollview: sv.clone(),
                            last: ev.position,
                            last_time: Instant::now(),
                            velocity: (0.0, 0.0),
                            start,
                        });
                        return;
                    }
                }
                // Within slop: keep tracking the press, just
                // update the over-flag for visual feedback.
                let new_over = self.pointer_over_node(&node, ev.position);
                if new_over != over {
                    set_state(&self.backend, &node, StateBits::PRESSED, new_over);
                }
                self.active_press = Some(ActivePress::Click {
                    node,
                    action,
                    over: new_over,
                    scrollview,
                    start,
                });
            }
        }
    }

    /// Pointer pressed. Picks an interaction based on what's
    /// under the pointer:
    /// - Pressable / Button → capture; fire on release-inside
    ///   (drag past `PAN_THRESHOLD` upgrades to scroll-pan if a
    ///   scrollview is under the press).
    /// - Toggle           → capture; flip on release-inside.
    /// - Slider           → capture; emit initial value, start drag.
    /// - TextInput        → focus immediately (iOS keyboard model).
    /// - Empty inside SV  → start pan immediately.
    /// - nothing          → drop any active TextInput focus.
    pub fn pointer_down(&mut self, ev: PointerEvent) {
        if !matches!(ev.button, PointerButton::Primary) {
            return;
        }
        self.pointer = ev.position;
        // Tap-to-catch: a press anywhere stops any in-flight
        // momentum scroll, matching `UIScrollView`'s behavior of
        // halting deceleration on touch-down.
        self.momentum = None;

        // Raw-touch responder chain runs first. A consumed `Began`
        // captures the interaction; the legacy widget-action and
        // scroll-pan paths below are skipped for the duration of
        // this pointer's lifecycle. Once the future Rust tap /
        // long-press recognizers replace Pressable's native path,
        // this short-circuit becomes the only branch.
        if self.dispatch_touch_began(&ev) {
            return;
        }

        // On-screen keyboard intercept. While an input is
        // focused, the keyboard overlay is painted at the bottom
        // of the viewport; presses inside it never reach the app
        // tree. A tap on a key synthesizes a `KeyEvent` and
        // forwards it through the same handler that processes
        // physical keys — same code path, same behavior. Presses
        // in the keyboard gutter (gaps between keys) are
        // swallowed so the app underneath doesn't twitch.
        if self.keyboard_visible() {
            let slide = self.sample_keyboard(Instant::now());
            let (kb_rect, hit) =
                keyboard::hit_test(self.skin.as_ref(), self.viewport, slide, ev.position);
            if let Some(action) = hit {
                // Flash the on-screen key chrome so the user
                // sees their tap register. The label lookup
                // walks the active skin's rows to find the
                // matching action — same source of truth the
                // hit-test uses, so they can never disagree.
                if let Some(label) = label_for_action(self.skin.as_ref(), action) {
                    self.flash_key_press(label);
                }
                let ke = keyboard::action_to_key_event(action);
                self.key(&ke);
                return;
            }
            // Inside keyboard frame but no key → swallow.
            if let Some(rect) = kb_rect {
                let inside = ev.position.0 >= rect.0
                    && ev.position.0 <= rect.0 + rect.2
                    && ev.position.1 >= rect.1
                    && ev.position.1 <= rect.1 + rect.3;
                if inside {
                    return;
                }
            }
        }

        // Device-chrome intercept. Taps inside the status bar
        // or the home-indicator / gesture-nav strip should not
        // reach the app — those zones are platform UI on a real
        // device. We just swallow the press.
        {
            let insets = self.skin.safe_area_insets();
            let (vw, vh) = self.viewport;
            let in_status_bar = ev.position.1 < insets.top;
            let in_home_strip = ev.position.1 > vh - insets.bottom;
            if (insets.top > 0.0 && in_status_bar)
                || (insets.bottom > 0.0 && in_home_strip)
            {
                let _ = vw;
                return;
            }
        }

        // Navigator-header intercept. A tap on the back chevron
        // or a `header_left` / `header_right` icon fires its
        // action directly and short-circuits — those slots sit
        // visually on top of the screen content and shouldn't
        // fall through to the screen's hit-test even if the
        // chrome happens to overlap. Iterated in reverse so a
        // header from a sliding top-screen (added later in the
        // render walk) wins over one from the under-screen
        // beneath it.
        let header_action = {
            let hits = self.header_hits.borrow();
            hits.iter().rev().find_map(|h| {
                let (rx, ry, rw, rh) = h.rect;
                let inside = ev.position.0 >= rx
                    && ev.position.0 <= rx + rw
                    && ev.position.1 >= ry
                    && ev.position.1 <= ry + rh;
                if inside {
                    // Upgrade the Weak — if the navigator was
                    // dropped between this frame's render and the
                    // tap, the hit is stale and we skip it. In
                    // practice this can only happen for a hit
                    // collected on a frame that overlapped with
                    // a teardown; the next render writes a fresh
                    // registry that won't include it.
                    h.navigator
                        .upgrade()
                        .map(|nav| (h.action.clone(), nav))
                } else {
                    None
                }
            })
        };
        if let Some((action, _navigator)) = header_action {
            match action {
                // Stack-Back and Drawer-Close header taps had their
                // dispatch removed alongside the legacy nav substrate.
                // The new per-kind SDK paths will repopulate this hook
                // when they're wired up for the wgpu backend.
                HeaderHitAction::Back | HeaderHitAction::CloseDrawer => {}
                HeaderHitAction::HeaderLeft(cb) | HeaderHitAction::HeaderRight(cb) => {
                    cb();
                }
            }
            crate::scheduler::request_redraw();
            return;
        }

        // Hit-test in one pass for both an interactive leaf and
        // the innermost scrollview ancestor at this point.
        let (hit, scrollview_at) = {
            let backend = self.backend.borrow();
            let Some(root) = backend.root() else { return };
            let h = hit_test_node(&backend, &root, 0.0, 0.0, ev.position);
            let sv = find_scroll_view_at(&backend, &root, 0.0, 0.0, ev.position);
            (h, sv)
        };

        // Note: we don't eagerly clear `focused_input` on
        // press-down. The keyboard should stay up while the
        // user is panning or pressing buttons; only a
        // *confirmed tap* outside the keyboard's bounds should
        // dismiss it. That decision happens at `pointer_up`,
        // when we know whether the press resolved as a tap or
        // a drag.
        let Some((node, frame_x, _frame_y, frame_w, _frame_h)) = hit else {
            // No interactive hit. If there's a scrollview under
            // the pointer, drag-to-scroll starts immediately —
            // the empty-area-grabs-the-scroll iOS behavior.
            if let Some(sv) = scrollview_at {
                self.active_press = Some(ActivePress::Pan {
                    scrollview: sv,
                    last: ev.position,
                    last_time: Instant::now(),
                    velocity: (0.0, 0.0),
                    start: ev.position,
                });
            }
            return;
        };

        let action = pick_action(&node);
        match action {
            HitAction::Click(cb) => {
                set_state(&self.backend, &node, StateBits::PRESSED, true);
                self.active_press = Some(ActivePress::Click {
                    node: node.clone(),
                    action: ReleaseAction::Fire(cb),
                    over: true,
                    scrollview: scrollview_at,
                    start: ev.position,
                });
            }
            HitAction::ToggleFlip { on_change, .. } => {
                set_state(&self.backend, &node, StateBits::PRESSED, true);
                self.active_press = Some(ActivePress::Click {
                    node: node.clone(),
                    action: ReleaseAction::FlipToggle(on_change),
                    over: true,
                    scrollview: scrollview_at,
                    start: ev.position,
                });
            }
            HitAction::SliderJump {
                min,
                max,
                step,
                on_change,
            } => {
                let v = slider_value_from_pointer(
                    ev.position.0, frame_x, frame_w, min, max, step,
                );
                on_change(v);
                self.active_press = Some(ActivePress::SliderDrag { node: node.clone() });
            }
            HitAction::FocusInput => {
                self.focused_input = Some(node.clone());
            }
            HitAction::Nothing => {
                // No actionable widget, but if there's a scrollview
                // here, allow pan from this position.
                if let Some(sv) = scrollview_at {
                    self.active_press = Some(ActivePress::Pan {
                        scrollview: sv,
                        last: ev.position,
                        last_time: Instant::now(),
                        velocity: (0.0, 0.0),
                        start: ev.position,
                    });
                }
            }
        }

        // FocusInput may have set / replaced `focused_input`.
        // Reconcile the keyboard slide.
        self.sync_keyboard();
    }

    /// Pointer released. Ends the active interaction. For a click
    /// press, fires the release action only if the pointer is
    /// still over the captured node (the iOS "drag-off-to-cancel"
    /// model).
    pub fn pointer_up(&mut self, ev: PointerEvent) {
        if self.dispatch_touch_ended(&ev, false) {
            return;
        }
        let Some(press) = self.active_press.take() else { return };
        // Decide whether *this* release should dismiss the
        // on-screen keyboard. iOS rule (as restated by the user):
        // pans never dismiss; only confirmed taps outside the
        // keyboard's bounds dismiss. We compute `is_tap_outside`
        // for each release flavor and clear `focused_input` after
        // the press is finalized.
        let mut is_tap_outside = false;
        match press {
            ActivePress::SliderDrag { .. } => {
                // Slider drag has no fire-on-release semantics; the
                // last `pointer_move` already pushed the final
                // value. Treat as not-a-dismiss — user was
                // interacting with a control.
            }
            ActivePress::Pan {
                scrollview,
                last_time,
                velocity,
                start,
                ..
            } => {
                // Compute total motion since press-down. A short
                // motion means the user tapped (on empty area or
                // through a button-then-drag-cancel that never
                // moved past `PAN_THRESHOLD` — that path doesn't
                // upgrade to Pan, so we only see "real" pans here
                // and pans that started immediately on empty area
                // without any drag yet).
                let dx_total = ev.position.0 - start.0;
                let dy_total = ev.position.1 - start.1;
                let total_motion = (dx_total * dx_total + dy_total * dy_total).sqrt();
                let was_pan = total_motion >= PAN_THRESHOLD;

                if was_pan {
                    // Real pan: maybe hand off to momentum, don't
                    // dismiss the keyboard.
                    let now = Instant::now();
                    let stale = now.duration_since(last_time).as_millis()
                        > SCROLL_MOMENTUM_STALE_MS;
                    let speed_sq = velocity.0 * velocity.0 + velocity.1 * velocity.1;
                    let min_sq =
                        SCROLL_MOMENTUM_MIN_VELOCITY * SCROLL_MOMENTUM_MIN_VELOCITY;
                    if !stale && speed_sq > min_sq {
                        self.momentum = Some(ScrollMotion {
                            scrollview: scrollview.clone(),
                            kind: ScrollMotionKind::Coast { velocity },
                            last_tick: now,
                        });
                        crate::scheduler::request_redraw();
                    }
                    // If the user let go past the rubber-band
                    // overshoot region, replace any coast we
                    // just set up with a spring-back so the
                    // scrollview snaps to its bound. The
                    // spring-back path wins over coast when both
                    // could apply — momentum past a wall would
                    // just buzz.
                    self.maybe_spring_back(&scrollview, now);
                } else {
                    // Tap on empty area inside the scrollview —
                    // treat as a dismiss gesture.
                    is_tap_outside = true;
                }
            }
            ActivePress::Click { node, action, over, .. } => {
                set_state(&self.backend, &node, StateBits::PRESSED, false);
                if over {
                    fire_release(&node, action);
                    // Tapped a non-input widget (Click captures
                    // Pressable / Button / Toggle — not Slider,
                    // and TextInput follows the FocusInput path
                    // which never sets a Click press). Confirmed
                    // tap outside the keyboard → dismiss.
                    is_tap_outside = true;
                }
                // Release-outside the captured node (over=false)
                // is a cancelled tap. We don't fire and we don't
                // dismiss — the user dragged off the button,
                // they didn't commit.
            }
        }
        if is_tap_outside && self.focused_input.is_some() {
            self.focused_input = None;
            self.sync_keyboard();
        }
    }

    /// Scroll event (wheel / two-finger pan). Routed to the
    /// innermost `ScrollView` under `ev.position`. If no scroll
    /// container hits, the event is dropped — there's no
    /// document-level scroll (the root frame is always the
    /// viewport size).
    ///
    /// `ev.delta` carries the platform's wheel delta. We pass it
    /// through `apply_scroll` with a positive sign so wheel-down
    /// scrolls down — the "reverse" convention requested at the
    /// example level. Flip the sign here if you want
    /// natural-scrolling later.
    pub fn scroll(&mut self, ev: ScrollEvent) {
        let hit = {
            let backend = self.backend.borrow();
            let Some(root) = backend.root() else { return };
            find_scroll_view_at(&backend, &root, 0.0, 0.0, ev.position)
        };
        let Some(node) = hit else { return };
        // Vertical scrollviews accept both axes of wheel input
        // because mice with only a vertical wheel still need to
        // scroll — winit emits the user's twist on `.1`.
        let (dx, dy) = (ev.delta.0, ev.delta.1);
        // Wheel doesn't get rubber-band: there's no "release"
        // event to spring back from, and trackpad inertia
        // already mimics a fling visually.
        self.apply_scroll(&node, dx, dy, false);
    }

    /// Apply a delta in *content* space to a scrollview's offset.
    /// `rubber_band = true` lets the offset overshoot the
    /// `[0, max_offset]` range with diminishing returns — used
    /// during active pan. `false` hard-clamps — used by wheel
    /// and momentum-coast (which would otherwise spin past the
    /// edge forever).
    fn apply_scroll(&self, sv: &WgpuNode, dx: f32, dy: f32, rubber_band: bool) {
        let (max_x, max_y, span_x, span_y) = {
            let backend = self.backend.borrow();
            let frame = backend.layout.frame_of(sv.borrow().layout);
            let extent = scrollview_content_extent(&backend, sv);
            (
                (extent.0 - frame.width).max(0.0),
                (extent.1 - frame.height).max(0.0),
                frame.width.max(1.0),
                frame.height.max(1.0),
            )
        };
        if let NodeKind::ScrollView {
            horizontal,
            offset_x,
            offset_y,
            ..
        } = &mut sv.borrow_mut().kind
        {
            if *horizontal {
                let raw = *offset_x + dx + dy;
                *offset_x = if rubber_band {
                    rubberband(raw, 0.0, max_x, span_x)
                } else {
                    raw.clamp(0.0, max_x)
                };
            } else {
                let raw = *offset_y + dy;
                *offset_y = if rubber_band {
                    rubberband(raw, 0.0, max_y, span_y)
                } else {
                    raw.clamp(0.0, max_y)
                };
            }
        }
        crate::node::fire_on_scroll(sv);
        crate::scheduler::request_redraw();
    }

    /// Inspect the scrollview's current offset and, if it's past
    /// either bound, install a spring-back motion that animates
    /// it to the nearest valid offset. Replaces any existing
    /// motion on the same scrollview — spring-back wins over
    /// coast at the wall (otherwise momentum would keep pressing
    /// against a clamped offset for the duration of its decay).
    fn maybe_spring_back(&mut self, sv: &WgpuNode, now: Instant) {
        let (max_x, max_y, cur_x, cur_y) = {
            let backend = self.backend.borrow();
            let frame = backend.layout.frame_of(sv.borrow().layout);
            let extent = scrollview_content_extent(&backend, sv);
            let max_x = (extent.0 - frame.width).max(0.0);
            let max_y = (extent.1 - frame.height).max(0.0);
            let (cx, cy) = match &sv.borrow().kind {
                NodeKind::ScrollView { offset_x, offset_y, .. } => (*offset_x, *offset_y),
                _ => return,
            };
            (max_x, max_y, cx, cy)
        };
        let target_x = cur_x.clamp(0.0, max_x);
        let target_y = cur_y.clamp(0.0, max_y);
        if (target_x - cur_x).abs() < f32::EPSILON
            && (target_y - cur_y).abs() < f32::EPSILON
        {
            return;
        }
        self.momentum = Some(ScrollMotion {
            scrollview: sv.clone(),
            kind: ScrollMotionKind::SpringBack {
                target: (target_x, target_y),
            },
            last_tick: now,
        });
        crate::scheduler::request_redraw();
    }

    /// Cancel an in-progress pointer interaction (window lost
    /// focus, OS-canceled touch). Clears PRESSED state without
    /// firing the release action.
    pub fn pointer_cancel(&mut self) {
        match self.active_press.take() {
            Some(ActivePress::Click { node, .. }) => {
                set_state(&self.backend, &node, StateBits::PRESSED, false);
            }
            _ => {}
        }
        // Slider / Pan: nothing to clean up beyond clearing the
        // active press (the `take()` above already did that for
        // any kind).

        // Drain every in-flight raw-touch interaction with a
        // `Cancelled` phase event. Position uses the last-known
        // pointer — handlers shouldn't be coordinate-sensitive on
        // cancellation (recognizer-reset only).
        if !self.active_touches.is_empty() {
            let ts = monotonic_ns();
            let pointer = self.pointer;
            let backend_ref = self.backend.clone();
            let active: Vec<(TouchId, ActiveTouch)> = self.active_touches.drain().collect();
            for (id, at) in active {
                let origin = absolute_origin(&backend_ref.borrow(), &at.node);
                let te = TouchEvent {
                    id,
                    phase: TouchPhase::Cancelled,
                    position: TouchPoint::new(pointer.0 - origin.0, pointer.1 - origin.1),
                    window_position: TouchPoint::new(pointer.0, pointer.1),
                    timestamp_ns: ts,
                    force: None,
                };
                let _ = (at.handler)(&te);
            }
        }
    }

    /// Hit-test `point` against `node`'s current absolute frame.
    /// Returns `true` if the deepest interactive hit at `point` is
    /// the same node — i.e. the user hasn't dragged onto a
    /// sibling. This is what flips the PRESSED state during a
    /// press-and-drag.
    fn pointer_over_node(&self, node: &WgpuNode, point: (f32, f32)) -> bool {
        let backend = self.backend.borrow();
        let Some(root) = backend.root() else { return false };
        let Some((hit, _, _, _, _)) = hit_test_node(&backend, &root, 0.0, 0.0, point) else {
            return false;
        };
        Rc::ptr_eq(&hit, node)
    }

    /// Process a key event against the currently focused input.
    /// No-op if nothing is focused. Returns `true` if the event
    /// produced a state change (useful for shells that want to
    /// decide whether to redraw).
    pub fn key(&mut self, event: &KeyEvent) -> bool {
        if !event.pressed {
            return false;
        }
        // Flash the corresponding on-screen key (if any) so
        // physical-keyboard typing animates the virtual keys
        // too. Resolves the label by walking the active skin's
        // rows — same source of truth the tap path uses.
        if self.keyboard_visible() {
            if let Some(label) = label_for_key_event(self.skin.as_ref(), event) {
                self.flash_key_press(label);
            }
        }
        let Some(node) = self.focused_input.clone() else { return false };

        let (current_value, on_change) = {
            let data = node.borrow();
            match &data.kind {
                NodeKind::TextInput { value, on_change, .. }
                | NodeKind::TextArea { value, on_change, .. } => {
                    (value.clone(), on_change.clone())
                }
                _ => return false,
            }
        };

        let new_value: Option<String> = match event.key {
            Key::Character => event.text.as_ref().and_then(|t| {
                let filtered: String = t.chars().filter(|c| !c.is_control()).collect();
                if filtered.is_empty() {
                    None
                } else {
                    let mut s = current_value.clone();
                    s.push_str(&filtered);
                    Some(s)
                }
            }),
            Key::Backspace => {
                if current_value.is_empty() {
                    None
                } else {
                    let mut s = current_value.clone();
                    s.pop();
                    Some(s)
                }
            }
            Key::Escape => {
                self.focused_input = None;
                self.sync_keyboard();
                return true;
            }
            // Cursor-movement keys are no-ops in the MVP — caret
            // is always at end-of-value. Shells should still pass
            // them through so future arrow / Home / End support
            // doesn't need a re-plumbing.
            Key::ArrowLeft
            | Key::ArrowRight
            | Key::ArrowUp
            | Key::ArrowDown
            | Key::Home
            | Key::End
            | Key::Delete
            | Key::Enter
            | Key::Tab
            | Key::Unknown => None,
        };

        if let Some(v) = new_value {
            on_change(v);
            true
        } else {
            false
        }
    }

    // ---------------- Internal -----------------------------------

    fn update_slider_drag(&self, node: &WgpuNode) {
        let backend = self.backend.borrow();
        let layout = node.borrow().layout;
        let frame = backend.layout.frame_of(layout);
        let frame_x = absolute_x(&backend, node);
        let frame_w = frame.width;
        let (min, max, step, on_change) = match &node.borrow().kind {
            NodeKind::Slider {
                min,
                max,
                step,
                on_change,
                ..
            } => (*min, *max, *step, on_change.clone()),
            _ => return,
        };
        drop(backend);
        let v = slider_value_from_pointer(self.pointer.0, frame_x, frame_w, min, max, step);
        on_change(v);
    }
}

/// Time (ms) to crossfade the Button press visual on press
/// down. Short and snappy — `UIKit`'s default highlight is
/// near-instant; we go just slow enough to feel intentional.
const BUTTON_PRESS_DOWN_MS: u32 = 80;
/// Time (ms) to crossfade the Button press visual on release.
/// Longer than the down-crossfade so the highlight has a
/// satisfying decay — Material's recommendation for state-layer
/// fade-out is ~150 ms.
const BUTTON_PRESS_UP_MS: u32 = 150;

/// Push a state bit toggle through the node's framework-supplied
/// setter, if one is installed (no-op otherwise — the setter is
/// only attached when the stylesheet has state overlays).
///
/// For `PRESSED` on a `Button`, also drive the
/// [`AnimProperty::PressProgress`] tween so the skin's
/// `button_press_visual(t)` paint hook sees a smooth 0→1 (down)
/// or 1→0 (up) instead of a snap. The animator's `tick` keeps
/// the host re-rendering until the tween settles.
fn set_state(
    backend: &Rc<RefCell<WgpuBackend>>,
    node: &WgpuNode,
    bit: StateBits,
    on: bool,
) {
    let setter = node.borrow().state_setter.clone();
    if let Some(setter) = setter {
        setter(bit, on);
    }
    if bit == StateBits::PRESSED
        && matches!(node.borrow().kind, NodeKind::Button { .. })
    {
        let layout = node.borrow().layout;
        let (target, duration) = if on {
            (1.0, BUTTON_PRESS_DOWN_MS)
        } else {
            (0.0, BUTTON_PRESS_UP_MS)
        };
        backend.borrow_mut().animator.animate(
            crate::animation::TweenKey::new(layout, crate::animation::AnimProperty::PressProgress),
            target,
            0.0,
            duration,
            runtime_core::Easing::EaseOut,
            Instant::now(),
        );
        crate::scheduler::request_redraw();
    }
}

/// Fire the captured release action for a click press. Toggle
/// reads the node's *current* value at fire time so an external
/// signal flip during the press doesn't cause a wrong-direction
/// toggle.
fn fire_release(node: &WgpuNode, action: ReleaseAction) {
    match action {
        ReleaseAction::Fire(cb) => cb(),
        ReleaseAction::FlipToggle(cb) => {
            let current = match &node.borrow().kind {
                NodeKind::Toggle { value, .. } => *value,
                _ => return,
            };
            cb(!current);
        }
    }
}

// ---------------------------------------------------------------------------
// Free-standing helpers shared with the renderer walk in `app.rs`.
// ---------------------------------------------------------------------------

enum HitAction {
    Click(Rc<dyn Fn()>),
    ToggleFlip {
        on_change: Rc<dyn Fn(bool)>,
    },
    SliderJump {
        min: f32,
        max: f32,
        step: Option<f32>,
        on_change: Rc<dyn Fn(f32)>,
    },
    FocusInput,
    Nothing,
}

fn pick_action(node: &WgpuNode) -> HitAction {
    match &node.borrow().kind {
        NodeKind::Pressable { on_click } => HitAction::Click(on_click.clone()),
        NodeKind::Button { on_click, .. } => HitAction::Click(on_click.clone()),
        NodeKind::Toggle { on_change, .. } => HitAction::ToggleFlip {
            on_change: on_change.clone(),
        },
        NodeKind::Slider {
            min,
            max,
            step,
            on_change,
            ..
        } => HitAction::SliderJump {
            min: *min,
            max: *max,
            step: *step,
            on_change: on_change.clone(),
        },
        NodeKind::TextInput { .. } | NodeKind::TextArea { .. } => HitAction::FocusInput,
        _ => HitAction::Nothing,
    }
}

/// Returns the innermost `ScrollView` containing `point`, or
/// `None`. Walks the tree the same way the renderer does — when
/// it enters a `ScrollView`, children are descended through
/// `point - offset` so a click at the user's cursor lines up
/// with the visually scrolled content.
/// Return the children of `node` that should receive pointer
/// events at this moment. For non-navigator nodes that's just
/// the full `children` Vec, in declaration order; for navigators
/// it's only the visible subset (stack: top of stack; tabs:
/// active tab; drawer: active body + sidebar-when-open).
///
/// Centralised here so every input traversal (hit_test,
/// scrollview lookup, touch responder chain) agrees on which
/// subtree is reachable. Without one source of truth, a tap
/// could miss a button on the visible screen yet still fire a
/// `touch_handler` on a covered one — the same propagation bug
/// that let spam-taps stack duplicate pushes through every
/// stacked home screen.
fn visible_children_for_input(node: &WgpuNode) -> Vec<WgpuNode> {
    match &node.borrow().kind {
        NodeKind::Navigator { .. } => {
            node.borrow().children.last().cloned().into_iter().collect()
        }
        NodeKind::TabNavigator { active_tab, .. } => {
            let idx = active_tab.get();
            node.borrow().children.get(idx).cloned().into_iter().collect()
        }
        NodeKind::DrawerNavigator { active_screen, sidebar, is_open, .. } => {
            // `active_screen` indexes into the *body* list — the
            // children vec with the sidebar filtered out. The
            // renderer applies the same filter (see
            // `Renderer::walk`'s DrawerNavigator arm), so without
            // matching it here we'd index past the wrong slot:
            // for a drawer that mounted a second body via
            // `Select`, children is `[body_a, sidebar, body_b]`
            // and `active_screen == 1` means `body_b` to the
            // renderer but would point at the sidebar here.
            let sidebar_rc = sidebar.borrow().clone();
            let body: Vec<WgpuNode> = node
                .borrow()
                .children
                .iter()
                .filter(|c| !sidebar_rc.as_ref().is_some_and(|s| Rc::ptr_eq(s, c)))
                .cloned()
                .collect();
            let idx = active_screen.get();
            let mut out: Vec<WgpuNode> = body.get(idx).cloned().into_iter().collect();
            // The drawer's tap-outside-to-close scrim hit-area
            // is dispatched separately via `header_hits`
            // (CloseDrawer); only the sidebar's own interactive
            // content needs to be in the walk here.
            if is_open.get() {
                if let Some(sb) = sidebar_rc {
                    out.push(sb);
                }
            }
            out
        }
        _ => node.borrow().children.clone(),
    }
}

fn find_scroll_view_at(
    backend: &WgpuBackend,
    node: &WgpuNode,
    parent_x: f32,
    parent_y: f32,
    point: (f32, f32),
) -> Option<WgpuNode> {
    let frame = backend.layout.frame_of(node.borrow().layout);
    let x = parent_x + frame.x;
    let y = parent_y + frame.y;
    let inside = point.0 >= x
        && point.0 <= x + frame.width
        && point.1 >= y
        && point.1 <= y + frame.height;
    if !inside {
        return None;
    }
    // Descend children first — favor the innermost scrollview
    // when nested.
    let (child_origin, this_is_scroll) = match &node.borrow().kind {
        NodeKind::ScrollView { offset_x, offset_y, .. } => ((x - *offset_x, y - *offset_y), true),
        _ => ((x, y), false),
    };
    let children = visible_children_for_input(node);
    for child in children.iter().rev() {
        if let Some(hit) = find_scroll_view_at(backend, child, child_origin.0, child_origin.1, point) {
            return Some(hit);
        }
    }
    if this_is_scroll {
        Some(node.clone())
    } else {
        None
    }
}

/// Returns the deepest interactive node containing `point` along
/// with its absolute *visual* frame (x, y, w, h), or None.
///
/// Hit-test uses an *inflated* rectangle for small native widgets:
/// any interactive node whose visual bounds are below the iOS
/// 44pt minimum touch target is expanded outward symmetrically.
/// The reported frame is still the visual one — callers that need
/// the widget's actual pixel rect (slider drag value mapping, for
/// instance) get the truth, while the touch zone is generous.
///
/// Containers (a plain `View`) are walked but never hit-tested
/// inflated — only the leaf-interactive node gets the bonus.
pub(crate) fn hit_test_node(
    backend: &WgpuBackend,
    node: &WgpuNode,
    parent_x: f32,
    parent_y: f32,
    point: (f32, f32),
) -> Option<(WgpuNode, f32, f32, f32, f32)> {
    let frame = backend.layout.frame_of(node.borrow().layout);
    let x = parent_x + frame.x;
    let y = parent_y + frame.y;
    let w = frame.width;
    let h = frame.height;

    // For interactive leaves, expand the touch rect outward to at
    // least the iOS minimum. For containers, use the literal frame
    // so we still recurse into children at their proper bounds.
    let kind_matches = matches!(
        &node.borrow().kind,
        NodeKind::Pressable { .. }
            | NodeKind::Button { .. }
            | NodeKind::Toggle { .. }
            | NodeKind::Slider { .. }
            | NodeKind::TextInput { .. }
            | NodeKind::TextArea { .. }
    );
    // Two components per axis:
    //   - the `44pt - size` correction so tiny controls grow to
    //     the iOS minimum touch target,
    //   - a constant `HIT_SLOP` on top, so even controls that are
    //     already wider than 44pt accept a few pixels of overshoot
    //     past their visual edge (UIKit apps do this routinely via
    //     `hitTest` overrides).
    let inflate_w = if kind_matches {
        ((IOS_MIN_HIT_TARGET - w) * 0.5).max(0.0) + HIT_SLOP
    } else {
        0.0
    };
    let inflate_h = if kind_matches {
        ((IOS_MIN_HIT_TARGET - h) * 0.5).max(0.0) + HIT_SLOP
    } else {
        0.0
    };
    let hx0 = x - inflate_w;
    let hy0 = y - inflate_h;
    let hx1 = x + w + inflate_w;
    let hy1 = y + h + inflate_h;
    let inside = point.0 >= hx0 && point.0 <= hx1 && point.1 >= hy0 && point.1 <= hy1;
    if !inside {
        return None;
    }

    // When entering a ScrollView, descend with the scroll offset
    // applied so children's hit rects line up with what the user
    // sees on screen.
    let (child_origin_x, child_origin_y) = match &node.borrow().kind {
        NodeKind::ScrollView { offset_x, offset_y, .. } => (x - *offset_x, y - *offset_y),
        _ => (x, y),
    };

    // Children first (deeper hits beat shallow ones). For
    // navigator kinds we restrict the walk to the children that
    // are actually visible — every other child sits *underneath*
    // a fully-occluding screen, so a tap that misses the visible
    // screen's interactive content must NOT fall through to the
    // covered one. Without this gate, spam-tapping a button on
    // the home screen still mounts duplicates of the pushed
    // screen because home is hit-testable through whatever was
    // pushed on top of it.
    let visible_children = visible_children_for_input(node);
    for child in visible_children.iter().rev() {
        if let Some(hit) = hit_test_node(backend, child, child_origin_x, child_origin_y, point) {
            // A child reported its hit in *scrolled* coordinates;
            // they're still valid absolute logical-px positions for
            // the slider drag math etc., so just propagate.
            return Some(hit);
        }
    }

    if kind_matches {
        // Return the visual frame, not the inflated one. The
        // slider value mapping uses these coordinates to map
        // pointer x → value, and we want it relative to the
        // *visible* track.
        Some((node.clone(), x, y, w, h))
    } else {
        None
    }
}

/// One entry on the touch-responder path. Records the node carrying a
/// `touch_handler` and its absolute origin (top-left, window-relative)
/// at the moment of hit-test. The dispatcher subtracts `origin` from
/// the window pointer position to produce node-local coordinates.
pub(crate) struct TouchPathEntry {
    pub node: WgpuNode,
    pub handler: TouchHandler,
    pub origin: (f32, f32),
}

/// Walks from `root` down to the leaf at `point`, collecting every
/// node along the path whose `touch_handler` is `Some`. Returned in
/// **root-first** order — the dispatcher iterates in reverse for the
/// deepest-first responder-chain bubble.
///
/// Returns an empty vec if `point` is outside the root or no ancestor
/// on the path subscribed. The "deepest hit" boundary matches
/// `hit_test_node`'s frame logic but without the interactive-leaf
/// inflation — touch handlers fire against their literal visual
/// rect, slop is the recognizer's job (see future `TapRecognizer`).
pub(crate) fn collect_touch_path(
    backend: &WgpuBackend,
    node: &WgpuNode,
    parent_x: f32,
    parent_y: f32,
    point: (f32, f32),
    out: &mut Vec<TouchPathEntry>,
) -> bool {
    let frame = backend.layout.frame_of(node.borrow().layout);
    let x = parent_x + frame.x;
    let y = parent_y + frame.y;
    let w = frame.width;
    let h = frame.height;
    let inside = point.0 >= x && point.0 <= x + w && point.1 >= y && point.1 <= y + h;
    if !inside {
        return false;
    }

    // Record self before recursing so the returned vec is root-first.
    // The handler clone is the only allocation per path step — fine on
    // touch-down which happens at human cadence.
    let self_entry = node
        .borrow()
        .touch_handler
        .as_ref()
        .map(|h| TouchPathEntry { node: node.clone(), handler: h.clone(), origin: (x, y) });
    if let Some(entry) = self_entry {
        out.push(entry);
    }

    // Descend with ScrollView offset applied so children's frames
    // line up with what's visible — mirrors `hit_test_node`.
    let (child_origin_x, child_origin_y) = match &node.borrow().kind {
        NodeKind::ScrollView { offset_x, offset_y, .. } => (x - *offset_x, y - *offset_y),
        _ => (x, y),
    };
    let children = visible_children_for_input(node);
    for child in children.iter().rev() {
        if collect_touch_path(backend, child, child_origin_x, child_origin_y, point, out) {
            return true;
        }
    }
    true
}

/// Process-monotonic timestamp in nanoseconds. Suitable for
/// computing velocity / inter-event durations in [`TouchEvent`];
/// the epoch is arbitrary (lazily fixed at first call). Not a
/// wall-clock time.
fn monotonic_ns() -> u64 {
    use std::sync::OnceLock;
    static EPOCH: OnceLock<Instant> = OnceLock::new();
    let epoch = EPOCH.get_or_init(Instant::now);
    Instant::now().duration_since(*epoch).as_nanos() as u64
}

/// Absolute (window-relative) origin of `node`'s top-left corner.
/// Walks the layout tree from each root; returns `(0, 0)` if the
/// node isn't reachable (which would be a bug — touch handlers
/// only fire on mounted nodes). Used by the touch dispatcher to
/// translate window-space pointer positions into node-local
/// coordinates per event, so layout shifts between events don't
/// hand the handler stale-origin moves.
pub(crate) fn absolute_origin(backend: &WgpuBackend, node: &WgpuNode) -> (f32, f32) {
    let target = node.borrow().layout;
    for root in &backend.roots {
        let mut out = (0.0, 0.0);
        if walk_for_origin(backend, root, target, (0.0, 0.0), &mut out) {
            return out;
        }
    }
    (0.0, 0.0)
}

fn walk_for_origin(
    backend: &WgpuBackend,
    node: &WgpuNode,
    target: LayoutNode,
    parent: (f32, f32),
    out: &mut (f32, f32),
) -> bool {
    let frame = backend.layout.frame_of(node.borrow().layout);
    let here = (parent.0 + frame.x, parent.1 + frame.y);
    if node.borrow().layout == target {
        *out = here;
        return true;
    }
    // Same scroll-offset descent rule as hit-test so coordinates
    // line up with what the user sees on screen.
    let (child_parent_x, child_parent_y) = match &node.borrow().kind {
        NodeKind::ScrollView { offset_x, offset_y, .. } => (here.0 - *offset_x, here.1 - *offset_y),
        _ => here,
    };
    let children: Vec<WgpuNode> = node.borrow().children.clone();
    for child in &children {
        if walk_for_origin(backend, child, target, (child_parent_x, child_parent_y), out) {
            return true;
        }
    }
    false
}

/// Sum the layout x-offsets from the root to `node`. Used by
/// slider drag — Taffy frames are parent-relative.
fn absolute_x(backend: &WgpuBackend, node: &WgpuNode) -> f32 {
    let target = node.borrow().layout;
    let Some(root) = backend.roots.first() else { return 0.0 };
    let mut accum = 0.0;
    if walk_for_x(backend, root, target, 0.0, &mut accum) {
        accum
    } else {
        0.0
    }
}

fn walk_for_x(
    backend: &WgpuBackend,
    node: &WgpuNode,
    target: LayoutNode,
    parent_x: f32,
    out: &mut f32,
) -> bool {
    let frame = backend.layout.frame_of(node.borrow().layout);
    let x = parent_x + frame.x;
    if node.borrow().layout == target {
        *out = x;
        return true;
    }
    let children: Vec<WgpuNode> = node.borrow().children.clone();
    for child in &children {
        if walk_for_x(backend, child, target, x, out) {
            return true;
        }
    }
    false
}

/// Total extent (width, height) of `sv`'s laid-out children in
/// the scrollview's local content space. Used both for clamping
/// scroll offsets and for sizing the scrollbar thumb. Computed
/// from each direct child's Taffy frame.
pub(crate) fn scrollview_content_extent(
    backend: &WgpuBackend,
    sv: &WgpuNode,
) -> (f32, f32) {
    let children = sv.borrow().children.clone();
    let mut max_x: f32 = 0.0;
    let mut max_y: f32 = 0.0;
    for c in &children {
        let f = backend.layout.frame_of(c.borrow().layout);
        max_x = max_x.max(f.x + f.width);
        max_y = max_y.max(f.y + f.height);
    }
    (max_x, max_y)
}

/// Locate the absolute `(y, height)` of the node with layout id
/// `target`, plus the innermost `ScrollView` ancestor enclosing
/// it. Returns `None` if the target isn't in the tree. Single
/// pre-order walk, applies scroll offsets so the returned y is
/// the user's-eye position (matching `walk` in the renderer).
fn find_input_in_scrollview(
    backend: &WgpuBackend,
    node: &WgpuNode,
    target: LayoutNode,
    parent_x: f32,
    parent_y: f32,
    enclosing_sv: Option<WgpuNode>,
) -> Option<(f32, f32, WgpuNode)> {
    let frame = backend.layout.frame_of(node.borrow().layout);
    let x = parent_x + frame.x;
    let y = parent_y + frame.y;

    if node.borrow().layout == target {
        return enclosing_sv.map(|sv| (y, frame.height, sv));
    }

    // Descend with scroll offset applied + scrollview tracking.
    let (child_x, child_y, new_enclosing) = match &node.borrow().kind {
        NodeKind::ScrollView { offset_x, offset_y, .. } => {
            (x - *offset_x, y - *offset_y, Some(node.clone()))
        }
        _ => (x, y, enclosing_sv),
    };
    let children = node.borrow().children.clone();
    for c in &children {
        if let Some(found) = find_input_in_scrollview(
            backend,
            c,
            target,
            child_x,
            child_y,
            new_enclosing.clone(),
        ) {
            return Some(found);
        }
    }
    None
}

/// iOS rubber-band saturation. `v` is the raw offset, `[min,
/// max]` the valid range, `span` the scrollview's main-axis
/// size. Inside the range, returns `v` unchanged. Past either
/// bound, damps the overshoot with `overshoot / (1 + overshoot
/// * resistance / span)` — a hyperbolic saturation that lets
/// the user drag arbitrarily far while the visible motion
/// shrinks asymptotically.
fn rubberband(v: f32, min: f32, max: f32, span: f32) -> f32 {
    if v >= min && v <= max {
        return v;
    }
    let (bound, overshoot, sign) = if v < min {
        (min, min - v, -1.0)
    } else {
        (max, v - max, 1.0)
    };
    let damped = overshoot / (1.0 + overshoot * SCROLL_RUBBERBAND_RESISTANCE / span.max(1.0));
    bound + sign * damped
}

/// Tiny easing-curve helper for the keyboard slide. The full
/// animation engine has the same suite (`animation::Animator`),
/// but the keyboard's animation isn't node-bound so it lives on
/// `Host` directly; this is the matching curve evaluator.
fn ease(easing: Easing, t: f32) -> f32 {
    let t = t.clamp(0.0, 1.0);
    match easing {
        Easing::Linear => t,
        Easing::EaseIn => t * t,
        Easing::EaseOut => {
            let inv = 1.0 - t;
            1.0 - inv * inv * inv
        }
        Easing::EaseInOut => {
            if t < 0.5 {
                2.0 * t * t
            } else {
                let f = -2.0 * t + 2.0;
                1.0 - f * f * 0.5
            }
        }
        // CSS default ≈ ease-out cubic; close enough here.
        Easing::Ease => {
            let inv = 1.0 - t;
            1.0 - inv * inv * inv
        }
        Easing::CubicBezier(_, _, _, _) => t,
    }
}

// ---------------------------------------------------------------------------
// EventSink — the formal contract a native shell drives this
// render backend through. Every method delegates to the
// matching inherent fn on `Host` (the bodies above) so we keep
// one source of truth for the actual behavior.
// ---------------------------------------------------------------------------

impl EventSink for Host {
    fn pointer_down(&mut self, ev: PointerEvent) {
        Host::pointer_down(self, ev)
    }
    fn pointer_move(&mut self, ev: PointerEvent) {
        Host::pointer_move(self, ev)
    }
    fn pointer_up(&mut self, ev: PointerEvent) {
        Host::pointer_up(self, ev)
    }
    fn pointer_cancel(&mut self) {
        Host::pointer_cancel(self)
    }
    fn scroll(&mut self, ev: ScrollEvent) {
        Host::scroll(self, ev)
    }
    fn key(&mut self, ev: &KeyEvent) -> bool {
        Host::key(self, ev)
    }
    fn set_viewport(&mut self, w: f32, h: f32) {
        Host::set_viewport(self, w, h)
    }
    fn tick(&mut self) -> bool {
        Host::tick(self)
    }
}

/// Map a pointer x position to a slider value, honoring optional
/// step quantization.
fn slider_value_from_pointer(
    point_x: f32,
    frame_x: f32,
    frame_w: f32,
    min: f32,
    max: f32,
    step: Option<f32>,
) -> f32 {
    let inset = SLIDER_THUMB_SIZE * 0.5;
    let track_x = frame_x + inset;
    let track_w = (frame_w - inset * 2.0).max(1.0);
    let t = ((point_x - track_x) / track_w).clamp(0.0, 1.0);
    let raw = min + t * (max - min);
    match step {
        Some(s) if s > 0.0 => {
            let steps = ((raw - min) / s).round();
            (min + steps * s).clamp(min, max)
        }
        _ => raw,
    }
}

/// Find the on-screen-keyboard label matching `action`. Walks
/// the active skin's row data so iOS / Android keyboards with
/// different layouts each route their own keys correctly.
fn label_for_action(
    skin: &dyn Painter,
    action: keyboard::KeyAction,
) -> Option<&'static str> {
    skin.keyboard_rows()
        .into_iter()
        .flatten()
        .find(|spec| spec.action == action)
        .map(|spec| spec.label)
}

/// Same lookup, but starting from a physical [`KeyEvent`].
/// `Key::Character` resolves through the event's `text` payload
/// (e.g. winit hands us `"a"` for an A keypress); the other
/// keys map to their fixed [`keyboard::KeyAction`] variants.
fn label_for_key_event(skin: &dyn Painter, event: &KeyEvent) -> Option<&'static str> {
    use keyboard::KeyAction;
    let action = match event.key {
        Key::Character => {
            let c = event.text.as_ref()?.chars().next()?;
            if c.is_control() {
                return None;
            }
            // Match the on-screen keyboard's lowercase letters
            // (an uppercase keystroke from a held Shift still
            // flashes the same key).
            KeyAction::Character(c.to_ascii_lowercase())
        }
        Key::Backspace => KeyAction::Backspace,
        Key::Enter => KeyAction::Enter,
        _ => return None,
    };
    label_for_action(skin, action)
}

// ---------------------------------------------------------------------------
// Device chrome — clock + status digit glyph cache, minute tick
// ---------------------------------------------------------------------------

/// Build a glyphon buffer for each label the active skin
/// declares via `chrome_glyph_labels`. Same pattern as the
/// keyboard glyph cache — one allocation up front, looked up
/// by `&'static str` key during paint.
///
/// The clock entry is keyed `"clock"` by convention; the host
/// re-shapes that buffer on every minute boundary so the
/// status-bar time stays accurate without per-frame work.
fn build_chrome_glyph_cache(
    font_system: &mut glyphon::FontSystem,
    skin: &dyn Painter,
) -> HashMap<&'static str, glyphon::Buffer> {
    use glyphon::{Attrs, Buffer, Family, Metrics, Shaping};
    let mut cache = HashMap::new();
    for (key, text, size) in skin.chrome_glyph_labels() {
        let initial = if key == "clock" {
            format_clock_label()
        } else {
            text
        };
        let mut buf = Buffer::new(font_system, Metrics::new(size, size * 1.2));
        buf.set_size(font_system, None, None);
        buf.set_text(
            font_system,
            &initial,
            &Attrs::new().family(Family::SansSerif),
            Shaping::Advanced,
            None,
        );
        buf.shape_until_scroll(font_system, false);
        cache.insert(key, buf);
    }
    cache
}

/// Wall-clock minute (`hour * 60 + minute`) for the local
/// time. Wraps midnight (so 23:59 → 1439, 00:00 → 0). The host
/// compares this against `chrome_clock_minute` each tick to
/// decide whether to re-shape the clock buffer.
pub(crate) fn current_clock_minute() -> i64 {
    // `web_time::SystemTime` is `std::time::SystemTime` on native
    // and a `performance.now()` + page-start anchor on wasm32 —
    // `std::time::SystemTime::now()` would panic on wasm, taking
    // down any snippet that mounts the simulator chrome (status
    // bar + clock). Same swap we did for `Instant` higher up in
    // this file; keep them paired.
    let now = web_time::SystemTime::now();
    let secs_since_epoch = now
        .duration_since(web_time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0);
    // Local-time hour/minute via the system's offset. `chrono`
    // would be the right dep here; rolling a tiny calculator
    // avoids pulling it in just for the status bar. The result
    // matches `SystemTime::now()` to within DST/TZ — good
    // enough for a frozen-design simulator clock.
    let secs_of_day = secs_since_epoch.rem_euclid(86_400);
    let hour = secs_of_day / 3600;
    let minute = (secs_of_day % 3600) / 60;
    hour * 60 + minute
}

/// Render the current local wall-clock as `"H:MM"` (24-hour, no
/// leading zero on the hour — matches iOS's status-bar
/// formatting). Used by `build_chrome_glyph_cache` and by the
/// per-minute refresh inside `Host::tick`.
fn format_clock_label() -> String {
    let m = current_clock_minute();
    let hour = (m / 60) as u32;
    let minute = (m % 60) as u32;
    // 24-hour by default. iOS apps normally render 12-hour with
    // an AM/PM suffix; switching can be a skin override later.
    format!("{hour}:{minute:02}")
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    //! End-to-end tests for the touch system at the wgpu backend.
    //!
    //! Two layers exercised:
    //!
    //! 1. **Walker → Backend wiring** — building a `view().on_touch(...)`
    //!    primitive through `runtime_core::render` reaches the
    //!    backend's `install_touch_handler`, which writes the handler
    //!    onto `NodeData.touch_handler`.
    //!
    //! 2. **Responder-chain hit-test** — [`collect_touch_path`] walks
    //!    a layout-computed tree and collects every ancestor with a
    //!    `touch_handler`, in root-first order, scoped to the hit.
    //!
    //! Host-level dispatcher methods (`dispatch_touch_began` / `_moved`
    //! / `_ended`) require constructing a real [`Host`] which wants a
    //! [`Painter`] impl. We don't exercise them at unit-test scope; their
    //! state-machine pieces (the active-touches map, the deepest-first
    //! bubble) are implicitly covered by `collect_touch_path` + the
    //! recognizer tests in runtime-core.
    use super::*;
    use runtime_core::{view, Backend, ColorScheme, TouchResponse};
    use std::cell::Cell;

    /// Build a bare-bones [`WgpuBackend`] suitable for headless
    /// touch-pipeline tests. `glyphon::FontSystem::new` scans the
    /// system font cache; that's slow on first call but the OS
    /// caches it for the rest of the process so subsequent test
    /// runs are cheap.
    /// Minimal `Painter` for headless tests. All paint hooks fall
    /// through to the trait defaults (which mostly no-op or
    /// return empty rule sets); the touch-pipeline tests below
    /// never enter the renderer, so the visual paths are dead
    /// code at this scope.
    struct TestPainter;
    impl crate::painter::Painter for TestPainter {
        // No-op paint impls — these tests never enter the renderer.
        // Signatures track `crate::painter::Painter`; refreshed when the
        // trait surface changed (the prior shortened forms were
        // pre-trait-update bit-rot).
        fn paint_toggle(
            &self,
            _x: f32,
            _y: f32,
            _w: f32,
            _h: f32,
            _t: f32,
            _tint: Option<[f32; 4]>,
            _rects: &mut Vec<crate::pipeline::Instance>,
        ) {
        }
        fn paint_slider(
            &self,
            _x: f32,
            _y: f32,
            _w: f32,
            _h: f32,
            _value: f32,
            _min: f32,
            _max: f32,
            _tint: Option<[f32; 4]>,
            _rects: &mut Vec<crate::pipeline::Instance>,
        ) {
        }
        fn paint_text_input<'a>(
            &self,
            _x: f32,
            _y: f32,
            _w: f32,
            _h: f32,
            _is_focused: bool,
            _draw_caret: bool,
            _is_placeholder: bool,
            _buffer: &'a glyphon::Buffer,
            _caret_x_local: f32,
            _text_color: [f32; 4],
            _field_bg: Option<[f32; 4]>,
            _rects: &mut Vec<crate::pipeline::Instance>,
            _texts: &mut Vec<crate::text::StagedText<'a>>,
        ) {
        }
        fn paint_activity_indicator(
            &self,
            _x: f32,
            _y: f32,
            _w: f32,
            _h: f32,
            _phase: f32,
            _tint: Option<[f32; 4]>,
            _rects: &mut Vec<crate::pipeline::Instance>,
        ) {
        }
        fn keyboard_rows(&self) -> Vec<Vec<crate::keyboard::KeySpec>> {
            Vec::new()
        }
        fn keyboard_layout_metrics(&self) -> crate::keyboard::LayoutMetrics {
            crate::keyboard::LayoutMetrics {
                key_gap: 0.0,
                row_gap: 0.0,
                side_margin: 0.0,
                vert_margin: 0.0,
            }
        }
        fn paint_keyboard<'a>(
            &self,
            _keyboard_rect: (f32, f32, f32, f32),
            _laid_keys: &[crate::keyboard::LaidKey],
            _pressed_label: Option<&'static str>,
            _glyphs: &'a std::collections::HashMap<&'static str, glyphon::Buffer>,
            _rects: &mut Vec<crate::pipeline::Instance>,
            _texts: &mut Vec<crate::text::StagedText<'a>>,
        ) {
        }
        fn paint_navigator_header<'a, 'b>(
            &self,
            _rect: (f32, f32, f32, f32),
            _chrome: crate::painter::NavigatorHeaderChrome<'a, 'b>,
            _rects: &mut Vec<crate::pipeline::Instance>,
            _texts: &mut Vec<crate::text::StagedText<'a>>,
            _hit_regions: &mut Vec<crate::painter::NavigatorHeaderHit>,
        ) {
        }
    }

    fn make_backend() -> Rc<RefCell<WgpuBackend>> {
        let text = Rc::new(RefCell::new(crate::text::TextStore::new()));
        let fs = Rc::new(RefCell::new(glyphon::FontSystem::new()));
        Rc::new(RefCell::new(WgpuBackend::new(
            text,
            fs,
            ColorScheme::Light,
            Rc::new(TestPainter) as Rc<dyn crate::painter::Painter>,
        )))
    }

    /// Force-size every node in `tree` to a flat 100×100 box via
    /// Taffy's `set_intrinsic_size`, then run a layout pass with
    /// the root constrained to 100×100. Without this the framework
    /// produces 0×0 frames (no styles applied), which means every
    /// hit-test would miss.
    fn force_layout(backend: &Rc<RefCell<WgpuBackend>>, root: &WgpuNode, w: f32, h: f32) {
        fn size_subtree(b: &mut WgpuBackend, n: &WgpuNode, w: f32, h: f32) {
            let lay = n.borrow().layout;
            b.layout.set_intrinsic_size(lay, w, h);
            let children: Vec<WgpuNode> = n.borrow().children.clone();
            for child in &children {
                size_subtree(b, child, w, h);
            }
        }
        let mut b = backend.borrow_mut();
        size_subtree(&mut b, root, w, h);
        let root_layout = root.borrow().layout;
        b.layout.compute(root_layout, w, h);
    }

    // -------------------------------------------------------------
    // Walker → Backend wiring
    // -------------------------------------------------------------

    #[test]
    fn render_view_with_on_touch_installs_handler() {
        let backend = make_backend();
        let fires = Rc::new(Cell::new(false));
        let f = fires.clone();
        let tree = view(Vec::new())
            .on_touch(move |_| {
                f.set(true);
                TouchResponse::CONSUMED
            })
            .into();
        // Owner kept alive on the stack — dropping it would also
        // drop any reactive scopes; the backend node tree survives
        // either way.
        let _owner = runtime_core::render(backend.clone(), tree);
        let root = backend.borrow().root().expect("no root after render");
        assert!(
            root.borrow().touch_handler.is_some(),
            "on_touch handler did not reach the backend node",
        );
    }

    #[test]
    fn render_view_without_on_touch_leaves_handler_unset() {
        let backend = make_backend();
        let tree = view(Vec::new()).into();
        let _owner = runtime_core::render(backend.clone(), tree);
        let root = backend.borrow().root().expect("no root");
        assert!(root.borrow().touch_handler.is_none());
    }

    #[test]
    fn installed_handler_is_callable() {
        // Smoke-test the round-trip: the handler the user wrote
        // should be the one stored on the node, callable as-is
        // (no wrapping that changes semantics).
        let backend = make_backend();
        let fires = Rc::new(Cell::new(0u32));
        let f = fires.clone();
        let tree = view(Vec::new())
            .on_touch(move |_| {
                f.set(f.get() + 1);
                TouchResponse::CONSUMED
            })
            .into();
        let _owner = runtime_core::render(backend.clone(), tree);
        let root = backend.borrow().root().expect("no root");
        let handler = root.borrow().touch_handler.clone().expect("no handler");
        let synthetic = TouchEvent {
            id: TouchId(1),
            phase: TouchPhase::Began,
            position: TouchPoint::new(5.0, 5.0),
            window_position: TouchPoint::new(5.0, 5.0),
            timestamp_ns: 0,
            force: None,
        };
        let response = handler(&synthetic);
        assert!(response.consumed);
        assert_eq!(fires.get(), 1);
    }

    // -------------------------------------------------------------
    // collect_touch_path
    // -------------------------------------------------------------

    fn always_consume() -> runtime_core::TouchHandler {
        Rc::new(|_| TouchResponse::CONSUMED)
    }

    #[test]
    fn path_single_subscribed_root() {
        let backend = make_backend();
        let root;
        {
            let mut b = backend.borrow_mut();
            root = b.create_view(&Default::default());
            b.install_touch_handler(&root, always_consume());
        }
        force_layout(&backend, &root, 100.0, 100.0);
        let mut path = Vec::new();
        collect_touch_path(&backend.borrow(), &root, 0.0, 0.0, (50.0, 50.0), &mut path);
        assert_eq!(path.len(), 1);
        assert!(Rc::ptr_eq(&path[0].node, &root));
    }

    #[test]
    fn path_outside_bounds_is_empty() {
        let backend = make_backend();
        let root;
        {
            let mut b = backend.borrow_mut();
            root = b.create_view(&Default::default());
            b.install_touch_handler(&root, always_consume());
        }
        force_layout(&backend, &root, 100.0, 100.0);
        let mut path = Vec::new();
        // 200,200 is outside the 100×100 root.
        collect_touch_path(&backend.borrow(), &root, 0.0, 0.0, (200.0, 200.0), &mut path);
        assert_eq!(path.len(), 0);
    }

    #[test]
    fn path_unsubscribed_root_returns_empty_even_when_hit() {
        let backend = make_backend();
        let root;
        {
            let mut b = backend.borrow_mut();
            root = b.create_view(&Default::default());
            // No install_touch_handler call.
        }
        force_layout(&backend, &root, 100.0, 100.0);
        let mut path = Vec::new();
        collect_touch_path(&backend.borrow(), &root, 0.0, 0.0, (50.0, 50.0), &mut path);
        assert_eq!(path.len(), 0);
    }

    #[test]
    fn path_parent_and_child_both_subscribed() {
        let backend = make_backend();
        let parent;
        let child;
        {
            let mut b = backend.borrow_mut();
            parent = b.create_view(&Default::default());
            child = b.create_view(&Default::default());
            // Tag child first so it's a leaf, then the insert
            // moves it under parent.
            b.install_touch_handler(&parent, always_consume());
            b.install_touch_handler(&child, always_consume());
            let mut parent_for_insert = parent.clone();
            b.insert(&mut parent_for_insert, child.clone());
        }
        force_layout(&backend, &parent, 100.0, 100.0);
        let mut path = Vec::new();
        collect_touch_path(&backend.borrow(), &parent, 0.0, 0.0, (50.0, 50.0), &mut path);
        // Root-first: parent at [0], child at [1]. The dispatcher
        // iterates in reverse for deepest-first delivery.
        assert_eq!(path.len(), 2);
        assert!(Rc::ptr_eq(&path[0].node, &parent));
        assert!(Rc::ptr_eq(&path[1].node, &child));
    }

    #[test]
    fn path_only_child_subscribed() {
        let backend = make_backend();
        let parent;
        let child;
        {
            let mut b = backend.borrow_mut();
            parent = b.create_view(&Default::default());
            child = b.create_view(&Default::default());
            b.install_touch_handler(&child, always_consume());
            let mut parent_for_insert = parent.clone();
            b.insert(&mut parent_for_insert, child.clone());
        }
        force_layout(&backend, &parent, 100.0, 100.0);
        let mut path = Vec::new();
        collect_touch_path(&backend.borrow(), &parent, 0.0, 0.0, (50.0, 50.0), &mut path);
        assert_eq!(path.len(), 1);
        assert!(Rc::ptr_eq(&path[0].node, &child));
    }

    #[test]
    fn path_only_parent_subscribed() {
        let backend = make_backend();
        let parent;
        let child;
        {
            let mut b = backend.borrow_mut();
            parent = b.create_view(&Default::default());
            child = b.create_view(&Default::default());
            b.install_touch_handler(&parent, always_consume());
            let mut parent_for_insert = parent.clone();
            b.insert(&mut parent_for_insert, child.clone());
        }
        force_layout(&backend, &parent, 100.0, 100.0);
        let mut path = Vec::new();
        collect_touch_path(&backend.borrow(), &parent, 0.0, 0.0, (50.0, 50.0), &mut path);
        assert_eq!(path.len(), 1);
        assert!(Rc::ptr_eq(&path[0].node, &parent));
    }

    #[test]
    fn path_origin_matches_node_top_left() {
        // collect_touch_path returns each entry's absolute origin
        // — the value the dispatcher subtracts from the window
        // pointer to produce node-local coordinates. Verify it's
        // the literal top-left for a root at (0,0).
        let backend = make_backend();
        let root;
        {
            let mut b = backend.borrow_mut();
            root = b.create_view(&Default::default());
            b.install_touch_handler(&root, always_consume());
        }
        force_layout(&backend, &root, 100.0, 100.0);
        let mut path = Vec::new();
        collect_touch_path(&backend.borrow(), &root, 0.0, 0.0, (50.0, 50.0), &mut path);
        assert_eq!(path.len(), 1);
        assert_eq!(path[0].origin, (0.0, 0.0));
    }

    // -------------------------------------------------------------
    // absolute_origin
    // -------------------------------------------------------------

    #[test]
    fn absolute_origin_returns_zero_for_unmounted_node() {
        // A node that isn't reachable from any backend root (in
        // practice an unmount race) safely returns (0, 0). The
        // dispatcher uses this for the per-event origin recompute;
        // a panic here would crash on a touch during a screen
        // teardown.
        let backend = make_backend();
        let node = {
            let mut b = backend.borrow_mut();
            let n = b.create_view(&Default::default());
            // Steal it out of `roots` to simulate an orphaned node.
            b.roots.retain(|x| !Rc::ptr_eq(x, &n));
            n
        };
        let origin = absolute_origin(&backend.borrow(), &node);
        assert_eq!(origin, (0.0, 0.0));
    }
}
