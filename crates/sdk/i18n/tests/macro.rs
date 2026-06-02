//! Integration tests for the `i18n!` macro: codegen correctness plus the
//! reactive behavior (a generated message recomputes when the locale or an
//! opt-in pack changes).

use std::collections::HashMap;

mod t {
    i18n::i18n! {
        locales: { En = "en" (default), Fr = "fr", Ja = "ja" (lazy) }

        greeting(name) {
            En: "Hello, {name}",
            Fr: "Bonjour, {name}",
        }

        hello {
            En: "Hi",
            Fr: "Salut",
        }

        items(count) {
            En: "{count} items",
            Fr: "{count} articles",
        }
    }
}

#[test]
fn locale_enum_metadata() {
    assert_eq!(t::Locale::DEFAULT, t::Locale::En);
    assert_eq!(t::Locale::En.code(), "en");
    assert_eq!(t::Locale::Ja.code(), "ja");
    assert_eq!(t::Locale::from_code("fr"), Some(t::Locale::Fr));
    assert_eq!(t::Locale::from_code("zz"), None);
    assert!(t::Locale::Ja.is_lazy());
    assert!(!t::Locale::En.is_lazy());
    assert_eq!(t::Locale::ALL.len(), 3);
    // Ja is lazy, so only En + Fr are bundled.
    assert_eq!(t::Locale::BUNDLED, &[t::Locale::En, t::Locale::Fr]);
}

#[test]
fn bundled_locales_switch_and_interpolate() {
    t::init();
    let greet = t::greeting("Ada");
    assert_eq!(greet.get(), "Hello, Ada");

    t::set_locale(t::Locale::Fr);
    assert_eq!(greet.get(), "Bonjour, Ada");
    assert_eq!(t::hello().get(), "Salut");
    assert_eq!(t::items(3).get(), "3 articles");

    t::set_locale(t::Locale::En);
    assert_eq!(t::items(1).get(), "1 items");
    assert_eq!(t::current_locale(), t::Locale::En);
}

#[test]
fn opt_in_locale_falls_back_then_upgrades() {
    t::init();
    let greet = t::greeting("Ada");

    // Switching to a lazy locale with no pack installed and no loader:
    // messages fall back to the default (en) string.
    t::set_locale(t::Locale::Ja);
    assert_eq!(greet.get(), "Hello, Ada");

    // Install the pack: the same value upgrades to the localized string.
    let mut pack = HashMap::new();
    pack.insert("greeting".to_string(), "こんにちは、{name}".to_string());
    i18n::install_pack("ja", pack);
    assert_eq!(greet.get(), "こんにちは、Ada");

    // A message the pack doesn't translate still falls back to default.
    assert_eq!(t::hello().get(), "Hi");

    t::set_locale(t::Locale::En);
}

#[test]
fn reactive_text_recomputes_inside_an_effect() {
    use runtime_core::{Effect, Signal};

    t::init();
    let greet = t::greeting("Ada");

    // Mirror what a `text()` node does: read the reactive value inside an
    // Effect so the runtime subscribes it to the locale signal.
    let captured: Signal<String> = Signal::new(String::new());
    let _effect = Effect::new({
        let greet = greet.clone();
        move || captured.set(greet.get())
    });
    assert_eq!(captured.get(), "Hello, Ada");

    // Changing the locale must re-fire the effect (no manual re-read).
    t::set_locale(t::Locale::Fr);
    assert_eq!(captured.get(), "Bonjour, Ada");

    // Installing an opt-in pack while on that locale must also re-fire.
    t::set_locale(t::Locale::Ja);
    assert_eq!(captured.get(), "Hello, Ada"); // fallback first
    let mut pack = HashMap::new();
    pack.insert("greeting".to_string(), "やあ、{name}".to_string());
    i18n::install_pack("ja", pack);
    assert_eq!(captured.get(), "やあ、Ada"); // upgraded via pack epoch

    t::set_locale(t::Locale::En);
}

#[test]
fn custom_formatter_swaps_in() {
    t::init();
    // A trivial "uppercase the whole rendered string" formatter stands in
    // for a richer Fluent/ICU layer.
    i18n::install_formatter(|template, args| {
        let mut s = template.to_string();
        for (k, v) in args {
            s = s.replace(&format!("{{{k}}}"), v);
        }
        s.to_uppercase()
    });
    assert_eq!(t::greeting("ada").get(), "HELLO, ADA");
    i18n::clear_formatter();
    assert_eq!(t::greeting("ada").get(), "Hello, ada");
}
