// Entry file. The fiddle server renames this to `mod.rs` under
// the template's `snippet/` directory, so siblings declared with
// `mod foo;` resolve to `widgets.rs`, `helpers/<name>.rs`, etc.
//
// `#[macro_use]` lifts the `#[component]`-generated invocation
// macro (e.g. `title!`) from `widgets.rs` into this module so
// the `ui!` DSL below can spell `Title(label = ...)`. The
// macro expands to `title(&TitleProps { ... })`, so the matching
// `use` line below brings BOTH the function and the Props type
// into the call-site scope where the expansion lands.
#[macro_use]
mod widgets;
use widgets::{title, TitleProps};

#[component]
pub fn app() -> Element {
    let count: Signal<i32> = signal!(0_i32);

    ui! {
        Stack(padding = StackPadding::Lg, gap = StackGap::Md) {
            Title(label = "Hello, fiddle!".to_string())
            text { text_fmt!("Tapped {} times", bind!(count)) }
            button(
                label = "Tap me",
                on_click = move || count.set(count.get() + 1),
            )
        }
    }
}
