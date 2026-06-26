//! Page bodies, one module per design group. Each `pub fn name() ->
//! Element` returns the page **body only** — a column of demo
//! `Section`s. The central frame in `lib.rs` adds the group overline,
//! title, status badge, lead, and the `Usage` panel from the catalog,
//! so bodies never render their own title/lead/scroll wrapper.

use runtime_core::{ui, Element};
use idea_ui::{Stack, StackGap};

pub mod overview;
pub mod foundations;
pub mod primitives;
pub mod layout;
pub mod status;
pub mod actions;
pub mod forms;
pub mod overlays;
pub mod navigation;
pub mod data;

/// Wrap a page body's sections in the standard vertical rhythm.
pub fn body(children: Vec<Element>) -> Element {
    ui! {
        Stack(gap = StackGap::Xl) { children }
    }
}

/// Placeholder body for a reference page still being assembled —
/// mirrors the design's `__fallback`. Kept as the fallback for any new
/// catalog entry whose page isn't written yet.
#[allow(dead_code)]
pub fn placeholder() -> Element {
    use crate::shell::{DemoSurface, P};
    ui! {
        Stack(gap = StackGap::Md) {
            DemoSurface {
                P(content = "Live preview being assembled.".to_string())
            }
        }
    }
}
