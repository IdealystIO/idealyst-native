//! [`TableGroup`] — a group of a table's rows under a band row: a settings
//! screen grouped by category, a crew grouped by heading.
//!
//! ```ignore
//! let open = signal(false);
//! ui! {
//!     Table(columns = columns, on_reorder = Some(move_row), on_group_reorder = Some(move_group)) {
//!         TableRow { … header … }
//!         for g in groups {
//!             TableGroup(
//!                 id = Some(g.id.clone().into()),
//!                 title = g.name.clone(),
//!                 collapsed = Some(g.collapsed),
//!                 actions = Some(ui! { IconButton(icon = Some(PLUS), label = Some("Add".into()), on_click = add(g.id.clone())) }),
//!             ) {
//!                 for item in g.items {
//!                     TableRow(id = Some(item.id.clone().into())) { … }
//!                 }
//!             }
//!         }
//!     }
//! }
//! ```
//!
//! - **The band** is a row of its own spanning the table's columns, in the
//!   group tone (`RowTone::Group`, the header band's neutral tint): the
//!   title, a collapse toggle when the group is collapsible, and trailing
//!   `actions`. In a `scroll_x` table the title and toggle stay pinned to
//!   the scroller's left edge (past the selection and grip columns) and
//!   the actions to its right edge while the columns scroll beneath them.
//! - **Collapsing.** `collapsed = Some(signal)` adds the toggle; the app
//!   owns the signal, so a collapse survives a rebuild of the table. A
//!   collapsed group's rows keep their place but take no height — the
//!   table is not rebuilt — and a column model keeps every column still.
//! - **Accessibility.** The group is its own row group (`<tbody
//!   role="rowgroup">` on web) named by its title, and the toggle is a
//!   button announced as expanded or collapsed.
//! - **Reorder.** A group's rows reorder among themselves only: the group
//!   is their `reorder_group` (a row's own `reorder_group` still wins).
//!   With `Table(on_group_reorder = …)` the band carries a grip, and
//!   dragging it moves the band with its rows among the draggable groups.
//! - **Density.** `density` sets the band's own row padding — a band is
//!   usually denser than its rows.

use std::rc::Rc;

use runtime_core::accessibility::{AccessibilityTraits, Role};
use runtime_core::{
    component, signal, text as text_node, ui, ChildList, Element, IdealystSchema, IntoElement,
    Reactive, Signal, StyleApplication,
};

use super::rows::{BandLead, BandMeta, RowMeta};
use super::styles::TableStyles;
use super::{ColSpan, InsetSide, RowId, RowTone, TableCell, TableDensity, TableRow};
use crate::components::icon::Icon;

/// The collapse toggle's chevron size, px.
const TOGGLE_ICON_PX: f32 = 16.0;

/// Chevron down: the group is open.
const EXPANDED_MARK: runtime_core::IconData = runtime_core::IconData {
    view_box: (24, 24),
    paths: &["M6 9l6 6 6-6"],
    fill_rule: runtime_core::FillRule::NonZero,
    filled: false,
};

/// Chevron right: the group is collapsed.
const COLLAPSED_MARK: runtime_core::IconData = runtime_core::IconData {
    view_box: (24, 24),
    paths: &["M9 6l6 6-6 6"],
    fill_rule: runtime_core::FillRule::NonZero,
    filled: false,
};

/// Props for [`TableGroup`].
#[runtime_core::props]
#[derive(IdealystSchema)]
pub struct TableGroupProps {
    /// The group's rows — `TableRow`s.
    pub children: Vec<Element>,
    /// The group's identity: the `reorder_group` of its rows. `None`
    /// scopes the rows to the group all the same.
    #[prop(static)]
    pub id: Option<RowId>,
    /// The band's title, and the group's accessible name.
    pub title: String,
    /// Band content in place of the title text — an icon beside the name,
    /// a count. The title still names the group for assistive tech.
    #[prop(static)]
    pub header: Option<Element>,
    /// The band's trailing content — an add button, a menu — pinned to the
    /// scroller's right edge.
    #[prop(static)]
    pub actions: Option<Element>,
    /// Makes the group collapsible: the band gets a toggle, and while the
    /// signal is `true` the group's rows take no height. The app owns the
    /// signal, so the state survives a rebuild.
    #[prop(static)]
    pub collapsed: Option<Signal<bool>>,
    /// The band's tint. `RowTone::Group` (the default) is the header
    /// band's neutral tint.
    pub tone: RowTone,
    /// The band row's padding, over the table's `density`.
    #[prop(static)]
    pub density: Option<TableDensity>,
    /// Whether the band carries a grip in `Table(on_group_reorder = …)`.
    /// Default `true`.
    #[prop(static)]
    pub draggable: bool,
}

impl Default for TableGroupProps {
    fn default() -> Self {
        Self {
            children: Vec::new(),
            id: None,
            title: Reactive::Static(String::new()),
            header: None,
            actions: None,
            collapsed: None,
            tone: Reactive::Static(RowTone::Group),
            density: None,
            draggable: true,
        }
    }
}

/// A group of rows under a band row — see the module docs. Place it
/// among a `Table`'s rows, after the header.
#[component(children)]
pub fn TableGroup(props: TableGroupProps) -> Element {
    let mut rows: Vec<Element> = Vec::with_capacity(props.children.len() + 1);
    for c in props.children {
        ChildList::append_to(c, &mut rows);
    }
    let label = props.title.get();
    let collapsed = props.collapsed;

    // The rows: scoped to the group for reorder, and collapsible.
    for row in &rows {
        RowMeta::update(row, |m| {
            if m.group.is_none() {
                m.group = props.id.clone();
            }
        });
        if let Some(c) = collapsed {
            table::set_row_collapsed(row, Rc::new(move || c.get()));
        }
    }

    // The band's leading content pins past the columns the table adds —
    // a width only the table knows, so it fills this in.
    let band_lead = Rc::new(std::cell::RefCell::new(BandLead::default()));
    let lead_of = band_lead.clone();
    let lead_style = move || {
        let lead = lead_of.borrow();
        let app = TableStyles::band_lead().into_style_application().with_inline_merged(runtime_core::StyleRules {
            left: Some(runtime_core::Tokenized::Literal(runtime_core::Length::Px(lead.pin.get()))),
            ..Default::default()
        });
        lead.inset.get().apply(app, InsetSide::Left)
    };
    let title = props.title.clone();
    let lead = match props.header {
        Some(header) => header,
        None => band_title(title),
    };
    let toggle = collapsed.map(|c| ui! { GroupToggle(collapsed = c, title = label.clone()) });
    let trail = props.actions.map(|a| ui! { view(style = TableStyles::band_trail()) { a } });
    let band = ui! {
        TableRow(tone = props.tone, selectable = false, draggable = false, density = props.density) {
            TableCell(span = ColSpan::Rest) {
                view(style = lead_style) {
                    toggle
                    lead
                }
                view(style = TableStyles::band_spacer()) {}
                trail
            }
        }
    };
    // The band's cell hands its horizontal padding to the pinned content.
    table::visit_row_cells(&band, |cell| {
        table::map_cell_application(cell, Rc::new(|app| app.with("row_kind", "band")));
    });
    RowMeta::update(&band, |m| m.band = Some(BandMeta { draggable: props.draggable, lead: band_lead }));
    rows.insert(0, band);
    table::table_group(table::TableGroupProps { children: rows, label: Some(label) })
}

/// The band's title text.
fn band_title(title: Reactive<String>) -> Element {
    let style = TableStyles::band_title();
    match title {
        Reactive::Static(s) => text_node(s).with_style(style).into_element(),
        Reactive::Dynamic(f) => text_node(move || f()).with_style(style).into_element(),
    }
}

/// Props for [`GroupToggle`].
#[runtime_core::props]
#[derive(IdealystSchema)]
struct GroupToggleProps {
    /// The group's collapsed state.
    #[prop(static)]
    pub collapsed: Signal<bool>,
    /// The group's name, for the toggle's accessible label.
    #[prop(static)]
    pub title: String,
}

impl Default for GroupToggleProps {
    fn default() -> Self {
        Self { collapsed: signal(false), title: String::new() }
    }
}

/// The band's collapse toggle: a chevron button announced as expanded or
/// collapsed. Rebuilt when the state flips — the accessibility state is
/// part of the built button.
#[component]
fn GroupToggle(props: &GroupToggleProps) -> Element {
    let collapsed = props.collapsed;
    // One copy of the name per branch: each branch is its own closure.
    let (expand, collapse) = (props.title.clone(), props.title.clone());
    ui! {
        if collapsed.get() {
            { toggle_button(collapsed, true, &expand) }
        } else {
            { toggle_button(collapsed, false, &collapse) }
        }
    }
}

/// The toggle for one state — see [`GroupToggle`].
pub(super) fn toggle_button(collapsed: Signal<bool>, is_collapsed: bool, title: &str) -> Element {
    let (mark, verb, traits) = if is_collapsed {
        (COLLAPSED_MARK, "Expand", AccessibilityTraits::COLLAPSED)
    } else {
        (EXPANDED_MARK, "Collapse", AccessibilityTraits::EXPANDED)
    };
    let icon = ui! { Icon(data = mark, size = TOGGLE_ICON_PX, tone = Some(crate::tone::Neutral.into())) };
    runtime_core::pressable(vec![icon], move || collapsed.update(|c| !*c))
        .with_style(TableStyles::group_toggle().into_style_application())
        .a11y_role(Role::Button)
        .a11y_traits(traits)
        .a11y_label(format!("{verb} {title}"))
        .into_element()
}

/// The band row's group grip and hover-revealed grip glyph style.
pub(super) fn grip_style(revealed: Option<Rc<dyn Fn() -> bool>>) -> impl Fn() -> StyleApplication {
    move || {
        let app = TableStyles::grip().into_style_application();
        match &revealed {
            Some(f) => app.with("shown", if f() { "on" } else { "off" }),
            None => app,
        }
    }
}
