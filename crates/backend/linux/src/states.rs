//! Interaction state (`hover` / `press` / `focus`) → the framework's
//! per-node state signal, and the author-facing `on_hover` handler.
//!
//! ## Why this file has to exist
//!
//! [`StyleOps::handles_states_natively`] is `false` on this backend (the
//! trait default), which is the correct answer for GTK: there is no CSS
//! pseudo-class layer, so state variants cannot be handed over
//! declaratively. `false` selects the **event-driven** path instead — the
//! framework hands each stateful node a `setter` through
//! [`StyleOps::attach_states`] and expects the BACKEND to call it as the
//! pointer enters, presses and leaves. Flipping that bit re-resolves the
//! node's sheet and re-applies it.
//!
//! `attach_states` has a no-op default, so a backend that never overrides
//! it type-checks, renders every node's BASE style correctly, and simply
//! never lights a single `state hover` / `state pressed` / `state focused`
//! variant. That is what this backend did: every hover highlight, press
//! feedback and focus ring in idea-ui was dead on Linux, with nothing in
//! the compiler or the test suite to say so.
//!
//! ## Mapping
//!
//! | Framework state | GTK source |
//! |---|---|
//! | [`StateBits::HOVERED`] | `EventControllerMotion` enter/leave |
//! | [`StateBits::PRESSED`] | `GestureClick` pressed / released + cancel |
//! | [`StateBits::FOCUSED`] | `EventControllerFocus` enter/leave |
//!
//! `DISABLED` is deliberately absent: it is not an input-driven state.
//! The framework sets it from the author's `disabled` prop and routes the
//! native side through `StyleOps::set_disabled`.
//!
//! ## Why PRESSED needs `cancel` as well as `released`
//!
//! A press that ends outside the widget — drag off the button, or GTK
//! handing the sequence to a scroll gesture mid-press — fires `cancel`,
//! not `released`. Wiring only `released` leaves the node stuck in its
//! pressed style forever, which is worse than having no press state at
//! all. `unpaired-release` covers the release-without-matching-press case
//! the same way.
//!
//! [`StyleOps::handles_states_natively`]: runtime_vocabulary::caps::StyleOps::handles_states_natively
//! [`StyleOps::attach_states`]: runtime_vocabulary::caps::StyleOps::attach_states
//! [`StateBits::HOVERED`]: runtime_shared::StateBits::HOVERED
//! [`StateBits::PRESSED`]: runtime_shared::StateBits::PRESSED
//! [`StateBits::FOCUSED`]: runtime_shared::StateBits::FOCUSED

use std::rc::Rc;

use gtk4::prelude::*;
use runtime_shared::StateBits;

/// Wire `setter` to `widget`'s pointer / focus events.
///
/// The controllers are owned by the widget, so they die with it — no
/// teardown bookkeeping, and no `on_node_unstyled` hook needed.
pub(crate) fn attach(widget: &gtk4::Widget, setter: Rc<dyn Fn(StateBits, bool)>) {
    // --- hover -------------------------------------------------------
    let motion = gtk4::EventControllerMotion::new();
    {
        let s = setter.clone();
        motion.connect_enter(move |_, _, _| s(StateBits::HOVERED, true));
    }
    {
        let s = setter.clone();
        motion.connect_leave(move |_| s(StateBits::HOVERED, false));
    }
    widget.add_controller(motion);

    // --- press -------------------------------------------------------
    //
    // A SECOND `GestureClick` alongside the one `create_pressable` /
    // `create_link` install for activation. Both observe the same
    // sequence: GTK delivers to every controller on the widget rather
    // than letting the first consume it, so adding this one does not
    // steal the activation click. Keeping them separate means a node
    // that is styled-but-not-pressable (a hoverable card) still gets
    // press feedback, and `attach_states` stays independent of which
    // primitive built the node.
    let click = gtk4::GestureClick::new();
    {
        let s = setter.clone();
        click.connect_pressed(move |_, _, _, _| s(StateBits::PRESSED, true));
    }
    {
        let s = setter.clone();
        click.connect_released(move |_, _, _, _| s(StateBits::PRESSED, false));
    }
    {
        // Press ended outside the widget, or GTK reassigned the sequence
        // (scroll took over). Without this the node stays visually
        // pressed forever.
        let s = setter.clone();
        click.connect_cancel(move |_, _| s(StateBits::PRESSED, false));
    }
    {
        let s = setter.clone();
        click.connect_unpaired_release(move |_, _, _, _, _| s(StateBits::PRESSED, false));
    }
    widget.add_controller(click);

    // --- focus -------------------------------------------------------
    let focus = gtk4::EventControllerFocus::new();
    {
        let s = setter.clone();
        focus.connect_enter(move |_| s(StateBits::FOCUSED, true));
    }
    {
        let s = setter;
        focus.connect_leave(move |_| s(StateBits::FOCUSED, false));
    }
    widget.add_controller(focus);
}

/// Wire the author's `.on_hover(…)` handler — `true` on enter, `false`
/// on leave. Separate from [`attach`]: that drives STYLE state, this is
/// an author callback, and a node can have either without the other.
pub(crate) fn install_hover(widget: &gtk4::Widget, handler: runtime_shared::HoverHandler) {
    let motion = gtk4::EventControllerMotion::new();
    {
        let h = handler.clone();
        motion.connect_enter(move |_, _, _| h(true));
    }
    {
        let h = handler;
        motion.connect_leave(move |_| h(false));
    }
    widget.add_controller(motion);
}
