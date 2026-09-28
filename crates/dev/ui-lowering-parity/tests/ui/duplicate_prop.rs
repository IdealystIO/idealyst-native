// A prop written twice: the second one used to be dropped.
use runtime_macros::ui;
use runtime_vocabulary::glue::Element;

fn main() {
    let _el: Element = ui! {
        view(on_hover = |_h: bool| {}, on_hover = |_h: bool| {}) {}
    };
}
