//! Benchmark runner — the page that drives each variant in
//! sequence through an iframe, collects per-iteration results,
//! and renders the comparison table. Built using the same
//! `framework-core` + `backend-web` stack as the idealyst-native
//! variant, so the runner is itself a dogfood case for the
//! framework.
//!
//! High-level architecture:
//!
//!   1. Hardcoded `VARIANTS` registry — id, label, URL.
//!   2. Hardcoded `rebuild` suite metadata (title, param defs).
//!      Suites live in `benchmark/suites/<name>.js` and run
//!      *inside* the variant iframe — the runner never imports
//!      them, it just builds the URL with the right query params
//!      and reads the postMessage'd results back.
//!   3. Reactive state via `Signal`s: selected variants, params,
//!      results map, current-variant pointer, run-in-progress flag.
//!   4. A `WebView` primitive in the main pane drives the iframe.
//!      Its `on_message` callback parses the JSON payload from the
//!      variant and updates the results signal; that re-renders
//!      the table. Its reactive `url` is wired to a signal we
//!      mutate as the run sequence advances.
//!   5. Run loop: when the Run button fires, we snapshot
//!      selections, shuffle, and walk the queue. Each variant's
//!      completion (signaled by a `bench-result` or `bench-error`
//!      message) advances to the next one.

use backend_web::WebBackend;
use framework_core::{
    button, signal, stylesheet, text_input, toggle, ui, web_view, AlignItems, Color,
    FlexDirection, JustifyContent, Length, Overflow, Primitive, Signal, TokenEntry, TokenValue,
    Tokenized,
};
use framework_theme::{install_theme, ThemeTokens};
use std::cell::RefCell;
use std::collections::{HashMap, HashSet};
use std::rc::Rc;
use wasm_bindgen::prelude::*;

// =============================================================================
// Theme
// =============================================================================

#[derive(Clone)]
pub struct Theme {
    pub bg: Tokenized<Color>,
    pub surface: Tokenized<Color>,
    pub surface_alt: Tokenized<Color>,
    pub border: Tokenized<Color>,
    pub text: Tokenized<Color>,
    pub muted: Tokenized<Color>,
    pub primary: Tokenized<Color>,
    pub primary_text: Tokenized<Color>,
    pub accent: Tokenized<Color>,
    pub winner_bg: Tokenized<Color>,
    pub winner_text: Tokenized<Color>,
    pub running: Tokenized<Color>,
    pub error: Tokenized<Color>,
}

fn tok_color(name: &'static str, fallback: &str) -> Tokenized<Color> {
    Tokenized::token(name, Color(fallback.into()))
}

fn bench_theme() -> Theme {
    Theme {
        bg:           tok_color("bench-bg",           "#0f1115"),
        surface:      tok_color("bench-surface",      "#14171c"),
        surface_alt:  tok_color("bench-surface-alt",  "#1a1d24"),
        border:       tok_color("bench-border",       "#2a2e3a"),
        text:         tok_color("bench-text",         "#e8eaf0"),
        muted:        tok_color("bench-muted",        "#9099a8"),
        primary:      tok_color("bench-primary",      "#5b6cff"),
        primary_text: tok_color("bench-primary-text", "#ffffff"),
        accent:       tok_color("bench-accent",       "#b7c2ff"),
        winner_bg:    tok_color("bench-winner-bg",    "#1e2540"),
        winner_text:  tok_color("bench-winner-text",  "#b7c2ff"),
        running:      tok_color("bench-running",      "#ffb84d"),
        error:        tok_color("bench-error",        "#ff6b6b"),
    }
}

impl ThemeTokens for Theme {
    fn tokens(&self) -> Vec<TokenEntry> {
        fn entry(t: &Tokenized<Color>) -> TokenEntry {
            let name = t.name().expect("theme fields must be Tokenized::Token");
            TokenEntry { name, value: TokenValue::Color(t.value().clone()) }
        }
        vec![
            entry(&self.bg), entry(&self.surface), entry(&self.surface_alt),
            entry(&self.border), entry(&self.text), entry(&self.muted),
            entry(&self.primary), entry(&self.primary_text), entry(&self.accent),
            entry(&self.winner_bg), entry(&self.winner_text),
            entry(&self.running), entry(&self.error),
        ]
    }
}

// =============================================================================
// Stylesheets
// =============================================================================

stylesheet! {
    pub Root<Theme> {
        base(_t) {
            flex_direction: FlexDirection::Row,
            min_height: Length::pct(100.0),
            background: Tokenized::token("bench-bg", Color("#0f1115".into())),
            color: Tokenized::token("bench-text", Color("#e8eaf0".into())),
        }
    }
}

stylesheet! {
    pub Sidebar<Theme> {
        base(_t) {
            width: 320.0,
            padding_horizontal: 20.0,
            padding_vertical: 16.0,
            background: Tokenized::token("bench-surface", Color("#14171c".into())),
            border_right_width: 1.0,
            border_right_color: Tokenized::token("bench-border", Color("#2a2e3a".into())),
            gap: Length::Px(12.0),
            flex_direction: FlexDirection::Column,
        }
    }
}

stylesheet! {
    pub SidebarH1<Theme> {
        base(_t) {
            font_size: 18.0,
            font_weight: framework_core::FontWeight::SemiBold,
            color: Tokenized::token("bench-text", Color("#e8eaf0".into())),
        }
    }
}

stylesheet! {
    pub Lede<Theme> {
        base(_t) {
            font_size: 12.0,
            color: Tokenized::token("bench-muted", Color("#9099a8".into())),
            margin_bottom: 6.0,
        }
    }
}

stylesheet! {
    pub SectionH2<Theme> {
        base(_t) {
            font_size: 11.0,
            font_weight: framework_core::FontWeight::SemiBold,
            color: Tokenized::token("bench-muted", Color("#9099a8".into())),
            letter_spacing: 1.0,
            margin_top: 10.0,
        }
    }
}

stylesheet! {
    pub ParamRow<Theme> {
        base(_t) {
            flex_direction: FlexDirection::Row,
            align_items: AlignItems::Center,
            justify_content: JustifyContent::SpaceBetween,
            gap: Length::Px(8.0),
            padding_vertical: 4.0,
        }
    }
}

stylesheet! {
    pub ParamLabel<Theme> {
        base(_t) {
            font_size: 13.0,
            color: Tokenized::token("bench-text", Color("#e8eaf0".into())),
        }
    }
}

stylesheet! {
    pub ParamInput<Theme> {
        base(_t) {
            width: 90.0,
            padding_vertical: 4.0,
            padding_horizontal: 8.0,
            border_radius: 4.0,
            border_width: 1.0,
            border_color: Tokenized::token("bench-border", Color("#2a2e3a".into())),
            background: Tokenized::token("bench-surface-alt", Color("#1a1d24".into())),
            color: Tokenized::token("bench-text", Color("#e8eaf0".into())),
            font_size: 13.0,
        }
    }
}

stylesheet! {
    pub VariantRow<Theme> {
        base(_t) {
            flex_direction: FlexDirection::Row,
            align_items: AlignItems::Center,
            justify_content: JustifyContent::SpaceBetween,
            padding_vertical: 4.0,
        }
    }
}

stylesheet! {
    pub VariantLabel<Theme> {
        base(_t) {
            flex_direction: FlexDirection::Row,
            align_items: AlignItems::Center,
            gap: Length::Px(8.0),
            font_size: 13.0,
            color: Tokenized::token("bench-text", Color("#e8eaf0".into())),
        }
    }
}

stylesheet! {
    pub RunButton<Theme> {
        base(_t) {
            margin_top: 16.0,
            padding_vertical: 10.0,
            background: Tokenized::token("bench-primary", Color("#5b6cff".into())),
            color: Tokenized::token("bench-primary-text", Color("#ffffff".into())),
            border_radius: 6.0,
            font_weight: framework_core::FontWeight::SemiBold,
            font_size: 14.0,
        }
    }
}

stylesheet! {
    pub Main<Theme> {
        base(_t) {
            flex_direction: FlexDirection::Column,
            padding_horizontal: 20.0,
            padding_vertical: 16.0,
            gap: Length::Px(16.0),
            // `flex: 1` is the default for non-fixed children; the
            // sidebar pinning at 320px leaves the rest for main.
        }
    }
}

stylesheet! {
    pub FrameWrap<Theme> {
        base(_t) {
            background: Tokenized::token("bench-surface-alt", Color("#1a1d24".into())),
            border_width: 1.0,
            border_color: Tokenized::token("bench-border", Color("#2a2e3a".into())),
            border_radius: 8.0,
            overflow: Overflow::Hidden,
        }
    }
}

stylesheet! {
    pub FrameHeader<Theme> {
        base(_t) {
            flex_direction: FlexDirection::Row,
            align_items: AlignItems::Center,
            justify_content: JustifyContent::SpaceBetween,
            padding_horizontal: 12.0,
            padding_vertical: 8.0,
            border_bottom_width: 1.0,
            border_bottom_color: Tokenized::token("bench-border", Color("#2a2e3a".into())),
            font_size: 12.0,
            color: Tokenized::token("bench-muted", Color("#9099a8".into())),
        }
    }
}

stylesheet! {
    pub FrameHeaderStrong<Theme> {
        base(_t) {
            font_weight: framework_core::FontWeight::SemiBold,
            color: Tokenized::token("bench-text", Color("#e8eaf0".into())),
        }
    }
}

stylesheet! {
    pub FrameHeaderElapsed<Theme> {
        base(_t) {
            color: Tokenized::token("bench-accent", Color("#b7c2ff".into())),
        }
    }
}

stylesheet! {
    pub IframeStyle<Theme> {
        base(_t) {
            width: Length::pct(100.0),
            height: 480.0,
        }
    }
}

stylesheet! {
    pub ResultsWrap<Theme> {
        base(_t) {
            background: Tokenized::token("bench-surface", Color("#14171c".into())),
            border_width: 1.0,
            border_color: Tokenized::token("bench-border", Color("#2a2e3a".into())),
            border_radius: 8.0,
            padding_horizontal: 16.0,
            padding_vertical: 12.0,
            overflow: Overflow::Hidden,
        }
    }
}

stylesheet! {
    pub TableHeaderRow<Theme> {
        base(_t) {
            flex_direction: FlexDirection::Row,
            padding_vertical: 4.0,
            border_bottom_width: 1.0,
            border_bottom_color: Tokenized::token("bench-border", Color("#262a35".into())),
        }
    }
}

stylesheet! {
    pub TableHeaderCell<Theme> {
        base(_t) {
            // Equal-width columns: every header cell gets the same
            // flex share. `flex_basis = 0` so growth starts from
            // zero and is divided evenly across cells regardless of
            // their content's intrinsic size. Without this, columns
            // with wider text (e.g. "10K WORST" vs "STATUS") would
            // pull more space and the rows wouldn't grid-align.
            flex_grow: 1.0,
            flex_basis: Length::Px(0.0),
            font_size: 11.0,
            font_weight: framework_core::FontWeight::SemiBold,
            color: Tokenized::token("bench-muted", Color("#9099a8".into())),
            letter_spacing: 0.5,
            padding_horizontal: 8.0,
        }
    }
}

stylesheet! {
    pub TableRow<Theme> {
        base(_t) {
            flex_direction: FlexDirection::Row,
            padding_vertical: 4.0,
            border_bottom_width: 1.0,
            border_bottom_color: Tokenized::token("bench-border", Color("#262a35".into())),
        }
    }
}

stylesheet! {
    pub TableCell<Theme> {
        base(_t) {
            // Equal-width columns: must match `TableHeaderCell`'s
            // flex shape exactly or header and body columns drift
            // apart visually. See that stylesheet for the rationale.
            flex_grow: 1.0,
            flex_basis: Length::Px(0.0),
            font_size: 12.0,
            padding_horizontal: 8.0,
            color: Tokenized::token("bench-text", Color("#e8eaf0".into())),
        }
        variant kind {
            #[default]
            normal(_t) {}
            label(_t) {
                font_weight: framework_core::FontWeight::Medium,
            }
            running(_t) {
                color: Tokenized::token("bench-running", Color("#ffb84d".into())),
            }
            error(_t) {
                color: Tokenized::token("bench-error", Color("#ff6b6b".into())),
            }
            pending(_t) {
                color: Tokenized::token("bench-muted", Color("#6b7280".into())),
            }
            winner(_t) {
                color: Tokenized::token("bench-winner-text", Color("#b7c2ff".into())),
                font_weight: framework_core::FontWeight::Bold,
                background: Tokenized::token("bench-winner-bg", Color("#1e2540".into())),
            }
        }
    }
}

// =============================================================================
// Static registry + suite metadata
// =============================================================================

struct VariantInfo {
    id: &'static str,
    label: &'static str,
    url: &'static str,
    /// Suite names this variant implements. The sidebar filters
    /// the variant checklist to only those compatible with the
    /// currently-selected suite. Variants that don't implement a
    /// suite's required hooks would error at runtime, so we hide
    /// them upfront to avoid the noise.
    supports: &'static [&'static str],
}

const VARIANTS: &[VariantInfo] = &[
    VariantInfo {
        id: "vanilla-css-vars",     label: "vanilla · css vars",     url: "./vanilla-css-vars/",
        supports: &["rebuild", "toggle"],
    },
    VariantInfo {
        id: "vanilla-classes",      label: "vanilla · per-elem",     url: "./vanilla-classes/",
        supports: &["rebuild", "toggle"],
    },
    VariantInfo {
        id: "vanilla-classes-bulk", label: "vanilla · bulk innerHTML", url: "./vanilla-classes-bulk/",
        supports: &["rebuild", "toggle"],
    },
    VariantInfo {
        id: "react-naive",          label: "react · naive",          url: "./react-naive/",
        supports: &["rebuild", "toggle", "hierarchy"],
    },
    VariantInfo {
        id: "react-cssvars",        label: "react · cssvars",        url: "./react-cssvars/",
        supports: &["rebuild", "toggle"],
    },
    VariantInfo {
        id: "react-memo",           label: "react · memo",           url: "./react-memo/",
        supports: &["rebuild", "toggle", "hierarchy"],
    },
    VariantInfo {
        id: "vue",                  label: "vue",                    url: "./vue/",
        supports: &["rebuild", "toggle", "hierarchy"],
    },
    VariantInfo {
        id: "svelte",               label: "svelte",                 url: "./svelte/",
        supports: &["rebuild", "toggle", "hierarchy"],
    },
    VariantInfo {
        id: "idealyst-native",      label: "idealyst-native",        url: "./idealyst-native/",
        supports: &["rebuild", "toggle", "hierarchy"],
    },
];

struct ParamInfo {
    name: &'static str,
    label: &'static str,
    default: f64,
}

/// How a suite wants the TOTAL column computed. PAINT clusters
/// across frameworks for workloads where the visible change is a
/// browser-side animation (CSS transition, GPU compositing) — for
/// those, the differentiating metric is APPLY, the synchronous JS
/// work that kicked the transition off. Suites declare which makes
/// sense for their measurement.
#[derive(Clone, Copy)]
enum TotalMetric {
    /// Sum of per-bucket PAINT medians. Meaningful when PAINT
    /// captures the full click-to-pixels time — i.e., when the
    /// visible change is finished by the time `firstPaint` fires
    /// (rebuild: rows are mounted and laid out by then).
    PaintSum,
    /// Sum of per-bucket APPLY medians. Meaningful when PAINT is
    /// dominated by browser-side work that's identical across
    /// frameworks — e.g., the toggle suite's 250ms CSS transition,
    /// where every variant pays the same GPU interpolation cost
    /// regardless of which framework wrote the CSS variables.
    ApplySum,
}

/// Suite metadata mirrored from the JS suite modules. The runner
/// uses this to drive the sidebar's suite picker, the param form,
/// and the result-table column shape. The actual suite logic still
/// runs *inside* the variant iframe — the runner just builds the
/// right `?suite=NAME&...` URL and reads the posted results.
///
/// `bucket_labels` defines the column groups in the result table,
/// in stable emit order. Each suite's per-iteration runs carry a
/// `bucket: u32` value the runner sorts numerically; the first
/// sorted bucket maps to `bucket_labels[0]`, the next to
/// `bucket_labels[1]`, etc. For `rebuild` that's the row count
/// (1000 < 10000 → LOW first, HIGH second). For `toggle` it's the
/// transition direction (0 = light→dark first, 1 = dark→light
/// second).
struct SuiteInfo {
    name: &'static str,
    title: &'static str,
    params: &'static [ParamInfo],
    bucket_labels: &'static [&'static str],
    total: TotalMetric,
}

/// Result-column count for a suite. `TOTAL` + per-bucket APPLY +
/// per-bucket PAINT + per-bucket WORST. Updated in lockstep with
/// the table header builder.
const fn ncols(suite: &SuiteInfo) -> usize {
    1 + 3 * suite.bucket_labels.len()
}

const SUITES: &[SuiteInfo] = &[
    SuiteInfo {
        name: "rebuild",
        title: "Rebuild",
        params: &[
            ParamInfo { name: "rowsA",        label: "Min rows",     default: 1000.0  },
            ParamInfo { name: "rowsB",        label: "Max rows",     default: 10000.0 },
            ParamInfo { name: "iterations",   label: "Iterations",   default: 10.0    },
            ParamInfo { name: "warmupCycles", label: "Warmup cycles", default: 1.0    },
        ],
        bucket_labels: &["LOW", "HIGH"],
        // Rebuild's visible change (mounting N rows) is finished
        // by the time `firstPaint` fires; PAINT genuinely captures
        // click-to-pixels.
        total: TotalMetric::PaintSum,
    },
    SuiteInfo {
        name: "toggle",
        title: "Theme toggle",
        params: &[
            ParamInfo { name: "rows",         label: "Rows",          default: 1000.0 },
            ParamInfo { name: "iterations",   label: "Iterations",    default: 10.0   },
            ParamInfo { name: "warmupCycles", label: "Warmup toggles", default: 2.0   },
        ],
        // 0 = light→dark, 1 = dark→light. The suite emits these
        // bucket values verbatim; the runner labels them in this
        // order.
        bucket_labels: &["L→D", "D→L"],
        // The 250ms CSS transition is GPU-driven and identical
        // across frameworks — every variant's PAINT clusters at
        // APPLY + ~16ms (next rAF). The framework differences
        // live entirely in APPLY (the JS work that kicked the
        // transition off), so TOTAL sums APPLY here.
        total: TotalMetric::ApplySum,
    },
    SuiteInfo {
        name: "hierarchy",
        title: "Hierarchical render",
        params: &[
            ParamInfo { name: "seed",         label: "Seed",          default: 42.0   },
            ParamInfo { name: "nodes",        label: "Target nodes",  default: 2000.0 },
            // 0 means auto-size from `nodes` (log_2.55(nodes) + 2,
            // floor 8). Bump this to force a deeper tree at a
            // given leaf count — useful for stressing deep
            // nesting rather than wide fanout.
            ParamInfo { name: "maxDepth",     label: "Max depth (0=auto)", default: 0.0 },
            ParamInfo { name: "iterations",   label: "Iterations",    default: 20.0   },
            ParamInfo { name: "warmupCycles", label: "Warmup pairs",  default: 2.0    },
        ],
        // 0 = branch update (one leaf re-reads), 1 = global update
        // (every leaf re-reads). Alternated by the suite per
        // iteration. Surfaces what fine-grained reactivity buys.
        bucket_labels: &["BRANCH", "GLOBAL"],
        // PAINT here is dominated by browser rAF wait (same shape
        // as toggle). APPLY is where framework differences live —
        // the JS time spent fanning out through the reactive
        // graph (or, for React without memo, re-running the
        // entire component subtree).
        total: TotalMetric::ApplySum,
    },
];

fn suite_by_name(name: &str) -> &'static SuiteInfo {
    SUITES.iter().find(|s| s.name == name).expect("suite name not in SUITES registry")
}

// =============================================================================
// State
// =============================================================================

/// Per-iteration record posted by the variant. `bucket` is the
/// suite-specific discriminator the runner uses to group runs into
/// columns — see `SuiteInfo.bucket_labels`. For `rebuild` it's the
/// row count; for `toggle` it's 0 (light→dark) or 1 (dark→light).
#[derive(Clone)]
struct VariantRun {
    bucket: u32,
    apply: f64,
    first_paint: f64,
    worst_frame: f64,
}

#[derive(Clone)]
enum VariantStatus {
    Idle,
    Pending,
    Running,
    Done,
    Error(String),
}

#[derive(Clone)]
struct VariantEntry {
    status: VariantStatus,
    runs: Vec<VariantRun>,
}

impl Default for VariantEntry {
    fn default() -> Self {
        Self { status: VariantStatus::Idle, runs: Vec::new() }
    }
}

/// The pieces of state the run loop mutates. Held in a
/// thread_local so the postMessage callback closure can reach
/// them without having to capture every single signal.
struct RunnerState {
    selected_variants:    Signal<Rc<HashSet<&'static str>>>,
    /// Active suite (by `SuiteInfo.name`). Drives the param form,
    /// the table column shape, and the URL the runner builds for
    /// each variant. Changing this clears results — cross-suite
    /// numbers aren't comparable.
    current_suite:        Signal<&'static str>,
    /// Param signals keyed by `(suite_name, param_name)` so each
    /// suite has its own isolated form values. Switching suites
    /// preserves whatever the user last entered for each one.
    params:               HashMap<(&'static str, &'static str), Signal<String>>,
    /// Map of variant id → entry. Wrapped in a version-bumping
    /// signal so consumers (the results table) re-render when
    /// any entry changes without having to clone the whole map
    /// into the signal value.
    results:              Rc<RefCell<HashMap<&'static str, VariantEntry>>>,
    results_version:      Signal<u64>,
    current_variant:      Signal<Option<&'static str>>,
    current_status:       Signal<String>,
    iframe_url:           Signal<String>,
    run_in_progress:      Signal<bool>,
    run_finalized:        Signal<bool>,
    elapsed_seconds:      Signal<f64>,
    /// Run-loop queue. Pre-shuffled at click time so the order
    /// is fixed for the rest of the run.
    queue:                RefCell<Vec<&'static str>>,
    /// Wall-clock start so `elapsed_seconds` can tick during
    /// the run.
    run_start_ms:         RefCell<f64>,
    /// rAF handle for the ticker. Held so we can cancel on
    /// finish/abort.
    ticker_handle:        RefCell<Option<framework_core::RafLoop>>,
    /// Snapshot of params + suite for the current run, so the
    /// user can edit the form mid-run without affecting
    /// in-flight numbers.
    current_params:       RefCell<HashMap<&'static str, String>>,
    current_run_suite:    RefCell<&'static str>,
}

thread_local! {
    static STATE: RefCell<Option<Rc<RunnerState>>> = const { RefCell::new(None) };
}

fn state() -> Rc<RunnerState> {
    STATE.with(|s| s.borrow().as_ref().expect("state not initialized").clone())
}

// =============================================================================
// Results helpers
// =============================================================================

fn bump_results(st: &RunnerState) {
    st.results_version.update(|v| *v += 1);
}

fn set_status(st: &RunnerState, id: &'static str, status: VariantStatus) {
    {
        let mut r = st.results.borrow_mut();
        let entry = r.entry(id).or_default();
        entry.status = status;
    }
    bump_results(st);
}

fn set_runs(st: &RunnerState, id: &'static str, runs: Vec<VariantRun>, status: VariantStatus) {
    {
        let mut r = st.results.borrow_mut();
        let entry = r.entry(id).or_default();
        entry.runs = runs;
        entry.status = status;
    }
    bump_results(st);
}

/// Median of a vec of f64s. Empty → None. Matches the same
/// definition the HTML runner used: average of two middle
/// elements for even lengths.
fn median(xs: &[f64]) -> Option<f64> {
    if xs.is_empty() { return None; }
    let mut s: Vec<f64> = xs.to_vec();
    s.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
    let mid = s.len() / 2;
    if s.len() % 2 == 0 {
        Some((s[mid - 1] + s[mid]) / 2.0)
    } else {
        Some(s[mid])
    }
}

/// Returns the column medians for a variant in the order the
/// table renders them:
///   `[TOTAL, b1_apply, b1_paint, b2_apply, b2_paint, …,
///     b1_worst, b2_worst, …]`
///
/// TOTAL = sum of PAINT medians across all buckets, and is `None`
/// until every bucket has at least one measured iteration — a
/// partial sum mid-run would drop as more buckets come in and
/// read as misleading.
///
/// Length is `ncols(suite)`. Slots that have no data (e.g. when
/// the variant hasn't produced any runs in a bucket yet) are
/// `None` and render as "—".
fn column_medians(entry: &VariantEntry, suite: &SuiteInfo) -> Vec<Option<f64>> {
    let mut by_bucket: HashMap<u32, Vec<&VariantRun>> = HashMap::new();
    for r in &entry.runs {
        by_bucket.entry(r.bucket).or_default().push(r);
    }
    let mut buckets: Vec<u32> = by_bucket.keys().copied().collect();
    buckets.sort();
    // Trim to the suite's declared bucket count. If a variant
    // emits extra buckets we don't recognize, ignore them rather
    // than silently corrupting the table.
    buckets.truncate(suite.bucket_labels.len());

    let med_for = |bucket: u32, get: fn(&&VariantRun) -> f64| -> Option<f64> {
        let runs = by_bucket.get(&bucket)?;
        let vals: Vec<f64> = runs.iter().map(get).collect();
        median(&vals)
    };

    let n = suite.bucket_labels.len();
    let mut out: Vec<Option<f64>> = Vec::with_capacity(ncols(suite));

    // TOTAL = sum of per-bucket medians of either PAINT or APPLY,
    // depending on what the suite declared. None until every
    // declared bucket has data — partial sums shrink as buckets
    // come in and read as misleading.
    let totals_input: Vec<Option<f64>> = match suite.total {
        TotalMetric::PaintSum => (0..n)
            .map(|i| buckets.get(i).and_then(|b| med_for(*b, |r| r.first_paint)))
            .collect(),
        TotalMetric::ApplySum => (0..n)
            .map(|i| buckets.get(i).and_then(|b| med_for(*b, |r| r.apply)))
            .collect(),
    };
    let total = if totals_input.iter().all(|m| m.is_some()) {
        Some(totals_input.iter().map(|m| m.unwrap()).sum())
    } else {
        None
    };
    out.push(total);

    // Per-bucket APPLY + PAINT, interleaved.
    for i in 0..n {
        let b = buckets.get(i).copied();
        out.push(b.and_then(|b| med_for(b, |r| r.apply)));
        out.push(b.and_then(|b| med_for(b, |r| r.first_paint)));
    }
    // Per-bucket WORST at the end.
    for i in 0..n {
        let b = buckets.get(i).copied();
        out.push(b.and_then(|b| med_for(b, |r| r.worst_frame)));
    }
    out
}

/// Table header labels, derived from suite metadata. Same order
/// as `column_medians`. The first two are fixed (VARIANT, STATUS).
fn table_headers(suite: &SuiteInfo) -> Vec<String> {
    let mut h = vec![
        "VARIANT".to_string(),
        "STATUS".to_string(),
        "TOTAL".to_string(),
    ];
    for label in suite.bucket_labels {
        h.push(format!("{} APPLY", label));
        h.push(format!("{} PAINT", label));
    }
    for label in suite.bucket_labels {
        h.push(format!("{} WORST", label));
    }
    h
}

// =============================================================================
// URL builder + message parsing
// =============================================================================

fn build_variant_url(variant: &VariantInfo, suite_name: &str, params: &HashMap<&'static str, String>) -> String {
    let mut url = format!("{}?suite={}", variant.url, suite_name);
    for (k, v) in params {
        url.push('&');
        url.push_str(k);
        url.push('=');
        url.push_str(v);
    }
    url
}

/// Parse one of the three message envelopes the suite posts back.
/// Hand-rolled JSON walking to avoid pulling in serde_json. The
/// payload shape is fixed (the suite controls it), so a tolerant
/// parser is fine here.
enum BenchMessage {
    Progress(Vec<VariantRun>),
    Result(Vec<VariantRun>),
    Error(String),
    /// Anything else — gracefully ignored.
    Unknown,
}

fn parse_message(json: &str) -> BenchMessage {
    let trimmed = json.trim();
    if !trimmed.starts_with('{') {
        return BenchMessage::Unknown;
    }
    let type_field = extract_string_field(trimmed, "\"type\"");
    match type_field.as_deref() {
        Some("bench-result") => BenchMessage::Result(parse_runs(trimmed)),
        Some("bench-progress") => BenchMessage::Progress(parse_runs(trimmed)),
        Some("bench-error") => {
            let err = extract_string_field(trimmed, "\"error\"")
                .unwrap_or_else(|| "(unspecified)".to_string());
            BenchMessage::Error(err)
        }
        _ => BenchMessage::Unknown,
    }
}

fn extract_string_field(json: &str, key: &str) -> Option<String> {
    let start = json.find(key)?;
    let after = &json[start + key.len()..];
    let colon = after.find(':')?;
    let after = after[colon + 1..].trim_start();
    if !after.starts_with('"') { return None; }
    let body = &after[1..];
    let mut out = String::new();
    let mut chars = body.chars();
    while let Some(c) = chars.next() {
        if c == '\\' {
            // Treat the next char literally. Good enough for the
            // unicode-free strings the suite emits.
            if let Some(n) = chars.next() { out.push(n); }
        } else if c == '"' {
            return Some(out);
        } else {
            out.push(c);
        }
    }
    None
}

fn parse_runs(json: &str) -> Vec<VariantRun> {
    let runs_key = "\"runs\"";
    let start = match json.find(runs_key) { Some(i) => i, None => return Vec::new() };
    let after = &json[start + runs_key.len()..];
    let bracket = match after.find('[') { Some(i) => i, None => return Vec::new() };
    let body = &after[bracket + 1..];
    let mut out = Vec::new();
    let mut depth = 0i32;
    let mut obj_start: Option<usize> = None;
    for (i, b) in body.bytes().enumerate() {
        match b {
            b'{' => {
                if depth == 0 { obj_start = Some(i); }
                depth += 1;
            }
            b'}' => {
                depth -= 1;
                if depth == 0 {
                    let start = obj_start.take().unwrap_or(i);
                    let obj = &body[start..=i];
                    if let Some(r) = parse_run_obj(obj) {
                        out.push(r);
                    }
                }
            }
            b']' if depth == 0 => break,
            _ => {}
        }
    }
    out
}

fn parse_run_obj(obj: &str) -> Option<VariantRun> {
    Some(VariantRun {
        bucket:      extract_number_field(obj, "\"bucket\"")? as u32,
        apply:       extract_number_field(obj, "\"apply\"")?,
        first_paint: extract_number_field(obj, "\"firstPaint\"")?,
        worst_frame: extract_number_field(obj, "\"worstFrame\"")?,
    })
}

fn extract_number_field(json: &str, key: &str) -> Option<f64> {
    let start = json.find(key)?;
    let after = &json[start + key.len()..];
    let colon = after.find(':')?;
    let after = after[colon + 1..].trim_start();
    // Read until the next delimiter (comma, brace, bracket).
    let end = after.find(|c: char| c == ',' || c == '}' || c == ']' || c.is_whitespace())
        .unwrap_or(after.len());
    after[..end].parse().ok()
}

// =============================================================================
// Run loop
// =============================================================================

fn now_ms() -> f64 {
    web_sys::window()
        .and_then(|w| w.performance())
        .map(|p| p.now())
        .unwrap_or(0.0)
}

fn shuffle_in_place<T>(v: &mut [T]) {
    // Linear-congruential shuffle — same shape as `Array#sort(()=>random-0.5)`
    // would do in JS but unbiased. We don't need crypto here; the
    // benchmark just wants a different visiting order each Run press.
    let mut seed = (now_ms() as u64).wrapping_mul(2654435761);
    let n = v.len();
    for i in (1..n).rev() {
        seed = seed.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
        let j = ((seed >> 33) as usize) % (i + 1);
        v.swap(i, j);
    }
}

/// Snapshot the active suite's param form into a flat name→value
/// map for URL building. Param edits made mid-run don't affect the
/// in-flight run — only the next Run press.
fn snapshot_params(st: &RunnerState, suite: &SuiteInfo) -> HashMap<&'static str, String> {
    let mut m = HashMap::new();
    for p in suite.params {
        if let Some(sig) = st.params.get(&(suite.name, p.name)) {
            m.insert(p.name, sig.get());
        }
    }
    m
}

fn start_ticker(st: Rc<RunnerState>) {
    let st_for_tick = st.clone();
    let handle = framework_core::raf_loop(move || {
        let now = now_ms();
        let start = *st_for_tick.run_start_ms.borrow();
        st_for_tick.elapsed_seconds.set((now - start) / 1000.0);
    });
    *st.ticker_handle.borrow_mut() = Some(handle);
}

fn stop_ticker(st: &RunnerState) {
    if let Some(mut handle) = st.ticker_handle.borrow_mut().take() {
        handle.cancel();
    }
}

fn on_run_clicked() {
    let st = state();
    if st.run_in_progress.get() { return; }

    // Snapshot selections + params at click time. Mid-run edits
    // don't affect the active run.
    let selected = (*st.selected_variants.get()).clone();
    if selected.is_empty() {
        st.current_status.set("Select at least one variant.".to_string());
        return;
    }
    let suite_name = st.current_suite.get();
    let suite = suite_by_name(suite_name);
    // Belt-and-suspenders: filter the queued variants by suite
    // support too. The sidebar already hides unsupported
    // variants, but a stale `selected_variants` set from before
    // the suite switch could still hold ids that don't apply
    // here. Filtering at run time avoids errored rows in the
    // table.
    let mut order: Vec<&'static str> = VARIANTS.iter()
        .filter(|v| selected.contains(v.id) && v.supports.contains(&suite_name))
        .map(|v| v.id)
        .collect();
    shuffle_in_place(&mut order);


    let params = snapshot_params(&st, suite);
    *st.current_params.borrow_mut() = params;
    *st.current_run_suite.borrow_mut() = suite_name;

    // Clear the table completely, then seed Pending entries
    // for the variants in this run. Deselecting a variant
    // before re-running removes its row entirely — leaving
    // stale results in the table would be misleading once the
    // selection has changed.
    {
        let mut r = st.results.borrow_mut();
        r.clear();
        for &id in &order {
            r.insert(id, VariantEntry {
                status: VariantStatus::Pending,
                runs: Vec::new(),
            });
        }
    }
    st.run_finalized.set(false);
    bump_results(&st);

    *st.queue.borrow_mut() = order;
    *st.run_start_ms.borrow_mut() = now_ms();
    st.run_in_progress.set(true);
    st.run_finalized.set(false);
    start_ticker(st.clone());

    advance_queue(&st);
}

fn advance_queue(st: &RunnerState) {
    let next = st.queue.borrow_mut().pop();
    match next {
        Some(id) => start_variant(st, id),
        None => finalize_run(st),
    }
}

fn start_variant(st: &RunnerState, id: &'static str) {
    let Some(info) = VARIANTS.iter().find(|v| v.id == id) else {
        // Unknown ID — skip and move on.
        advance_queue(st);
        return;
    };
    let url = build_variant_url(info, *st.current_run_suite.borrow(), &st.current_params.borrow());
    st.current_variant.set(Some(id));
    st.current_status.set(format!("running · {}", info.label));
    set_status(st, id, VariantStatus::Running);
    // Trigger navigation by writing the iframe url signal —
    // the WebView's reactive `url` source closure reads this
    // signal so the framework re-fires `update_web_view_url`.
    st.iframe_url.set(url);
    // No explicit timeout here. The HTML runner had one
    // (scaling with iterations × rowsB). For the framework
    // runner, a wedged variant just blocks the queue — visible
    // in the UI as "running · X" not advancing. Adding a
    // scheduler-based timeout is a small follow-up; the
    // primitive plumbing (scheduling::scheduler().after_ms) is
    // already in place.
}

fn finalize_run(st: &RunnerState) {
    stop_ticker(st);
    st.run_in_progress.set(false);
    st.run_finalized.set(true);
    st.current_variant.set(None);
    st.current_status.set("all done".to_string());
    // Force the table body to re-render so winner highlighting
    // appears. The last variant's result already bumped
    // `version_sig`, but `run_finalized` only flips true *here* —
    // and the body reads it. One extra bump pushes the table
    // through the finalized-true path.
    bump_results(st);
}

/// Called from the WebView's `on_message` handler for every
/// postMessage from the current iframe. Routes by message type
/// and advances the queue on terminal events.
fn handle_message(payload: String) {
    let st = state();
    let Some(active) = st.current_variant.get() else { return };
    match parse_message(&payload) {
        BenchMessage::Progress(runs) => {
            set_runs(&st, active, runs, VariantStatus::Running);
        }
        BenchMessage::Result(runs) => {
            set_runs(&st, active, runs, VariantStatus::Done);
            advance_queue(&st);
        }
        BenchMessage::Error(err) => {
            set_status(&st, active, VariantStatus::Error(err));
            advance_queue(&st);
        }
        BenchMessage::Unknown => {}
    }
}

// =============================================================================
// UI builders
// =============================================================================

fn sidebar() -> Primitive {
    let st = state();
    let selected = st.selected_variants.clone();
    let current_suite_sig = st.current_suite.clone();
    // Clone the run-in-progress signal up-front so the Run
    // button's `disabled` closure doesn't conflict with the
    // `st` move into the param_form match arm below.
    let run_in_progress = st.run_in_progress.clone();

    // Suite-picker rows. Each row is a Toggle (acting as a radio)
    // bound to a per-suite "is selected" derived signal.
    let suite_rows: Vec<Primitive> = SUITES.iter().map(|s| {
        let suite_name = s.name;
        let title = s.title;
        let selected_for_read = current_suite_sig.clone();
        let selected_for_write = current_suite_sig.clone();
        let row_signal = signal!(current_suite_sig.get() == suite_name);
        {
            let row_signal = row_signal.clone();
            let _e = framework_core::Effect::new(move || {
                let want = selected_for_read.get() == suite_name;
                if row_signal.get() != want {
                    row_signal.set(want);
                }
            });
        }
        ui! {
            View(style = VariantRow()) {
                View(style = VariantLabel()) {
                    {
                        let row_signal = row_signal.clone();
                        toggle(row_signal.clone(), move |new_val| {
                            // Radio semantics: clicking an
                            // already-checked row is a no-op, only
                            // a "select this one" turns the others
                            // off. Suite switch clears results so
                            // we don't render rebuild numbers
                            // under toggle headers.
                            if new_val && selected_for_write.get() != suite_name {
                                selected_for_write.set(suite_name);
                                let st = state();
                                st.results.borrow_mut().clear();
                                st.run_finalized.set(false);
                                bump_results(&st);
                            }
                        })
                    }
                    Text { title }
                }
            }
        }
    }).collect();

    // Variant checkbox list — filtered to variants that support
    // the current suite. Wrapped in a Switch on `current_suite_sig`
    // so flipping suites rebuilds the list with the right subset.
    // Variants that lack the suite's required hooks (e.g. vanilla
    // for the hierarchy suite) are simply hidden rather than
    // shown-and-errored.
    let variant_form = {
        let current_suite_sig_v = current_suite_sig.clone();
        let selected_v = selected.clone();
        ui! {
            match current_suite_sig_v.get() {
                suite_name => {
                    {
                        let suite_name: &'static str = suite_name;
                        let selected = selected_v.clone();
                        let rows: Vec<Primitive> = VARIANTS.iter()
                            .filter(|v| v.supports.contains(&suite_name))
                            .map(|v| {
                                let id = v.id;
                                let label = v.label;
                                let selected_for_read = selected.clone();
                                let selected_for_write = selected.clone();
                                let row_signal = signal!(selected.get().contains(id));
                                {
                                    let row_signal = row_signal.clone();
                                    let _e = framework_core::Effect::new(move || {
                                        let want = selected_for_read.get().contains(id);
                                        if row_signal.get() != want {
                                            row_signal.set(want);
                                        }
                                    });
                                }
                                ui! {
                                    View(style = VariantRow()) {
                                        View(style = VariantLabel()) {
                                            {
                                                let row_signal = row_signal.clone();
                                                let id_for_write = id;
                                                toggle(row_signal.clone(), move |new_val| {
                                                    let mut s = (*selected_for_write.get()).clone();
                                                    if new_val { s.insert(id_for_write); }
                                                    else { s.remove(id_for_write); }
                                                    selected_for_write.set(Rc::new(s));
                                                })
                                            }
                                            Text { label }
                                        }
                                    }
                                }
                            })
                            .collect();
                        ui! { View { { rows } } }
                    }
                }
            }
        }
    };

    // Param form — reactive on `current_suite`. The whole
    // `Switch` rebuilds when the suite changes; each arm builds
    // the param rows for its suite from the per-suite signals.
    // We clone `st` for capture into the match arm so the outer
    // `st` is still usable below.
    let st_for_params = st.clone();
    let param_form = {
        let current_suite_sig = current_suite_sig.clone();
        ui! {
            match current_suite_sig.get() {
                suite_name => {
                    {
                        let suite = suite_by_name(suite_name);
                        let rows: Vec<Primitive> = suite.params.iter().map(|p| {
                            let sig = st_for_params.params.get(&(suite.name, p.name))
                                .expect("param signal missing for active suite")
                                .clone();
                            ui! {
                                View(style = ParamRow()) {
                                    Text(style = ParamLabel()) { p.label }
                                    { text_input(sig.clone(), move |v| sig.set(v)).with_style(ParamInput()) }
                                }
                            }
                        }).collect();
                        ui! { View { { rows } } }
                    }
                }
            }
        }
    };

    ui! {
        View(style = Sidebar()) {
            Text(style = SidebarH1()) { "Benchmark" }
            Text(style = Lede()) {
                "Each selected variant is loaded in an iframe in random order, given the suite's params via the URL, and posts results back when done. Sequential — never two iframes running at once."
            }
            Text(style = SectionH2()) { "Suite" }
            { suite_rows }
            Text(style = SectionH2()) { "Params" }
            { param_form }
            Text(style = SectionH2()) { "Variants" }
            { variant_form }
            { button("Run", on_run_clicked).with_style(RunButton())
                .disabled(move || run_in_progress.get()) }
        }
    }
}

fn frame_header() -> Primitive {
    let st = state();
    let cv_for_label = st.current_variant.clone();
    let status_sig = st.current_status.clone();
    let elapsed_sig = st.elapsed_seconds.clone();
    let progress_sig = st.run_in_progress.clone();
    let suite_sig = st.current_suite.clone();
    // Reactivity comes from the `ui!` macro wrapping each Text body
    // in a closure that the framework re-runs on signal changes —
    // we drop the `move ||` here since `.get()` calls *inside* the
    // body are the reactive subscriptions.
    ui! {
        View(style = FrameHeader()) {
            View {
                Text(style = FrameHeaderStrong()) {
                    {
                        match cv_for_label.get() {
                            Some(id) => VARIANTS.iter().find(|v| v.id == id).map(|v| v.label.to_string()).unwrap_or_else(|| id.to_string()),
                            None => "Idle".to_string(),
                        }
                    }
                }
                Text { format!(" · {}", suite_by_name(suite_sig.get()).title) }
            }
            View {
                Text { status_sig.get() }
                Text(style = FrameHeaderElapsed()) {
                    {
                        if progress_sig.get() || elapsed_sig.get() > 0.0 {
                            format!(" {:.1}s", elapsed_sig.get())
                        } else { String::new() }
                    }
                }
            }
        }
    }
}

fn frame_wrap() -> Primitive {
    let st = state();
    let url_sig = st.iframe_url.clone();
    ui! {
        View(style = FrameWrap()) {
            { frame_header() }
            {
                web_view(move || url_sig.get())
                    .on_message(handle_message)
                    .with_style(IframeStyle())
            }
        }
    }
}

fn results_table() -> Primitive {
    let st = state();
    let results = st.results.clone();
    let version_sig = st.results_version.clone();
    let finalized_sig = st.run_finalized.clone();
    let suite_sig = st.current_suite.clone();

    // Two nested switches:
    //   - Outer on `suite_sig`: rebuilds the whole table (header
    //     + body shape) when the user picks a different suite.
    //     Captures `suite` for the inner body to read.
    //   - Inner on `version_sig`: rebuilds only the body rows
    //     when results change. Without this nesting, the body
    //     would only refresh on suite switch — reads of
    //     `version_sig` inside the outer Switch's arm body don't
    //     re-fire it, since Switch keys off the scrutinee value.
    let table = ui! {
        match suite_sig.get() {
            suite_name => {
                {
                    let suite = suite_by_name(suite_name);
                    let headers = table_headers(suite);
                    let ncols_metrics = ncols(suite);

                    let header_cells: Vec<Primitive> = headers.iter().map(|h| {
                        ui! { Text(style = TableHeaderCell()) { h.clone() } }
                    }).collect();

                    let results = results.clone();
                    let version_sig = version_sig.clone();
                    let finalized_sig = finalized_sig.clone();
                    let body = ui! {
                        match version_sig.get() {
                            _v => {
                                {
                                    let finalized = finalized_sig.get();
                                    let results = results.borrow();

                                    // Per-column winners.
                                    let mut winners: Vec<f64> = vec![f64::INFINITY; ncols_metrics];
                                    if finalized {
                                        for entry in results.values() {
                                            if !matches!(entry.status, VariantStatus::Done) { continue; }
                                            let cols = column_medians(entry, suite);
                                            for i in 0..ncols_metrics {
                                                if let Some(v) = cols[i] {
                                                    if v < winners[i] { winners[i] = v; }
                                                }
                                            }
                                        }
                                    }

                                    let body_rows: Vec<Primitive> = VARIANTS.iter().filter_map(|v| {
                                        let entry = results.get(v.id)?;
                                        let cols = column_medians(entry, suite);
                                        let (status_text, status_kind) = match &entry.status {
                                            VariantStatus::Idle => ("—".to_string(), TableCellKind::Pending),
                                            VariantStatus::Pending => ("pending".to_string(), TableCellKind::Pending),
                                            VariantStatus::Running => ("⟳ running".to_string(), TableCellKind::Running),
                                            VariantStatus::Done => ("done".to_string(), TableCellKind::Normal),
                                            VariantStatus::Error(msg) => (format!("× {}", msg), TableCellKind::Error),
                                        };
                                        let mut cells: Vec<Primitive> = Vec::with_capacity(ncols_metrics + 2);
                                        cells.push(ui! { Text(style = TableCell().kind(TableCellKind::Label)) { v.label } });
                                        cells.push(ui! { Text(style = TableCell().kind(status_kind)) { status_text } });
                                        for i in 0..ncols_metrics {
                                            let cell_val = cols[i];
                                            let text = match cell_val {
                                                Some(x) => format!("{:.1}", x),
                                                None => "—".to_string(),
                                            };
                                            let kind = if finalized && cell_val.is_some()
                                                && winners[i].is_finite()
                                                && {
                                                    let val = cell_val.unwrap();
                                                    let tolerance = (winners[i] * 0.03).max(0.5);
                                                    val <= winners[i] + tolerance
                                                }
                                            {
                                                TableCellKind::Winner
                                            } else {
                                                TableCellKind::Normal
                                            };
                                            cells.push(ui! { Text(style = TableCell().kind(kind)) { text } });
                                        }
                                        Some(ui! {
                                            View(style = TableRow()) {
                                                { cells }
                                            }
                                        })
                                    }).collect();

                                    ui! { View { { body_rows } } }
                                }
                            }
                        }
                    };

                    ui! {
                        View {
                            View(style = TableHeaderRow()) {
                                { header_cells }
                            }
                            { body }
                        }
                    }
                }
            }
        }
    };

    ui! {
        ScrollView(style = ResultsWrap()) {
            { table }
        }
    }
}

fn app() -> Primitive {
    install_theme(bench_theme());
    ui! {
        View(style = Root()) {
            { sidebar() }
            View(style = Main()) {
                { frame_wrap() }
                { results_table() }
            }
        }
    }
}

// =============================================================================
// Boot
// =============================================================================

#[wasm_bindgen]
pub fn start() {
    console_error_panic_hook::set_once();
    backend_web::install_scheduler();
    backend_web::install_time_source();

    // One param signal per (suite, param) pair so switching
    // suites preserves whatever the user last entered. Each
    // signal starts at its suite's declared default.
    let mut params: HashMap<(&'static str, &'static str), Signal<String>> = HashMap::new();
    for suite in SUITES {
        for p in suite.params {
            params.insert((suite.name, p.name), signal!(format_param(p.default)));
        }
    }
    let all_selected: HashSet<&'static str> = VARIANTS.iter().map(|v| v.id).collect();
    let initial_suite = SUITES.first().expect("SUITES must be non-empty").name;

    let state = Rc::new(RunnerState {
        selected_variants:    signal!(Rc::new(all_selected)),
        current_suite:        signal!(initial_suite),
        params,
        results:              Rc::new(RefCell::new(HashMap::new())),
        results_version:      signal!(0u64),
        current_variant:      signal!(None),
        current_status:       signal!("Choose variants and press Run".to_string()),
        iframe_url:           signal!("about:blank".to_string()),
        run_in_progress:      signal!(false),
        run_finalized:        signal!(false),
        elapsed_seconds:      signal!(0.0),
        queue:                RefCell::new(Vec::new()),
        run_start_ms:         RefCell::new(0.0),
        ticker_handle:        RefCell::new(None),
        current_params:       RefCell::new(HashMap::new()),
        current_run_suite:    RefCell::new(initial_suite),
    });
    STATE.with(|s| *s.borrow_mut() = Some(state));

    let backend = Rc::new(RefCell::new(WebBackend::new("#app")));
    let owner = framework_core::render(backend, app());
    OWNER.with(|s| *s.borrow_mut() = Some(owner));
}

thread_local! {
    static OWNER: RefCell<Option<framework_core::Owner>> = const { RefCell::new(None) };
}

fn format_param(v: f64) -> String {
    if v.fract() == 0.0 {
        format!("{}", v as i64)
    } else {
        format!("{}", v)
    }
}
