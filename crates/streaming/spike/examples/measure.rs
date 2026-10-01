//! Spike measurements: what a streamed component costs against the same
//! component compiled into the binary.
//!
//! Run with `cargo run --release -p stream-spike --example measure`. The
//! guest is always release-built; the host must be too, or the native
//! baseline is a debug build and every ratio lies.
//!
//! Every figure is the median of repeated runs on a host-mock scene, so it
//! measures the framework + interpreter, not a platform toolkit.

use std::hint::black_box;
use std::time::{Duration, Instant};

use host_mock::Harness;
use runtime_scene::Element;
use runtime_vocabulary::builders::{button, text, view};
use runtime_world::ReadSignal;
use stream_host::{Bundle, StreamEngine};
use stream_spike::{host_exports, GUEST_WASM};
use wasmi::CompilationMode;

fn median(mut v: Vec<Duration>) -> Duration {
    v.sort();
    v[v.len() / 2]
}

fn sample(runs: usize, mut f: impl FnMut() -> Duration) -> Duration {
    median((0..runs).map(|_| f()).collect())
}

fn fmt(d: Duration) -> String {
    let us = d.as_secs_f64() * 1e6;
    if us >= 1000.0 {
        format!("{:.2} ms", us / 1000.0)
    } else {
        format!("{us:.1} µs")
    }
}

/// The guest's `Counter`, written natively with the same structure.
fn native_counter(title: String, external: ReadSignal<i64>) -> Element {
    runtime_scene::component_scope(move || {
        let clicks = runtime_world::signal(0i64);
        let doubled = runtime_world::signal(0i64);
        runtime_world::effect(move || doubled.set(clicks.get() * 2));
        view()
            .child(text().content(title))
            .child(text().content(move || format!("external: {}", external.get())))
            .child(text().content(move || format!("clicks: {} doubled: {}", clicks.get(), doubled.get())))
            .child(button().label("add").on_press(move || clicks.update(|c| c + 1)))
            .child(view().child(text().content("[streamed]".to_string())))
            .build()
    })
}

fn native_compute(n: u32, seed: u32) -> i64 {
    let mut x = seed as u64 | 1;
    let mut acc = 0u64;
    for _ in 0..n {
        x ^= x << 13;
        x ^= x >> 7;
        x ^= x << 17;
        acc = acc.wrapping_add(x % 1000);
    }
    acc as i64
}

fn main() {
    println!("guest bundle: {} bytes ({:.1} KB)", GUEST_WASM.len(), GUEST_WASM.len() as f64 / 1024.0);

    // --- Load: validate + translate + instantiate + manifest check -------
    println!("\nload (Bundle::load), median of 50:");
    for (label, mode) in [
        ("eager", CompilationMode::Eager),
        ("lazy-translation (default)", CompilationMode::LazyTranslation),
        ("lazy", CompilationMode::Lazy),
    ] {
        let engine = StreamEngine::with_mode(mode);
        let mut drops = Vec::new();
        let d = sample(50, || {
            let t = Instant::now();
            let b = black_box(Bundle::load(&engine, GUEST_WASM, host_exports()).unwrap());
            let d = t.elapsed();
            let t = Instant::now();
            drop(b);
            drops.push(t.elapsed());
            d
        });
        println!("  {label:<28} {}   (unload {})", fmt(d), fmt(median(drops)));
    }

    // --- Mount: description from the guest + realize into the scene ------
    let engine = StreamEngine::new();
    let cold = sample(50, || {
        let h = Harness::new();
        let b = Bundle::load(&engine, GUEST_WASM, host_exports()).unwrap();
        let ext = h.world.signal(1i64);
        let t = Instant::now();
        let tree = h.world.enter(|| b.mount("Counter", b.props().value("title", "t".to_string()).read_signal("external", ext.read_only())));
        let r = h.mount(tree);
        h.flush();
        let d = t.elapsed();
        drop(r);
        d
    });
    let (warm, native_mount) = {
        let h = Harness::new();
        let b = Bundle::load(&engine, GUEST_WASM, host_exports()).unwrap();
        let ext = h.world.signal(1i64);
        // Pay lazy translation once, then measure steady-state mounts.
        drop(h.mount(h.world.enter(|| b.mount("Counter", b.props().value("title", "t".to_string()).read_signal("external", ext.read_only())))));
        let warm = sample(200, || {
            let t = Instant::now();
            let tree = h.world.enter(|| b.mount("Counter", b.props().value("title", "t".to_string()).read_signal("external", ext.read_only())));
            let r = h.mount(tree);
            h.flush();
            let d = t.elapsed();
            drop(r);
            d
        });
        let native = sample(200, || {
            let t = Instant::now();
            let tree = h.world.enter(|| native_counter("t".into(), ext.read_only()));
            let r = h.mount(tree);
            h.flush();
            let d = t.elapsed();
            drop(r);
            d
        });
        (warm, native)
    };
    println!("\nmount + realize + flush of Counter (5 nodes, 2 reactive texts, 1 effect):");
    println!("  streamed, first mount (cold)  {}", fmt(cold));
    println!("  streamed, warm                {}", fmt(warm));
    println!("  native                        {}", fmt(native_mount));

    // --- Update round trip: host set → flush → guest text closure --------
    const UPDATES: i64 = 10_000;
    let per_update = |streamed: bool| {
        let h = Harness::new();
        let b = Bundle::load(&engine, GUEST_WASM, host_exports()).unwrap();
        let ext = h.world.signal(0i64);
        let tree = h.world.enter(|| {
            if streamed {
                b.mount("Counter", b.props().value("title", "t".to_string()).read_signal("external", ext.read_only()))
            } else {
                native_counter("t".into(), ext.read_only())
            }
        });
        let _r = h.mount(tree);
        h.flush();
        let t = Instant::now();
        for i in 1..=UPDATES {
            ext.set(i);
            h.flush();
        }
        t.elapsed() / UPDATES as u32
    };
    println!("\nhost signal set → flush → text re-rendered, per update:");
    println!("  streamed  {}", fmt(per_update(true)));
    println!("  native    {}", fmt(per_update(false)));

    // --- Raw crossing cost: guest reads a host signal --------------------
    const READS: u32 = 1_000_000;
    let h = Harness::new();
    let b = Bundle::load(&engine, GUEST_WASM, host_exports()).unwrap();
    let sig = h.world.signal(3i64);
    let handle = b.__bench_handle(sig.read_only());
    b.__call_export("bench_reads", handle, 10); // translate before timing
    let t = Instant::now();
    black_box(b.__call_export("bench_reads", handle, READS));
    let guest_read = t.elapsed() / READS;
    b.__call_export("bench_raw_reads", handle, 10);
    let t = Instant::now();
    black_box(b.__call_export("bench_raw_reads", handle, READS));
    let raw_read = t.elapsed() / READS;
    let ro = sig.read_only();
    let t = Instant::now();
    let mut acc = 0i64;
    for _ in 0..READS {
        acc = acc.wrapping_add(black_box(ro).get());
    }
    black_box(acc);
    let native_read = t.elapsed() / READS;
    println!("\nsignal read, per read:");
    println!("  guest → host, typed    {:.0} ns", guest_read.as_nanos());
    println!("  guest → host, raw      {:.0} ns  (crossing only, no guest decode)", raw_read.as_nanos());
    println!("  native                 {:.0} ns", native_read.as_nanos());

    // --- Interpreter speed on pure compute --------------------------------
    const N: u32 = 20_000_000;
    b.__call_export("bench_compute", 10, 7);
    let t = Instant::now();
    let g = b.__call_export("bench_compute", N, 7);
    let guest_c = t.elapsed();
    let t = Instant::now();
    let n = native_compute(black_box(N), black_box(7));
    let native_c = t.elapsed();
    assert_eq!(g, n, "guest and native compute disagree");
    println!("\npure compute ({N} xorshift steps):");
    println!("  guest (wasmi)  {}", fmt(guest_c));
    println!("  native         {}", fmt(native_c));
    println!("  ratio          {:.1}x", guest_c.as_secs_f64() / native_c.as_secs_f64());
}
