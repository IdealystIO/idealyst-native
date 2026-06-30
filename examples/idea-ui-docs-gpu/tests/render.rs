//! REGRESSION: `idea-ui-docs` — a full `DrawerNavigator` app — must
//! render on the wgpu (GPU) backend via the backend-neutral primitive
//! `chrome` navigator handler, registered through the new
//! `RegisterNavigator` trait impl on `WgpuBackend`.
//!
//! Before the trait impl + `Screenshotter::with_color_scheme_and_skin`
//! existed, there was no way to host a navigator app on the GPU backend:
//! `create_navigator` would hit the "External/Navigator not registered"
//! panic (no wgpu drawer handler), and the only headless skin reported an
//! empty platform identity (mobile branch, no desktop chrome). This test
//! mounts the real app under a `NativeSkin(MacOs)` desktop identity and
//! asserts it rasterizes a non-trivial frame.
//!
//! Captures to `$CARGO_TARGET_TMPDIR/idea-ui-docs-gpu.png` for eyeballing.

use std::rc::Rc;

use render_wgpu::headless::Screenshotter;
use render_wgpu::NativeSkin;
use runtime_core::{ColorScheme, Platform};

#[test]
fn idea_ui_docs_renders_on_wgpu_backend() {
    // Desktop logical size — wide enough for idea-ui-docs to pin its
    // sidebar (`install_navigator_pin_width(900.0)`).
    let (w, h) = (1280u32, 832u32);

    let skin = Rc::new(NativeSkin::new(Platform::MacOs));
    let mut shot = match Screenshotter::with_color_scheme_and_skin(
        w,
        h,
        ColorScheme::Light,
        skin,
    ) {
        Ok(s) => s,
        // A headless GPU/software adapter isn't always available in every
        // CI sandbox; skip rather than fail spuriously when there's no
        // device at all (the windowed path is the real target anyway).
        Err(e) => {
            eprintln!("skipping: no headless adapter available: {e}");
            return;
        }
    };

    // The make-or-break step: register the DrawerNavigator's
    // backend-neutral desktop handler on the wgpu backend through the
    // generic `RegisterNavigator` path. This is the behavior the trait
    // impl unlocks — without it this call would not compile, and a mounted
    // navigator would panic at `create_navigator`.
    // `&mut *…` derefs the `RefMut` to a concrete `&mut WgpuBackend`; the
    // generic `register_native<B: RegisterNavigator>` infers `B` and won't
    // peel the `RefMut` for us.
    let backend = shot.backend();
    drawer_navigator::register_native(&mut *backend.borrow_mut());

    // Mount the real app and rasterize a frame. A panic here (e.g. an
    // unregistered External/Navigator leaf) fails the test.
    shot.mount(idea_ui_docs::app);
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
    // This is what distinguishes the `desktop` handler (StyleRules layout)
    // from the CSS-only `chrome` handler on a non-CSS backend: with the
    // latter the navigator collapsed to a single full-width column and the
    // body was pushed below the fold, so the right band at sidebar height
    // was empty background. Here, both bands must carry ink in the same
    // vertical region (the hero/catalog band, y∈[140,360]).
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
