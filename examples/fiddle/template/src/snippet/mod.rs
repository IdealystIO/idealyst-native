//! Auto-generated per /compile request — overwritten on every build.

#![allow(unused_imports)]
#![allow(dead_code)]

use crate::__rt::*;

// Entry file. The fiddle server renames this to `mod.rs` under
// the template's `snippet/` directory, so siblings declared with
// `mod foo;` resolve to `widgets.rs`, `helpers/<name>.rs`, etc.
mod widgets;

pub fn app() -> Primitive {
    let count: Signal<i32> = signal!(0_i32);
    let label = move || format!("Tapped {} times", count.get());

    ui! {
        Stack(padding = StackPadding::Lg, gap = StackGap::Md) {
            widgets::title("Hello, fiddle!")
            Text { label }
            Button(
                label = "Tap me",
                on_click = move || count.set(count.get() + 1),
            )
        }
    }
}

