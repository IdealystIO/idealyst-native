//! Regression (want_b06f208f): a non-block Button overrode its parent's
//! alignment. It hugged with `align_self: Center`, so in `Stack(align = Start)`
//! it sat in the middle of the column, under left-aligned text.
//!
//! It hugs by size now (`Length::FitContent`), which leaves the parent in
//! charge. Real layout (`tests/common`: host-mock tree mirrored into Taffy, what
//! every native backend runs). The web lowering is the same CSS: `css::length_css`
//! prints `fit-content`, pinned in that crate.

mod common;

use std::rc::Rc;

use common::lay_out;
use idea_ui::{Button, Stack, StackAlign, StackAxis, Typography};
use runtime_core::{ui, Element};

const WIDTH: f32 = 600.0;
const HEIGHT: f32 = 800.0;

fn column(align: Option<StackAlign>) -> Element {
    let long = "a line far longer than the button below it".to_string();
    match align {
        Some(align) => ui! {
            Stack(align = align) {
                Typography(content = long)
                Button(label = "Save".to_string(), on_click = Rc::new(|| {}) as Rc<dyn Fn()>)
            }
        },
        None => ui! {
            Stack {
                Typography(content = long)
                Button(label = "Save".to_string(), on_click = Rc::new(|| {}) as Rc<dyn Fn()>)
            }
        },
    }
}

/// The report's exact shape. Fails with `align_self: Center`.
#[test]
fn regression_button_follows_a_start_aligned_column() {
    let laid = lay_out(WIDTH, HEIGHT, || column(Some(StackAlign::Start)));
    let button = laid.frame("pressable");
    assert_eq!(button.x, laid.frame("text \"a line").x, "the button starts under the text:\n{}", laid.h.tree(laid.root));
    assert!(button.width < WIDTH / 2.0, "and hugs its label, got {}", button.width);
}

/// The default column stretches its children, and a hugging button doesn't
/// grow: it stays its own size at the start.
#[test]
fn button_hugs_at_the_start_of_a_default_column() {
    let laid = lay_out(WIDTH, HEIGHT, || column(None));
    let button = laid.frame("pressable");
    assert_eq!(button.x, 0.0, "{}", laid.h.tree(laid.root));
    assert!(button.width < WIDTH / 2.0, "not stretched across the column, got {}", button.width);
}

#[test]
fn button_follows_end_and_center_aligned_columns() {
    let laid = lay_out(WIDTH, HEIGHT, || column(Some(StackAlign::End)));
    let button = laid.frame("pressable");
    assert!((button.x + button.width - WIDTH).abs() < 0.5, "at the end, got {button:?}");

    let laid = lay_out(WIDTH, HEIGHT, || column(Some(StackAlign::Center)));
    let button = laid.frame("pressable");
    assert!((button.x + button.width / 2.0 - WIDTH / 2.0).abs() < 0.5, "centred, got {button:?}");
}

fn row(align: StackAlign) -> Element {
    let long = "a line".to_string();
    ui! {
        Stack(axis = StackAxis::Row, align = align) {
            Button(label = "Save".to_string(), on_click = Rc::new(|| {}) as Rc<dyn Fn()>)
            Stack(gap = idea_ui::StackGap::Md) {
                Typography(content = long.clone())
                Typography(content = long.clone())
                Typography(content = long)
            }
        }
    }
}

/// A row is the same rule on the other axis: the button doesn't stretch to the
/// line's height (the "buttons flex to the parent height" bug), and the row's
/// `align` places it.
#[test]
fn button_hugs_its_height_and_follows_a_row() {
    let laid = lay_out(WIDTH, HEIGHT, || row(StackAlign::Stretch));
    let button = laid.frame("pressable");
    let line = laid.frames[&laid.root].height;
    assert!(button.height < line, "hugs inside the {line} line, got {button:?}");
    assert_eq!(button.y, 0.0, "a stretching row leaves it at the start");

    let laid = lay_out(WIDTH, HEIGHT, || row(StackAlign::Center));
    let button = laid.frame("pressable");
    let line = laid.frames[&laid.root].height;
    assert!((button.y + button.height / 2.0 - line / 2.0).abs() < 0.5, "centred, got {button:?}");

    let laid = lay_out(WIDTH, HEIGHT, || row(StackAlign::End));
    let button = laid.frame("pressable");
    let line = laid.frames[&laid.root].height;
    assert!((button.y + button.height - line).abs() < 0.5, "at the end, got {button:?}");
}

/// `block` still fills, and a long label still wraps inside the available width
/// (fit-content caps at it, where max-content would overflow).
#[test]
fn block_button_fills_and_a_long_label_stays_inside_the_column() {
    let laid = lay_out(WIDTH, HEIGHT, || {
        ui! { Stack { Button(label = "Save".to_string(), block = true, on_click = Rc::new(|| {}) as Rc<dyn Fn()>) } }
    });
    assert_eq!(laid.frame("pressable").width, WIDTH);

    let label = "word ".repeat(60);
    let laid = lay_out(WIDTH, HEIGHT, || {
        ui! { Stack { Button(label = label, on_click = Rc::new(|| {}) as Rc<dyn Fn()>) } }
    });
    let button = laid.frame("pressable");
    assert!(button.width <= WIDTH, "stays inside the column, got {button:?}");
}
