//! `position: sticky` for the GTK backend.
//!
//! CSS semantics: the element lays out normally until it would scroll
//! past an edge of its enclosing scroll container, then pins there. The
//! layout engine never sees this — Taffy places the node at its natural
//! position and the backend applies a *visual* offset on top, so the
//! surrounding flow doesn't reflow as the pin engages.
//!
//! ## Model
//!
//! GTK follows the same shape as the macOS backend (see
//! `backend-macos/src/imp/sticky.rs`): a registry of sticky nodes
//! (`LinuxBackend::sticky_nodes`) keyed by the enclosing scroll node,
//! driven by the scroll container's own change notification rather than
//! a per-frame poll. Here that's `value-changed` on BOTH of the
//! `GtkScrolledWindow`'s adjustments, which fires only when an offset
//! actually moves — strictly cheaper than ticking every vsync, and the
//! same signals the `on_scroll` prop already rides. A programmatic
//! `set_node_scroll` re-pins directly (the hook can't re-borrow the
//! backend the caller holds), and every layout pass re-pins, because
//! reflow moves the natural position.
//!
//! The pin itself is `runtime_shared::sticky`: `StickyInsets::from_style`
//! resolves the per-edge thresholds at `apply_style`, and
//! `sticky::translate` turns (natural content-space origin, Taffy size,
//! both scroll offsets, scrollport size) into the `(dx, dy)` the node is
//! shifted by — so all four edges pin, on both axes, with exactly the
//! arithmetic every other native backend uses. The scrollport is the
//! scroll node's Taffy box; trailing pins (`bottom` / `right`) measure
//! against it.
//!
//! Unlike macOS this moves the node's **transform**
//! (`NodeTransform::sticky`), not its frame: `IdealystView` paints
//! children through their stored `GskTransform` and doesn't cull by
//! frame, so there's no equivalent of AppKit purging a transform-pinned
//! view's drawing.
//!
//! Paint order: a sticky node stacks at `StyleRules::stacking_z()`
//! (`STICKY_Z` unless the author sets `z_index`), so the pinned element
//! draws over the content scrolling beneath it — see
//! `LinuxBackend::reorder_siblings`.
//!
//! With no enclosing scroll container the node falls back to
//! `Relative`, which is what CSS does.
//!
//! Not modelled: CSS also confines a sticky element to its *containing
//! block*, so it stops pinning once its parent scrolls away. Every native
//! backend omits it alike (see the per-backend coverage notes on
//! `runtime_shared::Position::Sticky`).
