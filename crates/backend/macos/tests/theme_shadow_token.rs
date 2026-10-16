//! A token-colored box shadow (a theme "glow") against real AppKit: after a
//! token swap, re-applying the same rules must repaint the layer's
//! `shadowColor` with the NEW token value, not the fallback the rules carry.
//!
//! The re-apply is what the theme cohort does on every swap; the rules are
//! the same `Rc` (token names are theme-stable), so the only thing that can
//! carry the new color is the backend resolving the token at apply time.
//!
//! `harness = false` for the same reason as `focus_attach`: AppKit views are
//! main-thread only, and libtest runs cases on worker threads.
//!
//! ```sh
//! cargo test -p backend-macos --test theme_shadow_token
//! ```

#[cfg(not(target_os = "macos"))]
fn main() {}

#[cfg(target_os = "macos")]
fn main() {
    imp::run();
}

#[cfg(target_os = "macos")]
mod imp {
    use std::rc::Rc;

    use backend_apple_core::cg::CGColorRef;
    use backend_macos::{MacosBackend, MacosNode};
    use objc2::rc::Retained;
    use objc2::{msg_send, msg_send_id};
    use objc2_app_kit::NSView;
    use objc2_foundation::{MainThreadMarker, NSObject};
    use runtime_shared::accessibility::AccessibilityProps;
    use runtime_shared::{
        install_tokens, update_tokens, Color, Shadow, StyleRules, TokenEntry, TokenValue, Tokenized,
    };
    use runtime_vocabulary::caps::{StyleOps, ViewOps};

    const GLOW: &str = "test-glow";
    const MAGENTA: &str = "#ff00ff";
    const CYAN: &str = "#00ffff";

    extern "C" {
        fn CGColorGetNumberOfComponents(color: *const std::ffi::c_void) -> usize;
        fn CGColorGetComponents(color: *const std::ffi::c_void) -> *const f64;
    }

    fn view_of(node: &MacosNode) -> Retained<NSView> {
        match node {
            MacosNode::View(v) => v.clone(),
            MacosNode::Label(l) => Retained::into_super(Retained::into_super(l.clone())),
        }
    }

    /// The layer's `shadowColor` as `(r, g, b)`.
    fn shadow_rgb(view: &NSView) -> (f64, f64, f64) {
        let layer: Retained<NSObject> = unsafe { msg_send_id![view, layer] };
        let cg: CGColorRef = unsafe { msg_send![&layer, shadowColor] };
        let cg = cg.0;
        assert!(!cg.is_null(), "the layer carries no shadowColor");
        let n = unsafe { CGColorGetNumberOfComponents(cg) };
        assert!(n >= 3, "expected an RGB(A) shadow color, got {n} components");
        let c = unsafe { std::slice::from_raw_parts(CGColorGetComponents(cg), n) };
        (c[0], c[1], c[2])
    }

    fn tokens(color: &str) -> Vec<TokenEntry> {
        vec![TokenEntry { name: GLOW, value: TokenValue::Color(Color(color.into())) }]
    }

    fn regression_token_shadow_retints_on_theme_swap(backend: &mut MacosBackend) {
        install_tokens(&tokens(MAGENTA));
        // The fallback is deliberately a THIRD color: a backend reading the
        // fallback instead of the installed token fails both assertions.
        let rules = Rc::new(StyleRules {
            shadow: Some(Shadow {
                x: 0.0,
                y: 0.0,
                blur: 12.0,
                color: Tokenized::token(GLOW, Color("#000000".into())),
            }),
            ..Default::default()
        });
        let node = backend.create_view(&AccessibilityProps::default());
        let view = view_of(&node);

        backend.apply_style(&node, &rules);
        let (r, g, b) = shadow_rgb(&view);
        assert!(r > 0.9 && g < 0.1 && b > 0.9, "first theme paints magenta, got ({r}, {g}, {b})");

        update_tokens(&tokens(CYAN));
        backend.apply_style(&node, &rules);
        let (r, g, b) = shadow_rgb(&view);
        assert!(r < 0.1 && g > 0.9 && b > 0.9, "the swap re-tints to cyan, got ({r}, {g}, {b})");
    }

    pub fn run() {
        extern "C" {
            fn pthread_main_np() -> std::os::raw::c_int;
        }
        assert!(
            unsafe { pthread_main_np() } != 0,
            "theme_shadow_token must run on the main thread (harness = false)"
        );
        // SAFETY: checked just above — this is the main thread.
        let mtm = unsafe { MainThreadMarker::new_unchecked() };
        backend_macos::install_scheduler();
        let mut backend = MacosBackend::new(mtm);
        regression_token_shadow_retints_on_theme_swap(&mut backend);
        println!("test regression_token_shadow_retints_on_theme_swap ... ok");
    }
}
