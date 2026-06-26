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

use runtime_core::{
    component, ui, IdealystSchema, IntoElement, Element, Reactive, Signal, StyleApplication,
};

use crate::stylesheets::{PageButton, PaginationRow};

/// Pages shown without collapsing. Above this, the middle ellipsizes.
const WINDOW_FULL: usize = 7;

// Reactive-by-default: `#[props]` wraps the scalar `total` → `Reactive<usize>`;
// `page` (`Signal`) and `on_change` (Rc handler) are auto-skipped. `total`
// drives the windowed cell STRUCTURE (built imperatively in `build_row`); it's
// snapshotted at build — see the TODO in the body.
#[runtime_core::props]
#[cfg_attr(feature = "docs", derive(idea_ui::doc_controls::DocControls))]
#[derive(IdealystSchema)]
pub struct PaginationProps {
    /// Current page, 1-based. The host owns the signal.
    pub page: Signal<usize>,
    /// Total number of pages (>= 1).
    #[schema(constraint = ">= 1 (clamped)")]
    pub total: usize,
    /// Fires with the requested page when the user navigates.
    pub on_change: Rc<dyn Fn(usize)>,
}

impl Default for PaginationProps {
    fn default() -> Self {
        Self { page: Signal::new(1), total: Reactive::Static(1), on_change: Rc::new(|_| {}) }
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

/// Renders prev/next chevrons around a windowed run of page buttons,
/// collapsing the middle to ellipses for large `total`. The button
/// matching `page` is marked active; navigation fires `on_change`.
///
/// The whole row is rebuilt through [`switch`](runtime_core::switch) keyed
/// on `page`. A `#[component]` body builds **once**, so computing the
/// prev/next targets and the windowed cell list from a single `page.get()`
/// snapshot froze the arrows (they always fired the initial ±1 and the
/// window never slid — only the fine-grained active-highlight updated).
/// `switch` re-runs the builder with the live page on every change, so the
/// targets, the sliding window, and the highlight all stay correct.
#[component]
pub fn Pagination(props: PaginationProps) -> Element {
    let page = props.page;
    // TODO(reactive-sweep): route `total` reactively. It drives the windowed
    // cell STRUCTURE (the count + which cells ellipsize, rebuilt in
    // `build_row`). The row is already `switch`-keyed on `page`, not `total`, so
    // a live `total` signal won't slide the window in place; switching on
    // `(page, total)` is the fix. Snapshotted at build for now.
    let total = props.total.get().max(1);
    let on_change = props.on_change.clone();
    runtime_core::switch(
        move || page.get(),
        move |current| build_row(*current, total, on_change.clone()),
    )
}

/// Build the pagination row for a concrete `current` page. Called fresh by
/// `switch` on every page change, so all page-derived values (nav targets,
/// the windowed cells, the active mark) are computed from the live page.
fn build_row(current: usize, total: usize, on_change: Rc<dyn Fn(usize)>) -> Element {
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
                // `active` is baked from `current` (this row is rebuilt per
                // page change), not read reactively — no per-cell closure.
                let active = if current == n { "on" } else { "off" }.to_string();
                let row = runtime_core::pressable(vec![label], move || (on_change_for)(n))
                    .with_style(move || {
                        StyleApplication::new(PageButton::sheet()).with("active", active.clone())
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
    use super::*;
    use super::cells;

    /// REGRESSION: the row must be REACTIVE on `page` so the prev/next arrows
    /// (and the windowed cells) recompute as the page changes. The original
    /// built the row once in the component body, freezing the nav targets —
    /// `›` advanced one page then stuck. A reactive `switch` fixes it, so
    /// `Pagination` must build an `Element::Switch`, not a static `View`.
    #[test]
    fn pagination_is_reactive_switch_not_static_row() {
        let el = Pagination(PaginationProps {
            page: Signal::new(3),
            total: Reactive::Static(20),
            on_change: Rc::new(|_| {}),
        });
        assert!(
            matches!(el, Element::Switch { .. }),
            "Pagination must rebuild reactively via switch (else the arrows freeze)",
        );
    }

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
