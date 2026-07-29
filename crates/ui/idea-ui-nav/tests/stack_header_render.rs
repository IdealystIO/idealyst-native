//! `StackHeader` render coverage: it draws the active screen's title/slots from
//! `StackHeaderState`, shows the back arrow when wired + enabled, and
//! self-suppresses when the state is `native` (a native bar owns the header) or
//! `hidden`.
//!
//! Dual-core: the TEST BODIES are shared; only the mount harness forks —
//! old core mounts on `MockBackend` via the walker, new core realizes on
//! the scene-parity recorder (`LegacyBridge<FullRecorder>`). Both
//! reduce to a "rendered text dump" string the assertions grep.

// The new-core leg needs the scene-parity harness, which lives behind
// `new-core-harness` (NOT `new-core` — a consumer app graph must never
// drag scene-parity, whose old-side scenarios pin the SDKs' old-core
// surface; see Cargo.toml). Plain `new-core` compiles this file empty.
#![cfg(any(not(feature = "new-core"), feature = "new-core-harness"))]

use std::sync::OnceLock;

use idea_theme::theme::{install_idea_theme, light_theme};
use idea_ui_nav::{StackHeader, StackHeaderProps};
use runtime_core::primitives::navigator::{HeaderButton, StackHeaderState};

// The new-core alias: same-source `runtime_core::…` paths in this test
// resolve against the glue facade (see idea-ui-nav's lib.rs note).
#[cfg(feature = "new-core")]
extern crate runtime_facade as runtime_core;

use runtime_core::Reactive;

fn header(title: &str) -> StackHeaderState {
    StackHeaderState { title: title.to_string(), ..Default::default() }
}

/// Mount the header and return a text dump of everything rendered.
#[cfg(not(feature = "new-core"))]
fn rendered(state: StackHeaderState, show_back: bool) -> String {
    use mock_backend::MockBackend;
    use std::any::Any;
    use std::cell::RefCell;
    use std::rc::Rc;

    static INSTALLED: OnceLock<()> = OnceLock::new();
    INSTALLED.get_or_init(|| install_idea_theme(light_theme()));

    let backend = Rc::new(RefCell::new(MockBackend::new()));
    let on_back: Rc<dyn Fn()> = Rc::new(|| {});
    let _owner: Box<dyn Any> = Box::new(runtime_core::mount(backend.clone(), move || {
        StackHeader(StackHeaderProps {
            state: Reactive::Static(Some(state.clone())),
            show_back: Reactive::Static(show_back),
            on_back: Some(on_back.clone()),
        })
    }));
    let dump = backend.borrow().dump();
    dump
}

#[cfg(feature = "new-core")]
fn rendered(state: StackHeaderState, show_back: bool) -> String {
    use runtime_scene::{realize, Registry};
    use runtime_vocabulary::LegacyBridge;
    use runtime_world::World;
    use scene_parity::full::FullRecorder;
    use scene_parity::{Mode, Recorder};
    use std::cell::RefCell;
    use std::rc::Rc;

    // The theme installs per WORLD on the new core, so it runs inside
    // each mount's `enter` (idempotent — same tokens each time).
    let _ = OnceLock::<()>::new();

    type Bridged = LegacyBridge<FullRecorder>;
    let rec = Recorder::default();
    let backend: Rc<RefCell<Bridged>> = Rc::new(RefCell::new(LegacyBridge(FullRecorder::new(
        rec.clone(),
        Mode::Spliced,
    ))));
    let mut registry: Registry<Bridged> = Registry::new();
    runtime_vocabulary::register_builtins(&mut registry);
    let registry = Rc::new(registry);

    let world = World::new();
    let on_back: Rc<dyn Fn()> = Rc::new(|| {});
    let root = world.enter(|| {
        install_idea_theme(light_theme());
        StackHeader(StackHeaderProps {
            state: Reactive::Static(Some(state.clone())),
            show_back: Reactive::Static(show_back),
            on_back: Some(on_back.clone()),
        })
    });
    let _realized = world.enter(|| realize(&backend, &registry, root));
    world.flush();
    rec.take_ops().join("\n")
}

fn contains(dump: &str, needle: &str) -> bool {
    dump.contains(needle)
}

#[test]
fn draws_title_and_back_when_enabled() {
    let d = rendered(header("Detail"), true);
    assert!(contains(&d, "Detail"), "title renders:\n{d}");
    assert!(contains(&d, "‹"), "back chevron shows when wired + enabled:\n{d}");
}

#[test]
fn hides_back_at_root() {
    let d = rendered(header("Home"), false);
    assert!(contains(&d, "Home"), "title renders:\n{d}");
    assert!(!contains(&d, "‹"), "no back chevron at root (show_back=false):\n{d}");
}

#[test]
fn renders_left_and_right_slots() {
    let state = StackHeaderState {
        title: "Item".to_string(),
        left: Some(HeaderButton::text("Cancel")),
        right: Some(HeaderButton::text("Save")),
        ..Default::default()
    };
    let d = rendered(state, false);
    assert!(contains(&d, "Cancel"), "left slot renders:\n{d}");
    assert!(contains(&d, "Item"), "title renders:\n{d}");
    assert!(contains(&d, "Save"), "right slot renders:\n{d}");
}

#[test]
fn self_suppresses_when_native() {
    // native = true → a native bar owns the header, so the drawn header is empty.
    let state = StackHeaderState { title: "Native".to_string(), native: true, ..Default::default() };
    let d = rendered(state, true);
    assert!(!contains(&d, "Native"), "native header self-suppresses:\n{d}");
    assert!(!contains(&d, "‹"), "no back arrow when native:\n{d}");
}

#[test]
fn suppresses_when_hidden() {
    let state = StackHeaderState { title: "Hidden".to_string(), hidden: true, ..Default::default() };
    let d = rendered(state, false);
    assert!(!contains(&d, "Hidden"), "hidden header renders nothing:\n{d}");
}
