//! Interactive smoke test: exercises the WM_COMMAND routing path
//! (button click → signal write → reactive text update) plus the
//! native Toggle (checkbox) and TextInput (EDIT) primitives.
//!
//! ```text
//! cargo run -p host-win32 --example smoke_interactive
//! ```
//!
//! Clicking "Increment" bumps the counter; toggling the checkbox flips
//! the ON/OFF text; typing in the field echoes into the greeting —
//! all through the real reactive pipeline, proving control
//! notifications route from a control nested inside an IdealystView up
//! to the host and back into the framework.

use runtime_core::primitives::activity_indicator::activity_indicator;
use runtime_core::primitives::slider::slider;
use runtime_core::{button, signal, text, text_input, toggle, view, Element};

fn app() -> Element {
    let count = signal(0i32);
    let flag = signal(false);
    let name = signal(String::new());
    let level = signal(0.5f32);

    view(vec![
        text(move || format!("Count: {}", count.get())).into(),
        button("Increment", move || count.set(count.get() + 1)).into(),
        toggle(flag, move |v| flag.set(v)).into(),
        text(move || format!("Toggle is {}", if flag.get() { "ON" } else { "OFF" })).into(),
        text_input(name, move |s| name.set(s)).into(),
        text(move || format!("Hello, {}", name.get())).into(),
        text(move || format!("Level: {:.2}", level.get())).into(),
        slider(level, move |v| level.set(v)).into(),
        activity_indicator().into(),
    ])
    .into()
}

fn main() {
    let opts = host_win32::RunOptions {
        title: "Idealyst — Win32 interactive smoke".to_string(),
        width: 640,
        height: 480,
    };
    std::process::exit(host_win32::run(opts, app));
}
