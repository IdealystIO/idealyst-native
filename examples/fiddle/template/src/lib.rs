//! Wrapping crate for the user's snippet.
//!
//! The user writes a `pub fn app() -> Primitive` (plus any helpers).
//! `compile.rs` on the server prepends an ambient `use` line —
//! `use crate::__rt::*;` — so common framework imports are available
//! without the user having to remember which crate any given symbol
//! lives in.
//!
//! Output mode is picked at wasm-pack time via a cargo feature:
//!
//! - `simulator` mounts the user's `app()` inside a `host-web` wgpu
//!   simulator on a canvas. iOS skin chrome, real GPU paint. Best
//!   for previewing how the app will look on iOS / Android. Heavier
//!   bundle (drags in `host-web`, `render-wgpu`, `wgpu`, glyphon).
//! - `web` mounts the user's `app()` straight into the DOM via
//!   `backend-web`. Native `<button>` / `<input>` / `<div>` elements;
//!   what the snippet would look like as a real web app. Smaller
//!   bundle.
//!
//! The server's `/compile` handler maps the request's `mode` field
//! to `--features <name>` on the wasm-pack invocation. No `default`
//! feature on the crate — picking nothing is a compile-time error.

mod snippet;

// ---------------------------------------------------------------------------
// Mode invariant — exactly one of the two output features must be on.
// ---------------------------------------------------------------------------

#[cfg(all(feature = "simulator", feature = "web"))]
compile_error!("fiddle-snippet: enable exactly one of `simulator` or `web` (not both).");

#[cfg(not(any(feature = "simulator", feature = "web")))]
compile_error!("fiddle-snippet: enable one of `simulator` or `web` features.");

// ---------------------------------------------------------------------------
// Simulator mode — wgpu canvas + host-web + iOS skin.
// ---------------------------------------------------------------------------

#[cfg(all(target_arch = "wasm32", feature = "simulator", not(feature = "web")))]
mod sim_mode {
    use std::cell::RefCell;
    use std::rc::Rc;

    use framework_core::primitives::graphics::{OnReadyEvent, OnResizeEvent};
    use framework_core::{ui, view, ColorScheme, IntoPrimitive, Length, StyleRules, StyleSheet};
    use host_web::{DeviceProfile, Skin};
    use wasm_bindgen::prelude::*;

    #[global_allocator]
    static ALLOCATOR: lol_alloc::AssumeSingleThreaded<lol_alloc::FreeListAllocator> =
        unsafe { lol_alloc::AssumeSingleThreaded::new(lol_alloc::FreeListAllocator::new()) };

    thread_local! {
        static OWNER: RefCell<Option<framework_core::Owner>> = const { RefCell::new(None) };
        // `host-web::WebHostHandle` is `!Send + !Sync`; thread-local is
        // the right home.
        static HANDLE: RefCell<Option<host_web::WebHostHandle>> = const { RefCell::new(None) };
    }

    /// iPhone-portrait profile — matches `native-phone` so a snippet
    /// authored against the simulator lays out identically to the
    /// native preview window.
    const LOGICAL_W: u32 = 390;
    const LOGICAL_H: u32 = 844;
    /// CSS pixel width of the embedded canvas. Height is derived to
    /// preserve the logical aspect (the docs Simulator does the same
    /// — wide-but-short canvases stretch glyphs vertically).
    const PREVIEW_WIDTH_PX: f32 = 320.0;

    #[wasm_bindgen(start)]
    pub fn start() {
        console_error_panic_hook::set_once();
        backend_web::install_scheduler();
        backend_web::install_async_executor();
        backend_web::install_render_loop();
        // Default theme so `idea-ui` components (`Card`, `Heading`,
        // `Body`, `Avatar`, …) don't panic with
        // `no IdeaTheme installed`. First-call-wins on the
        // framework's theme slot — the snippet can still call
        // `idea_ui::set_idea_theme(...)` to swap at runtime.
        idea_ui::install_idea_theme(idea_ui::light_theme());

        let backend = Rc::new(RefCell::new(backend_web::WebBackend::new("#app")));
        let owner = framework_core::render(backend, simulator_tree());
        OWNER.with(|slot| *slot.borrow_mut() = Some(owner));
    }

    fn simulator_tree() -> framework_core::Primitive {
        let preview_height_px =
            PREVIEW_WIDTH_PX * (LOGICAL_H as f32) / (LOGICAL_W as f32);

        let graphics = framework_core::primitives::graphics::graphics(move |event: OnReadyEvent| {
            wasm_bindgen_futures::spawn_local(async move {
                let profile = DeviceProfile {
                    logical_size: (LOGICAL_W, LOGICAL_H),
                    position: None,
                    title: "Idealyst Fiddle".to_string(),
                    color_scheme: ColorScheme::Light,
                };
                let skin: Rc<dyn Skin> = Rc::new(ios_sim::IosSim::new());
                match host_web::mount(event.surface, event.size, profile, skin, || {
                    super::snippet::app()
                })
                .await
                {
                    Ok(handle) => HANDLE.with(|slot| *slot.borrow_mut() = Some(handle)),
                    Err(err) => web_sys::console::warn_1(
                        &format!("[fiddle-snippet] host-web mount failed: {err}").into(),
                    ),
                }
            });
        })
        .on_resize(|event: OnResizeEvent| {
            HANDLE.with(|slot| {
                if let Some(h) = slot.borrow().as_ref() {
                    h.resize(event.size);
                }
            });
        })
        .on_lost(|| {
            // Take the handle out FIRST, then let it drop after the
            // borrow releases — same pattern the docs Simulator uses.
            let stale = HANDLE.with(|slot| slot.borrow_mut().take());
            drop(stale);
        });

        // Pin the canvas to the device's aspect ratio (matches the
        // docs Simulator's wrapper-View trick — the web `Graphics`
        // primitive forces `width: 100%; height: 100%` inline on
        // the canvas, so a fixed-size wrapper carries the dimensions).
        let wrapper_sheet = Rc::new(StyleSheet::r#static(StyleRules {
            width: Some(Length::Px(PREVIEW_WIDTH_PX).into()),
            height: Some(Length::Px(preview_height_px).into()),
            ..Default::default()
        }));
        let wrapper = view(vec![graphics.into_primitive()]).with_style(wrapper_sheet);

        ui! { wrapper }
    }
}

#[cfg(all(target_arch = "wasm32", feature = "simulator", not(feature = "web")))]
pub use sim_mode::start;

// ---------------------------------------------------------------------------
// Web mode — plain DOM mount via backend-web. No canvas, no wgpu.
// ---------------------------------------------------------------------------

#[cfg(all(target_arch = "wasm32", feature = "web", not(feature = "simulator")))]
mod web_mode {
    use std::cell::RefCell;
    use std::rc::Rc;

    use wasm_bindgen::prelude::*;

    #[global_allocator]
    static ALLOCATOR: lol_alloc::AssumeSingleThreaded<lol_alloc::FreeListAllocator> =
        unsafe { lol_alloc::AssumeSingleThreaded::new(lol_alloc::FreeListAllocator::new()) };

    thread_local! {
        static OWNER: RefCell<Option<framework_core::Owner>> = const { RefCell::new(None) };
    }

    #[wasm_bindgen(start)]
    pub fn start() {
        console_error_panic_hook::set_once();
        backend_web::install_scheduler();
        backend_web::install_async_executor();
        backend_web::install_render_loop();
        // Same theme install as `sim_mode::start` — `idea-ui`
        // components read from `framework_core::active_theme()` and
        // panic if it's empty. Snippet authors can override via
        // `idea_ui::set_idea_theme(...)` at runtime.
        idea_ui::install_idea_theme(idea_ui::light_theme());

        let backend = Rc::new(RefCell::new(backend_web::WebBackend::new("#app")));
        // No simulator wrapper — the user's `app()` IS the page in
        // web mode. The iframe shell's `#app` div fills the
        // viewport, so any `flex_grow: 1` / `height: 100%` at the
        // snippet's root will take the full preview area.
        let owner = framework_core::render(backend, super::snippet::app());
        OWNER.with(|slot| *slot.borrow_mut() = Some(owner));
    }
}

#[cfg(all(target_arch = "wasm32", feature = "web", not(feature = "simulator")))]
pub use web_mode::start;

// ---------------------------------------------------------------------------
// Snippet-side runtime prelude — same for both modes.
// ---------------------------------------------------------------------------

/// Imports the user's snippet sees. The compile-time wrapper in
/// `examples/fiddle/src/compile.rs` injects `use crate::__rt::*;`
/// above the user's code so any of these symbols are in scope
/// without an explicit `use` line. Grow this list as snippets need
/// more — anything not re-exported here forces the user to type
/// out a full `framework_core::` / `idea_ui::` path.
#[allow(unused_imports)]
pub mod __rt {
    pub use framework_core::{
        button, component, pressable, signal, switch, text, ui, view, when, ColorScheme,
        Easing, Effect, Length, Primitive, Ref, Signal, StyleRules, StyleSheet,
    };
    pub use idea_ui::{body, card, heading, BodyTone, HeadingKind};
    pub use std::rc::Rc;
}
