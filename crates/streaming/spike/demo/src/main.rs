//! See the remote-component spike in a real window, live-reloading.
//!
//! ```sh
//! cargo run --release -p stream-spike --bin stream-serve   # terminal 1
//! cargo run --release -p stream-demo                        # terminal 2
//! ```
//!
//! On screen: `RemoteCounter` from `spike/components`, written with
//! `#[component]` + `ui!`, running from the `spike-remoteguest` bundle. Its
//! signals live in this app's graph and its tree is realized by this app's
//! own AppKit backend. Next to it is the SAME component compiled into the
//! app, for comparison. Edit `crates/streaming/spike/components/src/lib.rs`,
//! press **Refresh bundle**, and the bundle's copy changes while the native
//! copy does not — no app rebuild, no restart. With no server running the
//! app uses the bundle compiled into it, the way a shipped app falls back
//! when a fetch fails.
//!
//! What survives a refresh is the point: `external` and the signed-in user
//! are HOST signals, so their values carry over; the counter's own clicks
//! were the old bundle's state and start over.
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
use runtime_core::{provide, ui};
use runtime_world::{signal, Signal};
use spike_components::{CurrentUser, RemoteCounter};
use stream_abi::Wire;
use stream_host::kernel::{export_context, export_read_signal, ExportGuard, KernelBundle};
use stream_spike::{fetch, REMOTE_GUEST_WASM};

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

/// Fetch and load the served bridged bundle.
fn fetch_remote(engine: &wasmi::Engine) -> Result<(KernelBundle, String), String> {
    let url = std::env::var("STREAM_REMOTE_URL").unwrap_or_else(|_| fetch::DEFAULT_REMOTE_URL.to_string());
    let t = Instant::now();
    let fetched = fetch::fetch(&url)?;
    let fetch_ms = t.elapsed().as_secs_f64() * 1000.0;
    let t = Instant::now();
    let bundle = KernelBundle::load(engine, &fetched.wasm).map_err(|e| e.to_string())?;
    let load_us = t.elapsed().as_micros();
    Ok((
        bundle,
        format!("v{} from {url}: {} KB, fetched in {fetch_ms:.0} ms, loaded in {load_us} µs", fetched.version, fetched.wasm.len() / 1024),
    ))
}

/// What the bridged RemoteCounter's mount export reads: its `title`, then
/// the handle of the host signal it gets as `external`.
fn remote_args(title: &str, external: (u32, u32, u32)) -> Vec<u8> {
    let mut args = Vec::new();
    title.to_string().encode(&mut args);
    for v in [external.0, external.1, external.2] {
        v.encode(&mut args);
    }
    args
}

fn tinted(color: &str) -> StyleRules {
    StyleRules { background: Some(Tokenized::Literal(Color::from(color))), ..section() }
}

/// The bridged design: RemoteCounter from a bundle, next to the same
/// component compiled in.
fn bridged(external: Signal<i64>, user: Signal<String>) -> Element {
    // Host state handed to bundles: `external` as a read-only prop, the
    // signed-in user as context under the name the bundle registered. The
    // guards live as long as the region below.
    let (external_handle, g1) = export_read_signal(external.read_only());
    let (user_handle, g2) = export_read_signal(user.read_only());
    let g3 = export_context::<CurrentUser>("CurrentUser", move |_, out| {
        for v in [user_handle.0, user_handle.1, user_handle.2] {
            v.encode(out);
        }
    });
    let guards: Rc<Vec<ExportGuard>> = Rc::new(vec![g1, g2, g3]);

    let engine = stream_host::remote::engine();
    let (initial, initial_status) = match fetch_remote(&engine) {
        Ok(ok) => ok,
        Err(e) => (
            KernelBundle::load(&engine, REMOTE_GUEST_WASM).unwrap_or_else(|e| panic!("stream-demo: built-in remote bundle: {e}")),
            format!("using the built-in bundle:\n{e}"),
        ),
    };
    let generation = signal(0u32);
    // A panic in the bundle stops it; remount so the region shows the
    // panic message instead of a tree that can no longer reach the bundle.
    let watch = move |bundle: &KernelBundle| bundle.on_poison(move |_| generation.update(|g| g + 1));
    watch(&initial);
    let current = Rc::new(RefCell::new(Rc::new(initial)));
    let status: Signal<String> = signal(initial_status);
    REMOTE_REFRESH.with(|r| {
        let current = current.clone();
        *r.borrow_mut() = Some(Box::new(move || match fetch_remote(&engine) {
            Ok((bundle, msg)) => {
                watch(&bundle);
                *current.borrow_mut() = Rc::new(bundle);
                generation.update(|g| g + 1);
                status.set(msg);
            }
            Err(e) => status.set(format!("refresh failed, still running the previous bundle:\n{e}")),
        }));
    });

    view()
        .style(StyleRules { gap: px(10.0), ..StyleRules::default() })
        .child(text().content("Bridged remote component — RemoteCounter (spike/components/src/lib.rs)"))
        .child(text().content(move || status.get()))
        .child(view().style(tinted("#e6f4ea")).child(text().content("↓ From the wasm bundle: app graph, app backend")).child(dyn_element(
            move || {
                let _ = generation.get();
                let _ = &guards;
                let bundle = current.borrow().clone();
                let args = remote_args("Hello from a bundle", external_handle);
                let mounted = match bundle.mount_remote("rc_mount", &args) {
                    Ok(element) => element,
                    Err(e) => text().content(format!("⚠ {e}")).build(),
                };
                // The tree calls back into this bundle; keep it alive exactly
                // as long as the tree (a refresh swaps both together).
                runtime_world::on_scope_drop(move || drop(bundle));
                mounted
            },
        )))
        .child(view().style(tinted("#f1f3f4")).child(text().content("↓ The same component, compiled into the app")).child({
            let external = external.read_only();
            let title = "Hello from the app".to_string();
            ui! { RemoteCounter(title = title, external = external) }
        }))
        .build()
}

thread_local! {
    /// The bundle section's refresh, for the Refresh button.
    static REMOTE_REFRESH: RefCell<Option<Box<dyn Fn()>>> = const { RefCell::new(None) };
}

fn app() -> Element {
    let external = signal(0i64);
    let user = signal("ada".to_string());
    provide(CurrentUser(user.read_only()));
    let bridged_section = bridged(external, user);

    let refresh = move || {
        REMOTE_REFRESH.with(|r| {
            if let Some(f) = r.borrow().as_ref() {
                f()
            }
        });
    };

    view()
        .style(column())
        .child(button().label("Refresh bundle").on_press(refresh))
        .child(text().content("Native host controls"))
        .child(button().label("external + 1").on_press(move || external.update(|v| v + 1)))
        .child(button().label("external − 1").on_press(move || external.update(|v| v - 1)))
        .child(button().label(move || format!("Signed in as {} — switch user", user.get())).on_press(move || {
            user.update(|u| if u == "ada" { "grace".to_string() } else { "ada".to_string() })
        }))
        .child(bridged_section)
        .build()
}

#[cfg(target_os = "macos")]
fn main() {
    let opts = host_appkit::RunOptions { title: "Remote components".to_string(), width: 680.0, height: 980.0 };
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
