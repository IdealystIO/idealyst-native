//! [`FieldGrid`] — fields in columns on a wide screen, one column on a
//! phone (rule `field-grid`).

use runtime_core::{
    component, stylesheet, ui, ChildList, DisplayKind, Element, FlexDirection, GridPlacement, IdealystSchema,
    Length, TrackSize,
};

#[runtime_core::props]
#[derive(IdealystSchema)]
pub struct FieldGridProps {
    /// Columns from `Sm` up: 1, 2 or 3. A phone always has one.
    #[prop(static)]
    pub columns: usize,
    /// The fields, in reading order at every width. A field that spans
    /// the row is wrapped in a [`FieldGridRow`].
    pub children: Vec<Element>,
}

impl Default for FieldGridProps {
    fn default() -> Self {
        Self { columns: 2, children: Vec::new() }
    }
}

/// Fields in `columns` columns on a tablet or desktop and in one on a
/// phone, `md` apart both ways. Reading order is source order at every
/// width, so a form reads the same on both.
///
/// ```ignore
/// FieldGrid(columns = 2_usize) {
///     Field(label = Some("First name".to_string()), …)
///     Field(label = Some("Last name".to_string()), …)
///     FieldGridRow { Textarea(label = Some("Notes".to_string()), …) }
/// }
/// ```
#[component(children)]
pub fn FieldGrid(props: FieldGridProps) -> Element {
    // The `#[component(children)]` slot's flatten, not a staged list.
    let mut fields = Vec::with_capacity(props.children.len());
    for c in props.children {
        ChildList::append_to(c, &mut fields);
    }
    match props.columns {
        0 | 1 => ui! { view(style = FieldGridOne()) { fields } },
        2 => ui! { view(style = FieldGridTwo()) { fields } },
        _ => ui! { view(style = FieldGridThree()) { fields } },
    }
}

// One sheet per column count rather than a variant: a variant arm cannot
// be scoped to a breakpoint, and the phone is one column whatever the
// count.
stylesheet! {
    FieldGridOne<idea_ui::IdeaThemeRef> {
        base(t) {
            display: DisplayKind::Grid,
            grid_template_columns: vec![TrackSize::Fr(1.0)],
            gap: t.spacing.md(),
            width: Length::pct(100.0),
        }
    }
}

stylesheet! {
    FieldGridTwo<idea_ui::IdeaThemeRef> {
        base(t) {
            display: DisplayKind::Grid,
            grid_template_columns: vec![TrackSize::Fr(1.0)],
            gap: t.spacing.md(),
            width: Length::pct(100.0),
        }
        breakpoint sm(_t) {
            grid_template_columns: vec![TrackSize::Fr(1.0), TrackSize::Fr(1.0)],
        }
    }
}

stylesheet! {
    FieldGridThree<idea_ui::IdeaThemeRef> {
        base(t) {
            display: DisplayKind::Grid,
            grid_template_columns: vec![TrackSize::Fr(1.0)],
            gap: t.spacing.md(),
            width: Length::pct(100.0),
        }
        breakpoint sm(_t) {
            grid_template_columns: vec![TrackSize::Fr(1.0), TrackSize::Fr(1.0), TrackSize::Fr(1.0)],
        }
    }
}

#[runtime_core::props]
#[derive(IdealystSchema, Default)]
pub struct FieldGridRowProps {
    /// The field.
    pub children: Vec<Element>,
}

/// A child of a [`FieldGrid`] that spans every column: a notes box, an
/// address line.
///
/// ```ignore
/// FieldGrid(columns = 2_usize) {
///     Field(…)
///     FieldGridRow { Textarea(label = Some("Notes".to_string()), …) }
/// }
/// ```
#[component(children)]
pub fn FieldGridRow(props: FieldGridRowProps) -> Element {
    // The `#[component(children)]` slot's flatten, not a staged list.
    let mut field = Vec::with_capacity(props.children.len());
    for c in props.children {
        ChildList::append_to(c, &mut field);
    }
    ui! { view(style = FieldGridSpan()) { field } }
}

stylesheet! {
    FieldGridSpan<idea_ui::IdeaThemeRef> {
        base(_t) {
            flex_direction: FlexDirection::Column,
            grid_column: GridPlacement::SPAN_ALL,
        }
    }
}
