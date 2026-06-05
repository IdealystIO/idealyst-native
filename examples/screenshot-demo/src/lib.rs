//! `screenshot-demo` — exercises the native screen-capture debug
//! utility.
//!
//! The app renders one distinctive, *live* screen: a title, three
//! coloured badges, and a counter you can change with +/- buttons. The
//! point is to capture it via the Robot bridge's `screenshot` verb and
//! confirm the returned PNG shows the **real rendered native surface** —
//! including the current counter value, which proves the capture
//! reflects live state rather than a static re-render.
//!
//! ## How to capture
//!
//! Run the app under `idealyst dev` (which enables the `dev` feature, so
//! `mount()` auto-starts the Robot bridge and registers the native
//! `screenshot` verb):
//!
//! ```text
//! idealyst dev --macos --local        # or --ios / --android
//! ```
//!
//! Then capture with the bundled helper (auto-discovers the running
//! app's bridge port):
//!
//! ```text
//! python3 examples/screenshot-demo/capture.py
//! ```
//!
//! See `README.md` for the wire protocol and an MCP alternative.

use idea_ui::{
    install_idea_theme, light_theme, tone, typography_kind, variant, Badge, Button, Stack, StackGap,
    StackPadding, Typography,
};
use runtime_core::{component, rx, signal, ui, Element, Signal};

// ---------------------------------------------------------------------------
// Per-target registration hooks. Navigators/externals self-register at
// backend construction, so the host hook is a no-op; the sidecar hook is
// present for parity with the other examples (gated by `sidecar`, which
// only the generated runtime-server wrapper sets).
// ---------------------------------------------------------------------------

pub fn register_extensions<B: runtime_core::Backend>(_backend: &mut B) {}

#[cfg(feature = "sidecar")]
pub fn register_extensions_recorder(_backend: &mut dev_server::WireRecordingBackend) {}

// ---------------------------------------------------------------------------
// App entry — a single live screen.
// ---------------------------------------------------------------------------

#[component]
pub fn app() -> Element {
    install_idea_theme(light_theme());

    // State lives in the component (root) scope; the tree is built by a
    // plain helper. Keeping `text(closure)` out of the `#[component]` body
    // — same structure as `stack-demo` — avoids the macro's body-closure
    // rewriting colliding with the reactive `text` source.
    let count: Signal<i32> = signal!(0);
    screen(count)
}

fn screen(count: Signal<i32>) -> Element {
    // idea-ui `Button.on_click` is `Rc<dyn Fn()>`; bind with the explicit
    // type so the macro's `.into()` is the identity conversion.
    let increment: std::rc::Rc<dyn Fn()> = std::rc::Rc::new(move || count.update(|n| *n += 1));
    let decrement: std::rc::Rc<dyn Fn()> = std::rc::Rc::new(move || count.update(|n| *n -= 1));

    // Three coloured badges make the capture visually distinctive and
    // exercise themed colour rendering across backends.
    let badges: Vec<Element> = vec![
        ui! { Badge(label = "Native".to_string(), tone = tone::Success, variant = variant::Soft) },
        ui! { Badge(label = "Capture".to_string(), tone = tone::Info, variant = variant::Soft) },
        ui! { Badge(label = "Debug".to_string(), tone = tone::Warning, variant = variant::Soft) },
    ];

    // Children are assembled into a `Vec<Element>` first to keep the `ui!`
    // body unambiguous: a `Typography(...)` immediately followed by a
    // `{ ... }` brace-block in the same scope would be parsed as
    // `Typography(...) { children }` and error (Typography has no
    // children field). Same reasoning as `stack-demo`.
    //
    // `rx!(...)` makes the counter Typography *live*: it re-resolves when
    // `count` changes, so a screenshot taken after pressing + shows the
    // new value — the proof that the capture is of the live surface.
    let children: Vec<Element> = vec![
        ui! { Typography(content = "Screenshot Demo".to_string(), kind = typography_kind::H1) },
        ui! {
            Typography(
                content = "Capture this screen with the Robot bridge `screenshot` verb. The PNG is the real native surface — change the counter, capture again, and the value updates.".to_string(),
                muted = true,
            )
        },
        ui! { Stack(gap = StackGap::Sm, padding = StackPadding::None) { badges } },
        ui! { Typography(content = "Live state".to_string(), kind = typography_kind::H2) },
        ui! { Typography(content = rx!(format!("Counter: {}", count.get())), kind = typography_kind::H1) },
        ui! { Button(label = "+ increment".to_string(), on_click = increment, tone = tone::Primary, variant = variant::Filled) },
        ui! { Button(label = "- decrement".to_string(), on_click = decrement, tone = tone::Neutral, variant = variant::Soft) },
    ];

    ui! {
        Stack(gap = StackGap::Lg, padding = StackPadding::Lg) { children }
    }
}
