//! The course outline: numbered chapters of numbered lessons.
//!
//! [`CHAPTERS`] is the single source of truth. The landing page renders
//! all of it — including lessons not yet written, as the roadmap; the
//! sidebar lists the written ones under their chapter; prev/next walks the
//! written lessons in order; and each lesson's header reads its number and
//! duration from here. A lesson's number is its chapter's number and its
//! position (`1.2` is chapter 01's second lesson), so reordering this table
//! renumbers everything.

use runtime_core::Route;

use crate::routes::*;

/// Where a lesson stands. Authoring bookkeeping for whoever extends the
/// course; readers only ever see written vs. coming soon.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum Status {
    /// Written for the course.
    Done,
    /// Written, but an outline that should grow into a full lesson.
    Expand,
    /// Not written yet.
    Planned,
}

pub struct Lesson {
    pub title: &'static str,
    /// Estimated reading-and-doing time.
    pub minutes: u32,
    /// The lesson's page, once written.
    pub route: Option<&'static Route<()>>,
    pub status: Status,
}

pub struct Chapter {
    pub title: &'static str,
    pub summary: &'static str,
    pub lessons: &'static [Lesson],
}

const fn done(title: &'static str, minutes: u32, route: &'static Route<()>) -> Lesson {
    Lesson { title, minutes, route: Some(route), status: Status::Done }
}

const fn expand(title: &'static str, minutes: u32, route: &'static Route<()>) -> Lesson {
    Lesson { title, minutes, route: Some(route), status: Status::Expand }
}

const fn planned(title: &'static str, minutes: u32) -> Lesson {
    Lesson { title, minutes, route: None, status: Status::Planned }
}

pub const CHAPTERS: &[Chapter] = &[
    Chapter {
        title: "Get set up",
        summary: "Install the toolchain, create a project, and learn what each file in it does.",
        lessons: &[
            done("Install the CLI & toolchain", 8, &START_INSTALL_ROUTE),
            done("Your first project", 6, &START_CREATE_ROUTE),
            done("Hot reload & the dev loop", 10, &START_RUN_ROUTE),
            done("Editor setup: VS Code", 8, &START_EDITOR_ROUTE),
            done("AI-assisted development with MCP", 10, &START_MCP_ROUTE),
            done("Anatomy of a project", 10, &START_ANATOMY_ROUTE),
        ],
    },
    Chapter {
        title: "Building UI",
        summary: "Describe screens as a tree of primitives and components.",
        lessons: &[
            done("ui! and primitives", 12, &UI_PRIMITIVES_ROUTE),
            done("Components & props", 12, &UI_COMPONENTS_ROUTE),
            done("Children & composition", 10, &UI_CHILDREN_ROUTE),
            done("State & events", 12, &UI_STATE_ROUTE),
            done("Lists & conditionals", 12, &UI_FLOW_ROUTE),
            done("Static vs reactive props", 10, &UI_REACTIVE_PROPS_ROUTE),
        ],
    },
    Chapter {
        title: "Reactivity",
        summary: "Signals, the flush, effects and memos: the one engine everything runs on.",
        lessons: &[
            done("One reactive engine", 8, &CORE_ENGINE_ROUTE),
            done("Signals", 12, &RX_SIGNALS_ROUTE),
            done("The flush boundary", 10, &RX_FLUSH_ROUTE),
            done("Effects & cleanup", 12, &RX_EFFECTS_ROUTE),
            done("Derived state", 10, &RX_DERIVED_ROUTE),
            done("Under the hood: the flush", 10, &CORE_FLUSH_ROUTE),
        ],
    },
    Chapter {
        title: "Styling",
        summary: "Declare a look once and every platform renders it natively.",
        lessons: &[
            done("Your first stylesheet", 12, &STY_SHEETS_ROUTE),
            done("Layout with flexbox", 12, &STY_LAYOUT_ROUTE),
            done("Themes & style tokens", 14, &STY_THEMES_ROUTE),
            planned("Typography & fonts", 10),
        ],
    },
    Chapter {
        title: "Component stylesheets",
        summary: "Variants, interaction states and slots — and restyling a component library without forking it.",
        lessons: &[
            done("Variants", 10, &CS_VARIANTS_ROUTE),
            done("Interaction states", 10, &CS_STATES_ROUTE),
            done("Overrides & compounds", 10, &CS_OVERRIDES_ROUTE),
            done("Component styles: slots", 14, &CS_SLOTS_ROUTE),
            done("Restyling & extending idea-ui", 14, &CS_RESTYLE_ROUTE),
        ],
    },
    Chapter {
        title: "Responsive design",
        summary: "One layout that adapts from a phone to a desktop window.",
        lessons: &[
            done("Breakpoint overlays", 10, &MQ_BREAKPOINTS_ROUTE),
            done("Mobile-first", 8, &MQ_MOBILE_FIRST_ROUTE),
            done("The breakpoint signal", 8, &MQ_SIGNAL_ROUTE),
            done("Container queries", 10, &MQ_CONTAINER_ROUTE),
        ],
    },
    Chapter {
        title: "The idea-ui kit",
        summary: "A themeable component library built from everything so far.",
        lessons: &[
            planned("Installing idea-ui and its theme", 8),
            planned("Forms & inputs", 12),
            planned("Modals, menus & toasts", 12),
            planned("Tables & data", 12),
        ],
    },
    Chapter {
        title: "Navigation",
        summary: "Routes, stacks, tabs and drawers, with state that lives in the URL.",
        lessons: &[
            planned("Routes & screens", 10),
            planned("Stacks, tabs & drawers", 12),
            planned("Params & state in the URL", 10),
            planned("Deep links", 8),
        ],
    },
    Chapter {
        title: "Data & async",
        summary: "Talk to the network, persist data, and keep async work tied to the screen that started it.",
        lessons: &[
            planned("Fetching with net", 10),
            planned("spawn_then & component lifetimes", 10),
            planned("Persisting with storage", 10),
            planned("Server functions", 12),
        ],
    },
    Chapter {
        title: "Animation",
        summary: "Transitions, springs, and animating elements in and out.",
        lessons: &[
            planned("Style transitions", 8),
            planned("Animated values & springs", 12),
            planned("Presence: animating in and out", 10),
        ],
    },
    Chapter {
        title: "Accessibility",
        summary: "What the framework does for you, and the model behind it.",
        lessons: &[
            done("Accessible by default", 8, &A11Y_DEFAULTS_ROUTE),
            done("The accessibility model", 10, &A11Y_MODEL_ROUTE),
        ],
    },
    Chapter {
        title: "How it works",
        summary: "The layers under your app, and how to extend them.",
        lessons: &[
            done("The layered model", 10, &ARCH_OVERVIEW_ROUTE),
            done("Direct vs hosted runtime", 10, &ARCH_BACKENDS_ROUTE),
            done("Catalog, docs & MCP", 10, &ARCH_CATALOG_ROUTE),
            done("SDKs", 8, &ARCH_SDKS_ROUTE),
            expand("Custom backends", 12, &ADV_BACKENDS_ROUTE),
            expand("Interactive CLIs", 10, &ADV_CLI_ROUTE),
            expand("Embedded rendering", 10, &ADV_EMBEDDED_ROUTE),
        ],
    },
    Chapter {
        title: "Test & ship",
        summary: "End-to-end tests on every platform, then out to the web and the stores.",
        lessons: &[
            planned("End-to-end tests with the robot", 12),
            planned("Deploying to the web", 8),
            planned("Shipping to the App Store & Google Play", 12),
            planned("Over-the-air updates", 10),
        ],
    },
];

/// `00`, `01`, … — a chapter's printed number.
pub fn chapter_number(chapter: usize) -> String {
    format!("{chapter:02}")
}

/// `0.1`, `1.2`, … — a lesson's printed number.
pub fn lesson_number(chapter: usize, lesson: usize) -> String {
    format!("{chapter}.{}", lesson + 1)
}

/// Where a written lesson sits: (chapter index, lesson index, lesson).
pub fn find(route_name: &str) -> Option<(usize, usize, &'static Lesson)> {
    CHAPTERS.iter().enumerate().find_map(|(c, chapter)| {
        chapter
            .lessons
            .iter()
            .enumerate()
            .find(|(_, l)| l.route.is_some_and(|r| r.name() == route_name))
            .map(|(i, l)| (c, i, l))
    })
}

/// Every written lesson in course order, with its number.
pub fn written() -> Vec<(String, &'static Lesson)> {
    let mut out = Vec::new();
    for (c, chapter) in CHAPTERS.iter().enumerate() {
        for (i, lesson) in chapter.lessons.iter().enumerate() {
            if lesson.route.is_some() {
                out.push((lesson_number(c, i), lesson));
            }
        }
    }
    out
}

/// The written lessons before and after `route_name`, for prev/next.
pub fn neighbors(route_name: &str) -> (Option<(String, &'static Lesson)>, Option<(String, &'static Lesson)>) {
    let order = written();
    let Some(i) = order
        .iter()
        .position(|(_, l)| l.route.is_some_and(|r| r.name() == route_name))
    else {
        // The landing page leads into the first lesson.
        return (None, order.into_iter().next());
    };
    let prev = i.checked_sub(1).and_then(|p| order.get(p).cloned());
    let next = order.get(i + 1).cloned();
    (prev, next)
}

/// Totals for the landing page: (chapters, lessons, minutes).
pub fn totals() -> (usize, usize, u32) {
    let lessons = CHAPTERS.iter().map(|c| c.lessons.len()).sum();
    let minutes = CHAPTERS.iter().flat_map(|c| c.lessons).map(|l| l.minutes).sum();
    (CHAPTERS.len(), lessons, minutes)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_route_appears_in_the_course_once() {
        let names: Vec<&str> = CHAPTERS
            .iter()
            .flat_map(|c| c.lessons)
            .filter_map(|l| l.route.map(|r| r.name()))
            .collect();
        let mut unique = names.clone();
        unique.sort_unstable();
        unique.dedup();
        assert_eq!(names.len(), unique.len(), "a page is listed twice");
    }

    #[test]
    fn lessons_number_from_their_chapter() {
        let (c, i, lesson) = find(UI_COMPONENTS_ROUTE.name()).expect("listed");
        assert_eq!(lesson_number(c, i), "1.2");
        assert_eq!(lesson.title, "Components & props");
        assert_eq!(chapter_number(0), "00");
    }

    #[test]
    fn prev_next_skip_unwritten_lessons() {
        // 3.3 is followed by a planned lesson (3.4), then 4.1.
        let (prev, next) = neighbors(STY_THEMES_ROUTE.name());
        assert_eq!(prev.map(|(n, _)| n).as_deref(), Some("3.2"));
        assert_eq!(next.map(|(n, _)| n).as_deref(), Some("4.1"));
    }

    #[test]
    fn a_status_and_a_route_agree() {
        for lesson in CHAPTERS.iter().flat_map(|c| c.lessons) {
            assert_eq!(lesson.route.is_some(), lesson.status != Status::Planned, "{}", lesson.title);
        }
    }
}
