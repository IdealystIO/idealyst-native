//! macOS old-core wrapper for the Toolbar SDK.
//!
//! All the NSToolbar machinery (delegate, item construction, the
//! wipe+repopulate update, the 0-size placeholder) lives in the
//! core-free [`crate::macos_shared`] module, shared with the new-core
//! leg. This module contributes only what is old-core-specific:
//!
//! - the `register_external` registration on [`MacosBackend`]'s
//!   `ExternalRegistry`,
//! - the reactive items subscription via `runtime_core::effect!`, and
//! - the identity click discipline (`plan_items`'s `wrap_click` is
//!   `|cb| cb` here — the old core applies signal writes synchronously,
//!   so author clicks need no post-dispatch flush; the new-core leg
//!   wraps with `schedule_flush` instead).
//!
//! # Lifetime model (per-core half)
//!
//! The reactive `Effect` is owned by the active framework scope (the
//! render path's outer scope), so its drop is a no-op here; the bare
//! `effect!({ … })` shape mirrors webview-ios. The effect closure
//! captures the [`NativeToolbar`](crate::macos_shared) clone — that is
//! what keeps the weakly-held `ToolbarDelegate` retained for as long as
//! the primitive is mounted. See `macos_shared`'s module docs for the
//! full NSToolbar/delegate/window lifetime notes.

use crate::{macos_shared, ToolbarProps};
use backend_macos::{MacosBackend, MacosNode};
use std::rc::Rc;

pub(crate) use crate::macos_shared::OPS;

/// Register the macOS `Toolbar` external handler on `backend`. Call once
/// at app boot so `Toolbar` elements lower to a native `NSToolbar`.
pub fn register(backend: &mut MacosBackend) {
    backend.register_external::<ToolbarProps, _>(|props, b| build_toolbar(props, b));
}

fn build_toolbar(props: &Rc<ToolbarProps>, b: &mut MacosBackend) -> MacosNode {
    let mtm = b.mtm();

    // Find the host NSWindow via host_root's `window` property. The
    // host's `setContentView:` runs before render starts, so
    // host_root.window is non-nil by the time this handler fires.
    // If somehow the placeholder isn't in a window (host hasn't
    // wired the toolbar SDK + render against a window — unusual,
    // but defended), fall through to building a detached toolbar
    // that simply never displays.
    let window = b.host_root().and_then(macos_shared::window_of_view);

    // Build NSToolbar + delegate, configure, attach to the window.
    let native = macos_shared::build_native_toolbar(mtm, window.as_deref(), props.visible);

    // Reactive items: every Effect re-fire reads `props.items()` and
    // applies the new list. The delegate keeps the canonical record
    // list; we drive the NSToolbar via insert/remove ops.
    //
    // Effect handle is dropped at scope exit; since the framework's
    // mount runs inside a Scope, the drop is a no-op and the slot
    // is freed when that scope drops. The closure keeps strong
    // refs to `toolbar` + `delegate` alive (via the NativeToolbar
    // clone) in the meantime — see the lifetime notes in the module
    // docs + macos_shared.
    let native_for_effect = native.clone();
    let props_for_effect = props.clone();
    runtime_core::effect!({
        let items = (props_for_effect.items)();
        // Identity wrap: old-core clicks apply writes synchronously,
        // no post-dispatch flush needed (contrast macos_newcore.rs).
        let plan = macos_shared::plan_items(items, &|cb| cb);
        macos_shared::apply_items(&native_for_effect, plan);
    });

    // Return a zero-sized placeholder NSView (see make_placeholder_view
    // for the wantsLayer rationale).
    let placeholder = macos_shared::make_placeholder_view(mtm);
    b.register_external_view(&placeholder);

    MacosNode::View(placeholder)
}
