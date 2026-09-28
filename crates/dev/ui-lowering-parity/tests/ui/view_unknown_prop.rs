// `on_tuch` is a typo for `on_touch`: it used to compile and install
// nothing.
use runtime_macros::ui;
use runtime_vocabulary::glue::Element;

fn main() {
    let _el: Element = ui! {
        view(on_tuch = |_e| ()) {}
    };
}
