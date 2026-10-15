//! `text_input`'s soft-keyboard hint in server-rendered HTML. The first
//! paint must already carry the mobile keyboard (`inputmode`, plus the
//! autofill / auto-correct attributes) so a phone that focuses the field
//! before hydration gets the right keyboard, and the hydrating web backend
//! adopts an `<input>` whose attributes match what it would write.

use runtime_shared::primitives::text_input::KeyboardKind;
use runtime_vocabulary::builders::{text_input, view};

fn html(build: impl FnOnce() -> runtime_scene::Element) -> String {
    backend_ssr::newcore::render_path("/", build).html
}

/// REGRESSION: `text_input` had no keyboard hint, so an email field opened
/// the default keyboard (no `@` / `.com` row) and a phone field got no
/// digit pad. Every kind renders its `inputmode`, and none touches `type`
/// (owned by `secure`).
#[test]
fn regression_ssr_text_input_emits_inputmode_per_keyboard_kind() {
    let cases = [
        (KeyboardKind::Email, Some("email"), Some("email")),
        (KeyboardKind::Phone, Some("tel"), Some("tel")),
        (KeyboardKind::Number, Some("numeric"), None),
        (KeyboardKind::Decimal, Some("decimal"), None),
        (KeyboardKind::Url, Some("url"), Some("url")),
        (KeyboardKind::Default, None, None),
    ];
    for (kind, inputmode, autocomplete) in cases {
        let out = html(|| view().child(text_input().keyboard(kind)).build());
        assert!(out.contains(r#"type="text""#), "{kind:?}: {out}");
        match inputmode {
            Some(m) => assert!(out.contains(&format!(r#"inputmode="{m}""#)), "{kind:?}: {out}"),
            None => assert!(!out.contains("inputmode="), "{kind:?}: {out}"),
        }
        match autocomplete {
            Some(a) => assert!(out.contains(&format!(r#"autocomplete="{a}""#)), "{kind:?}: {out}"),
            None => assert!(!out.contains("autocomplete="), "{kind:?}: {out}"),
        }
        let off = matches!(kind, KeyboardKind::Email | KeyboardKind::Url);
        assert_eq!(out.contains(r#"autocapitalize="off""#), off, "{kind:?}: {out}");
        assert_eq!(out.contains(r#"spellcheck="false""#), off, "{kind:?}: {out}");
    }
}

/// A masked field keeps its keyboard hint: `secure` owns `type`, the hint
/// owns `inputmode`, so a numeric PIN renders as both.
#[test]
fn ssr_text_input_keyboard_composes_with_secure() {
    let out = html(|| {
        view()
            .child(text_input().secure(true).keyboard(KeyboardKind::Number))
            .build()
    });
    assert!(out.contains(r#"type="password""#), "{out}");
    assert!(out.contains(r#"inputmode="numeric""#), "{out}");
}

/// A live kind resolves at render time like a live `secure` does.
#[test]
fn ssr_text_input_live_keyboard_renders_its_current_kind() {
    let out = html(|| view().child(text_input().keyboard_dyn(|| KeyboardKind::Url)).build());
    assert!(out.contains(r#"inputmode="url""#), "{out}");
}
