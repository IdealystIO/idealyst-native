//! Editor UI for the fiddle. Idealyst tree mounted into `#app` by
//! `start()`; layout breaks into:
//!
//! - **Editor column** — `TextInput` bound to the source signal,
//!   a mode toggle (Simulator / Web), the Run button (disabled
//!   while a compile is in flight), and a scrollable status pane
//!   that holds either the build hash or the raw compile error.
//! - **Preview column** — `WebView` whose URL is driven by a
//!   signal pointing at `/compiled/<hash>/`. The iframe contents
//!   are the compiled snippet — either a wgpu simulator (Simulator
//!   mode) or a plain DOM mount (Web mode), depending on what the
//!   user picked when they hit Run.

mod fetch;

use std::cell::RefCell;
use std::rc::Rc;

use framework_core::primitives::scroll_view::scroll_view;
use framework_core::primitives::text_input::text_input;
use framework_core::primitives::web_view::web_view;
use framework_core::{
    button, signal, text, ui, view, FlexDirection, Length, Primitive, Signal, StyleRules,
    StyleSheet,
};
use idea_ui::{install_idea_theme, light_theme};
use wasm_bindgen::prelude::*;

use crate::fetch::Mode;

// Same smaller-WASM-allocator trick the docs site uses — trades a
// little per-alloc cost for a few KB off the bundle.
#[global_allocator]
static ALLOCATOR: lol_alloc::AssumeSingleThreaded<lol_alloc::FreeListAllocator> =
    unsafe { lol_alloc::AssumeSingleThreaded::new(lol_alloc::FreeListAllocator::new()) };

thread_local! {
    static OWNER: RefCell<Option<framework_core::Owner>> = const { RefCell::new(None) };
}

/// Starter snippet shown in the editor on first load. Carries an
/// explicit white background and padding because the wgpu renderer
/// clears its canvas to black (the design intent is that the bezel
/// + notch cutouts show through), so an unstyled root view would
/// render black-on-black. Snippets can replace the background with
/// any CSS color string, or drop the wrapper entirely in Web mode.
const STARTER_SOURCE: &str = "pub fn app() -> Primitive { let count: Signal<i32> = signal!(0_i32); let on_tap: Rc<dyn Fn()> = Rc::new(move || count.set(count.get() + 1)); let label = move || format!(\"Tapped {} times\", count.get()); let bg = Rc::new(StyleSheet::r#static(StyleRules { background: Some(\"white\".into()), flex_grow: Some(1.0.into()), padding_top: Some(Length::Px(80.0).into()), padding_left: Some(Length::Px(24.0).into()), padding_right: Some(Length::Px(24.0).into()), gap: Some(Length::Px(12.0).into()), ..Default::default() })); ui! { view(vec![ text(\"Hello, fiddle!\").into(), text(label).into(), button(\"Tap me\", move || on_tap()).into(), ]).with_style(bg) }}";

#[wasm_bindgen(start)]
pub fn start() {
    console_error_panic_hook::set_once();
    backend_web::install_scheduler();
    backend_web::install_async_executor();
    backend_web::install_render_loop();
    install_idea_theme(light_theme());

    let backend = Rc::new(RefCell::new(backend_web::WebBackend::new("#app")));
    let owner = framework_core::render(backend, app());
    OWNER.with(|slot| *slot.borrow_mut() = Some(owner));
}

fn app() -> Primitive {
    // ---- Signals owned at the root so they survive across renders
    let source: Signal<String> = signal!(STARTER_SOURCE.to_string());
    // `about:blank` keeps the iframe inert until the first
    // successful compile flips this to a real path.
    let iframe_url: Signal<String> = signal!("about:blank".to_string());
    let status: Signal<String> = signal!("Press Run to compile".to_string());
    // Disable the Run button while a compile is in flight so a
    // double-click doesn't queue redundant requests. The server
    // serializes them anyway via its compile lock, but the UI
    // shouldn't pretend they're independent.
    let is_compiling: Signal<bool> = signal!(false);
    // Selected output mode. Drives both the on-screen toggle's
    // labels and what gets sent in the /compile request body.
    // `Simulator` matches the default in the server-side enum.
    let mode: Signal<bool> = signal!(true); // true = simulator, false = web

    // ---- Editor
    let editor = text_input(source.clone(), move |new_value| source.set(new_value))
        .placeholder("pub fn app() -> Primitive { ... }".to_string());

    // ---- Mode toggle (two buttons, the active one is disabled to
    // make the selection obvious without bringing in a custom
    // toggle component).
    let mode_for_sim_btn = mode.clone();
    let mode_for_sim_active = mode.clone();
    let sim_button = button("Simulator", move || mode_for_sim_btn.set(true))
        .disabled(move || mode_for_sim_active.get());
    let mode_for_web_btn = mode.clone();
    let mode_for_web_active = mode.clone();
    let web_button = button("Web", move || mode_for_web_btn.set(false))
        .disabled(move || !mode_for_web_active.get());

    let mode_row_sheet = Rc::new(StyleSheet::r#static(StyleRules {
        flex_direction: Some(FlexDirection::Row),
        gap: Some(Length::Px(8.0).into()),
        ..Default::default()
    }));
    let mode_row =
        view(vec![sim_button.into(), web_button.into()]).with_style(mode_row_sheet);

    // ---- Status pane — wrapped in a scroll_view with a bounded
    // height so a long compile-error blob doesn't push the editor
    // off-screen. Status reads through a reactive `text(...)` closure
    // so signal updates land in the UI automatically.
    let status_for_text = status.clone();
    let status_label = text(move || status_for_text.get());
    let status_pane_sheet = Rc::new(StyleSheet::r#static(StyleRules {
        height: Some(Length::Px(200.0).into()),
        padding_top: Some(Length::Px(8.0).into()),
        padding_right: Some(Length::Px(8.0).into()),
        padding_bottom: Some(Length::Px(8.0).into()),
        padding_left: Some(Length::Px(8.0).into()),
        border_top_width: Some(1.0.into()),
        border_right_width: Some(1.0.into()),
        border_bottom_width: Some(1.0.into()),
        border_left_width: Some(1.0.into()),
        ..Default::default()
    }));
    let status_pane = scroll_view(vec![status_label.into()]).with_style(status_pane_sheet);

    // ---- Run button — POST current source + mode to /compile.
    let source_for_run = source.clone();
    let status_for_run = status.clone();
    let iframe_url_for_run = iframe_url.clone();
    let mode_for_run = mode.clone();
    let is_compiling_for_run = is_compiling.clone();
    let on_run: Rc<dyn Fn()> = Rc::new(move || {
        // Sample state at click time (not at closure creation), so
        // each click sees fresh signal values.
        let body = source_for_run.get();
        let picked = if mode_for_run.get() { Mode::Simulator } else { Mode::Web };
        let status = status_for_run.clone();
        let url = iframe_url_for_run.clone();
        let is_compiling = is_compiling_for_run.clone();
        is_compiling.set(true);
        status.set(match picked {
            Mode::Simulator => "Compiling for simulator…".to_string(),
            Mode::Web => "Compiling for web…".to_string(),
        });
        wasm_bindgen_futures::spawn_local(async move {
            match fetch::compile(&body, picked).await {
                Ok(hash) => {
                    // Cache-bust the iframe by appending a query
                    // string — a recompile of the SAME source hash
                    // yields the same URL otherwise, and the browser
                    // may not actually re-fetch the wasm.
                    url.set(format!(
                        "/compiled/{hash}/?t={}",
                        js_sys::Date::now() as u64
                    ));
                    status.set(format!("Built {hash}"));
                }
                Err(err) => status.set(err),
            }
            is_compiling.set(false);
        });
    });
    let is_compiling_for_disabled = is_compiling.clone();
    let run_button = button("Run", move || on_run())
        .disabled(move || is_compiling_for_disabled.get());

    // ---- Preview — WebView's URL is reactive on iframe_url, so
    // `iframe_url.set(...)` swaps the iframe target. No border /
    // radius — the snippet's own root (the host-web simulator in
    // Simulator mode, or the user's app() in Web mode) is what
    // should show, not a frame around it.
    let iframe_url_for_view = iframe_url.clone();
    let preview = web_view(move || iframe_url_for_view.get()).with_style(Rc::new(
        StyleSheet::r#static(StyleRules {
            // Phone-aspect preview area; the snippet's own canvas /
            // DOM fills this with whatever sizing it picked.
            width: Some(Length::Px(360.0).into()),
            height: Some(Length::Px(720.0).into()),
            ..Default::default()
        }),
    ));

    // ---- Layout
    let col_sheet = Rc::new(StyleSheet::r#static(StyleRules {
        flex_direction: Some(FlexDirection::Column),
        gap: Some(Length::Px(12.0).into()),
        flex_grow: Some(1.0.into()),
        ..Default::default()
    }));
    let row_sheet = Rc::new(StyleSheet::r#static(StyleRules {
        flex_direction: Some(FlexDirection::Row),
        gap: Some(Length::Px(16.0).into()),
        padding_top: Some(Length::Px(16.0).into()),
        padding_right: Some(Length::Px(16.0).into()),
        padding_bottom: Some(Length::Px(16.0).into()),
        padding_left: Some(Length::Px(16.0).into()),
        ..Default::default()
    }));

    let left = view(vec![
        editor.into(),
        mode_row.into(),
        run_button.into(),
        status_pane.into(),
    ])
    .with_style(col_sheet);
    let right = view(vec![preview.into()]);
    let row = view(vec![left.into(), right.into()]).with_style(row_sheet);

    ui! { row }
}
