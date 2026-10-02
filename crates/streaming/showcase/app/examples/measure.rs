//! What the showcase's remote screens cost, against the same source compiled
//! into the app.
//!
//! One binary per mode, never both in one process (a binary holding both
//! measures code layout as much as the design):
//!
//! ```text
//! cargo run --release -p remote-showcase --example measure                    # remote: from the bundle
//! cargo run --release -p remote-showcase --example measure --features inline  # native: same source, in-process
//! ```
//!
//! `../measure.sh` builds both under the profile a native app ships with
//! (cargo's default release: opt 3, no LTO) and prints them side by side.
//!
//! `MEASURE_BUNDLE=path/to/bundle.wasm` measures another build of the bundle
//! (a different profile, say) in place of the built-in one.
//!
//! Everything runs on host-mock, so it measures the framework and the
//! interpreter: a platform toolkit would add the same cost to both modes.
//! Each figure is the median of many runs; `MEASURE` lines are the
//! machine-readable copy the script joins on.

use std::hint::black_box;
use std::time::{Duration, Instant};

use host_mock::{pump, Harness};
use runtime_core::ui;
use runtime_scene::Element;
use runtime_world::Signal;
use remote_showcase::{FeedScreen, ShopNavigator, Theme};

const REMOTE: bool = !cfg!(feature = "inline");

/// The bundle under test: the built-in one, or `MEASURE_BUNDLE`.
fn bundle() -> &'static [u8] {
    static FROM_FILE: std::sync::OnceLock<Vec<u8>> = std::sync::OnceLock::new();
    match std::env::var("MEASURE_BUNDLE") {
        Ok(path) => FROM_FILE.get_or_init(|| std::fs::read(&path).unwrap_or_else(|e| panic!("{path}: {e}"))),
        Err(_) => remote_showcase::BUILT_IN,
    }
}

fn median(mut v: Vec<Duration>) -> Duration {
    v.sort();
    v[v.len() / 2]
}

fn fmt(d: Duration) -> String {
    let us = d.as_secs_f64() * 1e6;
    if us >= 1000.0 { format!("{:.2} ms", us / 1000.0) } else { format!("{us:.1} µs") }
}

fn report(key: &str, what: &str, d: Duration) {
    println!("  {what:<52} {}", fmt(d));
    println!("MEASURE\t{key}\t{}", d.as_nanos());
}

fn report_bytes(key: &str, what: &str, bytes: usize) {
    println!("  {what:<52} {:.1} KB", bytes as f64 / 1024.0);
    println!("MEASURE\t{key}\t{bytes}");
}

/// Resident set size of this process, in bytes (`ps`: no extra deps).
fn rss() -> usize {
    let out = std::process::Command::new("ps")
        .args(["-o", "rss=", "-p", &std::process::id().to_string()])
        .output()
        .expect("run ps");
    String::from_utf8_lossy(&out.stdout).trim().parse::<usize>().expect("ps rss") * 1024
}

/// The app's state the remote screens read, owned by the bench.
#[derive(Clone, Copy)]
struct State {
    accent: Signal<String>,
    compact: Signal<bool>,
    cart: Signal<u32>,
}

fn feed(s: State) -> Element {
    runtime_scene::component_scope(move || {
        runtime_world::provide(Theme { accent: s.accent.read_only(), compact: s.compact.read_only() });
        ui! { FeedScreen() }
    })
}

fn shop(s: State) -> Element {
    runtime_scene::component_scope(move || ui! { ShopNavigator(cart = s.cart) })
}

/// Mount `build`, flush; returns the time and the mounted tree.
fn mount(h: &Harness, build: impl FnOnce() -> Element) -> (Duration, runtime_scene::Realized<host_mock::Node>) {
    let t = Instant::now();
    let tree = h.world.enter(build);
    let r = h.mount(tree);
    h.flush();
    (t.elapsed(), r)
}

/// Unmount, and drop the handlers the mock kept (a real backend drops them
/// with the node), so nothing outlives its iteration.
fn unmount(h: &Harness, r: runtime_scene::Realized<host_mock::Node>) -> Duration {
    let t = Instant::now();
    drop(r);
    h.flush();
    let d = t.elapsed();
    h.forget_handlers();
    d
}

/// Time `op` + flush, `n` times.
fn per_op(h: &Harness, n: usize, mut op: impl FnMut(usize)) -> Duration {
    median(
        (0..n)
            .map(|i| {
                let t = Instant::now();
                op(i);
                h.flush();
                t.elapsed()
            })
            .collect(),
    )
}

fn main() {
    assert!(!cfg!(debug_assertions), "measure a release build (`--release`): a debug baseline makes every ratio lie");
    pump::install_executor();
    pump::install_scheduler();
    let mode = if REMOTE { "remote (screens from the bundle)" } else { "inline (same source, compiled into the app)" };
    println!("mode: {mode}");

    // --- Load ---------------------------------------------------------------
    let wasm = bundle();
    if REMOTE {
        println!("\nbundle:");
        report_bytes("bundle.raw", "size (raw wasm)", wasm.len());
        let fns = remote_showcase::host_fns();
        // A fresh engine per load, as the loader does (`remote::engine`).
        let load = || stream_host::kernel::KernelBundle::load_with(&stream_host::remote::engine(), wasm, &fns).unwrap();
        let mut unloads = Vec::new();
        let first = {
            let t = Instant::now();
            let b = load();
            let d = t.elapsed();
            drop(b);
            d
        };
        let rss_loads = rss();
        let loads: Vec<Duration> = (0..50)
            .map(|_| {
                let t = Instant::now();
                let b = black_box(load());
                let d = t.elapsed();
                let t = Instant::now();
                drop(b);
                unloads.push(t.elapsed());
                d
            })
            .collect();
        let reload_growth = rss().saturating_sub(rss_loads) / 50;
        report("load.first", "load (validate, instantiate), first in process", first);
        report("load", "load, median of 50", median(loads));
        report("unload", "unload", median(unloads));
        report_bytes("mem.per_reload", "RSS kept per load + unload (a reload)", reload_growth);
    }
    remote_showcase::install_from(wasm).expect("bundle loads");
    let rss_start = rss();

    let h = Harness::new();
    let s = h.world.enter(|| State {
        accent: runtime_world::signal("#2563eb".to_string()),
        compact: runtime_world::signal(false),
        cart: runtime_world::signal(0u32),
    });

    // --- Feed: mount ----------------------------------------------------------
    println!("\nFeedScreen (4 cards from app components, context, a host fn):");
    let (first, r) = mount(&h, || feed(s));
    report("feed.mount.first", "mount + realize, first in process", first);
    unmount(&h, r);
    let (mounts, unmounts): (Vec<_>, Vec<_>) = (0..200)
        .map(|_| {
            let (m, r) = mount(&h, || feed(s));
            (m, unmount(&h, r))
        })
        .unzip();
    report("feed.mount", "mount + realize, median of 200", median(mounts));
    report("feed.unmount", "unmount", median(unmounts));

    // --- Feed: updates --------------------------------------------------------
    let presses_before = h.shared.button_presses.borrow().len();
    let (_, r) = mount(&h, || feed(s));
    // The first like button: pressed in the app, handled by the screen.
    let like = h.shared.button_presses.borrow()[presses_before].clone();
    report("feed.like", "press ♥ (handler → screen state → its text)", per_op(&h, 2000, |_| like()));
    report(
        "feed.accent",
        "app sets Theme.accent (screen's heading restyles)",
        per_op(&h, 2000, |i| s.accent.set(if i % 2 == 0 { "#059669".into() } else { "#2563eb".into() })),
    );
    report("feed.compact", "app toggles Theme.compact (4 post bodies come and go)", per_op(&h, 1000, |_| s.compact.update(|c| !c)));
    drop(like);
    unmount(&h, r);

    // --- Shop: a navigator defined in the screen code -------------------------
    println!("\nShopNavigator (stack navigator, list of 4 products):");
    let (mounts, unmounts): (Vec<_>, Vec<_>) = (0..200)
        .map(|_| {
            let (m, r) = mount(&h, || shop(s));
            (m, unmount(&h, r))
        })
        .unzip();
    report("shop.mount", "mount + realize, median of 200", median(mounts));
    report("shop.unmount", "unmount", median(unmounts));

    let (presses_before, links_before) = (h.shared.button_presses.borrow().len(), h.shared.link_activations.borrow().len());
    let (_, r) = mount(&h, || shop(s));
    let back = h.shared.button_presses.borrow()[presses_before].clone();
    let open_first = h.shared.link_activations.borrow()[links_before].clone();
    report("shop.cart", "app sets the cart (header text)", per_op(&h, 2000, |i| s.cart.set(i as u32)));
    let (mut pushes, mut pops) = (Vec::new(), Vec::new());
    for _ in 0..200 {
        let t = Instant::now();
        open_first();
        h.flush();
        pushes.push(t.elapsed());
        let t = Instant::now();
        back();
        h.flush();
        pops.push(t.elapsed());
    }
    report("shop.push", "push product detail (link → new screen)", median(pushes));
    report("shop.pop", "pop back to the list (header ‹ Back)", median(pops));
    open_first();
    h.flush();
    let slide = h.shared.slider_changes.borrow().last().unwrap().clone();
    report("detail.slider", "slider change (handler → screen state → its text)", per_op(&h, 2000, |i| slide((i % 5 + 1) as f32)));
    drop((slide, back, open_first));
    unmount(&h, r);

    // --- Memory ---------------------------------------------------------------
    println!("\nmemory:");
    const KEPT: usize = 200;
    let wasm_before = remote_showcase::bundle_memory_bytes();
    let rss_before = rss();
    let kept: Vec<_> = (0..KEPT).map(|_| mount(&h, || feed(s)).1).collect();
    let rss_after = rss();
    let wasm_after = remote_showcase::bundle_memory_bytes();
    report_bytes("mem.rss.start", "process RSS before any screen", rss_start);
    report_bytes(
        "mem.per_feed",
        &format!("RSS per mounted FeedScreen ({KEPT} kept)"),
        rss_after.saturating_sub(rss_before) / KEPT,
    );
    if REMOTE {
        report_bytes("mem.wasm.before", "bundle linear memory before", wasm_before);
        report_bytes("mem.wasm.after", &format!("bundle linear memory with {KEPT} feeds"), wasm_after);
    }
    for r in kept {
        unmount(&h, r);
    }
    if REMOTE {
        assert_eq!(runtime_vocabulary::remote::host::live_trees(), 0, "every remote tree torn down");
    }
}
