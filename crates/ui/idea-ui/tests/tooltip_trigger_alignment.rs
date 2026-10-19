//! Regression (reported against idea-ui 8.0.0): a Tooltip moved its
//! trigger. The wrapper it puts around the trigger hugged with
//! `align_self: Center`, which overrides the parent's `align_items` — so
//! in a column the trigger was centred horizontally, and
//! `Stack(align = StackAlign::Start)` could not pull it back. A name with
//! a description tooltip sat off-centre over the left-aligned line below
//! it.
//!
//! A Tooltip adds behaviour, not layout: the trigger must land exactly
//! where it lands without one. Each case lays out the same parent twice —
//! once with the trigger wrapped in a Tooltip, once bare — and compares
//! the trigger's absolute frame. The cases cover both axes and the
//! parent alignments that matter: a column (stretch, start, centre) and a
//! row whose taller sibling stretches the line, where a hugging trigger
//! (Button) must stay vertically centred as it does on its own.
//!
//! The layout is real (`tests/common`: host-mock tree mirrored into
//! Taffy). It fails with the wrapper at
//! `align_self: Center`.

mod common;

use std::rc::Rc;

use common::{find, lay_out};
use idea_ui::{Button, Stack, StackAlign, StackAxis, Tooltip, Typography};
use runtime_core::{ui, Element};

const PANE_WIDTH: f32 = 600.0;
const PANE_HEIGHT: f32 = 800.0;

const SHORT: &str = "Short name";
const LONG: &str = "a much longer line than the name above it";

/// Lay `parent` out with and without a Tooltip around the trigger and
/// assert the node matching `trigger` (a host-mock kind prefix) has the
/// same absolute frame both ways.
fn assert_tooltip_is_transparent(
    case: &str,
    trigger: &str,
    parent: impl Fn(bool) -> Element,
) {
    let bare = lay_out(PANE_WIDTH, PANE_HEIGHT, || parent(false));
    let wrapped = lay_out(PANE_WIDTH, PANE_HEIGHT, || parent(true));
    let b = find(&bare.h, bare.root, trigger)
        .unwrap_or_else(|| panic!("{case}: no {trigger} bare:\n{}", bare.h.tree(bare.root)));
    let w = find(&wrapped.h, wrapped.root, trigger)
        .unwrap_or_else(|| panic!("{case}: no {trigger} wrapped:\n{}", wrapped.h.tree(wrapped.root)));
    let (fb, fw) = (bare.frames[&b], wrapped.frames[&w]);
    assert!(
        (fb.x - fw.x).abs() < 0.01
            && (fb.y - fw.y).abs() < 0.01
            && (fb.width - fw.width).abs() < 0.01
            && (fb.height - fw.height).abs() < 0.01,
        "{case}: the Tooltip moved its trigger — bare {fb:?}, wrapped {fw:?}\n{}",
        wrapped.h.tree(wrapped.root)
    );
}

/// A short name trigger over a longer line, in a column with `align`.
fn name_over_line(align: StackAlign, tooltip: bool) -> Element {
    ui! {
        Stack(align = align) {
            if tooltip {
                Tooltip(text = "Describes the name".to_string()) {
                    Typography(content = SHORT.to_string())
                }
            } else {
                Typography(content = SHORT.to_string())
            }
            Typography(content = LONG.to_string())
        }
    }
}

/// The report's exact shape: `Stack(align = Start)`.
#[test]
fn regression_tooltip_centres_its_trigger_in_a_start_aligned_column() {
    assert_tooltip_is_transparent("column, align start", "text \"Short", |t| {
        name_over_line(StackAlign::Start, t)
    });
}

/// The default Stack (stretch) — the trigger was centred here too.
#[test]
fn tooltip_trigger_keeps_its_place_in_a_stretch_column() {
    assert_tooltip_is_transparent("column, align stretch", "text \"Short", |t| {
        name_over_line(StackAlign::Stretch, t)
    });
}

/// A centring parent still centres the trigger.
#[test]
fn tooltip_trigger_keeps_its_place_in_a_centred_column() {
    assert_tooltip_is_transparent("column, align center", "text \"Short", |t| {
        name_over_line(StackAlign::Center, t)
    });
}

/// The case the wrapper's hug exists for: a row stretched by a taller
/// sibling. A Button hugs and centres itself on the cross axis; wrapped,
/// it must not drop to the top of a stretched wrapper.
#[test]
fn tooltip_button_trigger_stays_centred_in_a_stretched_row() {
    assert_tooltip_is_transparent("row, align stretch, Button", "pressable", |tooltip| {
        ui! {
            Stack(axis = StackAxis::Row) {
                if tooltip {
                    Tooltip(text = "Saves the draft".to_string()) {
                        Button(label = "Save".to_string(), on_click = Rc::new(|| {}) as Rc<dyn Fn()>)
                    }
                } else {
                    Button(label = "Save".to_string(), on_click = Rc::new(|| {}) as Rc<dyn Fn()>)
                }
                Stack(gap = idea_ui::StackGap::Md) {
                    Typography(content = LONG.to_string())
                    Typography(content = LONG.to_string())
                    Typography(content = LONG.to_string())
                }
            }
        }
    });
}
