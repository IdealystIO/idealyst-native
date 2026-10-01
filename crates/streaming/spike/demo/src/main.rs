//! See the streamed-component spike in a real window, live-reloading.
//!
//! ```sh
//! cargo run --release -p stream-spike --bin stream-serve   # terminal 1
//! cargo run --release -p stream-demo                        # terminal 2
//! ```
//!
//! Edit `crates/streaming/spike/guest/src/lib.rs`, press **Refresh bundle**,
//! and the tinted (wasm) sections remount from the new build — no app
//! rebuild, no restart. With no server running the app uses the bundle
//! compiled into it, the way a shipped app falls back when a fetch fails.
//!
//! Props are a checked contract. Each refresh is compared against the props
//! this app sends (`stream_spike::demo_props`); a bundle that changed a prop's type or
//! added a required one is rejected with the exact mismatch, and the app
//! keeps running the previous bundle. Adding a prop WITH a default, or
//! dropping one, is compatible.
//!
//! What survives a refresh is the point: `external` and `shared` are HOST
//! signals, so their values carry over; `clicks` inside `Counter` was the
//! old bundle's own state and starts over.
//!
//! The fetch blocks the main thread. Fine against localhost for a spike; a
//! real app fetches through an async host capability.
//!
//! Built with vocabulary builders rather than `ui!` to match the spike's
//! host-side code, which constructs the same builders from descriptions.

use std::cell::RefCell;
use std::rc::Rc;
use std::time::Instant;

use runtime_scene::{dyn_element, Element};
use runtime_shared::{Color, Length, StyleRules, Tokenized};
use runtime_vocabulary::{button, text, view};
use runtime_world::{signal, Signal};
use stream_host::{Bundle, HostProps, StreamEngine};
use stream_spike::{demo_props, fetch, host_exports, GUEST_WASM};

fn px(v: f32) -> Option<Tokenized<Length>> {
    Some(Tokenized::Literal(Length::Px(v)))
}

fn column() -> StyleRules {
    StyleRules { padding_top: px(16.0), padding_left: px(16.0), padding_right: px(16.0), gap: px(10.0), ..StyleRules::default() }
}

fn section() -> StyleRules {
    StyleRules {
        padding_top: px(10.0),
        padding_left: px(10.0),
        padding_right: px(10.0),
        padding_bottom: px(10.0),
        gap: px(6.0),
        // Tinted so it is obvious which part of the window came from wasm.
        background: Some(Tokenized::Literal(Color::from("#e8f0fe"))),
        ..StyleRules::default()
    }
}

fn bundle_url() -> String {
    std::env::var("STREAM_BUNDLE_URL").unwrap_or_else(|_| fetch::DEFAULT_URL.to_string())
}

/// Fetch, load, and contract-check the served bundle. `Err` leaves the
/// caller's current bundle untouched — like a server function answering
/// `IncompatibleVersion`, the app learns precisely what drifted and keeps
/// running what works.
fn fetch_bundle(engine: &StreamEngine, wanted: &[(&'static str, HostProps)]) -> Result<(Bundle, String), String> {
    let url = bundle_url();
    let t = Instant::now();
    let fetched = fetch::fetch(&url)?;
    let fetch_ms = t.elapsed().as_secs_f64() * 1000.0;
    let t = Instant::now();
    let bundle = Bundle::load(engine, &fetched.wasm, host_exports()).map_err(|e| e.to_string())?;
    let load_us = t.elapsed().as_micros();
    let problems: Vec<String> =
        wanted.iter().filter_map(|(name, props)| bundle.check(name, props).err()).map(|e| e.to_string()).collect();
    if !problems.is_empty() {
        return Err(format!("v{} rejected — {}", fetched.version, problems.join("\n")));
    }
    Ok((
        bundle,
        format!(
            "v{} from {url}: {} KB, fetched in {fetch_ms:.0} ms, loaded in {load_us} µs",
            fetched.version,
            fetched.wasm.len() / 1024,
        ),
    ))
}

fn app() -> Element {
    let external = signal(0i64);
    let shared = signal(100i64);

    let engine = Rc::new(StreamEngine::new());
    let (initial, initial_status) = match fetch_bundle(&engine, &demo_props(external, shared)) {
        Ok(ok) => ok,
        Err(e) => {
            let bundle = Bundle::load(&engine, GUEST_WASM, host_exports())
                .unwrap_or_else(|e| panic!("stream-demo: built-in bundle: {e}"));
            (bundle, format!("using the built-in bundle:\n{e}"))
        }
    };
    let current = Rc::new(RefCell::new(initial));
    // Bumped on every successful swap; the streamed region reads it, so a
    // swap tears down the old mounts and builds new ones.
    let generation = signal(0u32);
    let status: Signal<String> = signal(initial_status);

    let refresh = {
        let engine = engine.clone();
        let current = current.clone();
        move || match fetch_bundle(&engine, &demo_props(external, shared)) {
            Ok((bundle, msg)) => {
                *current.borrow_mut() = bundle;
                generation.update(|g| g + 1);
                status.set(msg);
            }
            Err(e) => status.set(format!("refresh failed, still running the previous bundle:\n{e}")),
        }
    };

    view()
        .style(column())
        .child(button().label("Refresh bundle").on_press(refresh))
        .child(text().content(move || status.get()))
        .child(text().content(
            "App sends: Counter(title: String, external: ReadSignal<i64>), Stepper(value: Signal<i64>), Camera(facing: String)",
        ))
        .child(text().content("Native host controls"))
        .child(button().label("external + 1").on_press(move || external.update(|v| v + 1)))
        .child(button().label("external − 1").on_press(move || external.update(|v| v - 1)))
        .child(text().content(move || format!("Host reads `shared` = {}", shared.get())))
        .child(dyn_element(move || {
            let _ = generation.get();
            // Clone out of the cell before mounting: the mount runs guest
            // code, and nothing should hold the cell across that.
            let bundle = current.borrow().clone();
            let mut column = view().style(StyleRules { gap: px(10.0), ..StyleRules::default() });
            for (name, props) in demo_props(external, shared) {
                // Already checked before the swap; the fallback is what an app
                // shows if it mounts without checking first.
                let mounted = match bundle.try_mount(name, props) {
                    Ok(element) => element,
                    Err(e) => text().content(format!("⚠ {e}")).build(),
                };
                column = column
                    .child(text().content(format!("↓ Streamed: {name}")))
                    .child(view().style(section()).child(mounted));
            }
            column.build()
        }))
        .build()
}

#[cfg(target_os = "macos")]
fn main() {
    let opts = host_appkit::RunOptions { title: "Streamed components".to_string(), width: 620.0, height: 820.0 };
    if let Err(e) = host_appkit::newcore::run(app, opts) {
        eprintln!("[stream-demo] failed to boot: {e}");
        std::process::exit(1);
    }
}

#[cfg(not(target_os = "macos"))]
fn main() {
    let _ = app;
    eprintln!("stream-demo only runs on macOS");
}
