//! Server-render `hydration_demo::app()` to a complete HTML document and
//! write it to a serve directory, alongside which the wasm bundle is
//! staged. Open it in a browser (served over HTTP) to see the client
//! HYDRATE the server markup (adopt the existing DOM).
//!
//!   cargo run -p hydration-demo --example ssr
//!   # then build the wasm into <out>/pkg and serve <out>:
//!   #   cargo build -p hydration-demo --target wasm32-unknown-unknown --release
//!   #   wasm-bindgen --target web --no-typescript \
//!   #     --out-dir <out>/pkg --out-name hydration_demo \
//!   #     target/wasm32-unknown-unknown/release/hydration_demo.wasm
//!   #   (python3 -m http.server --directory <out>)

#![cfg(not(target_arch = "wasm32"))]

use backend_ssr::{render_document, render_path};

fn main() {
    let out = std::env::args().nth(1).unwrap_or_else(|| "/tmp/hydration-demo".into());
    std::fs::create_dir_all(&out).expect("create out dir");

    // Render the app at "/" and wrap it with the bundle script so the
    // page boots `/pkg/hydration_demo.js`, which hydrates `#app`.
    let page = render_path("/", hydration_demo::app);
    let doc = render_document(&page, Some("/pkg/hydration_demo.js"));

    let path = format!("{out}/index.html");
    std::fs::write(&path, &doc).expect("write index.html");
    println!("wrote {} ({} bytes)", path, doc.len());
    println!("next: build the wasm into {out}/pkg and serve {out} over HTTP");
}
