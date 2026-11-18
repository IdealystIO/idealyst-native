//! One screen per layout.

mod content;
mod flow;
mod overlays;
mod pages;
mod shell_tabs;
mod splits;

pub(crate) use content::ContentScreen;
pub(crate) use flow::FlowScreen;
pub(crate) use overlays::OverlaysScreen;
pub(crate) use pages::{BlankScreen, TableScreen};
pub(crate) use shell_tabs::ShellTabsScreen;
pub(crate) use splits::{SplitDetailScreen, SplitNavScreen, WorkbenchScreen};

use std::rc::Rc;

use idea_ui::{typography_kind, Card, Stack, StackGap, Typography};
use runtime_core::{component, ui, Element};

pub(crate) fn noop() -> Rc<dyn Fn()> {
    Rc::new(|| {})
}

/// One figure of a `StatGrid`.
#[component]
pub(crate) fn Stat(label: String, value: String) -> Element {
    ui! {
        Card {
            Stack(gap = StackGap::Xs) {
                Typography(content = label, kind = typography_kind::BodySm, muted = true)
                Typography(content = value, kind = typography_kind::H2)
            }
        }
    }
}
