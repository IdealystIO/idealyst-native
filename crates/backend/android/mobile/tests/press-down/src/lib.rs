//! On-device regression app for the Android press-down `pressable`
//! (`PressActivation::Down`); `run.sh` drives it.
//!
//! - "Menu" is a `pressable(...).activation(PressActivation::Down)`. It sits
//!   inside "Outer", a release-mode pressable, inside a `scroll_view` tall
//!   enough to scroll. `run.sh` injects the down and the up as separate
//!   `MotionEvent`s and reads `presses: N, outer: M` between them: the
//!   press must count at the down, nothing more at the up, and "Outer" must
//!   never see the press. A swipe that starts on "Menu" must count once and
//!   not scroll the page.
//! - "Click handle" calls `PressableHandle::click` on "Menu" through its
//!   `Ref` (Android `performClick`): one press.
//! - "Run" calls `PressDownProbe.run` (`kotlin/…/PressDownProbe.kt`), which
//!   drives a bare `RustPressDownLayout` with synthesized `MotionEvent`s,
//!   key events and accessibility actions — the cases `adb input` cannot
//!   inject (a secondary mouse button, TalkBack's ACTION_CLICK) — and logs
//!   `[press-down]` lines.
//!
//! Bug it guards: a press-down pressable that fired on the release (or on
//! both the down and the release), let the press through to an ancestor, or
//! lost keyboard / accessibility / programmatic activation.

use runtime_core::{pressable, signal, ui, Element, PressActivation, PressableHandle, Ref};

fn log(msg: String) {
    runtime_core::logging::log(runtime_core::logging::LogLevel::Info, &format!("[press-down] {msg}"));
}

/// `PressDownProbe.run` with the Activity `ndk_context` publishes; the
/// number of failed checks.
fn run_probe() -> i32 {
    let ctx = ndk_context::android_context();
    let vm = unsafe { jni::JavaVM::from_raw(ctx.vm().cast()) }.expect("JavaVM");
    let mut env = vm.attach_current_thread().expect("attach");
    let activity = unsafe { jni::objects::JObject::from_raw(ctx.context().cast()) };
    let class = env.find_class("io/idealyst/pressdown/PressDownProbe").expect("PressDownProbe");
    let failed = env
        .call_static_method(&class, "run", "(Landroid/content/Context;)I", &[(&activity).into()])
        .and_then(|v| v.i());
    log(format!("run: {failed:?}"));
    failed.unwrap_or(-1)
}

pub fn app() -> Element {
    let presses = signal(0u32);
    let outer = signal(0u32);
    let failed = signal(-1i32);
    let menu: Ref<PressableHandle> = Ref::new();
    ui! {
        scroll_view {
            text { "presses: {presses}, outer: {outer}, failed: {failed}" }
            button(label = "Run".to_string(), on_click = move || failed.set(run_probe()))
            button(label = "Click handle".to_string(), on_click = move || {
                menu.with(|h| h.click());
            })
            pressable(
                vec![ui! {
                    view {
                        text { "Outer" }
                        pressable(vec![ui! { text { "Menu" } }], move || presses.set(presses.get() + 1))
                            .activation(PressActivation::Down)
                            .bind(menu)
                    }
                }],
                move || outer.set(outer.get() + 1),
            )
            for i in 0..60 {
                text { "filler {i}" }
            }
        }
    }
}

pub fn register_scene_extensions<H: runtime_scene::Host>(_registry: &mut runtime_scene::Registry<H>) {}

pub fn scene_app() -> Element {
    app()
}
