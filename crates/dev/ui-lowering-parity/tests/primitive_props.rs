//! Regressions for primitive props `ui!` used to accept and DROP.
//!
//! Every primitive emitter read a hand-picked handful of props and threw
//! the rest away without a word: `view(on_touch = …)`, `view(on_hover =
//! …)`, `view(bind = r)`, `image(on_load = …)` all compiled, and the
//! handler never reached the backend. The emission side is pinned in
//! `runtime-macros` (`regression_ui_*` in `ui.rs`); these tests prove
//! the behavioural half — the prop now ARRIVES, observed as the
//! capability call the recording `host-mock` backend logs when the
//! vocabulary installs it.
//!
//! The other half of the fix — an unknown prop is now a compile error —
//! can only be observed by a compiler, so it lives in
//! `tests/compile_fail.rs`.

use std::rc::Rc;

use host_mock::Harness;
use runtime_macros::ui;
use runtime_scene::realize;
use runtime_vocabulary::glue::{Element, Ref, TouchResponse, ViewHandle};

/// Mount `build`'s tree on a recording harness and return every op the
/// mount produced.
fn mount_ops(build: impl FnOnce() -> Element) -> Vec<String> {
    let h = Harness::new();
    h.record_all();
    let realized = h.world.enter(|| realize(&h.backend, &h.registry, build()));
    h.flush();
    let ops = h.take_log();
    drop(realized);
    ops
}

fn has_op(ops: &[String], prefix: &str) -> bool {
    ops.iter().any(|op| op.starts_with(prefix))
}

#[test]
fn regression_ui_view_on_touch_attaches_a_touch_handler() {
    let ops = mount_ops(|| ui! {
        view(on_touch = |_e| TouchResponse::CONSUMED) {
            text { "press" }
        }
    });
    assert!(
        has_op(&ops, "install_touch_handler"),
        "`view(on_touch = …)` must install a touch handler; ops:\n{}",
        ops.join("\n")
    );
}

#[test]
fn regression_ui_view_on_hover_and_on_wheel_attach() {
    let ops = mount_ops(|| ui! {
        view(
            on_hover = |_inside: bool| {},
            on_wheel = |_e| TouchResponse::IGNORED,
        ) {}
    });
    assert!(has_op(&ops, "install_hover_handler"), "on_hover dropped; ops:\n{}", ops.join("\n"));
    assert!(has_op(&ops, "install_wheel_handler"), "on_wheel dropped; ops:\n{}", ops.join("\n"));
}

#[test]
fn regression_ui_view_bind_fills_the_ref() {
    let h = Harness::new();
    let r: Ref<ViewHandle> = Ref::new();
    let realized = h.world.enter(|| {
        let el: Element = ui! { view(bind = r.clone()) {} };
        realize(&h.backend, &h.registry, el)
    });
    h.flush();
    assert!(r.is_mounted(), "`view(bind = r)` must fill the ref at mount");
    drop(realized);
}

#[test]
fn regression_ui_image_on_load_and_on_error_attach() {
    let loaded = Rc::new(std::cell::Cell::new(false));
    let seen = loaded.clone();
    let ops = mount_ops(move || ui! {
        image(
            src = "https://example.com/a.png",
            on_load = move |_e| seen.set(true),
            on_error = || {},
        )
    });
    assert!(
        has_op(&ops, "install_image_load_handler"),
        "`image(on_load = …)` must install a load handler; ops:\n{}",
        ops.join("\n")
    );
    assert!(
        has_op(&ops, "install_image_error_handler"),
        "`image(on_error = …)` must install an error handler; ops:\n{}",
        ops.join("\n")
    );
    let _ = loaded;
}

/// Control: a `view` that names none of them installs none of them, so
/// the assertions above are measuring the props and not the harness.
#[test]
fn a_bare_view_installs_no_input_handlers() {
    let ops = mount_ops(|| ui! { view() { text { "x" } } });
    assert!(!has_op(&ops, "install_touch_handler"), "{}", ops.join("\n"));
    assert!(!has_op(&ops, "install_hover_handler"), "{}", ops.join("\n"));
}
