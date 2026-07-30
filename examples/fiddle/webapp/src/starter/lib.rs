// Entry file. The fiddle server renames this to `mod.rs` under
// the template's `snippet/` directory, so siblings declared with
// `mod foo;` resolve to `widgets.rs`, `helpers/<name>.rs`, etc.
//
// `#[component]` generates a `Title` tag alias next to the component fn, so
// importing that one name is all a `ui!` call site needs.
mod widgets;
use widgets::Title;

#[component]
pub fn app() -> Element {
    let count: Signal<i32> = signal(0_i32);

    ui! {
        Stack(padding = StackPadding::Lg, gap = StackGap::Md) {
            Title(label = "Hello, fiddle!".to_string())
            text { "Tapped {count} times" }
            button(
                label = "Tap me",
                // One `set` per turn composes fine. Two increments in one
                // handler need `count.update(|n| n + 1)`: writes stage until
                // the flush, so a second `get()` would still read the old
                // value.
                on_click = move || count.set(count.get() + 1),
            )
        }
    }
}
