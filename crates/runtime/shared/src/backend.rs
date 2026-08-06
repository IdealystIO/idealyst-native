//! # The backend-author surface (substrate half)
//!
//! Everything a backend must **install into** the shared substrate at
//! boot, gathered behind one import so a backend author does not have to
//! discover five unrelated modules and guess which of their functions are
//! part of the contract.
//!
//! This module is deliberately re-exports only — each item lives next to
//! the machinery it drives (`host`, `scheduling`, `time`, `driver`) and is
//! documented there. What this module adds is the *set* and the *order*.
//!
//! ## The other half
//!
//! A backend also implements [`runtime_scene::Host`] plus the
//! `runtime_vocabulary::caps::*` capability traits, and boots a
//! `runtime_scene::Registry`. Those live above this crate, so the
//! one-stop prelude that covers both halves is
//! `runtime_vocabulary::backend` — prefer importing that. This module is
//! what it re-exports from the substrate.
//!
//! ## Boot order
//!
//! The installs are not interchangeable. A boot entry must run them in
//! this order:
//!
//! 1. **[`install_scheduler`]** — first, unconditionally. The flush
//!    driver rides the scheduler (it schedules the post-dispatch flush as
//!    a microtask), so nothing that can stage a reactive write may run
//!    before it exists.
//! 2. **[`install_wall_clock_source`]**, if the backend has a
//!    timezone-aware clock — *before* the monotonic default, because
//!    [`install_default_time_source`] also installs the UTC-only
//!    `SystemWallClockSource` and both slots are first-install-wins.
//! 3. **[`install_time_source`]** (or [`install_default_time_source`]) —
//!    the monotonic clock animation and presence timing read. Skipping it
//!    on wasm32 leaves `now_micros()` returning `0`, which silently
//!    zeroes every animation duration and every `PhaseTimer` reading.
//! 4. **[`install_async_executor`]** / [`install_render_loop_driver`], if
//!    the backend supports them.
//! 5. **The environment services** — platform identity, color scheme, URL
//!    opener, full-screen setter, accessibility announcer. Do not call
//!    the five installers below by hand: use
//!    `runtime_vocabulary::backend::install_env_services`, which reads
//!    them off the backend's own `AppEnvOps` / `A11yOps` impls and
//!    captures the announcer's backend handle *weakly*. It must run
//!    before the root build, since a component body may read
//!    [`platform()`](crate::platform) while constructing.
//! 6. **The registry + build** — `Registry::new`,
//!    `register_builtins_with::<H, S>`, the app's own `register` seam,
//!    then realize inside `World::enter`.
//!
//! ## The flush driver is not in this list
//!
//! Wrapping author callbacks so they schedule a flush, and firing a
//! post-dispatch hook after timers / animation frames / future polls, is
//! the one part of the contract with no shared seam to install into —
//! each backend owns its own `dispatch_hook` module and its own
//! `schedule_flush`. A backend that skips it compiles and mounts, and
//! then never commits an author write: the UI appears frozen with no
//! error. See `docs/backend.md` § "The flush driver is part of the
//! backend's job".

// ---------------------------------------------------------------------------
// Environment services — the boot-side half of the ambient author reads
// (`platform()`, `color_scheme()`, `open_url()`, `set_fullscreen()`,
// `announce()`). Prefer `runtime_vocabulary::backend::install_env_services`
// over calling these directly; it derives all five from the backend's own
// caps impls, which is the only way to keep them in sync with the traits.
// ---------------------------------------------------------------------------
pub use crate::host::{
    install_announcer, install_current_color_scheme, install_current_platform,
    install_fullscreen_setter, install_url_opener,
};

// The values those installers carry, and the author-side readers they
// feed — re-exported so a backend can name them without a second import.
pub use crate::accessibility::LiveRegionPriority;
pub use crate::host::{ColorScheme, Platform};

// ---------------------------------------------------------------------------
// Substrate sinks
// ---------------------------------------------------------------------------

/// The microtask / timer / animation-frame scheduler. Install first.
pub use crate::scheduling::{install_scheduler, is_scheduler_installed, Scheduler};

/// Monotonic clock (animation, presence, `PhaseTimer`) and wall clock
/// (`epoch_millis`, `local_offset_minutes`).
pub use crate::time::{
    install_default_time_source, install_default_wall_clock_source, install_time_source,
    install_wall_clock_source, TimeSource, WallClockSource,
};

/// Future polling and per-frame drive. Gated with the `async-driver`
/// feature that gates the module itself.
#[cfg(feature = "async-driver")]
pub use crate::driver::{
    install_async_executor, install_render_loop_driver, AsyncExecutor, RenderLoopDriver,
};
