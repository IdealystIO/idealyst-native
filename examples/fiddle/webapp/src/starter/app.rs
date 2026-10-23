// The entry file: it must define `pub fn app() -> Element`. Any other
// file in the project is a module of this one — `mod widgets;` below
// is `widgets.rs`.
use idea_ui::{install_idea_theme, light_theme, Stack, StackGap, StackPadding};
use runtime_core::{signal, ui, Element};

mod widgets;
use widgets::Title;

pub fn app() -> Element {
    // idea-ui components read their colors and spacing from a theme.
    install_idea_theme(light_theme());
    let count = signal(0);

    ui! {
        Stack(padding = StackPadding::Lg, gap = StackGap::Md) {
            Title(label = "Hello, fiddle!")
            text { "Tapped {count} times" }
            button(label = "Tap me", on_click = move || count.update(|n| n + 1))
        }
    }
}
