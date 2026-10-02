//! `#[component(remote)]` end to end. `spike-remoteattr`'s `Greeting` is
//! linked here natively — where the macro made it a stub — and its wasm
//! build is the bundle the stub mounts from. A `ui!` call site in app code
//! is all the app writes.

use host_mock::Harness;
use runtime_core::{ui, Element};
use runtime_world::{signal, Signal};
use spike_remoteattr::Greeting;
use stream_spike::REMOTE_ATTR_WASM;

struct App {
    h: Harness,
    count: Signal<i64>,
    likes: Signal<i64>,
    remote: stream_host::remote::RemoteApp,
}

fn app() -> App {
    let remote = stream_host::remote::install_with(REMOTE_ATTR_WASM, camera()).expect("bundle loads");
    let h = Harness::new();
    let (count, likes) = h.world.enter(|| (signal(1i64), signal(0i64)));
    App { h, count, likes, remote }
}

fn tree(a: &App) -> Element {
    let (count, likes) = (a.count, a.likes);
    a.h.world.enter(|| ui! { Greeting(name = "ada".to_string(), count = count.read_only(), likes = likes) })
}

fn text(a: &App, realized: &runtime_scene::Realized<host_mock::Node>) -> String {
    realized.collect_nodes().iter().map(|n| a.h.live_tree(*n)).collect::<Vec<_>>().join("\n")
}

#[test]
fn a_remote_component_mounts_from_its_bundle_with_the_apps_state() {
    let a = app();
    let realized = a.h.mount(tree(&a));
    a.h.flush();
    let t = text(&a, &realized);
    for want in ["hello ada", "count 1", "taps 0"] {
        assert!(t.contains(want), "missing {want:?}:\n{t}");
    }

    // The app's signal, read live by the bundle.
    a.count.set(7);
    a.h.flush();
    assert!(text(&a, &realized).contains("count 7"));

    // Bundle state, and a bundle write to the app's signal.
    let presses = a.h.shared.button_presses.borrow().clone();
    presses[0]();
    presses[1]();
    presses[1]();
    a.h.flush();
    assert!(text(&a, &realized).contains("taps 1"));
    assert_eq!(a.likes.get(), 2, "the bundle wrote the app's signal");
}

/// A reload remounts every mounted remote component from the new bundle.
#[test]
fn reloading_the_bundle_remounts_the_component() {
    let a = app();
    let realized = a.h.mount(tree(&a));
    a.h.flush();
    let presses = a.h.shared.button_presses.borrow().clone();
    presses[0]();
    a.h.flush();
    assert!(text(&a, &realized).contains("taps 1"));

    a.remote.reload(REMOTE_ATTR_WASM).expect("reloads");
    a.h.flush();
    let t = text(&a, &realized);
    assert!(t.contains("taps 0"), "remounted from the new bundle, with fresh bundle state:\n{t}");
    assert!(t.contains("count 1"), "app state survives the reload:\n{t}");
}

/// Reloading frees the replaced bundle's compiled code once its trees are
/// gone. wasmi's `Engine` keeps every function it compiled until the engine
/// drops, so a loader sharing one engine across reloads kept each old
/// bundle's code (~750 KB per reload of the showcase) for the app's life.
#[test]
fn regression_a_reload_frees_the_replaced_bundles_code() {
    let a = app();
    let realized = a.h.mount(tree(&a));
    a.h.flush();
    let old = a.remote.__engine();
    a.remote.reload(REMOTE_ATTR_WASM).expect("reloads");
    a.h.flush();
    // The old tree is gone; the handlers the mock kept would still own it.
    a.h.forget_handlers();
    assert!(old.upgrade().is_none(), "the replaced bundle's engine (and its compiled code) is still alive");
    assert!(a.remote.__engine().upgrade().is_some(), "the current bundle's engine lives");
    drop(realized);
}

/// Unmounting releases the props' exports: the app's signals are no longer
/// read by anything.
#[test]
fn unmounting_releases_the_props() {
    let a = app();
    let realized = a.h.mount(tree(&a));
    a.h.flush();
    assert!(a.count.subscriber_count() > 0);
    drop(realized);
    a.h.flush();
    a.h.shared.button_presses.borrow_mut().clear();
    assert_eq!(a.count.subscriber_count(), 0);
    assert_eq!(runtime_world::remote::pending_scopes(), 0);
}

/// A bundle that does not define the component the app asks for shows the
/// failure in its place.
#[test]
fn a_bundle_without_the_component_shows_an_error() {
    let a = app();
    a.remote.reload(stream_spike::KERNEL_GUEST_WASM).expect("loads");
    let realized = a.h.mount(tree(&a));
    a.h.flush();
    let t = text(&a, &realized);
    assert!(t.contains("remote component `Greeting`") && t.contains("__idealyst_remote_Greeting"), "{t}");
}

/// The remote component renders an APP component (`Panel`, not `remote`):
/// the bundle imports the app's copy by name, and every prop crosses.
#[test]
fn a_remote_component_uses_an_app_component_with_live_props() {
    let a = app();
    let realized = a.h.mount(tree(&a));
    a.h.flush();
    let t = text(&a, &realized);
    for want in ["panel of ada", "panel sees 0", "panel edit draft", "bundle child sees draft"] {
        assert!(t.contains(want), "missing {want:?}:\n{t}");
    }
    let presses = a.h.shared.button_presses.borrow().clone();
    // Greeting: tap, like; Panel (the app's own code): reset, app edits.
    assert_eq!(presses.len(), 4, "{t}");

    // Bundle state the app component reads natively (the signal was promoted).
    presses[0]();
    presses[0]();
    a.h.flush();
    let t = text(&a, &realized);
    assert!(t.contains("taps 2") && t.contains("panel sees 2"), "{t}");

    // The app component calls back into the bundle.
    presses[2]();
    a.h.flush();
    let t = text(&a, &realized);
    assert!(t.contains("taps 0") && t.contains("panel sees 0"), "{t}");

    // The app component writes a signal the bundle created; the bundle's own
    // text (the children it passed in) follows.
    presses[3]();
    a.h.flush();
    let t = text(&a, &realized);
    assert!(t.contains("panel edit from app") && t.contains("bundle child sees from app"), "{t}");
}

/// The app component's body is the app's: it is registered for import, and
/// the bundle does not carry it.
#[test]
fn app_components_are_registered_for_import() {
    let names = runtime_vocabulary::remote::host::app_component_names();
    assert!(names.contains(&"spike_remoteattr::Panel"), "{names:?}");
    assert!(!names.contains(&"spike_remoteattr::Greeting"), "a remote component is not an import: {names:?}");
}

/// Regression: wasmi's tail-call dispatch keeps the native stack flat only
/// if LLVM turns every handler call into a sibling call, which depends on
/// how wasmi and its dependencies are compiled — with wasmi at opt-level 3
/// it grew the stack per interpreted instruction, and a large bundle
/// overflowed a 2 MB thread mid-mount. `stream-host` uses portable (loop)
/// dispatch, which never grows the stack. The thread is sized explicitly so
/// the test does not depend on RUST_MIN_STACK or the harness default.
///
/// The workload is the heaviest the bridged design runs: a remote component
/// importing an app component, promoting two signals, and re-rendering
/// through both kernels on every update.
#[test]
fn regression_bundle_runs_on_a_2mb_thread() {
    std::thread::Builder::new()
        .stack_size(2 * 1024 * 1024)
        .spawn(|| {
            let a = app();
            let realized = a.h.mount(tree(&a));
            a.h.flush();
            let presses = a.h.shared.button_presses.borrow().clone();
            for i in 0..50 {
                a.count.set(i);
                presses[0]();
                a.h.flush();
            }
            let t = text(&a, &realized);
            assert!(t.contains("count 49") && t.contains("panel sees 50"), "{t}");
        })
        .unwrap()
        .join()
        .expect("mount + updates on a 2 MB thread");
}

// ---- containment: a panicking bundle never takes the app down ----

use spike_remoteattr::Fragile;

/// `Greeting` and `Fragile`, both from the bundle, next to app-owned text.
fn fragile_tree(a: &App, trigger: Signal<i64>) -> Element {
    let (count, likes) = (a.count, a.likes);
    a.h.world.enter(|| {
        ui! {
            view() {
                text { "app count {count}" }
                Greeting(name = "ada".to_string(), count = count.read_only(), likes = likes)
                Fragile(trigger = trigger.read_only())
            }
        }
    })
}

/// Regression: a panic in a bundle's press handler trapped the
/// interpreter, and the trap became a panic in the app — one bad handler in
/// a downloaded component crashed the whole app. Now the trap POISONS the
/// bundle: the handler's call answers nothing, and on the next flush every
/// component from that bundle shows the panic message in its place, while
/// the app's own UI keeps working.
#[test]
fn regression_a_panicking_handler_stops_the_bundle_not_the_app() {
    let a = app();
    let trigger = a.h.world.enter(|| runtime_world::signal(0i64));
    let realized = a.h.mount(fragile_tree(&a, trigger));
    a.h.flush();
    let t = text(&a, &realized);
    assert!(t.contains("hello ada") && t.contains("fragile sees 0"), "{t}");

    let presses = a.h.shared.button_presses.borrow().clone();
    let boom = presses.last().expect("Fragile's button").clone();
    boom();
    a.h.flush();

    let t = text(&a, &realized);
    assert!(t.contains("remote component `Greeting`") && t.contains("remote component `Fragile`"), "{t}");
    assert!(t.contains("boom pressed"), "the bundle's own panic message is shown: {t}");
    assert!(!t.contains("hello ada"), "the poisoned bundle's tree is gone: {t}");

    // The app is fine: its own state and UI keep updating.
    a.count.set(41);
    a.h.flush();
    assert!(text(&a, &realized).contains("app count 41"));
    // Handlers still held by the backend are inert, not crashes.
    for p in &presses {
        p();
    }
    a.h.flush();
}

/// The same for a panic inside a bundle EFFECT, which runs in the app's
/// flush: the flush completes, the bundle is stopped.
#[test]
fn regression_a_panicking_effect_stops_the_bundle_not_the_app() {
    let a = app();
    let trigger = a.h.world.enter(|| runtime_world::signal(0i64));
    let realized = a.h.mount(fragile_tree(&a, trigger));
    a.h.flush();
    trigger.set(13);
    a.h.flush();
    let t = text(&a, &realized);
    assert!(t.contains("unlucky trigger") && t.contains("remote component `Greeting`"), "{t}");
    a.count.set(7);
    a.h.flush();
    assert!(text(&a, &realized).contains("app count 7"));
}

/// A fresh bundle brings a poisoned app back: reloading remounts every
/// remote component from it.
#[test]
fn reloading_after_a_panic_recovers() {
    let a = app();
    let trigger = a.h.world.enter(|| runtime_world::signal(0i64));
    let realized = a.h.mount(fragile_tree(&a, trigger));
    a.h.flush();
    let boom = a.h.shared.button_presses.borrow().last().unwrap().clone();
    boom();
    a.h.flush();
    assert!(text(&a, &realized).contains("boom pressed"));

    a.remote.reload(REMOTE_ATTR_WASM).expect("reloads");
    a.h.flush();
    let t = text(&a, &realized);
    assert!(t.contains("hello ada") && t.contains("fragile sees 0") && !t.contains("boom pressed"), "{t}");
}

// ---- #[host_fn]: remote code calling native functions ----

use spike_remoteattr::Snapshot;

/// The app's allowlist of host functions bundles may call.
fn camera() -> Vec<stream_abi::host_fn::HostFnDef> {
    vec![spike_camera::battery_level::export(), spike_camera::take_photo::export()]
}

fn snapshot_tree(a: &App) -> Element {
    a.h.world.enter(|| ui! { Snapshot() })
}

/// A sync host function answers inline; an async one runs the app's real
/// future (here the fake camera's shutter timer, hand-pumped) and its
/// result reaches the bundle's `spawn_then`.
#[test]
fn a_remote_component_calls_sync_and_async_host_functions() {
    host_mock::pump::install_executor();
    host_mock::pump::install_scheduler();
    let a = app();
    let realized = a.h.mount(snapshot_tree(&a));
    a.h.flush();
    let t = text(&a, &realized);
    assert!(t.contains("battery 0.87") && t.contains("photo none"), "{t}");

    let shoot = a.h.shared.button_presses.borrow().last().unwrap().clone();
    shoot();
    host_mock::pump::pump_tasks();
    host_mock::pump::pump_timers();
    host_mock::pump::pump_tasks();
    a.h.flush();
    let t = text(&a, &realized);
    assert!(t.contains("photo #1 4032x3024"), "{t}");
}

/// A bundle calling a host function the app didn't allow is refused at
/// load, naming it — before any of its code runs.
#[test]
fn a_bundle_calling_an_unlisted_host_function_is_refused_at_load() {
    let err = stream_host::remote::install_with(REMOTE_ATTR_WASM, vec![spike_camera::battery_level::export()])
        .err()
        .expect("refused");
    assert!(err.contains("spike_camera::take_photo") && !err.contains("battery_level"), "{err}");
}

/// A host function whose signature changed since the bundle was built is
/// refused at load too.
#[test]
fn a_host_function_with_a_changed_signature_is_refused_at_load() {
    let mut drifted = spike_camera::take_photo::export();
    drifted.schema ^= 1;
    let err = stream_host::remote::install_with(REMOTE_ATTR_WASM, vec![spike_camera::battery_level::export(), drifted])
        .err()
        .expect("refused");
    assert!(err.contains("spike_camera::take_photo"), "{err}");
}

/// A photo still in flight when its bundle is stopped (it panicked) is
/// simply dropped: the app's future completes, and its result has nowhere
/// to go — no call into the poisoned bundle, no panic.
#[test]
fn a_host_result_for_a_stopped_bundle_is_dropped() {
    host_mock::pump::install_executor();
    host_mock::pump::install_scheduler();
    let a = app();
    let trigger = a.h.world.enter(|| runtime_world::signal(0i64));
    let realized = a.h.mount(a.h.world.enter(|| ui! { view() { Snapshot() Fragile(trigger = trigger.read_only()) } }));
    a.h.flush();
    let presses = a.h.shared.button_presses.borrow().clone();
    (presses[0])(); // shoot
    (presses[1])(); // boom: the bundle is stopped
    host_mock::pump::pump_tasks();
    host_mock::pump::pump_timers();
    host_mock::pump::pump_tasks();
    a.h.flush();
    let t = text(&a, &realized);
    assert!(t.contains("boom pressed") && !t.contains("photo #"), "{t}");
}

// ---- refs over the wasm transport ----

/// A remote component's ref reaches the app's real handle over the
/// `idealyst_ui.handle_call` import: its button focuses and types into a
/// text input the app mounted. Unmounting releases the app's entry.
#[test]
fn a_remote_components_ref_drives_the_apps_handle() {
    let a = app();
    host_mock::take_handle_log();
    let realized = a.h.mount(a.h.world.enter(|| ui! { spike_remoteattr::Focuser() }));
    a.h.flush();
    assert_eq!(runtime_vocabulary::remote::handles::held_handles(), 1);
    let press = a.h.shared.button_presses.borrow().last().unwrap().clone();
    press();
    a.h.flush();
    let calls = host_mock::take_handle_log();
    assert_eq!(calls.len(), 2, "{calls:?}");
    assert!(calls[0].starts_with("focus n") && calls[1].starts_with("insert_text \"from the bundle\""), "{calls:?}");
    drop(press);
    drop(realized);
    a.h.flush();
    a.h.forget_handlers();
    assert_eq!(runtime_vocabulary::remote::handles::held_handles(), 0, "the handle went with its tree");
    assert_eq!(runtime_vocabulary::remote::host::live_trees(), 0);
}


// ---- navigation: remote screens inside an app navigator ----

/// The app's stack navigator; its home screen is the REMOTE `Navigating`,
/// handed the navigator's handle as a prop; the detail screen is the app's.
fn navigator_app(a: &App) -> Element {
    use runtime_core::primitives::navigator::Route;
    use runtime_vocabulary::builders::{stack_navigator, text as text_b};
    use spike_remoteattr::{ItemId, Navigating, DETAIL};
    const HOME: Route = Route::new("home", "/");
    a.h.world.enter(|| {
        let nav = runtime_core::Ref::<runtime_vocabulary::prims::NavHandle>::new();
        stack_navigator(&HOME)
            .screen(HOME, move |()| ui! { Navigating(nav = nav) })
            .screen(DETAIL, |ItemId(id)| text_b().content(format!("detail {id}")).build())
            .on_handle(move |h| nav.fill(h))
            .build()
    })
}

fn screen_texts(a: &App) -> String {
    a.h.live_roots().iter().map(|n| a.h.live_tree(*n)).collect::<Vec<_>>().join("\n")
}

/// A remote screen drives the app's navigator: a typed push (its params
/// rebuilt by the app from the url `/items/7`), a pop, and a route link.
#[test]
fn a_remote_screen_navigates_the_apps_navigator() {
    let a = app();
    let realized = a.h.mount(navigator_app(&a));
    a.h.flush();
    let t = screen_texts(&a);
    assert!(t.contains("remote home"), "{t}");

    let presses = a.h.shared.button_presses.borrow().clone();
    presses[0](); // push 7
    a.h.flush();
    let t = screen_texts(&a);
    assert!(t.contains("detail 7"), "the app built its screen from the remote push:\n{t}");

    presses[1](); // pop
    a.h.flush();
    let t = screen_texts(&a);
    assert!(!t.contains("detail 7"), "{t}");

    (a.h.link_activation(0))(); // the remote route link
    a.h.flush();
    let t = screen_texts(&a);
    assert!(t.contains("detail 9"), "the remote link's typed route resolved in the app:\n{t}");

    // The app component the remote screen handed its `nav` to pops it.
    (a.h.shared.button_presses.borrow()[2].clone())(); // "app back"
    a.h.flush();
    let t = screen_texts(&a);
    assert!(!t.contains("detail 9") && t.contains("remote home"), "{t}");
    drop(presses);
    drop(realized);
    a.h.flush();
    a.h.forget_handlers();
    assert_eq!(runtime_vocabulary::remote::handles::held_handles(), 0);
}

// ---- navigation: a navigator DEFINED in a remote component ----

/// What a run showed after each step: every text, without node ids.
fn nav_steps(remote: bool) -> Vec<String> {
    let a = app();
    let tree = a.h.world.enter(|| if remote { ui! { spike_remoteattr::RemoteNavApp() } } else { ui! { spike_remoteattr::NativeNavApp() } });
    let realized = a.h.mount(tree);
    a.h.flush();
    let shown = |a: &App| {
        let mut texts: Vec<String> = screen_texts(a)
            .lines()
            .filter_map(|l| l.split_once(" text ").map(|(_, t)| t.to_string()))
            .collect();
        texts.sort();
        texts.join(" | ")
    };
    let mut steps = vec![shown(&a)];
    let press = |a: &App, label: &str| {
        let i = ["back", "push 7"].iter().position(|l| *l == label).unwrap();
        (a.h.shared.button_presses.borrow()[i].clone())();
        a.h.flush();
    };
    press(&a, "push 7");
    steps.push(shown(&a));
    press(&a, "back");
    steps.push(shown(&a));
    (a.h.link_activation(0))();
    a.h.flush();
    steps.push(shown(&a));
    drop(realized);
    a.h.flush();
    a.h.forget_handlers();
    steps.push(format!("held {} live {}", runtime_vocabulary::remote::handles::held_handles(), runtime_vocabulary::remote::host::live_trees()));
    steps
}

/// A stack navigator defined INSIDE a remote component — its screens,
/// typed params, layout and chrome options in the bundle, the navigator
/// machinery in the app — shows exactly what the same navigator compiled
/// into the app shows, through a typed push, a pop and a route link.
#[test]
fn a_navigator_defined_in_a_remote_component_behaves_like_the_native_one() {
    let native = nav_steps(false);
    let remote = nav_steps(true);
    assert_eq!(remote, native);
    assert!(native[0].contains("depth 1 back false title -") && native[0].contains("home"), "{native:?}");
    assert!(native[1].contains("item 7") && native[1].contains("depth 2 back true title Item"), "{native:?}");
    assert!(native[2].contains("depth 1 back false"), "{native:?}");
    assert!(native[3].contains("item 4"), "{native:?}");
    assert_eq!(native[4], "held 0 live 0");
}

fn tab_steps(remote: bool) -> Vec<String> {
    let a = app();
    let tree = a.h.world.enter(|| if remote { ui! { spike_remoteattr::RemoteTabs() } } else { ui! { spike_remoteattr::NativeTabs() } });
    let realized = a.h.mount(tree);
    a.h.flush();
    let shown = |a: &App| {
        let mut texts: Vec<String> =
            screen_texts(a).lines().filter_map(|l| l.split_once(" text ").map(|(_, t)| t.to_string())).collect();
        texts.sort();
        texts.join(" | ")
    };
    let mut steps = vec![shown(&a)];
    for i in [1, 0] {
        (a.h.shared.button_presses.borrow()[i].clone())();
        a.h.flush();
        steps.push(shown(&a));
    }
    drop(realized);
    a.h.flush();
    a.h.forget_handlers();
    steps.push(format!("held {} live {}", runtime_vocabulary::remote::handles::held_handles(), runtime_vocabulary::remote::host::live_trees()));
    steps
}

/// The same for a swap navigator: tabs selected through `SwapNav`.
#[test]
fn a_swap_navigator_defined_in_a_remote_component_behaves_like_the_native_one() {
    let native = tab_steps(false);
    let remote = tab_steps(true);
    assert_eq!(remote, native);
    assert!(native[0].contains("tab feed") && native[0].contains("feed screen"), "{native:?}");
    assert!(native[1].contains("tab profile") && native[1].contains("profile screen"), "{native:?}");
    assert!(native[2].contains("tab feed"), "{native:?}");
    assert_eq!(native[3], "held 0 live 0");
}

// ---- contexts: `#[remote_context]` ----

/// A remote component injects the app's `#[remote_context]` Theme — its
/// signal live — while unmarked context stays invisible to it.
#[test]
fn a_remote_component_injects_marked_app_context() {
    use spike_remoteattr::{Secret, Theme, Themed};
    assert!(runtime_vocabulary::remote::remote_context_names().contains(&"spike_remoteattr::Theme"));
    let a = app();
    let accent = a.h.world.enter(|| runtime_world::signal("blue".to_string()));
    let tree = a.h.world.enter(|| {
        runtime_world::provide(Theme { accent: accent.read_only(), compact: true });
        runtime_world::provide(Secret("s3cret".into()));
        ui! { Themed() }
    });
    let realized = a.h.mount(tree);
    a.h.flush();
    let t = text(&a, &realized);
    assert!(t.contains("accent blue compact true") && t.contains("secret hidden"), "{t}");
    accent.set("red".into());
    a.h.flush();
    assert!(text(&a, &realized).contains("accent red compact true"));
    drop(realized);
    a.h.flush();
    assert_eq!(accent.subscriber_count(), 0, "the export ended with the component");
}

/// Without the app providing it, `inject` finds nothing, as natively.
#[test]
fn an_unprovided_remote_context_injects_nothing() {
    let a = app();
    let realized = a.h.mount(a.h.world.enter(|| ui! { spike_remoteattr::Themed() }));
    a.h.flush();
    assert!(text(&a, &realized).contains("no theme"));
}

// ---- the SDK's own handle types as props ----

/// A remote screen whose prop is the SDK's `StackHandle` itself (here
/// `Option<StackHandle>`; not a `Ref` or `NavHandle`) pops the app's stack,
/// and hands the handle on to an app component that pops it too.
#[test]
fn an_sdk_stack_handle_crosses_both_ways() {
    use runtime_core::primitives::navigator::Route;
    use spike_remoteattr::{DetailScreen, ItemId, DETAIL};
    use stack_navigator::{StackBuilder, StackHandle, StackNavigator};
    const HOME: Route = Route::new("home", "/");
    let a = app();
    let nav = a.h.world.enter(runtime_core::Ref::<StackHandle>::new);
    let tree = a.h.world.enter(|| {
        runtime_core::IntoElement::into_element(
            StackNavigator::new(&HOME)
                .screen(HOME, |()| runtime_vocabulary::builders::text().content("home").build())
                .screen(DETAIL, move |ItemId(id)| ui! { DetailScreen(nav = nav.get(), id = id) })
                .bind(nav),
        )
    });
    let realized = a.h.mount(tree);
    a.h.flush();
    for (press, label) in [(0usize, "remote pop"), (1, "app pop")] {
        a.h.world.enter(|| nav.get().unwrap().push(&DETAIL, ItemId(5)));
        a.h.flush();
        let t = screen_texts(&a);
        assert!(t.contains("remote detail 5"), "{t}");
        let presses = a.h.shared.button_presses.borrow().clone();
        (presses[presses.len() - 2 + press])();
        a.h.flush();
        let t = screen_texts(&a);
        assert!(!t.contains("remote detail 5"), "`{label}` popped the app's stack:\n{t}");
    }
    drop(realized);
    a.h.flush();
    a.h.forget_handlers();
    assert_eq!(runtime_vocabulary::remote::handles::held_handles(), 0);
}
