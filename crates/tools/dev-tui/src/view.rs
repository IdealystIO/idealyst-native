//! What the panel shows, as lines: a pure function of the [`Model`], the
//! time, the spinner frame and the terminal's size.
//!
//! The panel is a character grid, so layout is line budgeting: every
//! block is a list of one-line entries, truncated to the width, and the
//! blocks share the height — the rows and the footer always fit, the
//! history gives way to an expanded error, and the log pane takes what
//! is left. The components in `crate::panel` render a [`Screen`] and do
//! no arithmetic of their own.

use dev_events::HotTier;

use crate::model::{Model, Save, State, Target};

/// How a line is coloured.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Tone {
    /// Secondary text: idle rows, times, section titles, the footer.
    Muted,
    /// Plain text.
    Normal,
    /// Something is in progress.
    Busy,
    /// Something landed.
    Ok,
    /// Something failed.
    Error,
}

impl Default for Tone {
    fn default() -> Self {
        Tone::Normal
    }
}

/// One line and its colour.
#[derive(Debug, Clone, Default, PartialEq, Eq, Hash)]
pub struct Line {
    pub text: String,
    pub tone: Tone,
    /// Unique within its block, and changes whenever the line does: the
    /// panel's keyed lists rebuild exactly the lines that changed.
    pub key: String,
}

impl Line {
    fn new(text: impl Into<String>, tone: Tone) -> Self {
        Self { text: text.into(), tone, key: String::new() }
    }
}

/// One target's row: its name, and its state in its state's colour.
#[derive(Debug, Clone, Default, PartialEq, Eq, Hash)]
pub struct Row {
    pub name: String,
    pub status: String,
    pub tone: Tone,
    /// As [`Line::key`].
    pub key: String,
}

/// Number each line's key by position and content.
fn keyed(lines: Vec<Line>) -> Vec<Line> {
    lines
        .into_iter()
        .enumerate()
        .map(|(i, mut l)| {
            l.key = format!("{i}\u{1f}{:?}\u{1f}{}", l.tone, l.text);
            l
        })
        .collect()
}

/// Panel toggles the keys flip.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Toggles {
    /// `l`: the log pane is open.
    pub log: bool,
    /// `e`: the error shows its full rendering.
    pub expanded: bool,
}

/// Everything on screen.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Screen {
    pub header: Line,
    pub rows: Vec<Row>,
    pub history: Vec<Line>,
    pub error: Vec<Line>,
    pub log: Vec<Line>,
    pub footer: Line,
}

/// The keys, as the footer lists them.
pub const FOOTER: &str = "r rebuild · l log · e error · c clear · q quit";

/// Braille spinner: one glyph per frame, no motion beyond that.
const SPINNER: [char; 10] = ['⠋', '⠙', '⠹', '⠸', '⠼', '⠴', '⠦', '⠧', '⠇', '⠏'];

/// Width of a target's name column.
const NAME_COL: usize = 16;
/// Width of the cargo progress bar.
const BAR: usize = 20;
/// History lines shown when nothing else competes for the height.
const HISTORY_MAX: usize = 8;
/// History lines kept when the log or an expanded error wants room.
const HISTORY_MIN: usize = 3;

/// Lay the model out for a `width` x `height` terminal at session time
/// `now_ms`, with spinner frame `frame`.
pub fn screen(model: &Model, toggles: Toggles, now_ms: u64, frame: u64, width: usize, height: usize) -> Screen {
    let fit = |s: String| truncate(&s, width);
    let header = Line::new(fit(header(model)), Tone::Normal);
    let rows: Vec<Row> = model
        .targets
        .iter()
        .map(|t| {
            let (status, tone) = status(t, now_ms, frame);
            let name = pad(&t.name, NAME_COL);
            let status = truncate(&status, width.saturating_sub(NAME_COL + 2));
            let key = format!("{}\u{1f}{tone:?}\u{1f}{status}", t.name);
            Row { name, status, tone, key }
        })
        .collect();

    // Fixed: header, blank, rows, blank, the saves title, blank, footer.
    let fixed = 1 + 1 + rows.len() + 1 + 1 + 1 + 1;
    let mut budget = height.saturating_sub(fixed);

    let mut error = Vec::new();
    if let Some(e) = &model.error {
        let head = match &e.location {
            Some(loc) => format!("  ✗ {loc}  {}", e.message),
            None => format!("  ✗ {}", e.message),
        };
        let more = if e.count > 1 { format!("  (+{} more)", e.count - 1) } else { String::new() };
        let hint = if toggles.expanded { "" } else { "  e: expand" };
        error.push(Line::new(fit(format!("{head}{more}{hint}")), Tone::Error));
        if toggles.expanded {
            // Keep room for a few saves and, if open, a few log lines.
            let reserve = HISTORY_MIN + if toggles.log { 4 } else { 0 };
            let room = budget.saturating_sub(1 + reserve + 1);
            for l in e.rendered.lines().take(room) {
                error.push(Line::new(fit(format!("    {l}")), Tone::Normal));
            }
        }
        error.push(Line::default());
    }
    budget = budget.saturating_sub(error.len());

    let history_room = if toggles.log || toggles.expanded { HISTORY_MIN } else { HISTORY_MAX };
    let mut history: Vec<Line> = model
        .history
        .iter()
        .take(history_room.min(budget))
        .map(|s| save_line(s, width))
        .collect();
    if history.is_empty() && budget > 0 {
        history.push(Line::new("  no saves yet", Tone::Muted));
    }
    budget = budget.saturating_sub(history.len());

    let mut log = Vec::new();
    if toggles.log && budget > 2 {
        // A blank line and a title, then the newest lines that fit.
        log.push(Line::default());
        log.push(Line::new("  log", Tone::Muted));
        let room = budget - 2;
        let skip = model.log.len().saturating_sub(room);
        for l in model.log.iter().skip(skip) {
            log.push(Line::new(fit(format!("  {l}")), Tone::Muted));
        }
    }

    Screen {
        header,
        rows,
        history: keyed(history),
        error: keyed(error),
        log: keyed(log),
        footer: Line::new(fit(FOOTER.to_string()), Tone::Muted),
    }
}

fn header(m: &Model) -> String {
    let mut parts = vec!["idealyst dev".to_string()];
    if !m.app.is_empty() {
        parts.push(m.app.clone());
    }
    if !m.mode.is_empty() {
        parts.push(m.mode.clone());
    }
    parts.extend(m.urls.iter().cloned());
    match &m.hot_tier {
        Some(HotTier::Armed) => parts.push("hot patch armed".into()),
        Some(HotTier::Off { reason }) => parts.push(format!("hot patch off ({reason})")),
        None => {}
    }
    parts.join(" · ")
}

/// A row's status text and colour.
fn status(t: &Target, now_ms: u64, frame: u64) -> (String, Tone) {
    let spin = SPINNER[(frame as usize) % SPINNER.len()];
    let elapsed = secs(now_ms.saturating_sub(t.since_ms));
    match &t.state {
        State::Starting => ("○ starting".into(), Tone::Muted),
        State::Watching => ("● watching".into(), Tone::Muted),
        State::Ready { ms } => (format!("● ready · built in {}", secs(*ms)), Tone::Muted),
        State::Changed { files } => (format!("{spin} change: {} · deciding", files.join(", ")), Tone::Busy),
        State::ApplyingOverlay { sites } => {
            (format!("{spin} applying overlay · {} · {elapsed}", sites_word(*sites)), Tone::Busy)
        }
        State::BuildingPatch { crates } => {
            (format!("{spin} building hot patch · {} · {elapsed}", crates.join(", ")), Tone::Busy)
        }
        State::Building { label, stage, compiled, total, current } => {
            let mut s = format!("{spin} {label}");
            if let Some(stage) = stage {
                s.push_str(&format!(" · {stage}"));
            }
            if stage.as_deref() == Some("cargo") || *compiled > 0 {
                match total {
                    Some(total) => s.push_str(&format!(" {compiled}/{total} {}", bar(*compiled, *total))),
                    None => s.push_str(&format!(" {compiled} crates")),
                }
                if let Some(c) = current {
                    s.push_str(&format!(" {c}"));
                }
            }
            s.push_str(&format!(" · {elapsed}"));
            (s, Tone::Busy)
        }
        State::Applied { sites, ms } => {
            (format!("✓ overlay applied · {} · {}", sites_word(*sites), ms_text(*ms)), Tone::Ok)
        }
        State::Patched { redirected, ms } => {
            let what = redirected.map(|n| format!(" · {n} fn")).unwrap_or_default();
            (format!("✓ hot patched{what} · {}", ms_text(*ms)), Tone::Ok)
        }
        State::Reloaded { ms } => (format!("✓ rebuilt · {} · reloaded", secs(*ms)), Tone::Ok),
        State::Unchanged { ms } => (format!("✓ rebuilt, nothing changed · {}", secs(*ms)), Tone::Ok),
        State::Failed { summary } => (format!("✗ build failed · {summary}"), Tone::Error),
    }
}

fn save_line(s: &Save, width: usize) -> Line {
    let files = if s.files.is_empty() { "—".to_string() } else { s.files.join(", ") };
    let ms = s.ms.map(ms_text).unwrap_or_else(|| "…".into());
    let ack = if s.acked { "  ✓ page" } else { "" };
    let text = format!(
        "  {}  {}  {}  {:>8}  {}{ack}",
        clock(s.at_ms),
        pad(&truncate(&files, 24), 24),
        pad(&s.tier, 10),
        ms,
        s.detail,
    );
    let tone = if s.failed {
        Tone::Error
    } else if s.ms.is_some() {
        Tone::Normal
    } else {
        Tone::Busy
    };
    Line::new(truncate(text.trim_end(), width), tone)
}

fn bar(done: u32, total: u32) -> String {
    let filled = if total == 0 { BAR } else { ((done as usize * BAR) / total as usize).min(BAR) };
    format!("{}{}", "━".repeat(filled), "─".repeat(BAR - filled))
}

fn sites_word(n: usize) -> String {
    if n == 1 { "1 site".into() } else { format!("{n} sites") }
}

fn ms_text(ms: u64) -> String {
    if ms >= 1000 { secs(ms) } else { format!("{ms} ms") }
}

fn secs(ms: u64) -> String {
    format!("{:.1}s", ms as f64 / 1000.0)
}

/// Session time as `mm:ss`.
fn clock(ms: u64) -> String {
    let s = ms / 1000;
    format!("{:02}:{:02}", s / 60, s % 60)
}

fn pad(s: &str, w: usize) -> String {
    let n = s.chars().count();
    if n >= w { s.to_string() } else { format!("{s}{}", " ".repeat(w - n)) }
}

/// At most `w` characters, the last one an ellipsis when cut.
pub fn truncate(s: &str, w: usize) -> String {
    let n = s.chars().count();
    if n <= w {
        return s.to_string();
    }
    if w == 0 {
        return String::new();
    }
    let mut out: String = s.chars().take(w - 1).collect();
    out.push('…');
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn truncation_keeps_the_width_and_marks_the_cut() {
        assert_eq!(truncate("abcdef", 4), "abc…");
        assert_eq!(truncate("abc", 4), "abc");
        assert_eq!(truncate("⠋ building", 3), "⠋ …");
    }

    #[test]
    fn the_bar_fills_in_proportion_and_never_overflows() {
        assert_eq!(bar(0, 10), "─".repeat(BAR));
        assert_eq!(bar(5, 10), format!("{}{}", "━".repeat(BAR / 2), "─".repeat(BAR / 2)));
        assert_eq!(bar(12, 10), "━".repeat(BAR));
    }

    #[test]
    fn everything_fits_the_height_with_the_log_open() {
        let mut m = Model::new(&["web".to_string()]);
        for i in 0..50 {
            m.log.push_back(format!("line {i}"));
        }
        let s = screen(&m, Toggles { log: true, expanded: false }, 0, 0, 100, 30);
        let total = 1 + 1 + s.rows.len() + 1 + 1 + s.history.len() + s.error.len() + s.log.len() + 1 + 1;
        assert!(total <= 30, "{total} lines for 30 rows");
        assert_eq!(s.log.last().unwrap().text, "  line 49", "the newest lines show");
    }
}
