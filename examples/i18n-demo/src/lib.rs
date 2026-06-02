//! `i18n` SDK demo.
//!
//! The whole translation catalog is declared inline in Rust via the
//! `i18n!` macro. `en`/`fr`/`es` are **bundled** (compiled in, switch
//! instantly and offline); `ja` is **opt-in** — its strings live in
//! `locales/ja.json` and load on demand (fetched on web, or installed by
//! the SSG build).
//!
//! Every message is strongly typed: a missing bundled translation or a
//! placeholder that doesn't match an argument would fail to compile.

use idea_ui::{
    install_idea_theme, light_theme, typography_kind, Button, Stack, StackAxis, StackGap,
    StackPadding, Typography,
};
use runtime_core::{ui, Element, IntoElement};
use std::rc::Rc;

/// The translation catalog. `pub` so the SSG example can read `Locale`.
pub mod t {
    i18n::i18n! {
        locales: { En = "en" (default), Fr = "fr", Es = "es", Ja = "ja" (lazy) }

        greeting(name) {
            En: "Hello, {name}",
            Fr: "Bonjour, {name}",
            Es: "Hola, {name}",
        }

        tagline {
            En: "Type-safe i18n, powered by core reactivity.",
            Fr: "Internationalisation typée, propulsée par la réactivité du cœur.",
            Es: "i18n con tipado seguro, impulsada por la reactividad del núcleo.",
        }

        items(count) {
            En: "{count} items in your cart",
            Fr: "{count} articles dans votre panier",
            Es: "{count} artículos en tu carrito",
        }

        pick_language {
            En: "Choose a language",
            Fr: "Choisissez une langue",
            Es: "Elige un idioma",
        }
    }
}

/// Backend extension registration hook the CLI's per-target wrapper calls
/// before mount. i18n is pure string replacement — it registers no SDK
/// externals — so this is a generic no-op valid for every backend.
pub fn register_extensions<B: runtime_core::Backend>(_backend: &mut B) {}

/// Recorder-side registration for the runtime-server sidecar. Gated by
/// `sidecar` (set only by the generated sidecar wrapper) so device/web
/// builds never pull `dev-server`. No SDK externals to register here.
#[cfg(feature = "sidecar")]
pub fn register_extensions_recorder(_backend: &mut dev_server::WireRecordingBackend) {}

/// The app the web client mounts (and the SSG example renders per locale).
pub fn app() -> Element {
    install_idea_theme(light_theme());

    // Install the opt-in-locale loader. It must be installed regardless of
    // where the tree runs: in the default `dev --web` (runtime-server) mode
    // the app tree executes on the HOST and only proxies Backend calls to
    // the browser, so a `cfg(wasm32)`-only install would never fire.
    //
    //   - real wasm (production `build --web`): fetch `/locales/<code>.json`
    //     from the served bundle over the network.
    //   - host / native (runtime-server dev, `--local`, SSR): read the pack
    //     straight off disk — there's no browser origin to fetch against.
    #[cfg(target_arch = "wasm32")]
    web_sys::console::log_1(&"i18n-demo: installing pack loader".into());
    i18n::set_pack_loader(|code: &str| {
        #[cfg(target_arch = "wasm32")]
        {
            web_sys::console::log_1(&format!("i18n-demo: loader CALLED for {code}").into());
            i18n::net_pack_loader("/locales")(code);
        }

        #[cfg(not(target_arch = "wasm32"))]
        {
            let path = format!("{}/locales/{code}.json", env!("CARGO_MANIFEST_DIR"));
            match std::fs::read_to_string(&path) {
                Ok(json) => {
                    let _ = i18n::install_pack_json(code, &json);
                }
                Err(e) => runtime_core::logging::log(
                    runtime_core::logging::LogLevel::Error,
                    &format!("i18n-demo: could not read locale pack {path}: {e}"),
                ),
            }
        }
    });

    // `on_click` is `Rc<dyn Fn()>`; bind each handler with that type so the
    // macro's `.into()` coercion is an identity (a bare closure literal
    // wouldn't unsize to `Rc<dyn Fn()>`).
    let pick_en: Rc<dyn Fn()> = Rc::new(|| t::set_locale(t::Locale::En));
    let pick_fr: Rc<dyn Fn()> = Rc::new(|| t::set_locale(t::Locale::Fr));
    let pick_es: Rc<dyn Fn()> = Rc::new(|| t::set_locale(t::Locale::Es));
    let pick_ja: Rc<dyn Fn()> = Rc::new(|| t::set_locale(t::Locale::Ja));

    ui! {
        Stack(gap = StackGap::Lg, padding = StackPadding::Lg) {
            Typography(content = t::greeting("Ada"), kind = typography_kind::H2)
            Typography(content = t::tagline(), kind = typography_kind::Body)
            Typography(content = t::items(3), kind = typography_kind::Body)
            Typography(content = t::pick_language(), kind = typography_kind::Caption)
            Stack(axis = StackAxis::Row, gap = StackGap::Sm) {
                Button(label = "English",  on_click = pick_en)
                Button(label = "Français", on_click = pick_fr)
                Button(label = "Español",  on_click = pick_es)
                Button(label = "日本語",   on_click = pick_ja)
            }
        }
    }
    .into_element()
}
