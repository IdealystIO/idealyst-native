//! The `text_input` soft-keyboard hint on the AUTHOR surface: `ui!`
//! accepts `keyboard = …` (a plain kind or a live `Signal`), and the kind
//! is spelled `runtime_core::KeyboardKind` — no `runtime-shared`
//! dependency. A dropped re-export or a missing macro prop would leave the
//! vocabulary tests green while every app failed to compile, so this file
//! is the compile-time pin.

use runtime_core::{signal, ui, Element, KeyboardKind};

#[allow(dead_code)]
fn email_and_live_fields() -> Element {
    let email = signal(String::new());
    let kind = signal(KeyboardKind::Number);
    ui! {
        view() {
            text_input(value = email, on_change = move |s| email.set(s), keyboard = KeyboardKind::Email)
            text_input(value = email, keyboard = kind, secure = true)
        }
    }
}

#[test]
fn keyboard_kind_is_nameable_from_runtime_core() {
    assert_eq!(KeyboardKind::default(), KeyboardKind::Default);
    assert_eq!(KeyboardKind::ALL.len(), 6);
    let _ = email_and_live_fields as fn() -> Element;
}
