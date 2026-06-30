//! Disabled press-block on the bare `pressable` primitive.
//!
//! Regression for: a `disabled` pressable still fired its `on_click`.
//! `Backend::set_disabled` only makes *native form controls* (a real
//! `<button>` / `UIButton`) inert — a bare pressable lowers to a
//! non-form-control node (`<div role=button>` on web, a plain view on
//! native) where the disabled attribute/flag is a CSS/state hook at
//! best and a no-op at worst, so the click/keydown handler kept firing.
//!
//! The fix wires the press-block at the handler level in the walker
//! (`walker/pressable.rs`): when a `disabled` source is present the
//! `on_click` closure is wrapped behind a shared flag the disabled
//! `Effect` drives, so the press is blocked uniformly across *every*
//! backend (rule #7) regardless of what that backend's `set_disabled`
//! does. These tests fire a press through `MockBackend::fire_press`
//! (the same closure mouse/keyboard/programmatic activation routes
//! through) and assert it is / isn't observed.

use std::cell::RefCell;
use std::rc::Rc;

use runtime_core::{pressable, signal, Signal};

use crate::common::{NodeId, TestRuntime};

/// A non-disabled pressable fires its `on_click`; a statically-disabled
/// one does not — even though the backend's `set_disabled` can't make a
/// bare pressable node natively inert.
#[test]
fn static_disabled_blocks_press_while_enabled_fires() {
    // Enabled baseline: the press is observed.
    let rt = TestRuntime::new();
    let fired = Rc::new(RefCell::new(0u32));
    let f = fired.clone();
    let _owner = rt.render(pressable(vec![], move || *f.borrow_mut() += 1).into());
    // The pressable is the first node the walker creates (mock mints from 0).
    assert!(rt.backend().fire_press(NodeId(0)), "pressable was registered");
    assert_eq!(*fired.borrow(), 1, "an enabled pressable fires on press");

    // Disabled: the same press is blocked at the handler level.
    let rt = TestRuntime::new();
    let fired = Rc::new(RefCell::new(0u32));
    let f = fired.clone();
    let _owner = rt.render(
        pressable(vec![], move || *f.borrow_mut() += 1)
            .disabled(true)
            .into(),
    );
    assert!(rt.backend().fire_press(NodeId(0)), "pressable was registered");
    assert_eq!(
        *fired.borrow(),
        0,
        "a disabled pressable must NOT fire on press"
    );
}

/// A reactive `disabled` source flips the press-block live: enabled →
/// press fires; flip the signal true → blocked; flip back → fires again.
/// Proves the block tracks the disabled state rather than being baked in
/// at build time.
#[test]
fn reactive_disabled_blocks_and_unblocks_press() {
    let rt = TestRuntime::new();
    let off: Signal<bool> = signal!(false);
    let fired = Rc::new(RefCell::new(0u32));
    let f = fired.clone();

    let off_for_disabled = off;
    let _owner = rt.render(
        pressable(vec![], move || *f.borrow_mut() += 1)
            .disabled(move || off_for_disabled.get())
            .into(),
    );

    // Starts enabled: press fires.
    rt.backend().fire_press(NodeId(0));
    assert_eq!(*fired.borrow(), 1, "enabled: press fires");

    // Flip disabled on: the disabled Effect updates the shared flag, so
    // the next press is swallowed.
    off.set(true);
    rt.backend().fire_press(NodeId(0));
    assert_eq!(*fired.borrow(), 1, "disabled: press is blocked (count unchanged)");

    // Flip back off: presses fire again.
    off.set(false);
    rt.backend().fire_press(NodeId(0));
    assert_eq!(*fired.borrow(), 2, "re-enabled: press fires again");
}
