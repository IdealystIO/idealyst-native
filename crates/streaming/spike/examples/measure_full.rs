//! Model B measured: the REAL framework inside the bundle vs the same
//! component native.
//!
//! `cargo run --release -p stream-spike --example measure_full`
//!
//! Three columns for the same `RemoteCounter`:
//! - **native**: realized straight into a host-mock scene (what an app gets
//!   linking the component normally);
//! - **native + wire**: realized natively against the wire recorder, encoded,
//!   decoded and replayed — isolates the record/codec/replay cost;
//! - **bundle**: the wasm bundle in wasmi doing the realize, then the same
//!   codec + replay — the cost of model B.

use std::hint::black_box;
use std::sync::mpsc;
use std::time::{Duration, Instant};

use dev_client::WireBackend;
use dev_server::newcore::SceneSession;
use dev_server::WireRecordingBackend;
use mock_backend::MockBackend;
use runtime_core::{provide, signal, ui};
use spike_components::{CurrentUser, RemoteCounter};
use stream_spike::full::{button_handler, engine, FullGuest};
use stream_spike::FULL_GUEST_WASM;
use wasmi::CompilationMode;
use wire::{Command, DevToApp};

fn median(mut v: Vec<Duration>) -> Duration {
    v.sort();
    v[v.len() / 2]
}

fn fmt(d: Duration) -> String {
    let us = d.as_secs_f64() * 1e6;
    if us >= 1000.0 { format!("{:.2} ms", us / 1000.0) } else { format!("{us:.1} µs") }
}

/// Replay into a fresh client. The receiver is returned so the reverse
/// (tap) channel stays open for as long as the client is in use.
fn replay(cmds: Vec<Command>) -> (WireBackend<MockBackend>, mpsc::Receiver<wire::AppToDev>) {
    let (tx, rx) = mpsc::channel();
    let mut client = WireBackend::new(MockBackend::new(), tx);
    client.apply_batch(cmds).unwrap();
    (client, rx)
}

/// The wire hop a remote bundle pays on every batch: encode on one side,
/// decode on the other. (The bundle encodes inside wasm; this is the
/// native-side equivalent for the "native + wire" column.)
fn codec(cmds: Vec<Command>) -> Vec<Command> {
    let bytes = wire::codec::encode(&DevToApp::Commands(cmds)).unwrap();
    match wire::codec::decode::<DevToApp>(&bytes).unwrap() {
        DevToApp::Commands(c) => c,
        _ => unreachable!(),
    }
}

fn main() {
    println!("bundle: {} KB raw", FULL_GUEST_WASM.len() / 1024);

    for (label, mode) in [("eager", CompilationMode::Eager), ("lazy-translation", CompilationMode::LazyTranslation)] {
        let e = engine(mode);
        let d = median((0..20).map(|_| {
            let t = Instant::now();
            black_box(FullGuest::load(&e, FULL_GUEST_WASM).unwrap());
            t.elapsed()
        }).collect());
        println!("load ({label}): {}", fmt(d));
    }

    let e = engine(CompilationMode::LazyTranslation);

    // ---- mount ---------------------------------------------------------
    let cold = median((0..20).map(|_| {
        let mut g = FullGuest::load(&e, FULL_GUEST_WASM).unwrap();
        let t = Instant::now();
        let cmds = g.mount("Hello", 1, "ada");
        black_box(replay(cmds));
        t.elapsed()
    }).collect());
    let mut g = FullGuest::load(&e, FULL_GUEST_WASM).unwrap();
    g.mount("Hello", 1, "ada");
    g.unmount();
    let warm = median((0..100).map(|_| {
        let t = Instant::now();
        let cmds = g.mount("Hello", 1, "ada");
        black_box(replay(cmds));
        let d = t.elapsed();
        g.unmount();
        d
    }).collect());

    let native = median((0..100).map(|_| {
        let h = host_mock::Harness::new();
        let t = Instant::now();
        let tree = h.world.enter(|| {
            provide(CurrentUser(signal("ada".to_string()).read_only()));
            let external = signal(1i64).read_only();
            let title = "Hello".to_string();
            ui! { RemoteCounter(title = title, external = external) }
        });
        let r = h.mount(tree);
        h.flush();
        let d = t.elapsed();
        drop(r);
        d
    }).collect());

    dev_server::scheduler::install();
    let native_wire = median((0..100).map(|_| {
        let t = Instant::now();
        let recorder = WireRecordingBackend::new();
        let session = SceneSession::mount(&recorder, |_| {}, || {
            provide(CurrentUser(signal("ada".to_string()).read_only()));
            let external = signal(1i64).read_only();
            let title = "Hello".to_string();
            ui! { RemoteCounter(title = title, external = external) }
        });
        session.flush();
        black_box(replay(codec(recorder.drain_commands())));
        let d = t.elapsed();
        drop(session);
        d
    }).collect());

    println!("\nmount RemoteCounter (6 nodes, 3 reactive texts, context, a button):");
    println!("  native                 {}", fmt(native));
    println!("  native + wire          {}", fmt(native_wire));
    println!("  bundle, first (cold)   {}", fmt(cold));
    println!("  bundle, warm           {}", fmt(warm));

    // ---- updates ---------------------------------------------------------
    const N: i64 = 2000;
    let mut g = FullGuest::load(&e, FULL_GUEST_WASM).unwrap();
    let cmds = g.mount("Hello", 0, "ada");
    let add = button_handler(&cmds, "add").unwrap();
    let (mut client, _taps) = replay(cmds);
    let t = Instant::now();
    for i in 1..=N {
        let c = g.set_external(i);
        client.apply_batch(c).unwrap();
    }
    let prop = t.elapsed() / N as u32;
    let t = Instant::now();
    for _ in 0..N {
        let c = g.dispatch(add);
        client.apply_batch(c).unwrap();
    }
    let tap = t.elapsed() / N as u32;
    let t = Instant::now();
    for i in 0..N {
        let c = g.set_user(if i % 2 == 0 { "grace" } else { "ada" });
        client.apply_batch(c).unwrap();
    }
    let ctx = t.elapsed() / N as u32;

    let h = host_mock::Harness::new();
    let ext = h.world.signal(0i64);
    let tree = h.world.enter(|| {
        provide(CurrentUser(signal("ada".to_string()).read_only()));
        let external = ext.read_only();
        let title = "Hello".to_string();
        ui! { RemoteCounter(title = title, external = external) }
    });
    let _r = h.mount(tree);
    h.flush();
    let t = Instant::now();
    for i in 1..=N {
        ext.set(i);
        h.flush();
    }
    let native_prop = t.elapsed() / N as u32;

    println!("\nper update (bundle includes codec + replay):");
    println!("  host prop change   bundle {}   native {}", fmt(prop), fmt(native_prop));
    println!("  tap                bundle {}", fmt(tap));
    println!("  context change     bundle {}", fmt(ctx));
}
