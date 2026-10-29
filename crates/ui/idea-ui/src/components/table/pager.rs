//! [`TablePager`] — the footer a paged table pages with: "26–50 of 312
//! projects" at the leading end, then "Rows [25 ⌄]" and the page numbers
//! at the trailing end. When every row fits on one page the footer reads
//! as a count ("6 projects", "No projects") with no page numbers, and the
//! "Rows" picker shows only while some page size would change what is
//! shown. Put it in the table's footer slot
//! (`Table(footer_slot = Some(ui! { TablePager(…) }))`): it sits inside
//! the table's frame, outside its scroller — pinned under the rows of a
//! `scroll_y` table — and takes the table's edge inset, so it lines up
//! with the first and last columns.

use super::styles::{TableSlotStyles, TableStyles};
use std::rc::Rc;

use runtime_core::{component, rx, signal, ui, Element, IdealystSchema, Length, Reactive, StyleRules};

use crate::components::pagination::Pagination;
use crate::components::select::{Select, SelectOption, SelectSize, SelectSlotStyles};
use crate::components::typography::Typography;

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

/// The footer's count: the bold lead and the muted tail after it.
/// Several pages: `("26–50", "of 312 projects")`. One page: `("6
/// projects", "")` — a range that covers everything says nothing the total
/// doesn't. None: `("No projects", "")`, never "0 of 0".
pub fn pager_count_text(offset: usize, page_size: usize, total: usize, noun: &str) -> (String, String) {
    if total == 0 {
        return (format!("No {noun}"), String::new());
    }
    if total <= page_size.max(1) {
        return (format!("{total} {noun}"), String::new());
    }
    (page_window_text(offset, page_size, total), format!("of {total} {noun}"))
}

/// Whether the "Rows" picker can change anything: some offered page size
/// is smaller than the total, so picking it pages differently. With every
/// row fitting under the smallest size, every choice shows the same page.
pub fn rows_picker_matters(page_sizes: &[usize], total: usize) -> bool {
    page_sizes.iter().any(|&n| n < total)
}

/// Props for [`TablePager`].
#[runtime_core::props(styles = TableStyles)]
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
            styles: Default::default(),
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
    let count = {
        let (offset, page_size, total) = (offset.clone(), page_size.clone(), total.clone());
        Rc::new(move || pager_count_text(offset.get(), page_size.get(), total.get(), &noun))
    };
    let lead = {
        let count = count.clone();
        rx!(count().0)
    };
    let tail = rx!(count().1);
    let picker_matters = {
        let total = total.clone();
        move || rows_picker_matters(&sizes, total.get())
    };
    // (pages, current page, page size), or `None` when everything fits on
    // one page and there is nothing to page between. The page numbers take
    // a static page count, so they rebuild when any of the three moves.
    let pages_key = move || {
        let size = page_size.get().max(1);
        let pages = total.get().div_ceil(size);
        let current = (offset.get() / size + 1).min(pages);
        (pages > 1).then_some((pages, current, size))
    };
    let bar = TableStyles::pager_bar().overridden(props.styles.pager_bar.as_ref());
    let group_override = props.styles.pager_group.clone();
    let group = move || TableStyles::pager_group().overridden(group_override.as_ref());
    ui! {
        view(style = bar) {
            view(style = group()) {
                Typography(content = lead, kind = typography_kind::BodySm)
                Typography(content = tail, kind = typography_kind::BodySm, muted = true)
            }
            view(style = group()) {
                if picker_matters() {
                    view(style = group()) {
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
                    Some((pages, current, size)) => {
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
                    None => {}
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{page_window_text, pager_count_text, rows_picker_matters};

    #[test]
    fn the_window_is_one_based_and_stops_at_the_total() {
        assert_eq!(page_window_text(25, 25, 312), "26\u{2013}50");
        assert_eq!(page_window_text(300, 25, 312), "301\u{2013}312");
        assert_eq!(page_window_text(0, 25, 0), "0");
        assert_eq!(page_window_text(0, 0, 3), "1\u{2013}1", "a zero page size reads as one row");
    }

    #[test]
    fn the_count_reads_as_a_range_only_across_pages() {
        let t = |o, n, total| pager_count_text(o, n, total, "projects");
        assert_eq!(t(25, 25, 312), ("26\u{2013}50".into(), "of 312 projects".into()));
        assert_eq!(t(0, 20, 6), ("6 projects".into(), String::new()));
        assert_eq!(t(0, 20, 20), ("20 projects".into(), String::new()), "exactly one full page");
        assert_eq!(t(0, 20, 0), ("No projects".into(), String::new()));
    }

    #[test]
    fn the_rows_picker_matters_only_when_a_size_pages_differently() {
        assert!(!rows_picker_matters(&[20, 50], 6), "every size shows all six");
        assert!(!rows_picker_matters(&[20, 50], 20), "20 rows fit the smallest size");
        assert!(rows_picker_matters(&[20, 50], 21));
        assert!(!rows_picker_matters(&[], 500), "no sizes offered, no picker");
    }
}
