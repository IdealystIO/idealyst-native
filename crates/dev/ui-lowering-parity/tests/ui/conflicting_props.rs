// `content` AND a body both give the text: the body used to win and
// `content` was dropped without a word.
use runtime_macros::ui;
use runtime_vocabulary::glue::Element;

fn main() {
    let _el: Element = ui! {
        text(content = "from the prop") { "from the body" }
    };
}
