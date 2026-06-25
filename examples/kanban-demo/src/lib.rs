//! `kanban-demo` — a smooth Kanban board on the `dnd` SDK.
//!
//! Drag a card within a column to reorder, or to another column. The other
//! cards slide to open a gap, and **on drop nothing jumps**.
//!
//! ## Why it's smooth: transform-only, never reorder the DOM
//!
//! The naive sortable reorders the DOM on drop, then has to hand off from
//! "cards shifted by a translate" to "cards in new layout slots" — two things
//! that never land in the same frame, so a card flickers (jump, then springs
//! back). react-beautiful-dnd / dnd-kit avoid this by never reordering the DOM:
//! every card is positioned purely by `transform`.
//!
//! This demo does the same. ALL cards live in one fixed-order absolute layer.
//! A card's position is `translate(column_x, header + slot × ROW)` where its
//! `(column, slot)` comes from a logical `order` state. Reordering just rewrites
//! that state → indices change → transforms spring. There is no layout reorder
//! to hand off to, so a commit moves nothing — the dragged card just settles
//! into the gap that's already open.

use dnd::{Activation, DragContext, Draggable, Droppable};
use idea_theme::{install_theme, ThemeTokens};
use runtime_core::animation::{AnimProp, AnimatedValue, SpringTo};
use runtime_core::{
    component, effect, signal, stylesheet, text, ui, view, AlignItems, Color, Element,
    FlexDirection, IntoElement, JustifyContent, Length, Position, Ref, Signal, TokenEntry,
    ViewHandle,
};

pub fn register_extensions<B: runtime_core::Backend>(_backend: &mut B) {}

#[cfg(feature = "sidecar")]
pub fn register_extensions_recorder(_backend: &mut dev_server::WireRecordingBackend) {}

struct EmptyTheme;
impl ThemeTokens for EmptyTheme {
    fn tokens(&self) -> Vec<TokenEntry> {
        Vec::new()
    }
}

// --- geometry (all cards are absolutely placed; these define the grid) -------
const COL_W: f32 = 240.0;
const COL_GAP: f32 = 18.0;
const COL_PAD: f32 = 12.0;
const CARD_H: f32 = 44.0;
const CARD_GAP: f32 = 10.0;
const ROW: f32 = CARD_H + CARD_GAP; // vertical stride per slot
const HEADER_H: f32 = 48.0; // space above the first card for the column title
const BOARD_H: f32 = 420.0;
const COL_TITLES: [&str; 3] = ["To Do", "In Progress", "Done"];

fn col_x(c: usize) -> f32 {
    c as f32 * (COL_W + COL_GAP) + COL_PAD
}
fn slot_y(slot: usize) -> f32 {
    HEADER_H + slot as f32 * ROW
}

#[derive(Clone, Copy, Default)]
struct Card {
    id: u32,
    title: &'static str,
}

const CARDS: [Card; 5] = [
    Card { id: 1, title: "Design the drag layer" },
    Card { id: 2, title: "Fix macOS hit-testing" },
    Card { id: 3, title: "Cache pointer rects" },
    Card { id: 4, title: "Animate the columns" },
    Card { id: 5, title: "Ship the SDK" },
];

/// Shared board state. `cols` is the logical id-order per column; cards position
/// themselves by their `(column, slot)` in it. `Default` is required by the
/// `#[component]` props derive; the real value is always passed explicitly.
#[derive(Clone, Default)]
struct Board {
    ctx: DragContext<u32>,
    cols: [Signal<Vec<u32>>; 3],
    /// The dragged card id, and the slot it would land in `(column, slot)`.
    dragging: Signal<Option<u32>>,
    over: Signal<Option<(usize, usize)>>,
}

/// The columns to *render* right now: committed `cols`, but while dragging with
/// a hovered slot the dragged id is moved there. On drop this becomes `cols`, so
/// nothing shifts.
fn displayed_cols(b: &Board) -> [Vec<u32>; 3] {
    let mut cols = [b.cols[0].get(), b.cols[1].get(), b.cols[2].get()];
    if let (Some(d), Some((oc, os))) = (b.dragging.get(), b.over.get()) {
        for c in cols.iter_mut() {
            c.retain(|x| *x != d);
        }
        let os = os.min(cols[oc].len());
        cols[oc].insert(os, d);
    }
    cols
}

/// The `(column, slot)` of card `id` in the displayed order.
fn card_pos(b: &Board, id: u32) -> (usize, usize) {
    let cols = displayed_cols(b);
    for (c, col) in cols.iter().enumerate() {
        if let Some(slot) = col.iter().position(|x| *x == id) {
            return (c, slot);
        }
    }
    (0, 0)
}

/// The committed `(column, slot)` of card `id`.
fn committed_pos(b: &Board, id: u32) -> (usize, usize) {
    for (c, col) in b.cols.iter().enumerate() {
        if let Some(slot) = col.get().iter().position(|x| *x == id) {
            return (c, slot);
        }
    }
    (0, 0)
}

pub fn app() -> Element {
    install_theme(EmptyTheme);
    ui! { KanbanBoard() }
}

#[component]
fn KanbanBoard() -> Element {
    let board = Board {
        ctx: DragContext::new(),
        cols: [
            signal!(vec![1u32, 2, 3]),
            signal!(vec![4u32]),
            signal!(vec![5u32]),
        ],
        dragging: signal!(None),
        over: signal!(None),
    };

    let cards_board = board.clone();
    let cols_board = board.clone();
    let drag_layer = dnd::drag_layer(&board.ctx);
    ui! {
        view(style = PageStyle()) {
            text(style = Heading()) { "Kanban board" }
            text(style = Caption()) { "Drag within a column to reorder, or to another column — nothing jumps." }
            // Horizontal scroller: the board is a fixed 756 px wide; on a narrow
            // screen (iOS) it overflows the full-width scroll view, which scrolls
            // left-right. `drag_layer` stays OUTSIDE the scroller — it's a
            // top-level window-space overlay, not board content.
            scroll_view(style = ScrollerStyle(), horizontal = true) {
                // Cards are DIRECT children of the sized board view. Don't wrap
                // them in an intermediate view: they're `position:absolute` (out
                // of flow), so a wrapper sizes to 0×0 and — though macOS still
                // renders them (no clipping) — AppKit `hitTest:` respects the
                // 0×0 bounds and never descends into them, so you can't grab a
                // card. They must live under the explicitly-sized `BoardStyle`.
                view(style = BoardStyle()) {
                    // Column backgrounds (static) — the cards float over them.
                    for c in 0..3usize {
                        ColumnBg(board = cols_board.clone(), col = c)
                    }
                    // One fixed-order absolute card layer, positioned by transform.
                    for card in CARDS {
                        CardView(board = cards_board.clone(), card = card)
                    }
                }
            }
            drag_layer
        }
    }
}

#[derive(Default)]
struct ColumnBgProps {
    board: Board,
    col: usize,
}

#[component]
fn ColumnBg(props: &ColumnBgProps) -> Element {
    let board = props.board.clone();
    let col = props.col;

    // The column tints when the pointer is over it.
    let bg_ref: Ref<ViewHandle> = Ref::new();
    let bg = AnimatedValue::new(COL_BG);
    bg.bind_color(bg_ref, AnimProp::BackgroundColor);
    let over = board.over;
    effect!({
        let hot = matches!(over.get(), Some((c, _)) if c == col);
        bg.set(if hot { COL_BG_OVER } else { COL_BG });
    });

    // The whole column is a drop zone for "below the last card" → slot = len.
    let drop_board = board.clone();
    let drop = Droppable::new(&board.ctx).on_enter(move |_| {
        let len = drop_board.cols[col].get().len();
        drop_board.over.set(Some((col, len)));
    });
    drop.bind(bg_ref);

    view(vec![text(COL_TITLES[col]).with_style(ColHeader()).into()])
        .with_style(column_box_style(col))
        .bind(bg_ref)
        .into_element()
}

#[derive(Default)]
struct CardViewProps {
    board: Board,
    card: Card,
}

#[component]
fn CardView(props: &CardViewProps) -> Element {
    let board = props.board.clone();
    let id = props.card.id;
    let title = props.card.title;

    let card_ref: Ref<ViewHandle> = Ref::new();
    let (c0, s0) = committed_pos(&board, id);
    let tx = AnimatedValue::new(col_x(c0));
    let ty = AnimatedValue::new(slot_y(s0));
    let opacity = AnimatedValue::new(1.0);
    tx.bind(card_ref, AnimProp::TranslateX);
    ty.bind(card_ref, AnimProp::TranslateY);
    opacity.bind(card_ref, AnimProp::Opacity);

    // Position = the card's (column, slot) in the DISPLAYED order, springing on
    // change. The OTHER cards slide to open a gap as `over` moves; the DRAGGED
    // card is hidden (its ghost flies in the drag layer) and is NOT animated
    // here — animating a hidden card just leaves its transform at some arbitrary
    // mid-spring point, so revealing it on release would jerk from there. Its
    // reveal + drop spring is driven by `on_release` below instead.
    let e_board = board.clone();
    let e_tx = tx.clone();
    let e_ty = ty.clone();
    let e_op = opacity.clone();
    effect!({
        let (c, s) = card_pos(&e_board, id);
        let dragged = e_board.dragging.get() == Some(id);
        if dragged {
            e_op.set(0.0);
        } else {
            e_op.set(1.0);
            e_tx.animate(SpringTo::new(col_x(c)).stiffness(520.0).damping(40.0));
            e_ty.animate(SpringTo::new(slot_y(s)).stiffness(520.0).damping(40.0));
        }
    });

    let s_start = board.clone();
    let s_rel = board.clone();
    let r_tx = tx.clone();
    let r_ty = ty.clone();
    let r_ctx = board.ctx.clone();
    let r_card_ref = card_ref;
    let drag = Draggable::new(&board.ctx, move || id)
        .activation(Activation::platform_default())
        .preview(move || ghost_view(title))
        .on_start(move || {
            s_start.dragging.set(Some(id));
            s_start.over.set(Some(committed_pos(&s_start, id)));
        })
        .on_release(move |_| {
            // Drop animation (rbd-style hand-off): reveal the hidden card at the
            // exact point the ghost was let go, then let the position effect
            // spring it into its slot. Snapping `tx`/`ty` here BEFORE clearing
            // `dragging` means the effect's `SpringTo(slot)` starts from the drop
            // point — a smooth, speed-independent glide instead of a jerk from
            // wherever a hidden spring happened to be (fast) or no motion at all
            // (slow).
            //
            // The card's translate is board-relative; the ghost position is in
            // window space. We need the board's window origin to convert. Derive
            // it from THIS card's own rect rather than a separate board ref:
            // while dragging, the hidden card's translate is frozen at its
            // pre-drag slot (the effect doesn't touch it), and `absolute_frame`
            // includes that translate on every backend — so
            // `board_origin = card.window_rect − card.translate`. (A wrapping
            // board ref view is NOT an option: the absolute cards would size it
            // to 0×0 and break macOS hit-testing.) Guarded on layout being ready.
            let (gx, gy) = r_ctx.ghost_position();
            if let Some(cf) = r_card_ref.with(|h| h.absolute_frame()).flatten() {
                let origin_x = cf.x - r_tx.get();
                let origin_y = cf.y - r_ty.get();
                r_tx.set(gx - origin_x);
                r_ty.set(gy - origin_y);
            }
            // Commit the displayed order. While `dragging` is still set the
            // effect leaves the dragged card hidden + untouched, preserving the
            // snap above; clearing `dragging` then reveals it and springs it.
            let disp = displayed_cols(&s_rel);
            for (i, col) in disp.into_iter().enumerate() {
                s_rel.cols[i].set(col);
            }
            s_rel.dragging.set(None);
            s_rel.over.set(None);
        });
    let handler = drag.handler();

    // Hovering this card sets the would-be slot to this card's (column, slot).
    let d_board = board.clone();
    let drop = Droppable::new(&board.ctx).on_enter(move |_| {
        d_board.over.set(Some(committed_pos(&d_board, id)));
    });
    drop.bind(card_ref);

    view(vec![text(title).with_style(CardTitle()).into()])
        .with_style(CardStyle())
        .on_touch(move |ev| handler(ev))
        .bind(card_ref)
        .into_element()
}

/// The dragged-card ghost shown in the drag layer.
fn ghost_view(title: &str) -> Element {
    view(vec![text(title.to_string()).with_style(CardTitle()).into()])
        .with_style(GhostStyle())
        .into_element()
}

// ---------------------------------------------------------------------------
// Colors + styles
// ---------------------------------------------------------------------------

const COL_BG: (f32, f32, f32, f32) = (0.1176, 0.1608, 0.2314, 1.0); // #1e293b
const COL_BG_OVER: (f32, f32, f32, f32) = (0.1569, 0.2392, 0.3608, 1.0); // #283d5c

stylesheet! {
    PageStyle<()> {
        base(_t) {
            flex_direction: FlexDirection::Column,
            gap: 16.0,
            padding: 28.0,
            background: Color("#0b1220".into()),
            align_items: AlignItems::FlexStart,
        }
    }
}

stylesheet! {
    Heading<()> {
        base(_t) { color: Color("#e2e8f0".into()), font_size: 24.0 }
    }
}

stylesheet! {
    Caption<()> {
        base(_t) { color: Color("#94a3b8".into()), font_size: 14.0 }
    }
}

// Full-width, board-tall viewport for the horizontal scroller. Width 100% so it
// is clamped to the screen and the wider board overflows → scrolls left-right;
// explicit height so it doesn't collapse around the absolute board content.
stylesheet! {
    ScrollerStyle<()> {
        base(_t) {
            width: Length::Percent(100.0),
            height: Length::Px(BOARD_H),
        }
    }
}

// The board is the positioned ancestor for the absolute column boxes + cards.
stylesheet! {
    BoardStyle<()> {
        base(_t) {
            position: Position::Relative,
            width: Length::Px(COL_W * 3.0 + COL_GAP * 2.0),
            height: Length::Px(BOARD_H),
        }
    }
}

// Background box for column `c`: absolute at its x, full board height.
fn column_box_style(c: usize) -> runtime_core::StyleApplication {
    use runtime_core::{StyleRules, StyleSheet, Tokenized};
    let rules = StyleRules {
        position: Some(Position::Absolute),
        top: Some(Tokenized::Literal(Length::Px(0.0))),
        left: Some(Tokenized::Literal(Length::Px(c as f32 * (COL_W + COL_GAP)))),
        width: Some(Tokenized::Literal(Length::Px(COL_W))),
        height: Some(Tokenized::Literal(Length::Px(BOARD_H))),
        // Static base fill so the column is visible from the first paint.
        // `bind_color` only applies on the next CHANGE after the view mounts
        // (the immediate apply at bind() skips because the Ref isn't filled
        // yet), so the constructed `COL_BG` would never paint on its own —
        // unlike the dnd-demo bins, these columns have no border to fall back
        // on. The animated value tints to `COL_BG_OVER` on hover over this;
        // the sheet is static (never re-rendered mid-drag) so there's no
        // dual-owner conflict with the animation.
        background: Some(Tokenized::Literal(runtime_core::Color("#1e293b".into()))),
        padding_top: Some(Tokenized::Literal(Length::Px(COL_PAD))),
        padding_left: Some(Tokenized::Literal(Length::Px(COL_PAD))),
        border_top_left_radius: Some(Tokenized::Literal(Length::Px(16.0))),
        border_top_right_radius: Some(Tokenized::Literal(Length::Px(16.0))),
        border_bottom_left_radius: Some(Tokenized::Literal(Length::Px(16.0))),
        border_bottom_right_radius: Some(Tokenized::Literal(Length::Px(16.0))),
        ..Default::default()
    };
    runtime_core::StyleApplication::new(std::rc::Rc::new(StyleSheet::r#static(rules)))
}

stylesheet! {
    ColHeader<()> {
        base(_t) { color: Color("#cbd5e1".into()), font_size: 13.0 }
    }
}

// A card: absolutely placed at the board's top-left; its (column, slot) is the
// bound translate, so it floats over the right column.
stylesheet! {
    CardStyle<()> {
        base(_t) {
            position: Position::Absolute,
            top: Length::Px(0.0),
            left: Length::Px(0.0),
            width: Length::Px(COL_W - COL_PAD * 2.0),
            height: Length::Px(CARD_H),
            background: Color("#334155".into()),
            padding: 12.0,
            border_radius: 10.0,
            // Center the title on BOTH axes. Without an explicit align_items
            // the flex default is `stretch`, which leaves the title pinned to
            // the cross-axis start instead of centered in the 44px card.
            justify_content: JustifyContent::Center,
            align_items: AlignItems::Center,
        }
    }
}

stylesheet! {
    CardTitle<()> {
        base(_t) { color: Color("#f1f5f9".into()), font_size: 15.0 }
    }
}

stylesheet! {
    GhostStyle<()> {
        base(_t) {
            width: Length::Px(COL_W - COL_PAD * 2.0),
            height: Length::Px(CARD_H),
            background: Color("#3b82f6".into()),
            padding: 12.0,
            border_radius: 10.0,
            justify_content: JustifyContent::Center,
            align_items: AlignItems::Center,
            border_width: 2.0,
            border_color: Color("#93c5fd".into()),
        }
    }
}
