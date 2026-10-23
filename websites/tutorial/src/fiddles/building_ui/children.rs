use runtime_core::{component, stylesheet, ui, Color, Element};

// A border so each card's edge is visible. Stylesheets get their own
// track later; for now, read this as "draw a box".
stylesheet! {
    pub CardBox<()> {
        base(_t) {
            padding: 12.0,
            gap: 6.0,
            border_width: 1.0,
            border_color: Color("#c8ccd4".into()),
            border_radius: 8.0,
        }
    }
}

// A parameter named `children` receives whatever the caller writes in the
// braces. Write `children` where they should appear.
#[component]
fn Card(title: String, children: Vec<Element>) -> Element {
    ui! {
        view(style = CardBox()) {
            text { "{title}" }
            children
        }
    }
}

pub fn app() -> Element {
    ui! {
        view {
            Card(title = "Plain text") {
                text { "Anything can go inside a card." }
            }
            Card(title = "Several children") {
                text { "One," }
                text { "two," }
                button(label = "three", on_click = || {})
            }
            // Components nest like primitives do.
            Card(title = "A card in a card") {
                Card(title = "Inner") {
                    text { "Composition all the way down." }
                }
            }
        }
    }
}
