//! Regression (want_b06f208f, the hugging components after Button): Tag, Chip,
//! Badge, IconButton, Avatar, Spinner hugged with `align_self: Center`, and
//! SegmentedControl and Calendar with `align_self: FlexStart`. An `align_self`
//! overrides the parent's `align_items`, so `Stack(align = Start)` could not
//! place them under left-aligned text, nor `Stack(align = End)` at the end.
//!
//! They hug by size now: a fixed square, or `Length::FitContent`. Real layout
//! (`tests/common`: host-mock tree mirrored into Taffy, what every native backend
//! runs). The web lowering is the same CSS: `css::length_css` prints `fit-content`,
//! pinned in that crate.

mod common;

use std::rc::Rc;

use common::{lay_out, Laid};
use idea_ui::{
    Alert, AlertClose, Avatar, Badge, Button, Calendar, CivilDate, Chip, IconButton, SegmentOption, SegmentedControl, Spinner,
    Stack, StackAlign, StackAxis, StackGap, Tag, Typography,
};
use runtime_core::{signal, ui, Element};
use runtime_layout::Frame;

const WIDTH: f32 = 1200.0;
const HEIGHT: f32 = 1200.0;
const LONG: &str = "a line far longer than the component under it";

type Build = fn() -> Element;

/// The frame of the root's `index`th child, the component under test: a
/// reactive hole's anchor has no box, so it reads through to what it holds.
fn child_frame(laid: &Laid, index: usize) -> Frame {
    let mut node = laid.h.children_of(laid.root)[index];
    while laid.h.kind_of(node).is_some_and(|k| k == "anchor") {
        node = laid.h.children_of(node)[0];
    }
    laid.frames[&node]
}

fn in_column(align: Option<StackAlign>, build: Build) -> (Laid, Frame) {
    let laid = lay_out(WIDTH, HEIGHT, || {
        let component = build();
        match align {
            Some(align) => ui! {
                Stack(align = align) {
                    Typography(content = LONG.to_string())
                    component
                }
            },
            None => ui! {
                Stack {
                    Typography(content = LONG.to_string())
                    component
                }
            },
        }
    });
    let frame = child_frame(&laid, 1);
    (laid, frame)
}

fn in_row(align: Option<StackAlign>, build: Build) -> (Laid, Frame) {
    let laid = lay_out(WIDTH, HEIGHT, || {
        let component = build();
        // A column taller than any of the components, so the line has room.
        let tall = ui! {
            Stack(gap = StackGap::Xl) {
                for _ in 0..40 {
                    Typography(content = "a line".to_string())
                }
            }
        };
        match align {
            Some(align) => ui! { Stack(axis = StackAxis::Row, align = align) { component tall } },
            None => ui! { Stack(axis = StackAxis::Row) { component tall } },
        }
    });
    let frame = child_frame(&laid, 0);
    (laid, frame)
}

/// The report's shape.
fn follows_a_start_aligned_column(build: Build) {
    let (laid, frame) = in_column(Some(StackAlign::Start), build);
    assert_eq!(frame.x, 0.0, "starts under the text:\n{}", laid.h.tree(laid.root));
}

fn follows_end_and_center_aligned_columns(build: Build) {
    let (laid, frame) = in_column(Some(StackAlign::End), build);
    assert!((frame.x + frame.width - WIDTH).abs() < 0.5, "at the end, got {frame:?}:\n{}", laid.h.tree(laid.root));
    let (laid, frame) = in_column(Some(StackAlign::Center), build);
    assert!((frame.x + frame.width / 2.0 - WIDTH / 2.0).abs() < 0.5, "centred, got {frame:?}:\n{}", laid.h.tree(laid.root));
}

/// A default column stretches its children. A hugging component doesn't grow:
/// it stays its own size, at the start.
fn hugs_at_the_start_of_a_default_column(build: Build) {
    let (laid, frame) = in_column(None, build);
    assert_eq!(frame.x, 0.0, "starts the column:\n{}", laid.h.tree(laid.root));
    assert!(frame.width > 0.0, "has a size, got {frame:?}");
    assert!(frame.width < WIDTH / 2.0, "isn't stretched across the column, got {frame:?}");
}

/// A row is the same rule on the other axis: no stretching to the line's height,
/// at the top of a stretching row, and the row's `align` places it.
fn hugs_its_height_and_follows_a_row(build: Build) {
    let (laid, frame) = in_row(None, build);
    let line = laid.frames[&laid.root].height;
    assert!(frame.height > 0.0, "has a size, got {frame:?}");
    assert!(frame.height < line / 2.0, "hugs inside the {line} line, got {frame:?}");
    assert_eq!(frame.y, 0.0, "at the top of a stretching row:\n{}", laid.h.tree(laid.root));

    let (laid, frame) = in_row(Some(StackAlign::Center), build);
    let line = laid.frames[&laid.root].height;
    assert!((frame.y + frame.height / 2.0 - line / 2.0).abs() < 0.5, "centred, got {frame:?}");

    let (laid, frame) = in_row(Some(StackAlign::End), build);
    let line = laid.frames[&laid.root].height;
    assert!((frame.y + frame.height - line).abs() < 0.5, "at the end, got {frame:?}");
}

/// One module of tests per component, so a failure names the component.
macro_rules! per_component {
    ($($component:ident => $build:expr,)*) => {$(
        mod $component {
            use super::*;
            const BUILD: Build = $build;
            #[test]
            fn regression_follows_a_start_aligned_column() {
                follows_a_start_aligned_column(BUILD);
            }
            #[test]
            fn regression_follows_end_and_center_aligned_columns() {
                follows_end_and_center_aligned_columns(BUILD);
            }
            #[test]
            fn regression_hugs_at_the_start_of_a_default_column() {
                hugs_at_the_start_of_a_default_column(BUILD);
            }
            #[test]
            fn regression_hugs_its_height_and_follows_a_row() {
                hugs_its_height_and_follows_a_row(BUILD);
            }
        }
    )*};
}

per_component! {
    tag => || ui! { Tag(label = "Tag".to_string()) },
    chip => || ui! { Chip(label = "Chip".to_string()) },
    chip_pressable => || ui! { Chip(label = "Chip".to_string(), on_select = Some(Rc::new(|| {}) as Rc<dyn Fn()>)) },
    badge => || ui! { Badge(label = "Badge".to_string()) },
    icon_button => || ui! { IconButton(glyph = "x".to_string()) },
    avatar => || ui! { Avatar(initials = "AB".to_string()) },
    spinner => || ui! { Spinner() },
    segmented_control => || {
        let value = signal("one".to_string());
        ui! {
            SegmentedControl(
                value = value,
                options = vec![SegmentOption::new("one", "One"), SegmentOption::new("two", "Two")],
                on_change = Rc::new(|_: String| {}) as Rc<dyn Fn(String)>,
            )
        }
    },
    calendar => || {
        let picked = signal(CivilDate::new(2026, 8, 3));
        ui! { Calendar(value = picked, on_change = Rc::new(|_: CivilDate| {}) as Rc<dyn Fn(CivilDate)>) }
    },
}

/// Fit-content caps at the available width, where `max-content` would overflow.
#[test]
fn a_long_label_stays_inside_the_column() {
    const NARROW: f32 = 300.0;
    let label = "word ".repeat(60);
    for (name, element) in [
        ("Tag", Box::new({ let label = label.clone(); move || ui! { Tag(label = label) } }) as Box<dyn FnOnce() -> Element>),
        ("Chip", Box::new({ let label = label.clone(); move || ui! { Chip(label = label) } })),
        ("Badge", Box::new({ let label = label.clone(); move || ui! { Badge(label = label) } })),
    ] {
        let laid = lay_out(NARROW, HEIGHT, || {
            let pill = element();
            ui! { Stack { pill } }
        });
        let frame = child_frame(&laid, 0);
        assert!(frame.width <= NARROW, "{name} stays inside the column, got {frame:?}");
    }
}

/// A banner's action relied on hugging with `align_self: Center`, so the Alert
/// centres its trailing slots itself: an action sits in the middle of a tall
/// banner, whatever element it is, and the built-in close stays at the top corner.
#[test]
fn regression_alert_centres_its_action_in_a_tall_banner() {
    let body = "word ".repeat(60);
    for (name, action) in [
        ("Button", Box::new(|| ui! { Button(label = "Retry".to_string(), on_click = Rc::new(|| {}) as Rc<dyn Fn()>) }) as Box<dyn FnOnce() -> Element>),
        ("IconButton", Box::new(|| ui! { IconButton(glyph = "x".to_string()) })),
        ("Tag", Box::new(|| ui! { Tag(label = "x".to_string()) })),
    ] {
        let body = body.clone();
        let laid = lay_out(WIDTH, HEIGHT, move || {
            let action = action();
            ui! {
                Alert(
                    title = "Title".to_string(),
                    body = body,
                    action = Some(action),
                    close = AlertClose::Button(Rc::new(|| {}) as Rc<dyn Fn()>),
                )
            }
        });
        let kids = laid.h.children_of(laid.root);
        let root = laid.frames[&laid.root];
        let (content, action, close) = (laid.frames[&kids[0]], laid.frames[&kids[1]], laid.frames[&kids[2]]);
        let middle = |f: Frame| f.y + f.height / 2.0;
        assert!(
            (middle(action) - middle(content)).abs() < 0.5,
            "{name} in the middle of the banner, got {action:?} beside {content:?} in {root:?}:\n{}",
            laid.h.tree(laid.root)
        );
        assert_eq!(close.y, content.y, "the built-in close stays at the top, got {close:?}");
    }
}
