//! On-device regression app for the Android host lifecycle; `run.sh`
//! drives it. Every observable is a logcat line tagged `[lifecycle]` or a
//! text on screen.
//!
//! - The root is a stack navigator whose only screen locks Back
//!   (`back_enabled(false)`). Bug: `RustStackPresenter` left its Back
//!   callback off at the root, so Back finished the Activity.
//! - The screen starts a timer and an HTTPS request through `spawn_then`
//!   and shows both results, so a relaunch in the same process shows
//!   whether async work still completes.
//! - "Finish" calls `Activity.finish()` on the Activity `ndk_context`
//!   publishes. Bug: the host published only the FIRST Activity, so after a
//!   relaunch every SDK (permissions, device-admin, camera …) called into a
//!   destroyed one; this finish then did nothing.
//! - A 5 s heartbeat logs `tick N` for as long as the process runs, and a
//!   device owner calls `device_admin::start_keep_alive`, so `run.sh
//!   --keep-alive` can check a backgrounded process is not frozen.

use runtime_core::{component, signal, spawn_then, ui, Element, Route, Screen};
use stack_navigator::{StackBuilder, StackNavigator, StackScreenExt};

const HOME: Route<()> = Route::<()>::new("home", "/");

fn log(msg: String) {
    runtime_core::logging::log(runtime_core::logging::LogLevel::Info, &format!("[lifecycle] {msg}"));
}

async fn sleep_ms(ms: i32) {
    let (tx, rx) = futures_channel::oneshot::channel::<()>();
    // The handle cancels the timer on drop: hold it across the await.
    let _task = runtime_shared::scheduling::after_ms(ms, move || {
        let _ = tx.send(());
    });
    let _ = rx.await;
}

/// `Activity.finish()` on the Activity `ndk_context` publishes.
fn finish_published_activity() {
    let ctx = ndk_context::android_context();
    let vm = unsafe { jni::JavaVM::from_raw(ctx.vm().cast()) }.expect("JavaVM");
    let mut env = vm.attach_current_thread().expect("attach");
    let activity = unsafe { jni::objects::JObject::from_raw(ctx.context().cast()) };
    let r = env.call_method(&activity, "finish", "()V", &[]);
    log(format!("finish: {r:?}"));
}

#[component]
fn Home() -> Element {
    let timer = signal("timer: pending".to_string());
    let http = signal("http: pending".to_string());
    spawn_then(
        async {
            sleep_ms(300).await;
        },
        move |()| timer.set("timer: ok".into()),
    );
    spawn_then(
        async { net::Client::new().get("https://example.com/").send().await.map(|r| r.status()) },
        move |r| http.set(format!("http: {r:?}")),
    );
    ui! {
        view {
            text { move || timer.get() }
            text { move || http.get() }
            button(label = "Finish".to_string(), on_click = move || finish_published_activity())
        }
    }
}

#[component]
pub fn app() -> Element {
    log("attached".into());
    if device_admin::is_device_owner() == Ok(true) {
        let r = device_admin::start_keep_alive("Lifecycle probe", "keeping the heartbeat alive");
        log(format!("start_keep_alive: {r:?}"));
    }
    runtime_shared::driver::spawn_async(async {
        let mut n = 0u32;
        loop {
            sleep_ms(5_000).await;
            n += 1;
            log(format!("tick {n}"));
        }
    });
    let nav = StackNavigator::new(&HOME).screen(HOME, |_| Screen::new(ui! { Home() }).back_enabled(false));
    ui! { nav }
}

pub fn register_scene_extensions<H: runtime_scene::Host>(_registry: &mut runtime_scene::Registry<H>) {}

pub fn scene_app() -> Element {
    app()
}
