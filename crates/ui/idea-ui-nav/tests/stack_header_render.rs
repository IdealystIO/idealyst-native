//! `StackHeader` render coverage: it draws the active screen's title/slots from
//! `StackHeaderState`, shows the back arrow when wired + enabled, and
//! self-suppresses when the state is `native` (a native bar owns the header) or
//! `hidden`.

use std::any::Any;
use std::cell::RefCell;
use std::rc::Rc;
use std::sync::OnceLock;

use idea_theme::theme::{install_idea_theme, light_theme};
use idea_ui_nav::{StackHeader, StackHeaderProps};
use mock_backend::MockBackend;
use runtime_core::primitives::navigator::StackHeaderState;
use runtime_core::Reactive;

fn theme() {
    static INSTALLED: OnceLock<()> = OnceLock::new();
    INSTALLED.get_or_init(|| install_idea_theme(light_theme()));
}

fn mount_header(state: StackHeaderState, show_back: bool) -> (Rc<RefCell<MockBackend>>, Box<dyn Any>) {
    let backend = Rc::new(RefCell::new(MockBackend::new()));
    let state_for = state;
    let on_back: Rc<dyn Fn()> = Rc::new(|| {});
    let owner: Box<dyn Any> = Box::new(runtime_core::mount(backend.clone(), move || {
        StackHeader(StackHeaderProps {
            state: Reactive::Static(Some(state_for.clone())),
            show_back: Reactive::Static(show_back),
            on_back: Some(on_back.clone()),
        })
    }));
    (backend, owner)
}

fn header(title: &str) -> StackHeaderState {
    StackHeaderState { title: title.to_string(), ..Default::default() }
}

#[test]
fn draws_title_and_back_when_enabled() {
    theme();
    let (backend, _owner) = mount_header(header("Detail"), true);
    let b = backend.borrow();
    assert!(b.contains_text("Detail"), "title renders:\n{}", b.dump());
    assert!(b.contains_text("‹"), "back chevron shows when wired + enabled:\n{}", b.dump());
}

#[test]
fn hides_back_at_root() {
    theme();
    let (backend, _owner) = mount_header(header("Home"), false);
    let b = backend.borrow();
    assert!(b.contains_text("Home"), "title renders:\n{}", b.dump());
    assert!(!b.contains_text("‹"), "no back chevron at root (show_back=false):\n{}", b.dump());
}

#[test]
fn renders_left_and_right_slots() {
    theme();
    let state = StackHeaderState {
        title: "Item".to_string(),
        left: Some(runtime_core::primitives::navigator::HeaderButton::text("Cancel")),
        right: Some(runtime_core::primitives::navigator::HeaderButton::text("Save")),
        ..Default::default()
    };
    let (backend, _owner) = mount_header(state, false);
    let b = backend.borrow();
    assert!(b.contains_text("Cancel"), "left slot renders:\n{}", b.dump());
    assert!(b.contains_text("Item"), "title renders:\n{}", b.dump());
    assert!(b.contains_text("Save"), "right slot renders:\n{}", b.dump());
}

#[test]
fn self_suppresses_when_native() {
    theme();
    // native = true → a native bar owns the header, so the drawn header is empty.
    let state = StackHeaderState { title: "Native".to_string(), native: true, ..Default::default() };
    let (backend, _owner) = mount_header(state, true);
    let b = backend.borrow();
    assert!(!b.contains_text("Native"), "native header self-suppresses:\n{}", b.dump());
    assert!(!b.contains_text("‹"), "no back arrow when native:\n{}", b.dump());
}

#[test]
fn suppresses_when_hidden() {
    theme();
    let state = StackHeaderState { title: "Hidden".to_string(), hidden: true, ..Default::default() };
    let (backend, _owner) = mount_header(state, false);
    let b = backend.borrow();
    assert!(!b.contains_text("Hidden"), "hidden header renders nothing:\n{}", b.dump());
}
