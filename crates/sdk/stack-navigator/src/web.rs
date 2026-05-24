//! Web-backend handler stub for Stack navigator.
//!
//! **Migration status — not yet wired.** The legacy `Primitive::Navigator`
//! path is still the operational stack on web (see
//! `backend-web/src/primitives/navigator.rs`). This module exists so
//! the SDK builds across the workspace, but its `register` call is a
//! no-op until the per-backend handler is ported through the new
//! `NavigatorHandler` contract.
//!
//! Port checklist (to be done in a follow-up commit):
//! 1. Add `navigator_handlers: NavigatorRegistry<WebBackend>` field to
//!    `WebBackend` + inherent `register_navigator::<P, F>(&mut self, factory)`
//!    that delegates to it.
//! 2. Override `Backend::create_navigator_extension` to look up by
//!    `type_id`, construct the handler via factory, call `handler.init(
//!    &mut self, host, presentation)`, store the handler keyed by node,
//!    install a dispatcher on `host.control` that routes `NavCommand`s
//!    to `handler.on_command(cmd)`.
//! 3. Port the existing web stack impl (the dispatcher closures + DOM
//!    machinery in `backend-web/src/primitives/navigator.rs::create_navigator`
//!    and friends) into `WebStackHandler::init` / `on_command` /
//!    `apply_slot_style` here.
//! 4. Replace the no-op `register` below with one that calls
//!    `backend.register_navigator::<StackPresentation, _>(|| Box::new(
//!    WebStackHandler::new()))`.

use backend_web::WebBackend;

/// No-op until the web handler is ported. See module doc.
pub fn register(_backend: &mut WebBackend) {
    // Intentionally empty. Apps using the legacy `runtime_core::Navigator`
    // builder are unaffected — the legacy `Primitive::Navigator` walker
    // path and `Backend::create_navigator` impl are still in place.
    // Apps using `stack_navigator::Navigator::new(...)` will hit the
    // framework's default `create_navigator_extension` panic until this
    // is wired.
}
