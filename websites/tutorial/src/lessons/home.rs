//! The course landing page: what the course is, where to start given the
//! reader's background, and the whole outline — written lessons as links,
//! the rest as the roadmap.
//!
//! Everything numeric (chapter and lesson counts, total time, each
//! lesson's number and duration) is read from `crate::course`, so the page
//! cannot disagree with the sidebar.

use std::rc::Rc;

use idea_ui::{SegmentOption, SegmentedControl};
use runtime_core::{switch, ui, Element, Route, Signal};

use crate::course::{self, Chapter, Status, CHAPTERS};
use crate::routes::{START_CREATE_ROUTE, START_INSTALL_ROUTE, UI_PRIMITIVES_ROUTE};
use crate::shell;
use crate::styles::{
    ChapterInfo, ChapterMeta, ChapterNumber, ChapterRow, ChapterSummary, ChapterTitle, Eyebrow,
    EyebrowRow,
    HeroLead, HeroTitle, LandingPad, LessonCell, LessonGrid, LessonItem, LessonItemPlanned, LessonMinutes,
    LessonNumber, LessonSoon, LessonTitle, PersonaAdvice, PersonaCard, PersonaPick,
    PersonaQuestion, StartButton, StartButtonText, StartCard, StartLessonMeta, StartLessonTitle,
    StatRow, StatText,
};

/// The reader's background, as the picker's segment ids.
const NEW_TO_BOTH: &str = "new";
const RUST_DEV: &str = "rust";
const FROM_REACT: &str = "react";

fn advice(background: &str) -> &'static str {
    match background {
        RUST_DEV => {
            "You know the language; what's new is declarative UI and reactivity. Install \
             the CLI, then give Building UI and Reactivity your full attention."
        }
        FROM_REACT => {
            "Most ideas carry over. Look for the \u{201c}Coming from React\u{201d} notes, \
             especially on signals: components run once and never re-render."
        }
        _ => {
            "Start at the very beginning. Each lesson explains the Rust it uses as it goes; \
             keep The Rust Book nearby for anything deeper."
        }
    }
}

fn start_route(background: &str) -> &'static Route<()> {
    match background {
        RUST_DEV => &START_CREATE_ROUTE,
        FROM_REACT => &UI_PRIMITIVES_ROUTE,
        _ => &START_INSTALL_ROUTE,
    }
}

/// `background` is owned by the app root, so the choice survives
/// navigating into a lesson and back.
pub fn page(background: Signal<String>) -> Element {
    let (chapters, lessons, minutes) = course::totals();
    let hours = (minutes as f32 / 60.0).round() as u32;
    let on_pick: Rc<dyn Fn(String)> = Rc::new(move |id| background.set(id));
    // idealyst-lint-disable-next-line prefer-ui-control-flow -- keyed rebuild of one shape: the start card per start lesson
    let start = switch(
        move || start_route(&background.get()).name(),
        |name: &&'static str| start_card(name),
    );

    shell::layout(ui! {
        view(style = LandingPad()) {
            view(style = EyebrowRow()) {
                { shell::sidebar_toggle() }
                text(style = Eyebrow()) { "The Idealyst Course" }
            }
            text(style = HeroTitle()) { "From zero to a shipping app, in Rust." }
            text(style = HeroLead()) {
                "One codebase for web, iOS, Android, macOS and the terminal. Every lesson has a \
                 live editor next to it, so you write real Idealyst code and see it run as you read."
            }
            view(style = StatRow()) {
                text(style = StatText()) { "{chapters} chapters" }
                text(style = StatText()) { "{lessons} lessons" }
                text(style = StatText()) { "about {hours} hours" }
            }

            view(style = PersonaCard()) {
                view(style = PersonaPick()) {
                    text(style = PersonaQuestion()) { "Where are you coming from?" }
                    SegmentedControl(
                        value = background,
                        on_change = on_pick,
                        options = vec![
                            SegmentOption::new(NEW_TO_BOTH, "New to Rust & UI"),
                            SegmentOption::new(RUST_DEV, "Rust dev, new to UI"),
                            SegmentOption::new(FROM_REACT, "Coming from React"),
                        ],
                    )
                    text(style = PersonaAdvice()) { move || advice(&background.get()).to_string() }
                }
                start
            }

            for (c, chapter) in CHAPTERS.iter().enumerate() {
                chapter_row(c, chapter)
            }
        }
    })
}

/// The "Start here" card for one lesson.
fn start_card(route_name: &'static str) -> Element {
    let Some((c, i, lesson)) = course::find(route_name) else {
        return ui! { view {} };
    };
    let route = lesson.route.expect("a start lesson is written");
    let title = format!("{} \u{00b7} {}", course::lesson_number(c, i), lesson.title);
    let meta = format!("{} min \u{00b7} {}", lesson.minutes, CHAPTERS[c].title);
    ui! {
        view(style = StartCard()) {
            text(style = Eyebrow()) { "Start here" }
            text(style = StartLessonTitle()) { title }
            text(style = StartLessonMeta()) { meta }
            link(route = route, params = ()) {
                view(style = StartButton()) {
                    text(style = StartButtonText()) { "Start lesson \u{2192}" }
                }
            }
        }
    }
}

fn chapter_row(c: usize, chapter: &'static Chapter) -> Element {
    let number = course::chapter_number(c);
    let minutes: u32 = chapter.lessons.iter().map(|l| l.minutes).sum();
    let count = chapter.lessons.len();
    let meta = format!("{count} lessons \u{00b7} {minutes} min");
    ui! {
        view(style = ChapterRow()) {
            view(style = ChapterInfo()) {
                text(style = ChapterNumber()) { number }
                text(style = ChapterTitle()) { chapter.title }
                text(style = ChapterSummary()) { chapter.summary }
                text(style = ChapterMeta()) { meta }
            }
            view(style = LessonGrid()) {
                for (i, lesson) in chapter.lessons.iter().enumerate() {
                    lesson_item(course::lesson_number(c, i), lesson)
                }
            }
        }
    }
}

fn lesson_item(number: String, lesson: &'static course::Lesson) -> Element {
    let minutes = format!("{} min", lesson.minutes);
    // An authoring marker for whoever is extending the course: shown in
    // debug builds, compiled out of a release.
    let expand = cfg!(debug_assertions) && lesson.status == Status::Expand;
    match lesson.route {
        // The cell is the grid's flex item; the link inside it fills it.
        Some(route) => ui! {
            view(style = LessonCell()) {
                link(route = route, params = ()) {
                    view(style = LessonItem()) {
                        text(style = LessonNumber()) { number }
                        text(style = LessonTitle()) { lesson.title }
                        if expand {
                            text(style = LessonSoon()) { "Expand" }
                        }
                        text(style = LessonMinutes()) { minutes }
                    }
                }
            }
        },
        None => ui! {
            view(style = LessonCell()) {
                view(style = LessonItemPlanned()) {
                    text(style = LessonNumber()) { number }
                    text(style = LessonTitle()) { lesson.title }
                    text(style = LessonSoon()) { "Soon" }
                    text(style = LessonMinutes()) { minutes }
                }
            }
        },
    }
}
