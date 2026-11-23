//! Compile-checked usage **recipes** for the keep-awake SDK. `recipe!`
//! self-gates on the `catalog` feature: with it off these expand to nothing.

use runtime_core::recipe;

recipe!(
    KeepAwake,
    /// Keep the screen on while a shift is open and this screen is mounted.
    /// [`use_keep_screen_on`](crate::use_keep_screen_on) releases when the
    /// condition turns false or the component goes.
    pub fn screen_on_while_open() -> ::runtime_core::Element {
        use crate::use_keep_screen_on;
        use ::runtime_core::{signal, text, ui};

        let open = signal(true);
        use_keep_screen_on(open);

        ui! {
            view {
                text(move || if open.get() { "Shift open: the screen stays on" } else { "Shift closed" }.to_string())
                button(label = "Toggle shift", on_click = move || open.set(!open.peek()))
            }
        }
    }
);
