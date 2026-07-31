//! # Per-world viewport / breakpoint state — the reactive viewport source
//!
//! The old engine keeps ONE thread-local viewport signal
//! (`runtime_shared::viewport_size`) plus a thread-local memoized bucket
//! (`runtime_shared::current_breakpoint`); backends push window resizes
//! into it and author code subscribes from effects. On the new kernel a
//! world effect cannot subscribe to an old-core signal, so until this
//! module existed the glue re-exported the old free fns VALUE-only and
//! every breakpoint-dependent `when` was frozen at its seed (found
//! live: the idea-ui-docs hamburger stayed visible at desktop width —
//! `when(!sidebar_pinned(Lg))` never re-fired).
//!
//! Shape mirrors [`crate::theme`]'s `ThemeCtx` exactly:
//!
//! - All state lives in a [`ViewportCtx`] stored in the owning world's
//!   context (`provide`/`inject`) — per-world isolation (app world +
//!   SSR per-request worlds don't share a viewport).
//! - The signal + derived-bucket memo are created via
//!   [`runtime_world::unscoped`] so they are world-root-owned, not
//!   adopted by whichever transient subtree first reads a breakpoint
//!   (the old `unscope` contract, same regression class).
//! - The bucket derivation is a **memo** (Derivation class), not a
//!   plain effect: a reaction that reads `current_breakpoint()` in the
//!   same flush as a viewport commit must see the settled bucket
//!   (memos-before-reactions), and a Reaction-staged write would not
//!   re-flush.
//! - `ViewportCtx::set` is **handler-safe** (no ambient world needed):
//!   platform resize listeners fire OUTSIDE `World::enter`, so the
//!   backend captures the ctx (or its signal handle) at boot and pushes
//!   from the raw event — capture, don't inject. Writes on a dead world
//!   are silent no-ops, so a listener surviving a `stop()` is inert.
//! - The ambient free fns ([`viewport_ctx`], glue's `viewport_size` /
//!   `current_breakpoint`) fall back to the thread's last ambient ctx
//!   when called outside `enter` (same fork as the theme surface —
//!   `sidebar_pinned` is read from drawer-link HANDLERS, which run
//!   outside enter).
//!
//! ## Seeding + platform sources
//!
//! A world's ctx seeds from the SHARED old-core viewport value at first
//! use (`runtime_shared::viewport_size().get()`), because every existing
//! platform seam writes there: SSR seeds `SSR_VIEWPORT` before render,
//! the web hydrate boot seeds the `data-ssr-viewport` attribute, and
//! the native backends sample their window into `set_viewport_size`.
//! LIVE updates additionally require the platform to push into the
//! world ctx:
//!
//! - **web** (`backend-web/src/newcore.rs`): wired — seeds from
//!   `window.inner{Width,Height}` at boot and installs a `resize`
//!   listener that pushes both sinks (old-core TLS for value parity +
//!   this ctx) and schedules a flush.
//! - **macOS** (`backend-macos/src/newcore.rs`, "Viewport source"):
//!   wired — the AppKit seams (`LayoutObserverView::setFrameSize:` +
//!   `finish`'s deferred first mirror) push both sinks; the boot
//!   captures this ctx's signal post-build and the seams forward via
//!   `forward_viewport` + one deduped `schedule_flush`.
//! - **iOS** (`backend-ios-mobile/src/newcore.rs`, "Viewport source"):
//!   wired — `LayoutObserverView::layoutSubviews` (rotation/resize +
//!   first measurement) pushes both sinks, same forward/flush shape.
//! - **Android** (`backend-android-mobile/src/newcore.rs`, "Viewport
//!   source"): wired — the deferred mirror microtask in
//!   `imp::viewport_size()` (fed by every layout pass, so rotation and
//!   configuration changes route through it) pushes both sinks; the
//!   sink reinstalls on Activity-recreation re-`start` and dead-world
//!   writes no-op, so a mirror racing teardown is inert.
//! - **GPU** (`render-wgpu/src/newcore.rs`, "Viewport source"): wired
//!   at its one source of truth — `Host::set_viewport`, the LOGICAL
//!   viewport report. Window resizes deliberately never change the
//!   logical size on the winit host (fixed `DeviceProfile::
//!   logical_size`, letterboxed), so bucket flips only follow logical
//!   reports; the windowed boot seeds the old TLS pre-mount
//!   (`host_winit::newcore::run_with`), while embedded sim boots
//!   (`start_in_world`) install NO sink so the sim profile can't
//!   clobber the embedding page's ctx.
//!
//! Wiring rule for future seams: every platform site that writes the
//! old TLS (`runtime_shared::set_viewport_size`) must forward the SAME
//! value to its backend's new-core `forward_viewport` — the two sinks
//! never diverge, and the push always stages outside `World::enter`
//! then rides the backend's deduped flush.
//!
//! ## What still reads the OLD signal
//!
//! `style_attach`'s native breakpoint-overlay merge reads the old-core
//! bucket value at apply time (correct values — every live source above
//! pushes both sinks with the same size, so the old bucket tracks the
//! ctx — but no re-fire of the merge itself) — moving the native
//! breakpoint-overlay re-application onto this ctx is the remaining
//! native gap, also noted in `theme.rs`.

use std::cell::RefCell;

use runtime_shared::{Breakpoint, ViewportSize};
use runtime_world::{inject, memo, provide, signal, unscoped, Memo, ReadSignal, Signal};

/// The per-world viewport context: the size signal plus the derived
/// breakpoint bucket. Cloneable (context values must be) — clones are
/// the same handles.
#[derive(Clone)]
pub struct ViewportCtx {
    size: Signal<ViewportSize>,
    breakpoint: Memo<Breakpoint>,
}

thread_local! {
    /// The most recent ambient world's ctx — the HANDLER fallback.
    /// Refreshed on every ambient [`viewport_ctx`] call; consulted only
    /// when no world is entered (see the module docs' capture-don't-
    /// inject rationale). Handles route to their OWN world, and writes
    /// on a dead world no-op, so a stale capture is safe — merely inert.
    static LAST_CTX: RefCell<Option<ViewportCtx>> = const { RefCell::new(None) };
}

/// The ambient world's viewport context, created (and `provide`d) on
/// first use. Outside `World::enter` this returns the thread's last
/// ambient ctx (handler fallback); with no ctx ever created it panics
/// with the canonical outside-enter message via the creation path.
pub fn viewport_ctx() -> ViewportCtx {
    if !runtime_world::is_entered() {
        if let Some(ctx) = LAST_CTX.with(|c| c.borrow().clone()) {
            return ctx;
        }
        // Fall through: the ambient path below panics with the
        // canonical creation-outside-enter message, which is the
        // correct diagnosis (no world ever owned a viewport).
    }
    if let Some(ctx) = inject::<ViewportCtx>() {
        LAST_CTX.with(|c| *c.borrow_mut() = Some(ctx.clone()));
        return ctx;
    }
    let ctx = unscoped(|| {
        // Seed from the shared old-core viewport value: every platform
        // seam (SSR seed, hydrate attribute, native samples) writes it
        // before the first build. A plain value read — no old-core
        // subscription is possible from world effects, which is the
        // whole reason this ctx exists.
        let seed = runtime_shared::viewport_size().get();
        let size = signal(seed);
        // Capture the table once, like the old thread-local memo did
        // (`install_breakpoints` is documented first-wins-before-first-
        // read on both cores).
        let table = runtime_shared::breakpoints();
        let breakpoint = memo(move || table.classify(size.get().width));
        ViewportCtx { size, breakpoint }
    });
    // World-root provision to match the world-root signals inside it —
    // see `theme_ctx` for the failure mode a scope-owned entry produces.
    unscoped(|| provide(ctx.clone()));
    LAST_CTX.with(|c| *c.borrow_mut() = Some(ctx.clone()));
    ctx
}

impl ViewportCtx {
    /// Push a new viewport size. **Handler-safe** — stages through the
    /// signal handle (routes to its own world), commits on that world's
    /// next flush, equality-guarded so per-pixel no-change events don't
    /// wake subscribers. Negative components clamp to zero (the old
    /// `set_viewport_size` contract).
    pub fn set(&self, size: ViewportSize) {
        let clamped = ViewportSize {
            width: size.width.max(0.0),
            height: size.height.max(0.0),
        };
        self.size.set(clamped);
    }

    /// The reactive size signal (Copy handle) — what glue's
    /// `viewport_size()` mirror hands to author code.
    pub fn size_signal(&self) -> Signal<ViewportSize> {
        self.size
    }

    /// The derived bucket as a `ReadSignal` — what glue's
    /// `current_breakpoint()` mirror returns (old-core signature
    /// parity). Re-fires only when the BUCKET changes (memo equality
    /// cut), not per pixel.
    pub fn breakpoint(&self) -> ReadSignal<Breakpoint> {
        self.breakpoint.read_only()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use runtime_world::{effect, World};
    use std::cell::Cell;
    use std::rc::Rc;

    /// The idea-ui-docs hamburger bug: a `when`-shaped effect reading
    /// the breakpoint must re-fire when the platform pushes a resize
    /// from OUTSIDE `World::enter` (the resize listener is a raw DOM
    /// callback), and must NOT re-fire when the bucket doesn't change.
    #[test]
    fn regression_hamburger_breakpoint_when_refires_on_resize_push() {
        let world = World::new();
        let (ctx, runs, last) = world.enter(|| {
            let ctx = viewport_ctx();
            let bp = ctx.breakpoint();
            let runs = Rc::new(Cell::new(0usize));
            let last = Rc::new(Cell::new(Breakpoint::Xs));
            let runs_c = runs.clone();
            let last_c = last.clone();
            // World-root effect standing in for the shell's
            // `when(!sidebar_pinned(Lg))` decider.
            let _e = effect(move || {
                last_c.set(bp.get());
                runs_c.set(runs_c.get() + 1);
            });
            (ctx, runs, last)
        });
        world.flush();
        assert_eq!(runs.get(), 1);
        assert_eq!(last.get(), Breakpoint::Xs, "seed-only world starts at 0-width");

        // Platform resize listener: fires outside enter, stages, flush commits.
        ctx.set(ViewportSize::new(1280.0, 800.0));
        world.flush();
        assert_eq!(last.get(), Breakpoint::Xl, "bucket followed the resize");
        assert_eq!(runs.get(), 2);

        // Same-bucket resize: pixel change, no bucket flip, no re-fire.
        ctx.set(ViewportSize::new(1290.0, 800.0));
        world.flush();
        assert_eq!(runs.get(), 2, "per-pixel resizes inside a bucket stay silent");

        // Drop below the Lg threshold — the hamburger's `when` flips.
        ctx.set(ViewportSize::new(700.0, 800.0));
        world.flush();
        assert_eq!(last.get(), Breakpoint::Sm);
        assert_eq!(runs.get(), 3);
    }

    /// Per-world isolation: each world owns its viewport; a push on one
    /// never re-fires the other's subscribers.
    #[test]
    fn viewport_is_per_world() {
        let a = World::new();
        let b = World::new();
        let ctx_a = a.enter(viewport_ctx);
        let ctx_b = b.enter(viewport_ctx);
        ctx_a.set(ViewportSize::new(1280.0, 800.0));
        a.flush();
        b.flush();
        let bp_a = a.enter(|| ctx_a.breakpoint().get());
        let bp_b = b.enter(|| ctx_b.breakpoint().get());
        assert_eq!(bp_a, Breakpoint::Xl);
        assert_eq!(bp_b, Breakpoint::Xs);
    }

    /// The handler fallback: reading the ctx outside enter (a drawer
    /// link handler calling `sidebar_pinned`) resolves to the last
    /// ambient world's ctx instead of panicking.
    #[test]
    fn viewport_ctx_outside_enter_falls_back_to_last_ambient() {
        let world = World::new();
        let ctx = world.enter(viewport_ctx);
        ctx.set(ViewportSize::new(1280.0, 800.0));
        world.flush();
        // Outside enter: same ctx, readable bucket.
        let outside = viewport_ctx();
        assert_eq!(outside.breakpoint().peek(), Breakpoint::Xl);
    }
}
