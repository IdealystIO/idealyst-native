//! Smoke-test binary for the `toolbar` SDK.
//!
//! Boots a host-appkit window with an `NSToolbar` attached. The
//! toolbar shows three buttons + a flexible spacer; clicking any
//! button increments a counter shown in the window body, and the
//! reactive `items` closure rebuilds the toolbar's "Count" button
//! label on every signal change so you can visually confirm
//! reactivity end-to-end.
//!
//! Run with:
//!
//! ```sh
//! cargo run -p toolbar-demo
//! ```
//!
//! This bypasses the `idealyst dev --macos` auto-bundle flow — the
//! point is to validate the SDK plus the `host_appkit::run_with`
//! registration hook in isolation, without anything else in the
//! build pipeline interfering.

#[cfg(target_os = "macos")]
fn main() {
    use runtime_core::{install_tokens, signal, view, Element};

    let count = signal!(0_i32);

    let app = move || -> Element {
        // `install_tokens` is required before render even when no
        // styles read tokens — see [[project_install_theme_required]].
        install_tokens(&[]);

        // Body is intentionally a bare empty view — not a `text(...)`
        // primitive — to dodge a pre-existing crash in the macOS
        // backend's `sync_gradient_sublayer` (NSTextField is
        // layer-optional and `view.layer` returns nil before the
        // backend's text-style applier forces wantsLayer; the
        // gradient sync path panics when this is the first text node
        // in the tree). Unrelated to the toolbar SDK; visible
        // verification of click behavior is done via the toolbar's
        // own "Count: N" label, which re-renders on every signal
        // change because the reactive `items` closure reads `count`.
        let c_for_items = count.clone();
        let c_inc = count.clone();
        let c_reset = count.clone();
        let bar = toolbar::Toolbar(toolbar::ToolbarProps {
            items: Box::new(move || {
                let n = c_for_items.get();
                eprintln!("[toolbar-demo] items closure re-evaluated, count={n}");
                vec![
                    toolbar::ToolbarItem::button("Increment")
                        .icon("plus")
                        .tooltip("Bump the counter")
                        .on_click({
                            let c = c_inc.clone();
                            move || c.set(c.get() + 1)
                        })
                        .into(),
                    toolbar::ToolbarItem::button("Reset")
                        .icon("arrow.counterclockwise")
                        .on_click({
                            let c = c_reset.clone();
                            move || c.set(0)
                        })
                        .into(),
                    toolbar::ToolbarItem::flexible_space(),
                    toolbar::ToolbarItem::button(format!("Count: {n}")).into(),
                ]
            }),
            ..Default::default()
        });

        view(vec![bar.into()]).into()
    };

    let opts = host_appkit::RunOptions {
        title: "Toolbar SDK Smoke Test".to_string(),
        width: 800.0,
        height: 400.0,
    };
    if let Err(e) = host_appkit::run_with(app, opts, |backend| {
        toolbar::register(backend);
    }) {
        eprintln!("toolbar-demo: runtime error: {e}");
        std::process::exit(1);
    }
}

#[cfg(not(target_os = "macos"))]
fn main() {
    eprintln!(
        "toolbar-demo: this smoke test only runs on macOS. On other \
         platforms the toolbar SDK's `register` is a no-op anyway, \
         so there's nothing to demo."
    );
}
