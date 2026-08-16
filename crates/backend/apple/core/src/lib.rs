//! Cross-Apple substrate (iOS + tvOS + macOS).
//!
//! Holds the pieces that don't care about the UI toolkit:
//!
//! - [`log`] — NSLog shim. Pure Foundation.
//! - [`scheduler`] — NSTimer + main-DispatchQueue scheduler. No
//!   UIKit or AppKit dependency.
//! - [`font`] — CoreText/CoreGraphics font registration + face
//!   matching. Returns PostScript names; UIFont/NSFont construction
//!   stays in the leaf crates.
//! - [`color`] — `runtime_shared::Color` → `(CGFloat, CGFloat,
//!   CGFloat, CGFloat)` parsing wrapper. UIColor/NSColor adapters
//!   stay in the leaf crates.
//!
//! Modules are gated on `cfg(any(target_os = "ios", target_os =
//! "tvos", target_os = "macos"))`; on the host target the crate
//! compiles as an empty rlib so workspace-wide `cargo check` keeps
//! working.

#[cfg(any(target_os = "ios", target_os = "tvos", target_os = "macos"))]
pub mod color;

#[cfg(any(target_os = "ios", target_os = "tvos", target_os = "macos"))]
pub mod font;

#[cfg(any(target_os = "ios", target_os = "tvos", target_os = "macos"))]
pub mod log;

#[cfg(any(target_os = "ios", target_os = "tvos", target_os = "macos"))]
pub mod scheduler;

/// Debug-only frame-pacing trace for diagnosing animation stutter. iOS/tvOS
/// use `CADisplayLink.displayLinkWithTarget:selector:` (UIKit); macOS uses
/// `NSScreen.displayLinkWithTarget:selector:` (AppKit, macOS 14+). Both give a
/// main-thread, common-mode vsync clock for measuring scroll-tracking stalls.
/// Self-installs from `install_scheduler`; compiled out of release builds.
#[cfg(all(any(target_os = "ios", target_os = "tvos", target_os = "macos"), debug_assertions))]
pub mod perf_trace;

/// Cooperative main-thread async executor — drives `spawn_async` futures
/// on the main run loop instead of `runtime-core`'s blocking `pollster`
/// fallback, so long-running futures (SSE / WebSocket `recv` loops) don't
/// freeze the UI. Installed by [`scheduler::install_scheduler`]. Gated on
/// `async-driver` since it needs `runtime_shared::driver`.
#[cfg(all(
    any(target_os = "ios", target_os = "tvos", target_os = "macos"),
    feature = "async-driver"
))]
pub mod async_executor;

/// SVG path parser, gated on Apple targets only so the host-build
/// path stays empty. Pure-Rust logic — no platform dependencies —
/// but kept inside the cfg to match the rest of the crate's
/// posture (cross-host workspace builds shouldn't link any
/// platform-specific code from here).
#[cfg(any(target_os = "ios", target_os = "tvos", target_os = "macos"))]
pub mod icon_path;

// Convenience re-export — `apple_log!` macro callers don't need to
// path through the module. Mirrors the prior `backend_ios_core::ios_log`
// shape so the iOS-core re-export stays a one-line `pub use`.
#[cfg(any(target_os = "ios", target_os = "tvos", target_os = "macos"))]
pub use log::apple_log;

/// Shared attributed-string assembly for styled text runs — the
/// toolkit-agnostic half of `Backend::create_styled_text` on Apple
/// (fragment appending + attribute dictionaries from finished
/// platform objects). OS-gated: it constructs Foundation objects.
#[cfg(any(target_os = "ios", target_os = "tvos", target_os = "macos"))]
pub mod styled_text;

/// Pure style decisions for native editable text controls (UITextField /
/// UITextView, NSTextField / NSTextView). NOT OS-gated — it's `runtime_shared`
/// only, so it builds AND unit-tests on the host while iOS + macOS share one
/// source of truth for "what background/color does an editable control get".
pub mod text_control_style;

/// The uniform-vs-per-side border routing decision shared by the iOS and
/// macOS backends. NOT OS-gated — pure `runtime_shared` logic, host-testable,
/// so both backends collapse the four CSS sides identically (Rule #7).
/// Typed CoreGraphics pointer newtypes (`CGColorRef`, `CGPathRef`) whose
/// Objective-C type encodings are pinned by host-run tests. A bare
/// `*const c_void` encodes as `^v` and makes objc2 abort the process at calls
/// like `setShadowPath:` / `setShadowColor:`. NOT OS-gated on purpose — the
/// backends' UIKit/AppKit modules are, so a guard living inside one of them
/// would never run on the host.
pub mod cg;

pub mod border;

/// Child-clipping decision (`clipsToBounds` / `masksToBounds`) shared by the
/// iOS and macOS backends. Clipping follows `overflow` — NEVER `border_radius`,
/// which rounds the background + border on its own and needs no mask. Owning
/// the rule here keeps the two backends converged (Rule #7) and keeps the
/// offscreen-render pass off every rounded view. NOT OS-gated: pure
/// `runtime_shared` logic, unit-tested on the host.
pub mod clip;

/// Where a view's box shadow gets painted — its own layer, or a synthesized
/// sibling layer in the parent when the view's own layer is bounds-masked and
/// would clip the shadow away. NOT OS-gated: pure `runtime_shared` logic,
/// unit-tested on the host, shared so iOS and macOS branch identically
/// (Rule #7).
pub mod shadow;

/// The CALayer half of [`shadow`] — writing/clearing shadow properties, tracing
/// the `shadowPath`, and the whole lifecycle of the synthesized sibling layer.
/// CALayer is the same class under UIKit and AppKit, so both backends share
/// this file verbatim and contribute only the view→layer lookup.
#[cfg(any(target_os = "ios", target_os = "tvos", target_os = "macos"))]
pub mod shadow_layer;

/// CSS `pointer-events` hit-test verdict shared by the UIKit + AppKit
/// hit-test overrides. NOT OS-gated — pure `runtime_shared` logic,
/// host-testable, so both backends decline/allow hits identically
/// (Rule #7).
pub mod pointer_events_policy;

/// Post-dispatch hook for the new-core flush drivers (idea-lite
/// migration): a thread-local `fn()` slot the scheduler/executor fire
/// after timer / frame / future-poll callbacks that may run author
/// code. No-op default — the old core never installs it. NOT OS-gated:
/// pure `std` state, host-testable, and the new-core glue in the leaf
/// backends compiles it on the host for its own unit tests.
pub mod dispatch_hook;
