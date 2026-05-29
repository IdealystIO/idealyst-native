//! SSR the real marketing site headlessly at each route.
//!
//!   cargo run -p website --example ssr
//!
//! Runs `website::app()` (the same entry the web bundle mounts) on the
//! host against `SsrBackend`, once per URL, and writes each rendered
//! document to `/tmp/ssr-website/<name>.html`. Proves render-at-path on
//! a real, complex DrawerNavigator app.

#![cfg(not(target_arch = "wasm32"))]

use backend_ssr::{render_document, render_path_with};

fn main() {
    let routes: &[(&str, &str)] = &[
        ("home", "/"),
        ("features", "/features"),
        ("cross-platform", "/features/cross-platform"),
        ("performance", "/features/performance"),
        ("type-safety", "/features/type-safety"),
        ("ssr", "/features/ssr"),
        ("install", "/install"),
        ("quickstart", "/quickstart"),
        ("concepts", "/concepts"),
        ("why-rust", "/why-rust"),
        ("demo", "/demo"),
        ("backends", "/backends"),
        ("targets", "/targets"),
    ];

    let out_dir = "/tmp/ssr-website";
    std::fs::create_dir_all(out_dir).expect("create out dir");

    println!("Rendering {} routes via SSR -> {out_dir}/", routes.len());
    for (name, path) in routes {
        // Each route renders on its own thread: the reactive arena, token
        // registry, and scheduler queue are thread-local, so a fresh
        // thread fully isolates one page's render from the next (no
        // cross-render signal-slot recycling).
        let path = path.to_string();
        let doc = std::thread::spawn(move || {
            let page = render_path_with(
                &path,
                |b| {
                    drawer_navigator::chrome::register(b);
                    idea_codeblock::register(b);
                },
                website::app,
            );
            // Pure rendered screen (no bundle script) — these files are
            // opened directly as a static SSR/SEO preview, not hydrated.
            render_document(&page, None)
        })
        .join()
        .expect("render thread panicked");

        let file = format!("{out_dir}/{name}.html");
        std::fs::write(&file, &doc).expect("write html");
        println!("  /{name:<16} -> {:>7} bytes  {file}", doc.len());
    }
}
