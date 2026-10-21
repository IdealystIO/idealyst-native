//! [`TablePager`] — the footer a paged table pages with: "26–50 of 312
//! projects" at the leading end, then "Rows [25 ⌄]" and the page numbers
//! at the trailing end. Put it in the table's footer slot
//! (`Table(footer_slot = Some(ui! { TablePager(…) }))`): it sits inside
//! the table's frame, outside its scroller — pinned under the rows of a
//! `scroll_y` table — and takes the table's edge inset, so it lines up
//! with the first and last columns.

use std::rc::Rc;

use runtime_core::{component, rx, signal, ui, Element, IdealystSchema, Length, Reactive, StyleRules};

use crate::components::pagination::Pagination;
use crate::components::select::{Select, SelectOption, SelectSize, SelectSlotStyles};
use crate::components::typography::Typography;
use crate::stylesheets::{TablePagerBar, TablePagerGroup};

/// The "Rows" picker's width floor: a three-digit page size and its
/// chevron, and no wider. It replaces the trigger's own `min_width` through
/// the trigger slot — a narrower box AROUND the Select can't shrink it,
/// because a `min_width` beats its container's width, and the trigger then
/// spills over the page numbers beside it.
const ROWS_PICKER_MIN_WIDTH: f32 = 84.0;
use crate::typography_kind;

/// "26–50": the window's first and last row, 1-based; "0" for an empty
/// set.
pub fn page_window_text(offset: usize, page_size: usize, total: usize) -> String {
    if total == 0 {
        return "0".to_string();
    }
    let first = offset.min(total.saturating_sub(1)) + 1;
    let last = (offset + page_size.max(1)).min(total);
    format!("{first}\u{2013}{last}")
}

/// Props for [`TablePager`].
#[runtime_core::props]
#[derive(IdealystSchema)]
pub struct TablePagerProps {
    /// The window's offset into the whole set — the index of its first
    /// row. Reactive.
    pub offset: usize,
    /// Rows per page. Reactive.
    pub page_size: usize,
    /// Rows across every page. Reactive.
    pub total: usize,
    /// What a row is, plural — "projects".
    #[prop(static)]
    pub noun: String,
    /// Fired with the offset of the page the reader picked.
    pub on_offset: Rc<dyn Fn(usize)>,
    /// The page sizes the "Rows" picker offers. Empty (the default)
    /// leaves the picker out.
    #[prop(static)]
    pub page_sizes: Vec<usize>,
    /// Fired with a newly picked page size. The host usually rewinds to
    /// the first page too.
    pub on_page_size: Option<Rc<dyn Fn(usize)>>,
}

impl Default for TablePagerProps {
    fn default() -> Self {
        Self {
            offset: Reactive::Static(0),
            page_size: Reactive::Static(25),
            total: Reactive::Static(0),
            noun: String::new(),
            on_offset: Rc::new(|_| {}),
            page_sizes: Vec::new(),
            on_page_size: None,
        }
    }
}

/// A paged table's footer — see the module docs.
///
/// ```ignore
/// Table(scroll_y = true, footer_slot = Some(ui! {
///     TablePager(
///         offset = offset, page_size = size, total = total,
///         noun = "projects".to_string(),
///         on_offset = Rc::new(move |o| offset.set(o)),
///         page_sizes = vec![10, 25, 50, 100],
///         on_page_size = Some(Rc::new(move |n| { size.set(n); offset.set(0) })),
///     )
/// })) { … }
/// ```
#[component]
pub fn TablePager(props: &TablePagerProps) -> Element {
    let (offset, page_size, total) = (props.offset.clone(), props.page_size.clone(), props.total.clone());
    let noun = props.noun.clone();
    let on_offset = props.on_offset.clone();
    let sizes = props.page_sizes.clone();
    let on_page_size = props.on_page_size.clone();
    let size_options: Vec<SelectOption> =
        sizes.iter().map(|n| SelectOption::new(n.to_string(), n.to_string())).collect();
    // The picker's own string signal, seeded from the page size; a pick
    // reports through `on_page_size`, and the picker follows the page
    // size the host then passes back.
    let size_text = signal(page_size.get().to_string());
    {
        let page_size = page_size.clone();
        let _ = runtime_core::effect(move || size_text.set(page_size.get().to_string()));
    }
    let on_size_text: Rc<dyn Fn(String)> = Rc::new(move |v: String| {
        if let (Ok(n), Some(cb)) = (v.parse::<usize>(), &on_page_size) {
            cb(n);
        }
    });
    let range = {
        let (offset, page_size, total) = (offset.clone(), page_size.clone(), total.clone());
        rx!(page_window_text(offset.get(), page_size.get(), total.get()))
    };
    let of_total = {
        let total = total.clone();
        rx!(format!("of {} {}", total.get(), noun))
    };
    // (pages, current page, page size): the page numbers take a static
    // page count, so they rebuild when any of the three moves.
    let pages_key = move || {
        let size = page_size.get().max(1);
        let pages = total.get().div_ceil(size).max(1);
        let current = (offset.get() / size + 1).min(pages);
        (pages, current, size)
    };
    ui! {
        view(style = TablePagerBar()) {
            view(style = TablePagerGroup()) {
                Typography(content = range, kind = typography_kind::BodySm)
                Typography(content = of_total, kind = typography_kind::BodySm, muted = true)
            }
            view(style = TablePagerGroup()) {
                if !sizes.is_empty() {
                    view(style = TablePagerGroup()) {
                        Typography(content = "Rows".to_string(), kind = typography_kind::BodySm, muted = true)
                        Select(
                            value = size_text,
                            on_change = on_size_text.clone(),
                            options = size_options.clone(),
                            size = SelectSize::Sm,
                            styles = SelectSlotStyles::default().trigger(StyleRules {
                                min_width: Some(Length::Px(ROWS_PICKER_MIN_WIDTH).into()),
                                ..Default::default()
                            }),
                        )
                    }
                }
                match pages_key() {
                    (pages, current, size) => {
                        Pagination(
                            page = signal(*current),
                            total = *pages,
                            on_change = {
                                let on_offset = on_offset.clone();
                                let size = *size;
                                Rc::new(move |p: usize| on_offset((p.max(1) - 1) * size))
                                    as Rc<dyn Fn(usize)>
                            },
                        )
                    }
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::page_window_text;

    #[test]
    fn the_window_is_one_based_and_stops_at_the_total() {
        assert_eq!(page_window_text(25, 25, 312), "26\u{2013}50");
        assert_eq!(page_window_text(300, 25, 312), "301\u{2013}312");
        assert_eq!(page_window_text(0, 25, 0), "0");
        assert_eq!(page_window_text(0, 0, 3), "1\u{2013}1", "a zero page size reads as one row");
    }
}
