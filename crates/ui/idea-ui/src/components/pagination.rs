//! `Pagination` — a windowed page selector (‹ 1 … 4 5 6 … 20 ›).
//!
//! ```ignore
//! let page = signal!(1usize);
//! ui! {
//!     Pagination(
//!         page = page,
//!         total = 20,
//!         on_change = move |p: usize| page.set(p),
//!     )
//! }
//! ```
//!
//! Controlled — `page` (1-based) is the source of truth, `on_change`
//! fires with the requested page. For large `total` the middle
//! collapses to ellipses around the current page; first and last are
//! always shown.

use std::rc::Rc;

use runtime_core::{component, ui, IntoElement, Element, Signal, StyleApplication};

use crate::stylesheets::{PageButton, PaginationRow};

/// Pages shown without collapsing. Above this, the middle ellipsizes.
const WINDOW_FULL: usize = 7;

#[cfg_attr(feature = "docs", derive(idea_ui::doc_controls::DocControls))]
pub struct PaginationProps {
    /// Current page, 1-based. The host owns the signal.
    pub page: Signal<usize>,
    /// Total number of pages (>= 1).
    pub total: usize,
    /// Fires with the requested page when the user navigates.
    pub on_change: Rc<dyn Fn(usize)>,
}

impl Default for PaginationProps {
    fn default() -> Self {
        Self { page: Signal::new(1), total: 1, on_change: Rc::new(|_| {}) }
    }
}

/// The page cells to render: `Some(n)` = a page button, `None` = an
/// ellipsis gap. First + last always present; a window surrounds
/// `current`.
fn cells(current: usize, total: usize) -> Vec<Option<usize>> {
    if total <= WINDOW_FULL {
        return (1..=total).map(Some).collect();
    }
    let mut pages: Vec<usize> = Vec::new();
    let lo = current.saturating_sub(1).max(2);
    let hi = (current + 1).min(total - 1);
    pages.push(1);
    for p in lo..=hi {
        pages.push(p);
    }
    pages.push(total);
    pages.dedup();

    // Insert ellipsis markers where there's a numeric gap > 1.
    let mut out: Vec<Option<usize>> = Vec::with_capacity(pages.len() + 2);
    for (i, &p) in pages.iter().enumerate() {
        if i > 0 && p - pages[i - 1] > 1 {
            out.push(None);
        }
        out.push(Some(p));
    }
    out
}

fn nav_button(glyph: &str, target: Option<usize>, on_change: Rc<dyn Fn(usize)>) -> Element {
    let disabled = target.is_none();
    let label = runtime_core::text(glyph.to_string()).into_element();
    let mut b = runtime_core::pressable(vec![label], move || {
        if let Some(t) = target {
            (on_change)(t);
        }
    })
    .with_style(|| StyleApplication::new(PageButton::sheet()));
    if disabled {
        b = b.disabled(move || true);
    }
    b.into_element()
}

#[component]
pub fn Pagination(props: PaginationProps) -> Element {
    let page = props.page;
    let total = props.total.max(1);
    let on_change = props.on_change.clone();
    let current = page.get();

    let mut kids: Vec<Element> = Vec::new();

    // Prev.
    let prev_target = if current > 1 { Some(current - 1) } else { None };
    kids.push(nav_button("\u{2039}", prev_target, on_change.clone()));

    // Page cells.
    for cell in cells(current, total) {
        match cell {
            Some(n) => {
                let on_change_for = on_change.clone();
                let label = runtime_core::text(n.to_string()).into_element();
                let row = runtime_core::pressable(vec![label], move || (on_change_for)(n))
                    .with_style(move || {
                        StyleApplication::new(PageButton::sheet())
                            .with("active", if page.get() == n { "on" } else { "off" }.to_string())
                    })
                    .into_element();
                kids.push(row);
            }
            None => {
                kids.push(
                    runtime_core::text("\u{2026}".to_string())
                        .with_style(|| StyleApplication::new(PageButton::sheet()))
                        .into_element(),
                );
            }
        }
    }

    // Next.
    let next_target = if current < total { Some(current + 1) } else { None };
    kids.push(nav_button("\u{203A}", next_target, on_change));

    ui! { view(style = PaginationRow()) { kids } }
}

#[cfg(test)]
mod tests {
    use super::cells;

    #[test]
    fn small_total_shows_every_page_no_ellipsis() {
        assert_eq!(
            cells(1, 5),
            vec![Some(1), Some(2), Some(3), Some(4), Some(5)]
        );
        assert!(cells(3, 7).iter().all(|c| c.is_some()));
    }

    #[test]
    fn first_and_last_always_present() {
        let c = cells(10, 20);
        assert_eq!(c.first(), Some(&Some(1)));
        assert_eq!(c.last(), Some(&Some(20)));
        assert!(c.contains(&Some(10)), "current page present");
    }

    #[test]
    fn middle_collapses_to_ellipses_around_current() {
        // current=10/20 → 1 … 9 10 11 … 20
        assert_eq!(
            cells(10, 20),
            vec![Some(1), None, Some(9), Some(10), Some(11), None, Some(20)]
        );
    }

    #[test]
    fn near_start_has_only_trailing_ellipsis() {
        // current=2/20 → 1 2 3 … 20
        assert_eq!(cells(2, 20), vec![Some(1), Some(2), Some(3), None, Some(20)]);
    }

    #[test]
    fn near_end_has_only_leading_ellipsis() {
        // current=19/20 → 1 … 18 19 20
        assert_eq!(cells(19, 20), vec![Some(1), None, Some(18), Some(19), Some(20)]);
    }
}
