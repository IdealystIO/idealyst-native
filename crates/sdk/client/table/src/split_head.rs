//! The split head of a [`TableProps::scroll_y`](super::TableProps) table:
//! the header rows ([`table_head`](super::table_head)) lay out in a strip
//! of their own ABOVE the rows' scroller, so the vertical scrollbar runs
//! beside the body only, under the header.
//!
//! ```text
//! surface (author style)
//! ├─ header slot
//! ├─ frame (role="table" on web)
//! │  ├─ head strip — horizontal scroll_view, no scrollbar, as wide as the
//! │  │  │            body's viewport
//! │  │  └─ head content (as wide as the body's grid) > head grid
//! │  └─ body scroller (vertical, or both axes with scroll_x)
//! │     ├─ probe (absolute, zero height: the viewport's width)
//! │     └─ body grid
//! └─ footer slot
//! ```
//!
//! Two grids cannot share column tracks, so the head FOLLOWS the body:
//!
//! - **Column widths.** Each grid gets a zero-height SIZER row first — one
//!   empty cell per column, hidden from assistive tech — whose cells'
//!   widths are that grid's column widths. The head grid starts at its
//!   natural widths; once both are measured, the body sizer's cells take
//!   the head's natural widths as `min_width` floors (so a header label
//!   never ends up wider than its column), and the head grid's tracks
//!   become the body's measured widths, exactly. From then on the head
//!   follows every change of the body (a resize, a fill column, a
//!   resize drag); the floors are the head's widths as first laid out.
//! - **Width.** The head content is as wide as the body grid, and the head
//!   strip as wide as the body scroller's VIEWPORT (the probe) — on web a
//!   classic vertical scrollbar takes width from the viewport, and the
//!   head must end where the body's visible columns end, or a column
//!   frozen to the right would pin at a different x in the head.
//! - **Horizontal scroll.** The strip is a scroller too, kept at the
//!   body's horizontal offset (and the body at the strip's, when the
//!   strip is scrolled — a trackpad over the header). Frozen columns pin
//!   against each scroller the way they always do, so a frozen head cell
//!   lines up with its column on every backend.
//!
//! The head's floors are taken once, so a header whose content grows
//! after the table is laid out keeps its first width as the column's
//! floor; a table rebuilt for new data measures again.

use super::*;
use std::cell::{Cell, RefCell};

use runtime_shared::primitives::scroll_view::ScrollViewHandle;
use runtime_shared::{LayoutSubscription, Ref, ViewHandle};

/// The measurements and offsets that keep a split head in step with its
/// body. Ownership mirrors [`EdgeWatch`]: the layout callbacks hold
/// `Weak`s, and the building scope's drop disarms the subscriptions so a
/// late layout delivery never writes a dropped signal.
pub(super) struct HeadSync {
    /// The head's natural column widths, captured until the head is
    /// forced to the body's.
    head: RefCell<Vec<Option<f32>>>,
    /// The body's column widths, as last measured.
    body: RefCell<Vec<Option<f32>>>,
    /// The body sizer's `min_width` per column — the head's natural
    /// widths, once known.
    floors: glue::Signal<Option<Vec<f32>>>,
    /// The head grid's tracks — the body's widths, once known.
    forced: glue::Signal<Option<Vec<f32>>>,
    /// The body grid's width — the head content's.
    body_width: glue::Signal<Option<f32>>,
    /// The body scroller's viewport width — the head strip's.
    viewport: glue::Signal<Option<f32>>,
    head_ref: Ref<ScrollViewHandle>,
    body_ref: Ref<ScrollViewHandle>,
    body_x: Cell<f32>,
    body_y: Cell<f32>,
    head_x: Cell<f32>,
    /// Whether the body floors / the head tracks are set — kept here
    /// rather than read back from the signals, whose writes only stage
    /// until the turn's flush.
    floored: Cell<bool>,
    forced_on: Cell<bool>,
    armed: Cell<bool>,
    subs: RefCell<Vec<LayoutSubscription>>,
}

/// Slack under which two horizontal offsets are the same — a programmatic
/// scroll lands on a whole device pixel.
const OFFSET_SLACK: f32 = 0.5;

impl HeadSync {
    pub(super) fn new(columns: usize) -> Rc<Self> {
        let sync = Rc::new(Self {
            head: RefCell::new(vec![None; columns]),
            body: RefCell::new(vec![None; columns]),
            floors: glue::signal(None),
            forced: glue::signal(None),
            body_width: glue::signal(None),
            viewport: glue::signal(None),
            head_ref: Ref::new(),
            body_ref: Ref::new(),
            body_x: Cell::new(0.0),
            body_y: Cell::new(0.0),
            head_x: Cell::new(0.0),
            floored: Cell::new(false),
            forced_on: Cell::new(false),
            armed: Cell::new(true),
            subs: RefCell::default(),
        });
        let on_drop = sync.clone();
        glue::on_scope_drop(move || on_drop.disarm());
        sync
    }

    fn disarm(&self) {
        self.armed.set(false);
        self.subs.borrow_mut().clear();
    }

    /// Track `handle`'s laid-out width into `record`.
    fn watch(self: &Rc<Self>, handle: ViewHandle, record: Rc<dyn Fn(&HeadSync, f32)>) {
        if !self.armed.get() {
            return;
        }
        if let Some(frame) = handle.frame().filter(|f| f.width > 0.0) {
            record(self, frame.width);
        }
        let weak = Rc::downgrade(self);
        let sub = handle.on_layout(move |w, _h| {
            if let Some(sync) = weak.upgrade().filter(|s| s.armed.get()) {
                record(&sync, w);
            }
        });
        self.subs.borrow_mut().push(sub);
    }

    /// A binder that watches one cell of the head's sizer row.
    pub(super) fn head_cell(self: &Rc<Self>, i: usize) -> Box<dyn FnOnce(ViewHandle)> {
        let sync = self.clone();
        Box::new(move |h| {
            sync.watch(
                h,
                Rc::new(move |s, w| {
                    // Natural widths only: once forced, the head's cells
                    // report the body's widths back.
                    if !s.forced_on.get() {
                        s.head.borrow_mut()[i] = Some(w);
                        s.settle();
                    }
                }),
            )
        })
    }

    /// A binder that watches one cell of the body's sizer row.
    pub(super) fn body_cell(self: &Rc<Self>, i: usize) -> Box<dyn FnOnce(ViewHandle)> {
        let sync = self.clone();
        Box::new(move |h| {
            sync.watch(
                h,
                Rc::new(move |s, w| {
                    s.body.borrow_mut()[i] = Some(w);
                    s.settle();
                }),
            )
        })
    }

    /// A binder that watches the body grid's width.
    pub(super) fn body_grid(self: &Rc<Self>) -> Box<dyn FnOnce(ViewHandle)> {
        let sync = self.clone();
        Box::new(move |h| sync.watch(h, Rc::new(|s, w| s.body_width.set(Some(w)))))
    }

    /// A binder that watches the body scroller's viewport (the probe).
    pub(super) fn probe(self: &Rc<Self>) -> Box<dyn FnOnce(ViewHandle)> {
        let sync = self.clone();
        Box::new(move |h| sync.watch(h, Rc::new(|s, w| s.viewport.set(Some(w)))))
    }

    /// Take the next step once a side is fully measured: floor the body at
    /// the head's natural widths, then force the head to the body's.
    fn settle(&self) {
        let all = |v: &RefCell<Vec<Option<f32>>>| v.borrow().iter().copied().collect::<Option<Vec<f32>>>();
        if !self.floored.get() {
            match all(&self.head) {
                Some(head) => {
                    self.floored.set(true);
                    self.floors.set(Some(head));
                }
                None => return,
            }
        }
        if let Some(body) = all(&self.body) {
            self.forced_on.set(true);
            self.forced.set(Some(body));
        }
    }

    /// The head grid's column specs: column `i`'s own spec until the body
    /// is measured, then exactly the body's width.
    pub(super) fn head_specs(self: &Rc<Self>, specs: &[Option<ColumnSpec>]) -> Vec<Option<ColumnSpec>> {
        specs
            .iter()
            .enumerate()
            .map(|(i, spec)| {
                let (forced, spec) = (self.forced, spec.clone());
                Some(ColumnSpec::live(move || match forced.with(|f| f.as_ref().and_then(|f| f.get(i).copied())) {
                    Some(w) => ColumnTrack { width: Some(w), ..Default::default() },
                    None => spec.as_ref().map(ColumnSpec::get).unwrap_or_default(),
                }))
            })
            .collect()
    }

    /// A sizer cell's style; the body's carries the head's floor.
    pub(super) fn sizer_style(self: &Rc<Self>, i: usize, floored: bool) -> StyleProp {
        let floors = self.floors;
        StyleProp::SheetDynamic(Box::new(move || {
            let app = StyleApplication::new(sizer_sheet());
            let floor = floored.then(|| floors.with(|f| f.as_ref().and_then(|f| f.get(i).copied()))).flatten();
            match floor {
                Some(w) => app.with_inline(StyleRules { min_width: Some(px(w)), ..Default::default() }),
                None => app,
            }
        }))
    }

    /// The head strip's style: as wide as the body's viewport.
    pub(super) fn strip_style(self: &Rc<Self>) -> StyleProp {
        let viewport = self.viewport;
        StyleProp::SheetDynamic(Box::new(move || {
            let app = StyleApplication::new(strip_sheet());
            match viewport.get() {
                Some(w) => app.with_inline(StyleRules { width: Some(px(w)), ..Default::default() }),
                None => app,
            }
        }))
    }

    /// The head content's style: as wide as the body grid.
    pub(super) fn content_style(self: &Rc<Self>) -> StyleProp {
        let width = self.body_width;
        StyleProp::SheetDynamic(Box::new(move || {
            let app = StyleApplication::new(head_content_sheet());
            match width.get() {
                Some(w) => app.with_inline(StyleRules { width: Some(px(w)), ..Default::default() }),
                None => app,
            }
        }))
    }

    /// The body scrolled: move the head strip to the same horizontal
    /// offset.
    pub(super) fn on_body_scroll(&self, x: f32, y: f32) {
        self.body_x.set(x);
        self.body_y.set(y);
        if (self.head_x.get() - x).abs() > OFFSET_SLACK {
            self.head_x.set(x);
            self.head_ref.with(|h| h.scroll_to(x, 0.0));
        }
    }

    /// The head strip scrolled (a trackpad or drag over the header): move
    /// the body to the same horizontal offset, keeping its vertical one.
    pub(super) fn on_head_scroll(&self, x: f32) {
        self.head_x.set(x);
        if (self.body_x.get() - x).abs() > OFFSET_SLACK {
            self.body_x.set(x);
            self.body_ref.with(|h| h.scroll_to(x, self.body_y.get()));
        }
    }

    pub(super) fn head_ref(&self) -> Ref<ScrollViewHandle> {
        self.head_ref
    }

    pub(super) fn body_ref(&self) -> Ref<ScrollViewHandle> {
        self.body_ref
    }
}

fn px(v: f32) -> glue::Tokenized<glue::Length> {
    glue::Tokenized::Literal(glue::Length::Px(v))
}

/// A sizer cell: no height, no padding, no border — it only measures (and
/// floors) its column.
fn sizer_sheet() -> Rc<runtime_shared::StyleSheet> {
    static KEY: u8 = 0;
    runtime_shared::cached_stylesheet(&KEY as *const u8 as usize, || {
        let zero = || Some(px(0.0));
        runtime_shared::StyleSheet::r#static(StyleRules {
            height: zero(),
            min_height: zero(),
            padding_top: zero(),
            padding_bottom: zero(),
            padding_left: zero(),
            padding_right: zero(),
            border_top_width: Some(glue::Tokenized::Literal(0.0)),
            border_bottom_width: Some(glue::Tokenized::Literal(0.0)),
            border_left_width: Some(glue::Tokenized::Literal(0.0)),
            border_right_width: Some(glue::Tokenized::Literal(0.0)),
            overflow: Some(runtime_shared::Overflow::Hidden),
            ..Default::default()
        })
        .premint_as("table.v1.sizer")
    })
}

/// The head strip: a horizontal scroller with no scrollbar of its own (the
/// body's is the one to drag), content laid on its main axis at its own
/// width, never shrinking below its rows.
fn strip_sheet() -> Rc<runtime_shared::StyleSheet> {
    static KEY: u8 = 0;
    runtime_shared::cached_stylesheet(&KEY as *const u8 as usize, || {
        runtime_shared::StyleSheet::r#static(StyleRules {
            flex_direction: Some(glue::FlexDirection::Row),
            align_items: Some(glue::AlignItems::FlexStart),
            flex_grow: Some(glue::Tokenized::Literal(0.0)),
            flex_shrink: Some(glue::Tokenized::Literal(0.0)),
            flex_basis: Some(glue::Tokenized::Literal(glue::Length::Auto)),
            scrollbar: Some(runtime_shared::ScrollbarVisibility::Hidden),
            ..Default::default()
        })
        .premint_as("table.v1.head_strip")
    })
}

/// The head content: its own width (the body grid's once measured), never
/// squeezed by the strip.
fn head_content_sheet() -> Rc<runtime_shared::StyleSheet> {
    static KEY: u8 = 0;
    runtime_shared::cached_stylesheet(&KEY as *const u8 as usize, || {
        runtime_shared::StyleSheet::r#static(StyleRules {
            flex_shrink: Some(glue::Tokenized::Literal(0.0)),
            ..Default::default()
        })
        .premint_as("table.v1.head_content")
    })
}

/// The frame around the head strip and the body scroller: sized like the
/// scroller it replaces in the surface (content-sized, shrinkable to 0, so
/// a bounded surface still caps the rows) — see
/// [`edge_watch_holder_rules`].
pub(super) fn frame_sheet() -> Rc<runtime_shared::StyleSheet> {
    static KEY: u8 = 0;
    runtime_shared::cached_stylesheet(&KEY as *const u8 as usize, || {
        runtime_shared::StyleSheet::r#static(edge_watch_holder_rules()).premint_as("table.v1.frame")
    })
}

/// The probe in the body scroller: absolutely placed across the
/// scroller's padding box at zero height, so its width is the viewport's
/// (a classic scrollbar excluded) whatever the content's width.
pub(super) fn probe_sheet() -> Rc<runtime_shared::StyleSheet> {
    static KEY: u8 = 0;
    runtime_shared::cached_stylesheet(&KEY as *const u8 as usize, || {
        runtime_shared::StyleSheet::r#static(StyleRules {
            position: Some(glue::Position::Absolute),
            left: Some(px(0.0)),
            right: Some(px(0.0)),
            top: Some(px(0.0)),
            height: Some(px(0.0)),
            ..Default::default()
        })
        .premint_as("table.v1.probe")
    })
}

/// The body scroller: the probe's containing block. Both lowerings use
/// it — the premint dump runs the native lowering, so a web-only sheet
/// would have no CSS in a `--premint-only` bundle.
pub(super) fn body_scroller_sheet() -> Rc<runtime_shared::StyleSheet> {
    static KEY: u8 = 0;
    runtime_shared::cached_stylesheet(&KEY as *const u8 as usize, || {
        runtime_shared::StyleSheet::r#static(StyleRules {
            position: Some(glue::Position::Relative),
            ..Default::default()
        })
        .premint_as("table.v1.body_scroller")
    })
}

/// Split a table's children into its header sections and the rest.
pub(super) fn take_head(rows: Vec<Element>) -> (Vec<Element>, Vec<Element>) {
    rows.into_iter().partition(is_head)
}

/// The probe view.
pub(super) fn probe(sync: &Rc<HeadSync>) -> Element {
    let fill = sync.probe();
    runtime_vocabulary::builders::view()
        .style(StyleProp::Sheet(Box::new(StyleApplication::new(probe_sheet()))))
        .on_handle(move |h| fill(h))
        .build()
}

/// The head strip around the head grid.
pub(super) fn head_strip(sync: &Rc<HeadSync>, head_grid: Element) -> Element {
    let content = glue::view(vec![head_grid]).with_style(sync.content_style()).into_element();
    let on_scroll = sync.clone();
    glue::scroll_view(vec![content])
        .axis(runtime_shared::ScrollAxis::Horizontal)
        .bounces(false)
        .on_scroll(move |x, _y| on_scroll.on_head_scroll(x))
        .bind(sync.head_ref())
        .with_style(sync.strip_style())
        .into_element()
}

// ============================================================================
// Web
// ============================================================================

/// Say every row's and cell's role explicitly — the rows of a split-head
/// table's `<table>`s, which are presentational (the frame is the table).
fn set_roles(rows: &mut [Element]) {
    for r in rows.iter_mut() {
        visit_rows_mut(r, |row| {
            if let Element::Item { data, .. } = &*row {
                if let Some(c) = data.downcast_ref::<PrimCell<TableRowPrim>>() {
                    c.with_mut(|p| p.roles = true);
                }
            }
            visit_row_cells(row, |cell| {
                visit_cell(cell, &mut |_, table_cell| {
                    if let Some(p) = table_cell {
                        p.roles = true;
                    }
                });
            });
        });
    }
}

/// A web sizer row: `<tr aria-hidden>` of `columns` empty `<td>`s.
fn web_sizer_row(
    columns: usize,
    style: impl Fn(usize) -> StyleProp,
    fill: impl Fn(usize) -> Box<dyn FnOnce(ViewHandle)>,
) -> Element {
    let cells = (0..columns)
        .map(|i| {
            let cell = item_lowering::cell_item(false, ColSpan::default(), Vec::new(), Some(style(i)));
            let mut f = Some(fill(i));
            visit_cell(&cell, &mut |_, table_cell| {
                if let (Some(p), Some(f)) = (table_cell, f.take()) {
                    p.ref_fill = Some(f);
                    p.roles = true;
                }
            });
            cell
        })
        .collect();
    item(
        PrimCell::new(TableRowPrim {
            style: None,
            ref_fill: None,
            grid: false,
            data: None,
            collapsed: None,
            presentational: true,
            roles: true,
        }),
        cells,
    )
}

/// The web split-head table (see the module docs). `rows` are the
/// table's children with spans resolved and row collapse applied; `grid`
/// is the body's layout mode.
#[allow(clippy::too_many_arguments)]
pub(super) fn web(
    rows: Vec<Element>,
    columns: usize,
    specs: Vec<Option<ColumnSpec>>,
    grid: bool,
    style: Option<StyleProp>,
    scroll_x: bool,
    header_slot: Option<Element>,
    footer_slot: Option<Element>,
    watch: Option<Rc<EdgeWatch>>,
) -> Element {
    let sync = HeadSync::new(columns);
    let (mut head, mut body) = take_head(rows);
    set_roles(&mut head);
    set_roles(&mut body);

    // Body: the sizer row first, then the rows, in the body's own mode.
    body.insert(
        0,
        web_sizer_row(columns, |i| sync.sizer_style(i, true), |i| sync.body_cell(i)),
    );
    let body_fill: Box<dyn FnOnce(ViewHandle)> = {
        let measure = sync.body_grid();
        match watch.clone() {
            Some(watch) => Box::new(move |h: ViewHandle| {
                watch.measure(h.clone(), |w| &w.content);
                measure(h);
            }),
            None => measure,
        }
    };
    let fills = grid && has_flex_column(&specs);
    let body_table = if grid {
        item(
            PrimCell::new(TablePrim {
                style: Some(grid_columns_style(specs.clone())),
                scroll_x,
                scroll_y: true,
                grid: true,
                fills,
                framed: true,
                ref_fill: Some(body_fill),
            }),
            into_grid_rows(body),
        )
    } else {
        item(
            PrimCell::new(TablePrim {
                style: None,
                scroll_x,
                scroll_y: true,
                grid: false,
                fills: false,
                framed: true,
                ref_fill: Some(body_fill),
            }),
            body,
        )
    };

    // Head: always grid tracks, so they can be forced to the body's.
    head.push(web_sizer_row(columns, |i| sync.sizer_style(i, false), |i| sync.head_cell(i)));
    let head_table = item(
        PrimCell::new(TablePrim {
            style: Some(grid_columns_style(sync.head_specs(&specs))),
            // Natural widths, never wrapped: the head starts at its
            // content's width and is then held to the body's.
            scroll_x: true,
            scroll_y: true,
            grid: true,
            fills: false,
            framed: true,
            ref_fill: None,
        }),
        into_grid_rows(head),
    );

    let on_scroll = sync.clone();
    let body_scroller = edge_watched_scroller(
        glue::scroll_view(vec![probe(&sync), body_table])
            .bind(sync.body_ref())
            .with_style(StyleProp::Sheet(Box::new(StyleApplication::new(body_scroller_sheet())))),
        scroller_axis(scroll_x, true),
        watch,
        Some(Rc::new(move |x, y| on_scroll.on_body_scroll(x, y))),
    );
    let frame = item(
        PrimCell::new(TableFramePrim {
            style: Some(StyleProp::Sheet(Box::new(StyleApplication::new(frame_sheet())))),
        }),
        vec![head_strip(&sync, head_table), body_scroller],
    );
    surface_with_slots(frame, style, header_slot, footer_slot)
}

// ============================================================================
// Native
// ============================================================================

/// A native sizer row: `columns` empty grid cells.
#[cfg(not(target_arch = "wasm32"))]
pub(super) fn native_sizer_row(
    columns: usize,
    style: impl Fn(usize) -> StyleProp,
    fill: impl Fn(usize) -> Box<dyn FnOnce(ViewHandle)>,
) -> NativeRow {
    let cells = (0..columns)
        .map(|i| {
            let f = fill(i);
            runtime_vocabulary::builders::view().style(style(i)).on_handle(move |h| f(h)).build()
        })
        .collect();
    NativeRow { cells, spans: vec![1; columns], slots: None }
}

#[cfg(test)]
mod tests {
    use super::*;
    use runtime_world::World;

    fn widths(spec: &[Option<ColumnSpec>]) -> Vec<Option<f32>> {
        spec.iter().map(|s| s.as_ref().and_then(|s| s.get().width)).collect()
    }

    fn record_head(sync: &HeadSync, i: usize, w: f32) {
        if !sync.forced_on.get() {
            sync.head.borrow_mut()[i] = Some(w);
            sync.settle();
        }
    }

    fn record_body(sync: &HeadSync, i: usize, w: f32) {
        sync.body.borrow_mut()[i] = Some(w);
        sync.settle();
    }

    /// Run `f` in `world`, then commit what it staged.
    fn step<R>(world: &World, f: impl FnOnce() -> R) -> R {
        let r = world.enter(f);
        world.flush();
        r
    }

    /// The head waits for both sides, floors the body at its natural
    /// widths, then takes the body's widths exactly — and keeps following
    /// the body, never re-reading its own (forced) widths as natural.
    #[test]
    fn head_floors_the_body_then_follows_it() {
        let world = World::new();
        let (sync, head) = step(&world, || {
            let sync = HeadSync::new(2);
            let specs = vec![Some(ColumnSpec::new(ColumnTrack { width: Some(120.0), ..Default::default() })), None];
            let head = sync.head_specs(&specs);
            (sync, head)
        });
        let floors = || world.enter(|| sync.floors.with_untracked(|f| f.clone()));
        assert_eq!(world.enter(|| widths(&head)), vec![Some(120.0), None], "unforced: the columns' own specs");

        step(&world, || {
            record_body(&sync, 0, 120.0);
            record_body(&sync, 1, 40.0);
        });
        assert_eq!(floors(), None, "no floors before the head is measured");
        assert_eq!(world.enter(|| widths(&head)), vec![Some(120.0), None], "not forced before the head is measured");

        step(&world, || {
            record_head(&sync, 0, 120.0);
            record_head(&sync, 1, 64.0);
        });
        assert_eq!(floors(), Some(vec![120.0, 64.0]));
        assert_eq!(world.enter(|| widths(&head)), vec![Some(120.0), Some(40.0)], "forced to the body's widths");

        // The floor takes: the body's column grows, the head follows.
        step(&world, || record_body(&sync, 1, 64.0));
        assert_eq!(world.enter(|| widths(&head)), vec![Some(120.0), Some(64.0)]);
        // A forced head reports the body's width back; it is not a new
        // natural width.
        step(&world, || {
            record_head(&sync, 1, 64.0);
            record_body(&sync, 1, 300.0);
        });
        assert_eq!(floors(), Some(vec![120.0, 64.0]), "floors are taken once");
        assert_eq!(world.enter(|| widths(&head)), vec![Some(120.0), Some(300.0)], "a wider body column widens the head's");
    }

    /// A body cell's sizer carries the head's floor; the head's does not.
    #[test]
    fn only_the_body_sizer_is_floored() {
        let world = World::new();
        let sync = step(&world, || {
            let sync = HeadSync::new(1);
            sync.floors.set(Some(vec![72.0]));
            sync
        });
        world.enter(|| {
            let min = |prop: StyleProp| match prop {
                StyleProp::SheetDynamic(f) => f().inline().and_then(|r| r.min_width.clone()),
                _ => panic!("sizer styles are live sheet applications"),
            };
            assert_eq!(min(sync.sizer_style(0, true)), Some(px(72.0)));
            assert_eq!(min(sync.sizer_style(0, false)), None);
        });
    }

    /// Scrolling either side moves the other to the same horizontal
    /// offset once — the echo of that programmatic scroll is a no-op.
    #[test]
    fn scroll_offsets_sync_both_ways_without_echo() {
        let world = World::new();
        world.enter(|| {
            let sync = HeadSync::new(1);
            sync.on_body_scroll(80.0, 30.0);
            assert_eq!(sync.head_x.get(), 80.0, "the head strip is sent to the body's offset");
            sync.on_head_scroll(80.0);
            assert_eq!(sync.body_x.get(), 80.0, "the strip's echo does not move the body");
            sync.on_head_scroll(140.0);
            assert_eq!(sync.body_x.get(), 140.0, "scrolling the strip moves the body");
            assert_eq!(sync.body_y.get(), 30.0, "…keeping its vertical offset");
        });
    }
}
