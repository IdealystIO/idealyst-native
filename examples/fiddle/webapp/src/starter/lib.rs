// Entry file. The fiddle server renames this to `mod.rs` under
// the template's `snippet/` directory, so siblings declared with
// `mod foo;` resolve to `widgets.rs`, `helpers/<name>.rs`, etc.
mod widgets;

pub fn app() -> Primitive {
    let count: Signal<i32> = signal!(0_i32);

    ui! {
        Stack(padding = StackPadding::Lg, gap = StackGap::Md) {
            widgets::title("Hello, fiddle!")
            Text { text_fmt!("Tapped {} times", bind!(count)) }
            Button(
                label = "Tap me",
                on_click = move || count.set(count.get() + 1),
            )
        }
    }
}
