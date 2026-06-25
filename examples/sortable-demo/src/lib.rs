//! `sortable-demo` — a live-reorder list on the `dnd` SDK.
//!
//! Drag a row up or down. The other rows slide aside to open a gap exactly where
//! it would land, and **on drop nothing jumps** — the order is already what you
//! see.
//!
//! ## The key idea: position by transform, never reorder the DOM
//!
//! A sortable that *reorders the DOM* on drop has to hand off from "rows shifted
//! by a translate" to "rows in new layout slots" — and if those two don't land
//! in the exact same frame, the row flickers (jump, then springs back). So this
//! demo never reorders the DOM:
//!
//! - The rows are rendered **once in a fixed order** (keyed by id) and never
//!   reconciled-reordered.
//! - A row's vertical position is **purely a transform**: `translateY = its
//!   index in the logical `order` signal × ROW_H`, springing on change.
//! - Reordering just rewrites `order` → indices change → transforms spring.
//!   There is no layout reorder to hand off to, so a commit moves nothing.
//!
//! During a drag the *displayed* order is `order` with the dragged id moved to
//! the hovered slot; on drop that displayed order simply becomes `order`.
//!
//! ```text
//! idealyst dev --web
//! idealyst dev --macos --local
//! ```

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

/// Row height + gap = the per-slot vertical stride (each row is `ROW_BODY` tall
/// with a `GAP` below it).
const ROW_BODY: f32 = 44.0;
const GAP: f32 = 10.0;
const ROW_H: f32 = ROW_BODY + GAP;

#[derive(Clone, Copy, Default)]
struct Item {
    id: u32,
    title: &'static str,
}

const ITEMS: [Item; 5] = [
    Item { id: 1, title: "Design the drag layer" },
    Item { id: 2, title: "Fix macOS hit-testing" },
    Item { id: 3, title: "Cache pointer rects" },
    Item { id: 4, title: "Live-reorder preview" },
    Item { id: 5, title: "Ship the SDK" },
];

/// Shared sortable state. `order` is the logical order of ids; rows position
/// themselves by their index in it. `Default` is required by the `#[component]`
/// props derive; the real value is always passed explicitly.
#[derive(Clone, Default)]
struct Sort {
    ctx: DragContext<u32>,
    order: Signal<Vec<u32>>,
    /// While dragging: the id being dragged, and the slot it would land in.
    dragging: Signal<Option<u32>>,
    over: Signal<Option<usize>>,
}

/// The order to *render* right now: the committed `order`, but while a drag is
/// in flight with a hovered slot, the dragged id is moved to that slot. On drop
/// this becomes the committed order — so nothing shifts.
fn displayed_order(s: &Sort) -> Vec<u32> {
    let order = s.order.get();
    match (s.dragging.get(), s.over.get()) {
        (Some(d), Some(o)) => array_move(order, d, o),
        _ => order,
    }
}

/// `order` with `id` moved to slot `to` (standard array-move).
fn array_move(mut v: Vec<u32>, id: u32, to: usize) -> Vec<u32> {
    if let Some(from) = v.iter().position(|x| *x == id) {
        let item = v.remove(from);
        v.insert(to.min(v.len()), item);
    }
    v
}

/// Index of `id` in the committed order.
fn slot_in_order(order: Signal<Vec<u32>>, id: u32) -> usize {
    order.get().iter().position(|x| *x == id).unwrap_or(0)
}

pub fn app() -> Element {
    install_theme(EmptyTheme);
    ui! { SortableList() }
}

#[component]
fn SortableList() -> Element {
    let sort = Sort {
        ctx: DragContext::new(),
        order: signal!(ITEMS.iter().map(|i| i.id).collect::<Vec<_>>()),
        dragging: signal!(None),
        over: signal!(None),
    };

    // The list container is the positioned ancestor for the absolutely-placed
    // rows, and a drop zone for "below the last row" (→ slot = len).
    let list_ref: Ref<ViewHandle> = Ref::new();
    let tail_over = sort.over;
    let tail = Droppable::new(&sort.ctx).on_enter(move |_| tail_over.set(Some(ITEMS.len())));
    tail.bind(list_ref);

    // Rows render ONCE in a fixed order (static `for` over a plain array) — the
    // DOM order never changes; only transforms do.
    let rows_sort = sort.clone();
    let drag_layer = dnd::drag_layer(&sort.ctx);
    ui! {
        view(style = PageStyle()) {
            text(style = Heading()) { "Sortable list" }
            text(style = Caption()) { "Drag a row — the others slide; on drop nothing jumps." }
            view(style = ListStyle()) {
                for item in ITEMS {
                    SortableRow(sort = rows_sort.clone(), item = item)
                }
            }
            .bind(list_ref)
            drag_layer
        }
    }
}

#[derive(Default)]
struct SortableRowProps {
    sort: Sort,
    item: Item,
}

#[component]
fn SortableRow(props: &SortableRowProps) -> Element {
    let sort = props.sort.clone();
    let id = props.item.id;
    let title = props.item.title;

    let row_ref: Ref<ViewHandle> = Ref::new();
    let ty = AnimatedValue::new(slot_in_order(sort.order, id) as f32 * ROW_H);
    let opacity = AnimatedValue::new(1.0);
    ty.bind(row_ref, AnimProp::TranslateY);
    opacity.bind(row_ref, AnimProp::Opacity);

    // Position = the row's slot in the DISPLAYED order × ROW_H, springing on
    // change. The dragged row is hidden (its ghost flies in the drag layer).
    // Because this is the ONLY thing that moves a row, a commit (order == the
    // displayed order it already shows) springs to the same slot → no motion.
    let e_sort = sort.clone();
    let e_ty = ty.clone();
    let e_op = opacity.clone();
    effect!({
        let disp = displayed_order(&e_sort);
        let slot = disp.iter().position(|x| *x == id).unwrap_or(0) as f32;
        let dragged = e_sort.dragging.get() == Some(id);
        e_op.set(if dragged { 0.0 } else { 1.0 });
        e_ty.animate(SpringTo::new(slot * ROW_H).stiffness(460.0).damping(40.0));
    });

    // Lift: hide this row + mark its starting slot.
    let s_start = sort.clone();
    let s_rel = sort.clone();
    let drag = Draggable::new(&sort.ctx, move || id)
        .activation(Activation::platform_default())
        .preview(move || ghost_view(title))
        .on_start(move || {
            s_start.dragging.set(Some(id));
            s_start.over.set(Some(slot_in_order(s_start.order, id)));
        })
        .on_release(move |_| {
            // Commit the displayed order, then clear — the displayed order is
            // unchanged by this (dragging→None gives back `order`), so nothing
            // springs. Only the dragged row fades back in at its slot.
            let committed = displayed_order(&s_rel);
            s_rel.order.set(committed);
            s_rel.dragging.set(None);
            s_rel.over.set(None);
        });
    let handler = drag.handler();

    // Detect: hovering this row sets the would-be slot to this row's index.
    let d_over = sort.over;
    let d_order = sort.order;
    let drop = Droppable::new(&sort.ctx).on_enter(move |_| {
        d_over.set(Some(slot_in_order(d_order, id)));
    });
    drop.bind(row_ref);

    view(vec![text(title).with_style(RowTitle()).into()])
        .with_style(RowStyle())
        .on_touch(move |ev| handler(ev))
        .bind(row_ref)
        .into_element()
}

/// The dragged-row ghost shown in the drag layer.
fn ghost_view(title: &str) -> Element {
    view(vec![text(title.to_string()).with_style(RowTitle()).into()])
        .with_style(GhostStyle())
        .into_element()
}

// ---------------------------------------------------------------------------
// Styles
// ---------------------------------------------------------------------------

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

// Positioned ancestor for the absolute rows; tall enough for every slot + a
// little tail room so a drop below the last row is reachable.
stylesheet! {
    ListStyle<()> {
        base(_t) {
            position: Position::Relative,
            width: Length::Px(320.0),
            height: Length::Px(324.0),
        }
    }
}

// Each row is absolutely positioned at the container's top-left; its slot comes
// entirely from the bound `translateY`.
stylesheet! {
    RowStyle<()> {
        base(_t) {
            position: Position::Absolute,
            top: Length::Px(0.0),
            left: Length::Px(0.0),
            width: Length::Percent(100.0),
            height: Length::Px(ROW_BODY),
            background: Color("#334155".into()),
            padding: 12.0,
            border_radius: 10.0,
            justify_content: JustifyContent::Center,
        }
    }
}

stylesheet! {
    RowTitle<()> {
        base(_t) { color: Color("#f1f5f9".into()), font_size: 15.0 }
    }
}

stylesheet! {
    GhostStyle<()> {
        base(_t) {
            width: Length::Px(320.0),
            height: Length::Px(ROW_BODY),
            background: Color("#3b82f6".into()),
            padding: 12.0,
            border_radius: 10.0,
            justify_content: JustifyContent::Center,
            border_width: 2.0,
            border_color: Color("#93c5fd".into()),
        }
    }
}
