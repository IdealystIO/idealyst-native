use runtime_core::{
    stylesheet, ui, AlignItems, Color, Element, FlexDirection, FlexWrap, JustifyContent,
};

// Every view is a flex container, and its children stack in a COLUMN
// unless you say otherwise.
stylesheet! {
    pub Screen<()> {
        base(_t) { gap: 12.0, padding: 16.0, background: Color("#f3f4f6".into()) }
    }
}

// A row: children side by side, pushed to opposite ends, centered vertically.
stylesheet! {
    pub Header<()> {
        base(_t) {
            flex_direction: FlexDirection::Row,
            justify_content: JustifyContent::SpaceBetween,
            align_items: AlignItems::Center,
            padding: 12.0,
            border_radius: 8.0,
            background: Color("#1f2328".into()),
        }
    }
}

// Text takes its color from its own style, not from the view around it.
stylesheet! {
    pub HeaderText<()> {
        base(_t) { color: Color("#ffffff".into()) }
    }
}

// A row that wraps: tiles flow onto the next line when they run out of room.
stylesheet! {
    pub Grid<()> {
        base(_t) { flex_direction: FlexDirection::Row, flex_wrap: FlexWrap::Wrap, gap: 8.0 }
    }
}

// Each tile starts 120px wide and grows to share the space left over.
stylesheet! {
    pub Tile<()> {
        base(_t) {
            flex_grow: 1.0,
            flex_basis: 120.0,
            height: 64.0,
            align_items: AlignItems::Center,
            justify_content: JustifyContent::Center,
            border_radius: 8.0,
            background: Color("#dbe4ff".into()),
        }
    }
}

pub fn app() -> Element {
    ui! {
        view(style = Screen()) {
            view(style = Header()) {
                text(style = HeaderText()) { "Dashboard" }
                text(style = HeaderText()) { "Settings" }
            }
            view(style = Grid()) {
                view(style = Tile()) { text { "One" } }
                view(style = Tile()) { text { "Two" } }
                view(style = Tile()) { text { "Three" } }
                view(style = Tile()) { text { "Four" } }
                view(style = Tile()) { text { "Five" } }
            }
        }
    }
}
