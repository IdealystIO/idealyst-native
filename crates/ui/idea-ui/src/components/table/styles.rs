//! The table family's styled sections — one [`TableStyles`] for the
//! frame, the head and body cells and their text, the frame slots, the
//! resize handle, the pager and the narrow-form cards.
//!
//! The frame slot lands on the `table` SDK's surface view around the
//! `<table>` / native grid — never the `<table>` itself (browsers ignore a
//! radius on a collapsed-border table); `head_cell` and `body_cell` land
//! on `<th>` and `<td>` (padding + row divider). Border-bottom on each
//! cell + `border-collapse: collapse` on the table merges into one
//! continuous row boundary per row.
//!
//! Every table-wide or row-wide setting (density, variant, tone, footer
//! section, clickable-row hover) is an AXIS on the cell slots rather than
//! an override, because the cells are already built when `Table` /
//! `TableRow` run: they re-select arms on each built cell, which keeps the
//! cells preminted. Axis arms resolve in axis-name order, which several
//! comments below rely on ("sorts before").

use runtime_core::{
    component_styles, AlignItems, Color, Cursor, DisplayKind, FlexDirection, FontWeight, JustifyContent, Length,
    Overflow, Position, TextAlign, TextTransform,
};

use crate::theme::IdeaThemeRef;

component_styles! {
    /// The styled sections of [`Table`].
    pub Table<IdeaThemeRef> {
        slot frame {
            base(t) {
                background: t.color.surface(),
                border_top_width: 1.0,
                border_right_width: 1.0,
                border_bottom_width: 1.0,
                border_left_width: 1.0,
                border_top_color: t.color.border(),
                border_right_color: t.color.border(),
                border_bottom_color: t.color.border(),
                border_left_color: t.color.border(),
                border_top_left_radius: t.radius.lg(),
                border_top_right_radius: t.radius.lg(),
                border_bottom_left_radius: t.radius.lg(),
                border_bottom_right_radius: t.radius.lg(),
                // Clip children to the rounded frame: the corner cells'
                // square backgrounds (the header band) and scroll-x columns
                // otherwise paint over the rounded corners. Safe on every
                // table because this sheet lands on the SDK's surface VIEW,
                // never on the `<table>` — a clip there would shave the outer
                // half of its collapsed border.
                overflow: Overflow::Hidden,
            }
            // `Table(variant = …)`. `plain` drops the card: no border, no
            // radius, no fill of its own — it sits on the surface around it
            // (an app that wants a fill styles the container), and the rows'
            // dividers are the only lines.
            variant variant {
                #[default]
                framed(_t) {}
                plain(_t) {
                    background: Color("transparent".into()),
                    border_top_width: 0.0,
                    border_right_width: 0.0,
                    border_bottom_width: 0.0,
                    border_left_width: 0.0,
                    border_top_left_radius: 0.0,
                    border_top_right_radius: 0.0,
                    border_bottom_left_radius: 0.0,
                    border_bottom_right_radius: 0.0,
                }
            }
            // `Table(scroll_y = true)`: the table owns its vertical scroll, so
            // it takes the height its parent gives it — growing into a bounded
            // column, shrinking below its rows (`min_height: 0`; the framework
            // `overflow` is a clip, not a Taffy scroll container, so the
            // automatic minimum would otherwise hold it at its content) — and
            // its scroller takes what is left. In an unbounded parent it is as
            // tall as its rows. `max_height` caps it on top (an override).
            variant scroll {
                #[default]
                off(_t) {}
                fill(_t) {
                    flex_grow: 1.0,
                    flex_shrink: 1.0,
                    min_height: 0.0,
                }
            }
            transitions {
                background: 250ms EaseInOut,
                border_top_color: 250ms EaseInOut,
            }
        }

        slot head_cell {
            base(t) {
                // The header band has its own token (defaulting to the
                // `surface_alt` value) so an app can retint table headers
                // without moving every other `surface_alt` consumer —
                // cards, field wells, row hover — with them.
                background: t.color.table_header(),
                padding_horizontal: t.spacing.lg(),
                border_bottom_width: 1.0,
                border_bottom_color: t.color.border(),
                // Override the browser UA default `th { text-align: center }`.
                // The inner text node shrink-wraps (display: inline), so its
                // own `text_align: Left` can't win — the cell's alignment is
                // what positions the inline span. Pin it Left so header cells
                // match body cells on web (native is unaffected: there the
                // text node's alignment already applies). See `body_cell`.
                text_align: TextAlign::Left,
            }
            // Row density (`Table(density = …)`). Vertical padding only — a
            // row's height is its tallest cell, so padding is the one lever
            // that moves every row on every backend the same way (`min-height`
            // on a `<td>` is ignored by browsers, and `height` there means
            // "at least", unlike a native box). `comfortable` puts a single-
            // line row at the 44–52pt touch-target band.
            variant density {
                compact(t) { padding_vertical: t.spacing.sm() }
                #[default]
                standard(t) { padding_vertical: t.spacing.md() }
                comfortable(t) { padding_vertical: t.spacing.lg() }
            }
            // Horizontal padding (`TableCell(padding = …)` /
            // `TableColumn::padding`): `icon` is the compact padding a column
            // of icons or icon buttons takes — a row menu, a checkbox, a drag
            // grip. Named to sort BEFORE `inset_left` / `inset_right`, so a
            // table's edge inset still wins on the row's first and last cell.
            variant cell_padding {
                #[default]
                standard(_t) {}
                icon(t) { padding_horizontal: t.spacing.sm() }
            }
            // Column alignment (`TableCell(align = …)`). On the CELL as well
            // as on the inner text/content: the cell's `text-align` is what
            // positions inline content on web (and overrides the UA `th {
            // text-align: center }`), the inner node's is what a native text
            // node reads.
            variant align {
                #[default]
                left(_t) { text_align: TextAlign::Left }
                center(_t) { text_align: TextAlign::Center }
                right(_t) { text_align: TextAlign::Right }
            }
            // Single-line truncation (`TableCell(truncate = true)`). On the
            // CELL for web — the text node is an inline span, which
            // `text-overflow` never applies to, while a `<td>` is a block
            // container that truncates its inline content (measured: a
            // `width`+`max-width` cell stays at its width and ends in "…").
            // The text sheets carry the same axis for native, where the
            // text node itself is what truncates. Only meaningful in a
            // column with an exact `width` — an auto column grows to fit an
            // unwrapped line.
            variant truncate {
                #[default]
                off(_t) {}
                on(_t) { max_lines: 1u32 }
            }
            // Clickable-row axes — see `body_cell` for the rationale.
            variant interactive {
                #[default]
                off(_t) {}
                on(_t) { cursor: Cursor::Pointer }
            }
            // Every arm STATES the background, including the resting one.
            // An empty `off` arm sets nothing, and a style re-resolve that
            // mentions no background cannot undo one a previous resolve
            // applied — so the tint went on at press and stayed on after
            // release. The `pinned` arms below always set a background,
            // which is exactly why a frozen column looked immune while its
            // neighbours stayed lit.
            variant row_hovered {
                #[default]
                off(t) { background: t.color.table_header() }
                on(t) { background: t.color.surface_alt() }
            }
            // Frozen-column axis (`TableCell(pinned = …)` in a
            // `Table(scroll_x = true)`). `position: Sticky` + the zero
            // inset pins the cell inside the table's horizontal scroller
            // on every backend — the browser natively on web, the shared
            // sticky registry (which also raises pinned cells above the
            // content sliding beneath them) on native. The head cell's own
            // `table_header` background already makes it opaque; the inner-
            // edge hairline marks where content slides underneath.
            variant pinned {
                #[default]
                none(_t) {}
                left(t) {
                    position: Position::Sticky,
                    left: 0,
                    border_right_width: 1.0,
                    border_right_color: t.color.border(),
                }
                right(t) {
                    position: Position::Sticky,
                    right: 0,
                    border_left_width: 1.0,
                    border_left_color: t.color.border(),
                }
            }
            // Pinned header row (`Table(scroll_y = true)`): the head cells stick
            // to the top of the table's own scroller. `TableCell(header = true)`
            // selects `off` explicitly — that key is how `Table` tells a head
            // cell from a body cell — and `Table` selects `on` for the leading
            // head rows. Stacking among the cells (all siblings of one grid on
            // native, one table on web): a body cell pinned to a side paints at
            // the sticky default (`STICKY_Z`, 1); a row lifted by a reorder drag
            // above that (2, its frozen cells 3 — see the `body_cell` slot's `drag`);
            // the header row above every body row (4) so rows, lifted ones
            // included, slide UNDER the header; and a corner cell — pinned to
            // the top AND a side — above all of them (5), so columns slide under
            // it sideways and rows slide under it vertically.
            variant sticky_header {
                #[default]
                off(_t) {}
                on(_t) {
                    position: Position::Sticky,
                    top: 0,
                    z_index: 4,
                }
            }
            compound (sticky_header: on, pinned: left)(_t) {
                z_index: 5,
            }
            compound (sticky_header: on, pinned: right)(_t) {
                z_index: 5,
            }
            // A resizable column's header cell (`TableColumn::resizable`) holds
            // its drag handle absolutely positioned on its trailing edge, so
            // the cell must be the handle's containing block: a `<th>` that is
            // not positioned is not one, and the handle would pin to the table
            // instead. `relative` — and the axis sorts BEFORE `pinned` and
            // `sticky_header`, so a frozen or pinned-header cell keeps its
            // `Sticky` (itself a containing block) rather than unfreezing.
            variant anchor {
                #[default]
                off(_t) {}
                on(_t) { position: Position::Relative }
            }
            // Table edge inset (`Table(inset_x = …)`): the outer padding of
            // a row's FIRST cell (`inset_left`) and LAST cell (`inset_right`),
            // so the row dividers run the table's full width while the text
            // lines up with the page margin. Selected per edge cell by
            // `Table`; the `none` default leaves the base padding.
            variant inset_left {
                #[default]
                none(_t) {}
                xs(t) { padding_left: t.spacing.xs() }
                sm(t) { padding_left: t.spacing.sm() }
                md(t) { padding_left: t.spacing.md() }
                lg(t) { padding_left: t.spacing.lg() }
                xl(t) { padding_left: t.spacing.xl() }
                xxl(t) { padding_left: t.spacing.xxl() }
            }
            variant inset_right {
                #[default]
                none(_t) {}
                xs(t) { padding_right: t.spacing.xs() }
                sm(t) { padding_right: t.spacing.sm() }
                md(t) { padding_right: t.spacing.md() }
                lg(t) { padding_right: t.spacing.lg() }
                xl(t) { padding_right: t.spacing.xl() }
                xxl(t) { padding_right: t.spacing.xxl() }
            }
            // Drag-and-drop feedback for a CUSTOM row interaction (the
            // built-in reorder uses the `body_cell` slot's `drag` axis instead):
            // `dragging` dims an in-flight row, `drop_target` highlights the
            // slot under it. Inert until an implementation selects them via
            // `table::map_cell_style`. Shared-signal axes like `row_hovered`.
            // Row divider (the cell's bottom border). `Table` selects `off`
            // on the row it renders LAST when nothing sits below the rows:
            // there the frame's own bottom border is the edge, and the
            // divider stacked on it read as a 2px double line.
            variant divider {
                #[default]
                on(_t) {}
                off(_t) { border_bottom_width: 0.0 }
            }
            variant dragging {
                #[default]
                off(_t) {}
                on(_t) { opacity: 0.4 }
            }
            variant drop_target {
                #[default]
                off(_t) {}
                on(t) { background: t.color.surface_alt() }
            }
            // `Table(variant = Plain)`: no header band — the head row is
            // muted text over a divider, on whatever the table sits on. A
            // frozen head cell still has to be opaque (columns slide beneath
            // it), so it paints the surface, like a frozen body cell.
            variant variant {
                #[default]
                framed(_t) {}
                plain(_t) {}
            }
            compound (variant: plain, row_hovered: off)(_t) {
                background: Color("transparent".into()),
            }
            compound (variant: plain, pinned: left, row_hovered: off)(t) {
                background: t.color.surface(),
            }
            compound (variant: plain, pinned: right, row_hovered: off)(t) {
                background: t.color.surface(),
            }
            // A frozen column's inner-edge hairline, for a `plain` table:
            // `Table` selects `hidden` while nothing is scrolled beneath that
            // edge, so the line appears only once content slides under the
            // column. The colour goes transparent rather than the width to 0,
            // so the column does not shift a pixel as its edge comes and goes.
            // Compounds, because `pinned` sorts after `pin_edge` and its
            // arms would otherwise restate the colour.
            variant pin_edge {
                #[default]
                shown(_t) {}
                hidden(_t) {}
                // A frozen column with another frozen column between it and
                // the scrolling ones (a selection column beside a frozen name
                // column): its inner edge is inside the frozen block, so it
                // never draws.
                interior(_t) {}
            }
            compound (pinned: left, pin_edge: hidden)(_t) {
                border_right_color: Color("transparent".into()),
            }
            compound (pinned: right, pin_edge: hidden)(_t) {
                border_left_color: Color("transparent".into()),
            }
            compound (pinned: left, pin_edge: interior)(_t) {
                border_right_color: Color("transparent".into()),
            }
            compound (pinned: right, pin_edge: interior)(_t) {
                border_left_color: Color("transparent".into()),
            }
            transitions {
                background: 250ms EaseInOut,
                border_bottom_color: 250ms EaseInOut,
                border_left_color: 150ms EaseInOut,
                border_right_color: 150ms EaseInOut,
                opacity: 150ms EaseInOut,
            }
        }

        slot body_cell {
            base(t) {
                padding_horizontal: t.spacing.lg(),
                border_bottom_width: 1.0,
                border_bottom_color: t.color.border(),
                // Explicit (matches the UA `td` default) so head + body cells
                // share one alignment source of truth — see `head_cell`.
                text_align: TextAlign::Left,
            }
            // Row density (`Table(density = …)`). Vertical padding only — a
            // row's height is its tallest cell, so padding is the one lever
            // that moves every row on every backend the same way (`min-height`
            // on a `<td>` is ignored by browsers, and `height` there means
            // "at least", unlike a native box). `comfortable` puts a single-
            // line row at the 44–52pt touch-target band.
            variant density {
                compact(t) { padding_vertical: t.spacing.sm() }
                #[default]
                standard(t) { padding_vertical: t.spacing.md() }
                comfortable(t) { padding_vertical: t.spacing.lg() }
            }
            // Horizontal padding (`TableCell(padding = …)` /
            // `TableColumn::padding`): `icon` is the compact padding a column
            // of icons or icon buttons takes — a row menu, a checkbox, a drag
            // grip. Named to sort BEFORE `inset_left` / `inset_right`, so a
            // table's edge inset still wins on the row's first and last cell.
            variant cell_padding {
                #[default]
                standard(_t) {}
                icon(t) { padding_horizontal: t.spacing.sm() }
            }
            // Column alignment (`TableCell(align = …)`). On the CELL as well
            // as on the inner text/content: the cell's `text-align` is what
            // positions inline content on web (and overrides the UA `th {
            // text-align: center }`), the inner node's is what a native text
            // node reads.
            variant align {
                #[default]
                left(_t) { text_align: TextAlign::Left }
                center(_t) { text_align: TextAlign::Center }
                right(_t) { text_align: TextAlign::Right }
            }
            // Single-line truncation (`TableCell(truncate = true)`). On the
            // CELL for web — the text node is an inline span, which
            // `text-overflow` never applies to, while a `<td>` is a block
            // container that truncates its inline content (measured: a
            // `width`+`max-width` cell stays at its width and ends in "…").
            // The text sheets carry the same axis for native, where the
            // text node itself is what truncates. Only meaningful in a
            // column with an exact `width` — an auto column grows to fit an
            // unwrapped line.
            variant truncate {
                #[default]
                off(_t) {}
                on(_t) { max_lines: 1u32 }
            }
            // Clickable-row axes (`TableRow` with `on_row_click`). Enumerated
            // variants rather than runtime `with_overrides` so every arm has
            // build-time CSS and the flip is a preminted CLASS SWAP — the
            // override form kept every table cell on the live engine, which
            // was one of the last two `--premint-only` blockers on the docs
            // corpus. `row_hovered` is a shared-signal axis, NOT a
            // `state hovered` pseudo: hovering ANY cell highlights the WHOLE
            // row via the row's one signal, which per-cell `:hover` cannot
            // express — see `components::table::make_row_cell_interactive`.
            variant interactive {
                #[default]
                off(_t) {}
                on(_t) { cursor: Cursor::Pointer }
            }
            // Explicit resting background — see `head_cell`. A body cell
            // has none of its own, so resting is TRANSPARENT rather than a
            // token: the table surface shows through exactly as before, and
            // the arm now has something to say instead of nothing.
            variant row_hovered {
                #[default]
                off(_t) { background: Color("transparent".into()) }
                on(t) { background: t.color.surface_alt() }
            }
            // Frozen-column axis — see `head_cell`. A body cell has no
            // background of its own, so the pinned arms pin an OPAQUE
            // surface background too: without it the cells sliding
            // underneath would show straight through the frozen column.
            variant pinned {
                #[default]
                none(_t) {}
                left(t) {
                    position: Position::Sticky,
                    left: 0,
                    background: t.color.surface(),
                    border_right_width: 1.0,
                    border_right_color: t.color.border(),
                }
                right(t) {
                    position: Position::Sticky,
                    right: 0,
                    background: t.color.surface(),
                    border_left_width: 1.0,
                    border_left_color: t.color.border(),
                }
            }
            // Table edge inset (`Table(inset_x = …)`): the outer padding of
            // a row's FIRST cell (`inset_left`) and LAST cell (`inset_right`),
            // so the row dividers run the table's full width while the text
            // lines up with the page margin. Selected per edge cell by
            // `Table`; the `none` default leaves the base padding.
            variant inset_left {
                #[default]
                none(_t) {}
                xs(t) { padding_left: t.spacing.xs() }
                sm(t) { padding_left: t.spacing.sm() }
                md(t) { padding_left: t.spacing.md() }
                lg(t) { padding_left: t.spacing.lg() }
                xl(t) { padding_left: t.spacing.xl() }
                xxl(t) { padding_left: t.spacing.xxl() }
            }
            variant inset_right {
                #[default]
                none(_t) {}
                xs(t) { padding_right: t.spacing.xs() }
                sm(t) { padding_right: t.spacing.sm() }
                md(t) { padding_right: t.spacing.md() }
                lg(t) { padding_right: t.spacing.lg() }
                xl(t) { padding_right: t.spacing.xl() }
                xxl(t) { padding_right: t.spacing.xxl() }
            }
            // A frozen column's opacity is NOT decided by the `pinned` arms
            // above. Axis overlays merge in ALPHABETICAL axis order
            // (`StyleSheet::variants` is a `BTreeMap`), and `row_hovered`
            // sorts after `pinned` — so the resting arm's `transparent`
            // erased the pinned background and the columns sliding
            // underneath showed straight through the frozen one. These
            // compounds state the combination outright; `resolve` layers
            // them after every axis, so they are what actually decides.
            //
            // They also close the gap the same bug hid: a pinned cell now
            // takes the row tint WITH its row instead of sitting inert while
            // its neighbours highlight.
            compound (pinned: left, row_hovered: off)(t) {
                background: t.color.surface(),
            }
            compound (pinned: right, row_hovered: off)(t) {
                background: t.color.surface(),
            }
            compound (pinned: left, row_hovered: on)(t) {
                background: t.color.surface_alt(),
            }
            compound (pinned: right, row_hovered: on)(t) {
                background: t.color.surface_alt(),
            }
            // Row reorder (`Table(on_reorder = …)`). While a row is dragged,
            // every body row is `behind` or `lifted`:
            //
            // - `behind` paints the plain surface: body cells have no fill of
            //   their own, and the rows sliding aside to open the landing gap
            //   cross each other — transparent cells would show one row's text
            //   through another's. A compound, because `row_hovered`'s resting
            //   `transparent` sorts after `drag`.
            // - `lifted` is the held row. `relative` + a z-index raise it above
            //   the rows it crosses (cells paint in document order, so a row
            //   dragged down would slide UNDER the rows below it), and it rides
            //   above other rows' frozen cells (sticky, 1) but under the
            //   pinned header row (4). The axis sorts BEFORE `pinned`, so a
            //   lifted frozen cell keeps `Sticky` — it stays frozen during the
            //   drag — and the compounds give it 3, above its own row's
            //   scrolling cells. The table selects `row_hovered: on` with it,
            //   which is the lifted row's opaque tint.
            variant drag {
                #[default]
                idle(_t) {}
                behind(_t) {}
                lifted(_t) {
                    position: Position::Relative,
                    z_index: 2,
                }
            }
            compound (drag: behind, row_hovered: off)(t) {
                background: t.color.surface(),
            }
            compound (drag: lifted, pinned: left)(_t) {
                z_index: 3,
            }
            compound (drag: lifted, pinned: right)(_t) {
                z_index: 3,
            }
            // Footer section (`TableFooter`): the header band's tint, so a
            // totals row reads as the table's footer rather than as data.
            // The axis sorts after `row_hovered`, so it already beats the
            // resting `transparent`; the compounds beat the PINNED compounds
            // above, which would otherwise repaint a frozen footer cell with
            // the plain surface.
            variant section {
                #[default]
                body(_t) {}
                foot(t) { background: t.color.table_header() }
            }
            compound (section: foot, pinned: left)(t) {
                background: t.color.table_header(),
            }
            compound (section: foot, pinned: right)(t) {
                background: t.color.table_header(),
            }
            // Row tone (`TableRow(tone = …)`). A whole-row tint that must
            // COMPOSE with the clickable-row hover and with a frozen column:
            //
            // - The axis alone would decide nothing reliably: `tone` sorts
            //   after `row_hovered`, so a resting tone would also beat the
            //   hover (a toned row would stop responding to the pointer), and
            //   the pinned compounds above would repaint frozen cells.
            // - So every (tone, row_hovered) pair is stated as a compound,
            //   declared AFTER the pinned and footer compounds. Compounds
            //   resolve in declaration order and premint as equal-specificity
            //   selectors emitted in that same order, so these win on both
            //   paths.
            // - The tints are the `table-row-*` tokens, OPAQUE by contract:
            //   a frozen toned cell covers the columns sliding beneath it.
            variant tone {
                #[default]
                none(_t) {}
                highlight(t) { background: t.color.table_row_highlight() }
                warning(t) { background: t.color.table_row_warning() }
                danger(t) { background: t.color.table_row_danger() }
            }
            compound (tone: highlight, row_hovered: off)(t) {
                background: t.color.table_row_highlight(),
            }
            compound (tone: highlight, row_hovered: on)(t) {
                background: t.color.table_row_highlight_hover(),
            }
            compound (tone: warning, row_hovered: off)(t) {
                background: t.color.table_row_warning(),
            }
            compound (tone: warning, row_hovered: on)(t) {
                background: t.color.table_row_warning_hover(),
            }
            compound (tone: danger, row_hovered: off)(t) {
                background: t.color.table_row_danger(),
            }
            compound (tone: danger, row_hovered: on)(t) {
                background: t.color.table_row_danger_hover(),
            }
            // Custom drag-and-drop feedback (`dragging`, `drop_target`) —
            // see `head_cell`.
            // Row divider (the cell's bottom border). `Table` selects `off`
            // on the row it renders LAST when nothing sits below the rows:
            // there the frame's own bottom border is the edge, and the
            // divider stacked on it read as a 2px double line.
            variant divider {
                #[default]
                on(_t) {}
                off(_t) { border_bottom_width: 0.0 }
            }
            variant dragging {
                #[default]
                off(_t) {}
                on(_t) { opacity: 0.4 }
            }
            variant drop_target {
                #[default]
                off(_t) {}
                on(t) { background: t.color.surface_alt() }
            }
            // A frozen column's inner-edge hairline, for a `plain` table:
            // `Table` selects `hidden` while nothing is scrolled beneath that
            // edge, so the line appears only once content slides under the
            // column. The colour goes transparent rather than the width to 0,
            // so the column does not shift a pixel as its edge comes and goes.
            // Compounds, because `pinned` sorts after `pin_edge` and its
            // arms would otherwise restate the colour.
            variant pin_edge {
                #[default]
                shown(_t) {}
                hidden(_t) {}
                // A frozen column with another frozen column between it and
                // the scrolling ones (a selection column beside a frozen name
                // column): its inner edge is inside the frozen block, so it
                // never draws.
                interior(_t) {}
            }
            compound (pinned: left, pin_edge: hidden)(_t) {
                border_right_color: Color("transparent".into()),
            }
            compound (pinned: right, pin_edge: hidden)(_t) {
                border_left_color: Color("transparent".into()),
            }
            compound (pinned: left, pin_edge: interior)(_t) {
                border_right_color: Color("transparent".into()),
            }
            compound (pinned: right, pin_edge: interior)(_t) {
                border_left_color: Color("transparent".into()),
            }
            // `Table(variant = …)` — inert here: a body cell has no band to
            // drop, and its frozen arms already paint the surface. Declared so
            // the table can select it on every cell without asking which
            // sheet a cell carries.
            variant variant {
                #[default]
                framed(_t) {}
                plain(_t) {}
            }
            // Cell text tone (`TableCell(tone = …)`): one value's colour — an
            // overdue date, a failed check. On the CELL so rich children that
            // inherit text colour (web) take it; `body_text` carries the
            // same axis for the text node itself (native text does not
            // inherit).
            variant text_tone {
                #[default]
                none(_t) {}
                muted(t) { color: t.color.text_muted() }
                success(t) { color: t.intent.success.fg() }
                warning(t) { color: t.intent.warning.fg() }
                danger(t) { color: t.intent.danger.fg() }
            }
            transitions {
                border_bottom_color: 250ms EaseInOut,
                border_left_color: 150ms EaseInOut,
                border_right_color: 150ms EaseInOut,
                // Fade the whole-row hover highlight the `row_hovered` axis
                // flips on/off.
                background: 150ms EaseInOut,
                opacity: 150ms EaseInOut,
            }
        }

        /// Text styling applied to the `text` node INSIDE each cell. The cell
        /// stylesheets above handle the table-cell concerns (padding +
        /// border); these handle typography. Splitting keeps the cell's
        /// `display: table-cell` intact while letting the inner text inherit
        /// the theme's font + color tokens.
        slot head_text {
            base(t) {
                font_size: 11.0,
                font_weight: FontWeight::SemiBold,
                letter_spacing: 0.8,
                text_transform: TextTransform::Uppercase,
                color: t.color.text_muted(),
                text_align: TextAlign::Left,
            }
            // Mirrors the cell's `align` axis — see `head_cell`.
            variant align {
                #[default]
                left(_t) { text_align: TextAlign::Left }
                center(_t) { text_align: TextAlign::Center }
                right(_t) { text_align: TextAlign::Right }
            }
            // Single-line truncation — see the `body_cell` slot's `truncate`.
            variant truncate {
                #[default]
                off(_t) {}
                on(_t) { max_lines: 1u32 }
            }
            transitions {
                color: 250ms EaseInOut,
            }
        }

        slot body_text {
            base(t) {
                font_size: 14.0,
                color: t.color.text(),
                text_align: TextAlign::Left,
            }
            // Cell text tone — see the `body_cell` slot's `text_tone`.
            variant text_tone {
                #[default]
                none(_t) {}
                muted(t) { color: t.color.text_muted() }
                success(t) { color: t.intent.success.fg() }
                warning(t) { color: t.intent.warning.fg() }
                danger(t) { color: t.intent.danger.fg() }
            }
            // Mirrors the cell's `align` axis — see `head_cell`.
            variant align {
                #[default]
                left(_t) { text_align: TextAlign::Left }
                center(_t) { text_align: TextAlign::Center }
                right(_t) { text_align: TextAlign::Right }
            }
            // Single-line truncation — see the `body_cell` slot's `truncate`.
            variant truncate {
                #[default]
                off(_t) {}
                on(_t) { max_lines: 1u32 }
            }
            transitions {
                color: 250ms EaseInOut,
            }
        }

        /// Inner wrapper for `TableCell { … }` rich-children blocks. A `<div
        /// display: flex>` placed directly inside a `<td>` expands to the
        /// cell's full width (a quirk of flex containers under `display:
        /// table-cell`). Setting `justify_content: FlexStart` keeps flex-grow
        /// children (Tags, Buttons) at their natural width, sitting left-
        /// aligned inside the cell instead of stretching across it. Authors
        /// who want stretched children can override at the call site.
        slot cell_inner {
            base(t) {
                flex_direction: FlexDirection::Row,
                align_items: AlignItems::Center,
                justify_content: JustifyContent::FlexStart,
                gap: t.spacing.sm(),
            }
            // The cell's `align` axis, for rich children: the row-flex
            // wrapper places its children, which `text-align` cannot move.
            // Same justification as before for `left` — children keep their
            // natural width (a status chip must not stretch its column).
            variant align {
                #[default]
                left(_t) { justify_content: JustifyContent::FlexStart }
                center(_t) { justify_content: JustifyContent::Center }
                right(_t) { justify_content: JustifyContent::FlexEnd }
            }
        }

        /// Frame slots (`Table(header_slot = …, footer_slot = …)`): content drawn
        /// inside the table's frame but outside its horizontal scroller. Cell
        /// padding, so slot content lines up with the first column's text; the
        /// header slot draws the hairline between itself and the rows (the
        /// footer slot needs none — the last row keeps its own divider when a
        /// footer slot follows it; see the `divider` axis on the cell sheets).
        slot frame_slot {
            base(t) {
                padding_vertical: t.spacing.md(),
                padding_horizontal: t.spacing.lg(),
            }
            // The table's edge inset (`Table(inset_x = …)`) on both sides, so
            // slot content lines up with the first column's text — see the
            // cell sheets' `inset_left` / `inset_right`.
            variant inset_left {
                #[default]
                none(_t) {}
                xs(t) { padding_left: t.spacing.xs() }
                sm(t) { padding_left: t.spacing.sm() }
                md(t) { padding_left: t.spacing.md() }
                lg(t) { padding_left: t.spacing.lg() }
                xl(t) { padding_left: t.spacing.xl() }
                xxl(t) { padding_left: t.spacing.xxl() }
            }
            variant inset_right {
                #[default]
                none(_t) {}
                xs(t) { padding_right: t.spacing.xs() }
                sm(t) { padding_right: t.spacing.sm() }
                md(t) { padding_right: t.spacing.md() }
                lg(t) { padding_right: t.spacing.lg() }
                xl(t) { padding_right: t.spacing.xl() }
                xxl(t) { padding_right: t.spacing.xxl() }
            }

            variant edge {
                #[default]
                top(t) {
                    border_bottom_width: 1.0,
                    border_bottom_color: t.color.border(),
                }
                bottom(_t) {}
            }
            transitions {
                border_bottom_color: 250ms EaseInOut,
            }
        }

        /// The drag strip on a resizable column's header cell
        /// (`TableColumn::resizable`): absolutely positioned on the cell's trailing
        /// edge, the full height of the cell, with the column-resize cursor.
        ///
        /// REVEALED, not drawn at rest: a hairline on every heading turns a plain
        /// table back into a grid, so the divider shows only while the heading is
        /// hovered or a drag is in flight (`revealed`) — where the person looking
        /// for it already is. At rest the border is transparent rather than absent,
        /// so revealing it moves nothing, and the strip stays grabbable. Hovering
        /// the strip itself steps up to the strong border.
        slot resize_handle {
            base(_t) {
                position: Position::Absolute,
                top: 0.0,
                bottom: 0.0,
                right: 0.0,
                width: 8.0,
                cursor: Cursor::ColResize,
                border_right_width: 1.0,
                border_right_color: Color("transparent".into()),
            }
            variant revealed {
                #[default]
                off(_t) {}
                on(t) { border_right_color: t.color.border() }
            }
            state hovered(t) {
                border_right_width: 2.0,
                border_right_color: t.color.border_strong(),
            }
            transitions {
                border_right_color: 120ms EaseInOut,
            }
        }

        /// A sortable header's idle direction glyph (`TableCell(sort =
        /// Some(SortState::None))`): the up-down pair, faint, so the column reads
        /// as sortable without competing with the one that is sorted.
        slot sort_idle {
            base(_t) {
                opacity: 0.45,
            }
        }

        /// `TablePager`: the range text at the leading end, the "Rows" picker and
        /// page numbers at the trailing end, wrapping to two lines on a narrow
        /// table. Sits in the table's footer slot, which supplies the padding and
        /// the edge inset.
        slot pager_bar {
            base(t) {
                flex_direction: FlexDirection::Row,
                flex_wrap: runtime_core::FlexWrap::Wrap,
                align_items: AlignItems::Center,
                justify_content: JustifyContent::SpaceBetween,
                gap: t.spacing.md(),
            }
        }

        slot pager_group {
            base(t) {
                flex_direction: FlexDirection::Row,
                align_items: AlignItems::Center,
                gap: t.spacing.sm(),
            }
        }

        /// `TableCards` — a table's narrow form. A card per row: surface, hairline
        /// border, radius, and a gap between cards (the list's `gap`). A reflowed
        /// row is two-dimensional — a title line over a grid of labelled cells — so
        /// the card boundary is what says where one record ends; a shared hairline
        /// between rows would compete with the rule above the cells.
        slot cards_list {
            base(_t) {
                flex_grow: 1.0,
                flex_shrink: 1.0,
                min_height: 0.0,
            }
        }

        slot card {
            base(t) {
                background: t.color.surface(),
                border_top_width: 1.0,
                border_right_width: 1.0,
                border_bottom_width: 1.0,
                border_left_width: 1.0,
                border_top_color: t.color.border(),
                border_right_color: t.color.border(),
                border_bottom_color: t.color.border(),
                border_left_color: t.color.border(),
                border_top_left_radius: t.radius.lg(),
                border_top_right_radius: t.radius.lg(),
                border_bottom_left_radius: t.radius.lg(),
                border_bottom_right_radius: t.radius.lg(),
                padding_vertical: t.spacing.md(),
                padding_horizontal: t.spacing.lg(),
                gap: t.spacing.sm(),
            }
            // A card whose row has `on_row_click`.
            variant interactive {
                #[default]
                off(_t) {}
                on(_t) { cursor: Cursor::Pointer }
            }
        }

        /// The title line: [selection box] title … opposite [row menu].
        slot card_title_line {
            base(t) {
                flex_direction: FlexDirection::Row,
                align_items: AlignItems::Center,
                gap: t.spacing.sm(),
            }
        }

        slot card_title {
            base(t) {
                flex_grow: 1.0,
                flex_shrink: 1.0,
                min_width: 0.0,
                font_size: 15.0,
                font_weight: FontWeight::SemiBold,
                color: t.color.text(),
            }
        }

        slot card_opposite {
            base(_t) {
                flex_shrink: 0.0,
                align_items: AlignItems::FlexEnd,
            }
        }

        /// The labelled cells: two to a line under a hairline, label over value.
        slot card_fields {
            base(t) {
                flex_direction: FlexDirection::Row,
                flex_wrap: runtime_core::FlexWrap::Wrap,
                row_gap: t.spacing.md(),
                padding_top: t.spacing.md(),
                margin_top: t.spacing.xs(),
                border_top_width: 1.0,
                border_top_color: t.color.border(),
            }
        }

        slot card_field {
            base(t) {
                width: Length::Percent(50.0),
                padding_right: t.spacing.sm(),
                gap: t.spacing.xs(),
                min_width: 0.0,
            }
        }

        slot card_label {
            base(t) {
                font_size: 11.0,
                font_weight: FontWeight::SemiBold,
                letter_spacing: 0.6,
                text_transform: TextTransform::Uppercase,
                color: t.color.text_muted(),
            }
        }

        /// The "+N more" control under the cells — opens the row's detail sheet.
        /// A row, so the hugging button sits at the card's leading edge (a
        /// non-block button centres itself on a column's cross axis).
        slot card_more {
            base(_t) {
                flex_direction: FlexDirection::Row,
                justify_content: JustifyContent::FlexStart,
            }
        }

        slot cards_empty {
            base(t) {
                padding_vertical: t.spacing.xl(),
                padding_horizontal: t.spacing.lg(),
                align_items: AlignItems::Center,
            }
        }

        /// One labelled cell in a card's detail sheet.
        slot detail_row {
            base(t) {
                gap: t.spacing.xs(),
                padding_vertical: t.spacing.sm(),
                border_bottom_width: 1.0,
                border_bottom_color: t.color.border(),
            }
        }
    }
}

