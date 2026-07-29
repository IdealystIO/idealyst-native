//! SSG parity dump — renders a set of site routes headlessly and writes
//! the HTML + head CSS to `target/…/ssg-{oldcore,newcore}/`, one run per
//! core, so the two outputs can be diffed byte-for-byte (the idea-lite
//! migration's website SSR gate):
//!
//! ```text
//! # old core
//! cargo test -p website --features ssr --test ssg_parity
//! # new core (backend-ssr's newcore entries are drop-in substitutes)
//! cargo test -p website --no-default-features \
//!     --features new-core,ssr,backend-ssr/new-core --test ssg_parity
//! # then diff target/tmp/website-ssg/oldcore/ vs …/newcore/
//! ```
//!
//! Route choice: `/roadmap`, `/further-reading`, `/comparisons` carry no
//! code panels, so their output is expected byte-identical. `/` includes
//! one code panel, where a KNOWN one-sided difference exists: the old
//! core has no host-side codeblock SSR handler (the codeblock SDK only
//! registers its DOM handler on wasm32), so old SSR emits the External
//! placeholder there, while the new core renders the real `<pre>`/span
//! DOM through the website's scene-registry handler (src/newcore.rs).

#![cfg(feature = "ssr")]

const PATHS: &[(&str, &str)] = &[
    ("/", "home"),
    ("/roadmap", "roadmap"),
    ("/further-reading", "further-reading"),
    ("/comparisons", "comparisons"),
];

#[test]
fn dump_ssg_pages() {
    let core = if cfg!(feature = "new-core") {
        "newcore"
    } else {
        "oldcore"
    };
    let dir = std::path::Path::new(env!("CARGO_TARGET_TMPDIR"))
        .join("website-ssg")
        .join(core);
    std::fs::create_dir_all(&dir).expect("create dump dir");

    for (path, name) in PATHS {
        #[cfg(not(feature = "new-core"))]
        let page = backend_ssr::render_path_with(
            path,
            |b| website::register_ssr_extensions(b),
            website::app,
        );
        #[cfg(feature = "new-core")]
        let page = backend_ssr::newcore::render_path_with(
            path,
            website::newcore::register_handlers,
            website::app,
        );

        assert!(
            !page.html.is_empty(),
            "SSG render of {path} produced empty html"
        );
        std::fs::write(dir.join(format!("{name}.html")), &page.html).unwrap();
        std::fs::write(dir.join(format!("{name}.head.css")), &page.head_css).unwrap();
        println!(
            "[ssg-{core}] {path} -> {} bytes html / {} bytes head css",
            page.html.len(),
            page.head_css.len()
        );
    }
    println!("[ssg-{core}] wrote {}", dir.display());
}
