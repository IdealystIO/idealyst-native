use runtime_core::{stylesheet, view, Element, FlexDirection, IntoElement};

stylesheet! {
    pub Card<()> {
        base(_t) {                            // narrow: stacked
            flex_direction: FlexDirection::Column,
            padding: 12.0,
        }
        container (min_width: 400px)(_t) {    // container >= 400 dp: side-by-side
            flex_direction: FlexDirection::Row,
            padding: 20.0,
        }
    }
}

// Mark the box the card measures itself against. `.container()` is a
// builder modifier on the view, so the wrapper is built rather than
// declared as a ui! attribute.
fn framed(card: Element) -> Element {
    view(vec![card]).container().into_element()
}
