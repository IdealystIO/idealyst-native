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
//! parent alignments that matter: a column (stretch, start, centre), a
//! row whose taller sibling stretches the line, where a hugging trigger stays
//! at the top as it does on its own, and a Button in a row the parent centres.
//!
//! The layout is real (`tests/common`: host-mock tree mirrored into
//! Taffy). It fails with the wrapper at `align_self: Center`, and with a
//! `justify_content: Center` wrapper in the stretched row.

mod common;

use std::rc::Rc;

use common::{find, lay_out};
use idea_ui::{Button, Field, IconButton, Stack, StackAlign, StackAxis, StackGap, Tag, Tooltip, Typography};
use runtime_core::{ui, Element};
use runtime_layout::Frame;

const PANE_WIDTH: f32 = 600.0;
const PANE_HEIGHT: f32 = 800.0;

const SHORT: &str = "Short name";
const LONG: &str = "a much longer line than the name above it";

/// Lay `parent` out without and with a Tooltip around the trigger, and return the
/// absolute frame of the node matching `trigger` (a host-mock kind prefix) both ways.
fn bare_and_wrapped(case: &str, trigger: &str, parent: impl Fn(bool) -> Element) -> (Frame, Frame) {
    let bare = lay_out(PANE_WIDTH, PANE_HEIGHT, || parent(false));
    let wrapped = lay_out(PANE_WIDTH, PANE_HEIGHT, || parent(true));
    let b = find(&bare.h, bare.root, trigger)
        .unwrap_or_else(|| panic!("{case}: no {trigger} bare:\n{}", bare.h.tree(bare.root)));
    let w = find(&wrapped.h, wrapped.root, trigger)
        .unwrap_or_else(|| panic!("{case}: no {trigger} wrapped:\n{}", wrapped.h.tree(wrapped.root)));
    (bare.frames[&b], wrapped.frames[&w])
}

/// The trigger has the same absolute frame with and without a Tooltip.
fn assert_tooltip_is_transparent(case: &str, trigger: &str, parent: impl Fn(bool) -> Element) {
    let (fb, fw) = bare_and_wrapped(case, trigger, parent);
    assert!(
        (fb.x - fw.x).abs() < 0.01
            && (fb.y - fw.y).abs() < 0.01
            && (fb.width - fw.width).abs() < 0.01
            && (fb.height - fw.height).abs() < 0.01,
        "{case}: the Tooltip moved its trigger — bare {fb:?}, wrapped {fw:?}"
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

/// A row stretched by a taller sibling: the wrapper stretches, and each trigger
/// sits at its top, where it sits bare. It fails with a `justify_content: Center`
/// wrapper, which drops the trigger to the middle.
fn in_a_stretched_row(build: fn() -> Element) -> impl Fn(bool) -> Element {
    move |tooltip| {
        ui! {
            Stack(axis = StackAxis::Row) {
                if tooltip {
                    Tooltip(text = "A hint".to_string()) {
                        { build() }
                    }
                } else {
                    { build() }
                }
                Stack(gap = idea_ui::StackGap::Md) {
                    Typography(content = LONG.to_string())
                    Typography(content = LONG.to_string())
                    Typography(content = LONG.to_string())
                }
            }
        }
    }
}

#[test]
fn regression_tooltip_button_trigger_moved_in_a_stretched_row() {
    assert_tooltip_is_transparent("Button", "pressable", in_a_stretched_row(|| {
        ui! { Button(label = "Save".to_string(), on_click = Rc::new(|| {}) as Rc<dyn Fn()>) }
    }));
}

#[test]
fn regression_tooltip_tag_trigger_moved_in_a_stretched_row() {
    assert_tooltip_is_transparent("Tag", "text \"New", in_a_stretched_row(|| ui! { Tag(label = "New".to_string()) }));
}

#[test]
fn regression_tooltip_icon_button_trigger_moved_in_a_stretched_row() {
    assert_tooltip_is_transparent("IconButton", "pressable", in_a_stretched_row(|| ui! { IconButton(glyph = "x".to_string()) }));
}

/// A trigger that stretches to the line keeps its place. It keeps to its own
/// height in the wrapper, whose main axis that is, so only the position compares.
#[test]
fn regression_tooltip_text_trigger_moved_in_a_stretched_row() {
    let (bare, wrapped) = bare_and_wrapped(
        "Typography",
        "text \"Short",
        in_a_stretched_row(|| ui! { Typography(content = SHORT.to_string()) }),
    );
    assert_eq!((bare.x, bare.y), (wrapped.x, wrapped.y), "bare {bare:?}, wrapped {wrapped:?}");
}

/// A Button trigger in a row the parent aligns: wrapped or bare, same frame.
#[test]
fn tooltip_button_trigger_keeps_its_place_in_a_centred_row() {
    assert_tooltip_is_transparent("row, align center, Button", "pressable", |tooltip| {
        ui! {
            Stack(axis = StackAxis::Row, align = StackAlign::Center) {
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

/// want_77d7d4a1's exact fail case: a name in a `Stack(align = Start)`, over a
/// longer line. Reported: the name renders centred over that line. Filed with
/// `ControlLabel`, since removed; a `Typography` label is the same hugging text.
#[test]
fn regression_want_77d7d4a1_control_label_stays_at_the_start_over_a_longer_line() {
    let tree = lay_out(PANE_WIDTH, PANE_HEIGHT, || {
        ui! {
            Stack(gap = StackGap::Xs, align = StackAlign::Start) {
                Tooltip(text = "Describes the name".to_string()) {
                    Typography(content = "Short name".to_string())
                }
                Typography(content = "a much longer line than the name".to_string())
            }
        }
    });
    let name = find(&tree.h, tree.root, "text \"Short").expect("the name");
    let line = find(&tree.h, tree.root, "text \"a much").expect("the line");
    let (name, line) = (tree.frames[&name], tree.frames[&line]);
    assert_eq!(name.x, line.x, "the name must start where the line starts: {name:?} over {line:?}");
    assert!(name.width < line.width, "the name must hug, or it cannot have looked centred: {name:?} {line:?}");
}

/// Triggers that fill must keep filling: a hugging wrapper must not collapse them.
#[test]
fn tooltip_block_button_trigger_keeps_its_width() {
    assert_tooltip_is_transparent("block Button", "pressable", |tooltip| {
        ui! {
            Stack {
                if tooltip {
                    Tooltip(text = "Saves".to_string()) {
                        Button(label = "Save".to_string(), block = true, on_click = Rc::new(|| {}) as Rc<dyn Fn()>)
                    }
                } else {
                    Button(label = "Save".to_string(), block = true, on_click = Rc::new(|| {}) as Rc<dyn Fn()>)
                }
            }
        }
    });
}

#[test]
fn tooltip_field_trigger_keeps_its_width() {
    let field = || {
        let v = runtime_core::signal(String::new());
        ui! { Field(value = v, label = Some("Name".to_string())) }
    };
    assert_tooltip_is_transparent("Field", "text_input", |tooltip| {
        ui! {
            Stack {
                if tooltip {
                    Tooltip(text = "Your name".to_string()) {
                        { field() }
                    }
                } else {
                    { field() }
                }
            }
        }
    });
}
