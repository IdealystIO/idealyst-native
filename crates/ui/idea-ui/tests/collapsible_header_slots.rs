//! Collapsible's header takes custom content, a trailing element and a
//! chevron side. Reported against idea-ui 8.0.0: the header was title text
//! with the chevron fixed at the end, so CrewForge hand-rolled a disclosure
//! row (chevron left, icon, name over a count, an "open in new tab" button
//! at the end).
//!
//! Real layout (`tests/common`: host-mock tree mirrored into Taffy), so the
//! assertions are about where things land, plus one structural check: the
//! trailing element is not inside the toggle's pressable, so pressing it
//! can't open or close the body.

mod common;

use std::rc::Rc;

use common::lay_out;
use idea_ui::{Collapsible, CollapsibleChevronSide};
use runtime_core::{signal, ui, Element};

const PANE_WIDTH: f32 = 480.0;
const PANE_HEIGHT: f32 = 600.0;

/// The chevron's text node. Its glyph is bound reactively, so host-mock
/// records it with empty static content; it is the toggle's only such text.
const CHEVRON: &str = "text \"\"";

fn collapsible(
    side: CollapsibleChevronSide,
    header: Option<Element>,
    trailing: Option<Element>,
) -> Element {
    let open = signal(false);
    ui! {
        Collapsible(
            title = "Plain title".to_string(),
            header = header,
            trailing = trailing,
            chevron_side = side,
            value = open,
            on_change = Rc::new(move |v: bool| open.set(v)) as Rc<dyn Fn(bool)>,
        ) {
            text { "Body".to_string() }
        }
    }
}

/// Regression: there was no way to put the chevron first.
#[test]
fn regression_collapsible_chevron_can_sit_at_the_start() {
    let laid = lay_out(PANE_WIDTH, PANE_HEIGHT, || {
        collapsible(CollapsibleChevronSide::Start, None, None)
    });
    let chevron = laid.frame(CHEVRON);
    let title = laid.frame("text \"Plain title");
    assert!(
        chevron.x + chevron.width <= title.x,
        "chevron first, then the title, packed left (chevron {chevron:?}, title {title:?}):\n{}",
        laid.h.tree(laid.root)
    );
    assert!(
        title.x < PANE_WIDTH / 2.0,
        "the title stays beside the chevron, not pushed to the far end ({title:?})"
    );
}

/// The default is unchanged: title first, chevron at the far end.
#[test]
fn collapsible_chevron_defaults_to_the_end() {
    let laid = lay_out(PANE_WIDTH, PANE_HEIGHT, || {
        collapsible(CollapsibleChevronSide::End, None, None)
    });
    let chevron = laid.frame(CHEVRON);
    let title = laid.frame("text \"Plain title");
    assert!(title.x < chevron.x, "title, then chevron");
    assert!(chevron.x > PANE_WIDTH / 2.0, "the chevron is pushed to the end ({chevron:?})");
}

/// Regression: the header could only be text. A custom header renders in
/// the toggle in place of the title.
#[test]
fn regression_collapsible_takes_a_custom_header_in_the_toggle() {
    let laid = lay_out(PANE_WIDTH, PANE_HEIGHT, || {
        collapsible(
            CollapsibleChevronSide::Start,
            Some(ui! { text { "Daily report".to_string() } }),
            None,
        )
    });
    assert!(laid.find("text \"Plain title").is_none(), "the custom header replaces the title");
    let toggle = laid.find("pressable").expect("the toggle");
    let header = laid.find("text \"Daily report").expect("the custom header");
    assert!(
        common::find(&laid.h, toggle, "text \"Daily report") == Some(header),
        "the custom header is inside the toggle, so pressing it toggles:\n{}",
        laid.h.tree(laid.root)
    );
    assert!(laid.frame(CHEVRON).x < laid.frame("text \"Daily report").x, "chevron first");
}

/// Regression: there was no slot for a trailing action. It sits at the end
/// of the header row, OUTSIDE the toggle, and the toggle takes the rest of
/// the row.
#[test]
fn regression_collapsible_trailing_sits_outside_the_toggle_at_the_end() {
    let laid = lay_out(PANE_WIDTH, PANE_HEIGHT, || {
        collapsible(
            CollapsibleChevronSide::Start,
            Some(ui! { text { "Daily report".to_string() } }),
            Some(ui! { text { "Open".to_string() } }),
        )
    });
    let toggle = laid.find("pressable").expect("the toggle");
    assert!(
        common::find(&laid.h, toggle, "text \"Open").is_none(),
        "the trailing element must not be inside the toggle's pressable:\n{}",
        laid.h.tree(laid.root)
    );
    let toggle_frame = laid.frames[&toggle];
    let open = laid.frame("text \"Open");
    assert!(
        open.x >= toggle_frame.x + toggle_frame.width,
        "trailing after the toggle (toggle {toggle_frame:?}, trailing {open:?})"
    );
    assert!(
        open.x > PANE_WIDTH / 2.0 && toggle_frame.width > PANE_WIDTH / 2.0,
        "the toggle grows and the trailing element ends the row:\n{}",
        laid.h.tree(laid.root)
    );
}
