//! Static site generation, one fully-localized document per locale.
//!
//!   cargo run -p i18n-demo --example ssg
//!
//! For each locale we set the active locale, render `i18n_demo::app()` on
//! the SSR backend, and write `dist/<code>/index.html`. Bundled locales
//! render from their compiled-in strings; the opt-in `ja` locale has its
//! pack installed from `locales/ja.json` first (and that file is copied to
//! `dist/locales/ja.json` for the web client's runtime fetch).
//!
//! This proves the SSR/SSG path end-to-end with no browser: the strings
//! are baked into the first paint, per locale, tied to a path.

#![cfg(not(target_arch = "wasm32"))]

use backend_ssr::{render_document, render_path};
use i18n_demo::t::Locale;

fn main() {
    let out = "dist";
    std::fs::create_dir_all(format!("{out}/locales")).expect("create dist/locales");

    // Opt-in packs are static files: SSG inlines them for first paint AND
    // serves them for the client's on-demand fetch.
    let ja_json = include_str!("../locales/ja.json");
    std::fs::write(format!("{out}/locales/ja.json"), ja_json).expect("write ja pack");

    for &locale in Locale::ALL {
        let code = locale.code().to_string();
        let ja = ja_json.to_string();

        // Each locale renders on its own thread: the reactive arena, locale
        // signal, and pack registry are all thread-local, so one page's
        // render is fully isolated from the next.
        let doc = std::thread::spawn(move || {
            i18n::set_locale_code(&code);
            if locale.is_lazy() {
                i18n::install_pack_json(&code, &ja).expect("ja.json is a flat string map");
            }
            let page = render_path("/", i18n_demo::app);
            // Static SSR/SEO preview (no hydration bundle script).
            render_document(&page, None, None)
        })
        .join()
        .expect("render thread panicked");

        let dir = format!("{out}/{}", locale.code());
        std::fs::create_dir_all(&dir).expect("create locale dir");
        let file = format!("{dir}/index.html");
        std::fs::write(&file, &doc).expect("write html");
        println!("  {:<3} -> {file} ({} bytes)", locale.code(), doc.len());
    }
}
