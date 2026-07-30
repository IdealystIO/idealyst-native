//! Smoke app for the wgpu backend's boot path
//! (`host_winit::newcore::run` → `render_wgpu::newcore::start`).
//!
//! Everything here is DIRECT vocabulary-builder calls — no `ui!`, no
//! `jsx!` — deliberately, so this crate proves the `runtime_scene`
//! registry-dispatch render path against the live GPU node tree
//! independent of the macro-lowering work (the sanctioned deviation
//! from CLAUDE.md §9.2, same as every other newcore smoke: this crate
//! gates the layer *under* the macro). The tree mirrors
//! `newcore-macos-smoke` — same coverage, another backend.
//!
//! Coverage: static + reactive `text`, `button` (event → staged write →
//! dispatch-driver flush), a two-way `toggle`, a structural Dyn hole
//! (closure child), a keyed list with add/remove/reverse, and one
//! literal `StyleRules` (the `StyleOps` delegation on the native
//! apply-style path).
//!
//! Two live-verification modes (see Cargo.toml header):
//! - `NEWCORE_SMOKE_SELFTEST=1` (windowed): a real winit-scheduler
//!   timer stages a write; committing it proves the dispatch-hook flush
//!   route end-to-end on the live event loop. Prints
//!   `[SMOKE-SELFTEST] committed=… views=…` and exits 0/1.
//! - `NEWCORE_SMOKE_HEADLESS=<prefix>` (offscreen): renders the mounted
//!   mounted tree on a real wgpu device (Metal on macOS) via the
//!   headless `Screenshotter`, presses the button through the wrapped
//!   `on_click`, flushes, and captures before/after PNGs — pixel
//!   evidence that the staged write committed into the rendered tree.

use std::rc::Rc;

use runtime_shared::{ColorScheme, Length, Platform, StyleRules, Tokenized};
use runtime_scene::{keyed, Element};
use runtime_vocabulary::builders::IntoSceneElement;
use runtime_vocabulary::{button, text, toggle, view};
use runtime_world::signal;

/// A minimal literal style — exercises the `StyleOps` delegation (on
/// wgpu: per-node `apply_style` into the Taffy tree + RenderStyle
/// projection, not class minting).
fn padded_column() -> StyleRules {
    StyleRules {
        padding_top: Some(Tokenized::Literal(Length::Px(16.0))),
        padding_left: Some(Tokenized::Literal(Length::Px(16.0))),
        gap: Some(Tokenized::Literal(Length::Px(8.0))),
        ..StyleRules::default()
    }
}

/// The app tree. Runs inside `World::enter` (the boot path wraps it),
/// so the free `signal()` constructor works; these top-level signals
/// are world-root-owned and live for the app.
pub fn app() -> Element {
    let count = signal(0i32);
    let on = signal(false);
    let rows = signal(vec![1u32, 2, 3]);
    let next_row = signal(4u32);

    // Windowed live-verification hook (`NEWCORE_SMOKE_SELFTEST=1`):
    // from a real winit-scheduler timer, stage a write and later assert
    // it was COMMITTED by the flush driver — a staged `set` is only
    // observable through `get` after a `world.flush()`, and the only
    // route from an `after_ms` callback to a flush is the scheduler's
    // post-dispatch hook (`render_wgpu::dispatch_hook`), so success is
    // live proof of the hook route. Also count the live GPU nodes to
    // prove realize/finish attached a real tree. Exits 0 on success /
    // 1 on failure so a CI-ish caller can gate on it. Scheduled from
    // inside the build (the winit scheduler is installed before
    // `resumed` mounts).
    if std::env::var("NEWCORE_SMOKE_SELFTEST").as_deref() == Ok("1") {
        runtime_shared::scheduling::after_ms_detached(1500, move || {
            count.set(41); // stages — the driver must commit it
            runtime_shared::scheduling::after_ms_detached(700, move || {
                let committed = count.get() == 41;
                let views = selftest::live_node_count();
                println!("[SMOKE-SELFTEST] committed={committed} views={views}");
                // The static tree alone mounts well over 10 nodes
                // (column + 3 texts + 3 buttons + toggle + dyn hole +
                // keyed anchor + 3 rows); a low count means
                // realize/finish didn't attach.
                std::process::exit(if committed && views > 10 { 0 } else { 1 });
            });
        });
    }

    view()
        .style(padded_column())
        .child(text().content("New-core GPU smoke"))
        .child(text().content(move || format!("count = {}", count.get())))
        .child(
            button()
                .label("Increment")
                .on_press(move || count.update(|n| n + 1)),
        )
        .child(toggle().value(on).on_change(move |v| on.set(v)))
        // Structural Dyn hole: a closure child rebuilds when its reads
        // change (`SceneChild` lowers it to `dyn_element`).
        .child(move || {
            if on.get() {
                view()
                    .child(text().content("toggle is ON"))
                    .into_scene_element()
            } else {
                text().content("toggle is OFF").into_scene_element()
            }
        })
        .child(button().label("Add row").on_press(move || {
            let id = next_row.peek();
            next_row.set(id + 1);
            rows.update(move |r| {
                let mut r = r.clone();
                r.push(id);
                r
            });
        }))
        .child(
            button()
                .label("Remove first")
                .on_press(move || rows.update(|r| r.iter().copied().skip(1).collect())),
        )
        .child(
            button()
                .label("Reverse")
                .on_press(move || rows.update(|r| r.iter().rev().copied().collect())),
        )
        // Keyed list (anchored full-rebuild on this no-splice backend —
        // the old walker's contract here too).
        .child(keyed(
            move || rows.get(),
            |n| *n,
            |n| text().content(format!("row #{n}")).build(),
        ))
        .build()
}

mod selftest {
    use render_wgpu::WgpuNode;

    fn count_recursive(node: &WgpuNode) -> usize {
        let children: Vec<WgpuNode> = node.borrow().children.clone();
        1 + children.iter().map(count_recursive).sum::<usize>()
    }

    /// Total live GPU nodes under the mounted root — the wgpu analogue
    /// of the macOS smoke's NSView walk (this backend has no window
    /// hierarchy to count; the boot path exposes the backend seam).
    pub fn live_node_count() -> usize {
        render_wgpu::newcore::with_backend(|b| {
            b.borrow().root().map(|r| count_recursive(&r)).unwrap_or(0)
        })
        .unwrap_or(0)
    }
}

/// The real host OS identity for `NativeSkin` (author code reading
/// `runtime_shared::platform()` takes its genuine native branch — the
/// universal-native contract). No Windows/Linux Platform variants exist
/// yet; `Custom` self-reports there, same as the trait guidance.
fn host_platform() -> Platform {
    #[cfg(target_os = "macos")]
    {
        Platform::MacOs
    }
    #[cfg(not(target_os = "macos"))]
    {
        Platform::Custom("gpu-desktop")
    }
}

/// Offscreen capture mode: mount on the headless Screenshotter's
/// backend, render (real Metal/Vulkan device, same shaders as the
/// window), press the button through the wrapped `on_click`, flush via
/// the buffered-microtask drain, and render again. Two PNGs +
/// tree-state assertions = live verification without a window.
fn headless_capture(prefix: &str) {
    use render_wgpu::headless::Screenshotter;
    use render_wgpu::{newcore, NativeSkin, NodeKind, WgpuNode};

    let mut shot = Screenshotter::with_color_scheme_and_skin(
        480,
        620,
        ColorScheme::Light,
        Rc::new(NativeSkin::new(host_platform())),
    )
    .expect("headless wgpu device");
    println!(
        "[SMOKE-HEADLESS] adapter={}",
        if shot.software { "software" } else { "hardware" }
    );

    let app_state = newcore::start(shot.backend(), |_| {}, app);

    let png_a = shot.capture_png().expect("capture A");
    std::fs::write(format!("{prefix}-a.png"), &png_a).expect("write A");

    // Press "Increment" exactly the way the interaction Host does on
    // pointer release: invoke the installed (wrapped) on_click.
    fn find_button(node: &WgpuNode, label_want: &str) -> Option<WgpuNode> {
        if let NodeKind::Button { label, .. } = &node.borrow().kind {
            if label == label_want {
                return Some(node.clone());
            }
        }
        let children: Vec<WgpuNode> = node.borrow().children.clone();
        children.iter().find_map(|c| find_button(c, label_want))
    }
    fn texts(node: &WgpuNode, out: &mut Vec<String>) {
        if let NodeKind::Text { content } = &node.borrow().kind {
            out.push(content.clone());
        }
        let children: Vec<WgpuNode> = node.borrow().children.clone();
        for c in &children {
            texts(c, out);
        }
    }

    let root = newcore::with_backend(|b| b.borrow().root())
        .flatten()
        .expect("root mounted");
    let on_click = match &find_button(&root, "Increment").expect("button").borrow().kind {
        NodeKind::Button { on_click, .. } => on_click.clone(),
        _ => unreachable!(),
    };
    on_click();
    // The wrapped callback queued a flush microtask; the headless
    // scheduler buffers it — drain commits (same seam `start` uses).
    runtime_shared::scheduling::drain_buffered_microtasks();

    let mut t = Vec::new();
    texts(&root, &mut t);
    let committed = t.iter().any(|s| s == "count = 1");
    let views = {
        fn count(node: &WgpuNode) -> usize {
            let children: Vec<WgpuNode> = node.borrow().children.clone();
            1 + children.iter().map(count).sum::<usize>()
        }
        count(&root)
    };

    let png_b = shot.capture_png().expect("capture B");
    std::fs::write(format!("{prefix}-b.png"), &png_b).expect("write B");

    let pixels_changed = png_a != png_b;
    println!(
        "[SMOKE-HEADLESS] committed={committed} views={views} pixels_changed={pixels_changed}"
    );
    drop(app_state); // exercises teardown (Realized drop) before exit
    std::process::exit(if committed && views > 10 && pixels_changed {
        0
    } else {
        1
    });
}

fn main() {
    if let Ok(prefix) = std::env::var("NEWCORE_SMOKE_HEADLESS") {
        headless_capture(&prefix);
        return;
    }

    let profile = host_winit::DeviceProfile {
        logical_size: (480, 620),
        position: None,
        title: "New-core GPU smoke".to_string(),
        color_scheme: ColorScheme::Light,
    };
    let skin = Rc::new(render_wgpu::NativeSkin::new(host_platform()));
    if let Err(e) = host_winit::newcore::run(profile, skin, app) {
        eprintln!("[newcore-gpu-smoke] failed to boot: {e}");
        std::process::exit(1);
    }
}
