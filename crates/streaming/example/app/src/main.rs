//! A native app, a standard component and a remote component, in one file.
//!
//! `ScoreCard` and `Scoreboard` take the same props and are written the same
//! way. The only difference is `#[component(remote)]`: `ScoreCard` is
//! compiled into the app, `Scoreboard` ships in the bundle.
//!
//! `Badge` is used by both. Every component NOT marked `remote` lives in the
//! app binary, so when the bundle's `Scoreboard` renders a `Badge` it uses
//! the app's own copy, by name — the bundle doesn't contain `Badge` at all.
//! `Scoreboard` hands it `taps`, a signal the bundle created; the app takes
//! the value over (it now lives in the app's arena, and both sides share
//! it).
//!
//! This file compiles twice:
//!
//! - as the **app** (`cargo run --release -p remote-example`): `App` and
//!   `main` compile, and `Scoreboard`'s body does NOT — `#[component(remote)]`
//!   replaces it with a stub that sends its props and mounts it from the
//!   bundle;
//! - as the **bundle** (`remote-example-bundle`, wasm32, built by this
//!   package's build script and by stream-serve): only `Scoreboard`'s body
//!   compiles, plus a mount export.
//!
//! The app's state crosses into the remote component as props: `score` is a
//! `ReadSignal` (the bundle reads the app's signal live), `cheers` is a
//! `Signal` (the bundle writes the app's signal), `player` is a plain value
//! (copied at mount). The bundle's own state (`taps`) lives in the app's
//! reactive graph too — there is one graph, and the app's own backend draws
//! everything.
//!
//! Live reload: run `cargo run --release -p stream-spike --bin stream-serve`,
//! edit `Scoreboard` below, press **Reload remote**. The remote section
//! changes; the app is not rebuilt. (Editing `App` needs an app rebuild, as
//! it would for any native code.)
//!
//! Remote components can use every leaf primitive (`view`, `text`, `button`,
//! `image`, `icon`, `link`, `toggle`, `slider`, `text_input`, `text_area`,
//! `scroll_view`, …) and their event handlers; anything not carried yet
//! shows the bundle's error in the component's place.

#![cfg_attr(idealyst_stream_guest, allow(dead_code, unused_imports))]

use runtime_core::{component, signal, ui, Element, ReadSignal, Signal};
use runtime_core::host_fn;

/// A host function: native code the remote component calls. In the app
/// this is the function as written; in the bundle, a stub that asks the app
/// to run it. The app allows it by listing `os_name::export()` when it
/// installs the bundle (below). Wasm itself has no OS to report — run there,
/// `std::env::consts::OS` would say "unknown".
#[host_fn]
pub fn os_name() -> String {
    std::env::consts::OS.to_string()
}

/// An app component used by BOTH the native app and the remote component.
#[component]
pub fn Badge(label: String, value: ReadSignal<i64>) -> Element {
    ui! {
        view() {
            text { "[{label}: {value}] (drawn by the app's Badge)" }
        }
    }
}

/// A standard component: compiled into the app, like any other. Changing it
/// needs an app rebuild.
#[component]
pub fn ScoreCard(player: String, score: ReadSignal<i64>, cheers: Signal<i64>) -> Element {
    let taps = signal(0i64);
    ui! {
        view() {
            text { "Standard component — playing as {player}" }
            text { "Score (the app's signal, read live): {score}" }
            text { "Taps (this component's own state): {taps}" }
            button(label = "Tap", on_click = move || taps.update(|t| t + 1))
            button(label = "Cheer (writes the app's signal)", on_click = move || cheers.update(|c| c + 1))
        }
    }
}

/// A remote component: its body ships in the bundle. Edit it, press
/// **Reload remote**, and it changes without an app rebuild.
#[component(remote)]
pub fn Scoreboard(player: String, score: ReadSignal<i64>, cheers: Signal<i64>) -> Element {
    let taps = signal(0i64);
    let raps = signal(0i64);
    let os = os_name();
    ui! {
        view() {
            text { "Remote component 123 — playing as {player}" }
            text { "Score (the app's signal, read live): {score}" }
            text { "Taps (state inside the bundle): {taps}" }
            text { "Raps: {raps}" }
            text { "Running on (asked the app): {os}" }
            Badge(label = "bundle taps".to_string(), value = taps.read_only())
            button(label = "Tap", on_click = move || taps.update(|t| t + 1))
            button(label = "Cheer (writes the app's signal)", on_click = move || cheers.update(|c| c + 1))
            button(label = "Rap (writes the raps signal)", on_click = move || raps.update(|c| c + 1))
        }
    }
}

#[cfg(not(idealyst_stream_guest))]
mod app {
    use super::*;
    use std::cell::RefCell;

    use stream_host::remote::RemoteApp;

    /// The bundle built from this file at compile time — used until a
    /// reload brings a newer one.
    const BUILT_IN: &[u8] = include_bytes!(concat!(env!("OUT_DIR"), "/bundle.wasm"));
    const SERVED: &str = "http://127.0.0.1:7878/example.wasm";

    thread_local! {
        static REMOTE: RefCell<Option<RemoteApp>> = const { RefCell::new(None) };
    }

    fn reload(status: Signal<String>) {
        let result = stream_host::fetch::fetch(SERVED).and_then(|fetched| {
            REMOTE.with(|r| r.borrow().as_ref().expect("installed").reload(&fetched.wasm))?;
            Ok(format!("loaded v{} ({} KB) from {SERVED}", fetched.version, fetched.wasm.len() / 1024))
        });
        status.set(result.unwrap_or_else(|e| format!("reload failed, keeping the current bundle: {e}")));
    }

    /// The native app.
    #[component]
    pub fn App() -> Element {
        let score = signal(0i64);
        let cheers = signal(0i64);
        let status = signal(format!("built-in bundle ({} KB)", BUILT_IN.len() / 1024));
        // Scrolls: the page is taller than the window once the remote
        // component grows, and an unscrolled view just clips the bottom.
        ui! {
            scroll_view() {
                text { "Native app" }
                button(label = "Score + 1", on_click = move || score.update(|s| s + 1))
                text { "Cheers the app has received 123: {cheers}" }
                button(label = "Reload remote", on_click = move || reload(status))
                text { "{status}" }
                Badge(label = "app score".to_string(), value = score.read_only())
                ScoreCard(player = "grace".to_string(), score = score.read_only(), cheers = cheers)
                Scoreboard(player = "ada".to_string(), score = score.read_only(), cheers = cheers)
            }
        }
    }

    pub fn install() {
        let remote = stream_host::remote::install_with(BUILT_IN, vec![os_name::export()]).unwrap_or_else(|e| panic!("built-in bundle: {e}"));
        REMOTE.with(|r| *r.borrow_mut() = Some(remote));
    }
}

#[cfg(all(not(idealyst_stream_guest), target_os = "macos"))]
fn main() {
    app::install();
    let opts = host_appkit::RunOptions { title: "Remote components".to_string(), width: 560.0, height: 900.0 };
    if let Err(e) = host_appkit::newcore::run(|| ui! { app::App() }, opts) {
        eprintln!("[remote-example] failed to boot: {e}");
        std::process::exit(1);
    }
}

#[cfg(all(not(idealyst_stream_guest), not(target_os = "macos")))]
fn main() {
    eprintln!("remote-example's window is macOS-only (host-appkit)");
}
