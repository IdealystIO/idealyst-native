//! The panel's components: an idealyst app on the framework's own
//! terminal backend.
//!
//! They render a [`crate::view::Screen`] — the layout is decided there,
//! as lines — so a component here is only structure and colour. Styles
//! are in terminal cells (the panel runs at one layout pixel per cell);
//! colour is the only styling a line carries, and only where it says
//! something: busy, landed, failed, secondary.

use std::rc::Rc;

use runtime_core::{
    component, raf_loop_scoped, stylesheet, ui, Color, Element, FlexDirection, Length, ReadSignal,
};

use crate::view::{Line, Row, Tone};

stylesheet! {
    pub Frame<()> {
        base(_t) {
            flex_direction: FlexDirection::Column,
            width: Length::pct(100.0),
            height: Length::pct(100.0),
        }
    }
}

// Takes the height the blocks leave, so the footer sits on the last row.
stylesheet! {
    pub Fill<()> {
        base(_t) {
            flex_grow: 1.0,
        }
    }
}

stylesheet! {
    pub RowLayout<()> {
        base(_t) {
            flex_direction: FlexDirection::Row,
        }
    }
}

// A target's name column: wide enough for any target name, so the
// statuses line up.
stylesheet! {
    pub NameColumn<()> {
        base(_t) {
            flex_direction: FlexDirection::Row,
            width: 18.0,
            flex_shrink: 0.0,
        }
    }
}

// A word, in its colour, `margin_left` cells after the previous one.
stylesheet! {
    pub Ink<()> {
        base(_t) {}
        variant tone {
            #[default]
            normal(_t) {}
            muted(_t) {
                color: Color("#8a8f98".into()),
            }
            busy(_t) {
                color: Color("#e5b443".into()),
            }
            ok(_t) {
                color: Color("#5cbf7c".into()),
            }
            error(_t) {
                color: Color("#e5676b".into()),
            }
        }
        override margin_left: f32
    }
}

fn ink(tone: Tone) -> InkTone {
    match tone {
        Tone::Normal => InkTone::Normal,
        Tone::Muted => InkTone::Muted,
        Tone::Busy => InkTone::Busy,
        Tone::Ok => InkTone::Ok,
        Tone::Error => InkTone::Error,
    }
}

/// Split a line into its words, each with the number of spaces before it.
///
/// The terminal's `text` primitive wraps on whitespace and collapses runs
/// of it (as a browser does), so a line whose spacing means something —
/// columns, indentation, the caret under a rustc error — cannot carry its
/// spaces as characters. It carries them as layout instead: one text per
/// word, `margin_left` cells after the last. The grid then has every
/// character exactly where the string had it.
pub fn words(line: &str) -> Vec<(usize, String)> {
    let mut out = Vec::new();
    let mut gap = 0;
    let mut word = String::new();
    for c in line.chars() {
        if c == ' ' {
            if !word.is_empty() {
                out.push((gap, std::mem::take(&mut word)));
                gap = 0;
            }
            gap += 1;
        } else {
            word.push(c);
        }
    }
    if !word.is_empty() {
        out.push((gap, word));
    }
    out
}

/// The whole panel. Every block is a keyed list of lines, keyed by what
/// the line says, so a line that changes is rebuilt and one that does not
/// stays put; `on_frame`, when given, runs every frame (the live panel
/// drains the session's events there — tests drive it by hand instead).
#[component]
pub fn Panel(
    header: ReadSignal<Line>,
    rows: ReadSignal<Vec<Row>>,
    history: ReadSignal<Vec<Line>>,
    error: ReadSignal<Vec<Line>>,
    log: ReadSignal<Vec<Line>>,
    footer: ReadSignal<Line>,
    #[prop(static)] on_frame: Option<Rc<dyn Fn()>>,
) -> Element {
    if let Some(frame) = on_frame {
        raf_loop_scoped(move || frame());
    }
    let saves = Line { text: "  saves".into(), tone: Tone::Muted, key: String::new() };
    ui! {
        view(style = Frame()) {
            // Single-spaced text: nothing for the terminal to collapse.
            text { move || header.get().text }
            text { " " }
            for row in rows, key = row.key.clone() {
                TargetRow(row = row.clone())
            }
            text { " " }
            Words(line = saves)
            for line in history, key = line.key.clone() {
                Words(line = line.clone())
            }
            for line in error, key = line.key.clone() {
                Words(line = line.clone())
            }
            for line in log, key = line.key.clone() {
                Words(line = line.clone())
            }
            view(style = Fill()) {}
            text(style = Ink().tone(InkTone::Muted)) { move || footer.get().text }
        }
    }
}

/// One target: its name, then its state in its state's colour.
#[component]
pub fn TargetRow(row: Row) -> Element {
    let row = row.get();
    let name = Line { text: format!("  {}", row.name.trim_end()), tone: Tone::Muted, key: String::new() };
    let status = Line { text: row.status, tone: row.tone, key: String::new() };
    ui! {
        view(style = RowLayout()) {
            view(style = NameColumn()) {
                Words(line = name)
            }
            Words(line = status)
        }
    }
}

/// One line in its colour, its spacing kept (see [`words`]). A blank line
/// still takes its row.
#[component]
pub fn Words(line: Line) -> Element {
    let line = line.get();
    let tone = ink(line.tone);
    let words = words(&line.text);
    if words.is_empty() {
        return ui! { text { " " } };
    }
    ui! {
        view(style = RowLayout()) {
            for (gap, word) in words {
                text(style = Ink().tone(tone).margin_left(gap as f32)) { word }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::words;

    #[test]
    fn words_keep_every_gap() {
        assert_eq!(
            words("  a  bc d"),
            vec![(2, "a".to_string()), (2, "bc".to_string()), (1, "d".to_string())]
        );
        assert_eq!(words("   |      ^^^"), vec![(3, "|".to_string()), (6, "^^^".to_string())]);
        assert!(words("   ").is_empty());
    }
}
