//! On-device regression app for the Android `text` primitive; `run.sh`
//! drives it.
//!
//! - "Run" calls `LabelParity.run` (`kotlin/…/LabelParity.kt`), which
//!   builds each case as the `TextView` the backend used to make and as the
//!   `RustLabel` it makes now, and checks they measure to the same size and
//!   draw the same pixels. Bug it guards: a label that drew its own text
//!   could drift from `TextView` (a dip size span drawn at px, a line limit
//!   measuring every line), and nothing on the host can see it.
//! - The result is a `text` that changes on every run (`runs: N, failed:
//!   M`), so `run.sh` also sees a reactive text update reach the screen,
//!   and reads it through accessibility, which is where TalkBack and
//!   `uiautomator` find a label's text.
//! - "Plain label" is a static `text`, which must still report the class
//!   name `android.widget.TextView` to accessibility, and shows no
//!   selection menu when long-pressed.
//! - "Selectable label" is `text(selectable = true)`: a long-press must
//!   open Android's text selection (its Copy action). Bug it guards: no
//!   text on Android could be selected.
//! - "Styled selectable" is selectable styled runs with a dotted underline
//!   (`RustUnderlineSpan` on the selectable `TextView`).

use runtime_core::{signal, styled_text, ui, Element, TextRun, TextRunStyle};

fn log(msg: String) {
    runtime_core::logging::log(runtime_core::logging::LogLevel::Info, &format!("[label-parity] {msg}"));
}

/// `LabelParity.run` with the Activity `ndk_context` publishes; the number
/// of failed checks.
fn run_parity() -> i32 {
    let ctx = ndk_context::android_context();
    let vm = unsafe { jni::JavaVM::from_raw(ctx.vm().cast()) }.expect("JavaVM");
    let mut env = vm.attach_current_thread().expect("attach");
    let activity = unsafe { jni::objects::JObject::from_raw(ctx.context().cast()) };
    let class = env.find_class("io/idealyst/labelparity/LabelParity").expect("LabelParity");
    let failed = env
        .call_static_method(&class, "run", "(Landroid/content/Context;)I", &[(&activity).into()])
        .and_then(|v| v.i());
    log(format!("run: {failed:?}"));
    failed.unwrap_or(-1)
}

/// "Styled selectable": a bold run and a dotted underline.
fn styled_runs() -> Vec<TextRun> {
    use runtime_shared::styled_text::{RunUnderline, UnderlineStyle};
    let mut bold = TextRunStyle::default();
    bold.font_weight = Some(runtime_core::FontWeight::Bold);
    let mut dotted = TextRunStyle::default();
    dotted.underline = Some(RunUnderline { style: UnderlineStyle::Dotted, color: None });
    vec![
        TextRun::styled("Styled", bold),
        TextRun::plain(" "),
        TextRun::styled("selectable", dotted),
    ]
}

pub fn app() -> Element {
    let runs = signal(0u32);
    let failed = signal(0i32);
    ui! {
        view {
            text { "Plain label" }
            text(selectable = true) { "Selectable label" }
            styled_text(styled_runs()).selectable(true)
            button(label = "Run".to_string(), on_click = move || {
                failed.set(run_parity());
                runs.set(runs.get() + 1);
            })
            text { "runs: {runs}, failed: {failed}" }
        }
    }
}

pub fn register_scene_extensions<H: runtime_scene::Host>(_registry: &mut runtime_scene::Registry<H>) {}

pub fn scene_app() -> Element {
    app()
}
