//! Minifies the embedded JS runtime shims (`runtime/js/*.js`) into
//! `OUT_DIR/js-min/` so `defaults.rs`'s `include_str!` sites embed
//! comment-free, whitespace-trimmed source instead of the annotated
//! originals. The hand-written shims are ~50% comments — shipping them
//! verbatim put ~33 KB of commentary into every all-off wasm (54 KB
//! all-on). The repo files stay fully commented; only the embedded
//! copy is stripped. Transform semantics (and their safety argument)
//! live in `build_support/js_min.rs`, pinned by `tests/minify_shims.rs`.

include!("build_support/js_min.rs");

fn main() {
    let js_dir = std::path::Path::new("runtime/js");
    let out_dir = std::path::PathBuf::from(std::env::var("OUT_DIR").unwrap()).join("js-min");
    std::fs::create_dir_all(&out_dir).expect("create OUT_DIR/js-min");
    println!("cargo:rerun-if-changed=runtime/js");
    println!("cargo:rerun-if-changed=build_support/js_min.rs");
    for entry in std::fs::read_dir(js_dir).expect("read runtime/js") {
        let path = entry.expect("dir entry").path();
        if path.extension().and_then(|e| e.to_str()) != Some("js") {
            continue;
        }
        let src = std::fs::read_to_string(&path)
            .unwrap_or_else(|e| panic!("read {}: {e}", path.display()));
        let min = minify_js(&src);
        let out = out_dir.join(path.file_name().unwrap());
        std::fs::write(&out, min).unwrap_or_else(|e| panic!("write {}: {e}", out.display()));
    }
}
