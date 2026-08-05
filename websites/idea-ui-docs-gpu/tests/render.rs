//! REGRESSION: `idea-ui-docs` — a full swap-navigator + AppShell app —
//! must render on the wgpu (GPU) backend.
//!
//! Two things this pins, both of which have been broken before:
//!
//! 1. **A navigator app can be hosted on the GPU backend at all.** There
//!    used to be no wgpu navigator handler, so `create_navigator` hit the
//!    "External/Navigator not registered" panic; and the only headless
//!    skin reported an empty platform identity (mobile branch, no desktop
//!    chrome). The navigator is a vocabulary built-in now
//!    (`register_builtins`), and `NativeSkin(MacOs)` +
//!    `Screenshotter::with_color_scheme_and_skin` supply the desktop
//!    identity.
//! 2. **The desktop LAYOUT shape** — a pinned sidebar column beside the
//!    body — which depends on the AppShell's `__bp_lg` breakpoint overlay
//!    resolving from the live viewport width on a non-CSS backend.
//!
//! It drives the same offscreen path `crates/dev/newcore-gpu-smoke`'s
//! `NEWCORE_SMOKE_HEADLESS` mode uses: `render_wgpu::newcore::start` on
//! the headless `Screenshotter`'s backend (real Metal/Vulkan device, same
//! shaders as the window, PNG/RGBA readback). `Screenshotter::mount` is
//! the old walker's entry and is gone.
//!
//! Captures to the OS temp dir under `IDEALYST_DUMP_PNG=1`.

use std::rc::Rc;

use render_wgpu::headless::Screenshotter;
use render_wgpu::{newcore, NativeSkin};
use runtime_shared::{ColorScheme, Platform};

#[test]
fn idea_ui_docs_renders_on_wgpu_backend() {
    // Desktop logical size — wide enough for idea-ui-docs to pin its
    // AppShell sidebar (`install_breakpoints` sets `lg_min: 900.0`).
    let (w, h) = (1280u32, 832u32);

    let skin = Rc::new(NativeSkin::new(Platform::MacOs));
    let mut shot = match Screenshotter::with_color_scheme_and_skin(w, h, ColorScheme::Light, skin) {
        Ok(s) => s,
        // A headless GPU/software adapter isn't always available in every
        // CI sandbox; skip rather than fail spuriously when there's no
        // device at all (the windowed path is the real target anyway).
        Err(e) => {
            eprintln!("skipping: no headless adapter available: {e}");
            return;
        }
    };

    // Mount the real app on the Screenshotter's backend and rasterize a
    // frame. `register_scene_extensions` is the app's own boot seam
    // (codeblock + table payload handlers); without it the docs pages'
    // code panels and PropsTables have no registry entry and realize
    // panics. A panic anywhere here fails the test.
    //
    // The returned app state owns the World — hold it for the whole test
    // so the tree isn't torn down before the captures.
    let _app = newcore::start(
        shot.backend(),
        idea_ui_docs::register_scene_extensions,
        idea_ui_docs::app,
    );
    let rgba = shot.capture_rgba();

    assert_eq!(rgba.len(), (w * h * 4) as usize, "RGBA buffer size");

    // The frame must not be a single flat color: a working render draws
    // the sidebar, header, and page content over the background. A
    // blank/failed mount yields one uniform color.
    let first = &rgba[0..4];
    let any_different = rgba
        .chunks_exact(4)
        .any(|px| px[0] != first[0] || px[1] != first[1] || px[2] != first[2]);
    assert!(
        any_different,
        "rendered frame is a single flat color — the app tree did not render"
    );

    // LAYOUT REGRESSION: assert the *desktop* shape — a pinned sidebar
    // column on the left AND the body content beside it. A pixel is "ink"
    // if it's clearly off the near-white background (min channel < 200),
    // which catches text and colored chrome.
    //
    // The AppShell pins its sidebar via the `__bp_lg` breakpoint overlay
    // (real StyleRules, resolved from the live viewport width on non-CSS
    // backends). If the overlay failed to apply, the panel would sit
    // translated off-canvas and the content full-bleed — the left band at
    // sidebar height would be empty background. Both bands must carry ink
    // in the same vertical region (the hero/catalog band, y∈[140,360]).
    let ink_in = |x0: u32, x1: u32, y0: u32, y1: u32| -> usize {
        let mut n = 0;
        for y in y0..y1 {
            for x in x0..x1 {
                let i = ((y * w + x) * 4) as usize;
                let (r, g, b) = (rgba[i], rgba[i + 1], rgba[i + 2]);
                if r.min(g).min(b) < 200 {
                    n += 1;
                }
            }
        }
        n
    };
    let sidebar_ink = ink_in(16, 240, 140, 360);
    let body_ink = ink_in(340, 1100, 140, 360);
    assert!(
        sidebar_ink > 200,
        "sidebar column rendered no content (ink={sidebar_ink}) — \
         the pinned sidebar is missing"
    );
    assert!(
        body_ink > 200,
        "body column rendered no content at sidebar height (ink={body_ink}) \
         — navigator collapsed to one column instead of sidebar+body"
    );

    // Optional PNG dump for manual inspection — only when explicitly
    // requested, so a normal `cargo test` run never litters the tree.
    // `IDEALYST_DUMP_PNG=1 cargo test -p idea-ui-docs-gpu` writes it to
    // the OS temp dir.
    if std::env::var_os("IDEALYST_DUMP_PNG").is_some() {
        if let Ok(png) = shot.capture_png() {
            let path = std::env::temp_dir().join("idea-ui-docs-gpu.png");
            if std::fs::write(&path, &png).is_ok() {
                eprintln!("wrote {} ({} bytes, {w}x{h})", path.display(), png.len());
            }
        }
    }
}
