// `image` has no `on_click`; it used to compile and do nothing.
use runtime_macros::ui;
use runtime_vocabulary::glue::Element;

fn main() {
    let _el: Element = ui! {
        image(src = "a.png", on_click = || {})
    };
}
