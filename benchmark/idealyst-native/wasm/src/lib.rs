//! Arena variant: idealyst-native framework theme-toggle benchmark.
//!
//! Renders the same 1000-row screen the other variants render
//! (`spec.md` defines it: 36px-tall padded rows with alternating
//! parity backgrounds, transitioning bg + color + border-bottom over
//! 250ms ease-in-out, inside a 500px scroller with chrome around it).
//!
//! Exposes two functions to JS:
//!   - `start()` — wires up the wasm boot, mounts the screen.
//!   - `toggle()` — flips the theme. Wrapped in `runToggle()` from
//!     the arena's shared instrument.js.
//!
//! Nothing here is novel — it's a slimmed-down version of the
//! framework's own perf-screen body that exists only to fit the
//! arena's static-file contract. The screen body is identical in
//! spirit to `examples/hello-world`'s PerfRow / PerfList stylesheets so
//! the comparison is apples-to-apples with the other arena
//! variants (which mirror those same dimensions).

use backend_web::WebBackend;
use framework_core::{
    signal, stylesheet, ui, view, AlignItems, Color, FlexDirection, IntoPrimitive, JustifyContent,
    Length, Overflow, Primitive, Signal, TokenEntry, TokenValue, Tokenized,
};
use idea_ui::{install_theme, set_theme, ThemeTokens};
use std::cell::RefCell;
use std::rc::Rc;
use wasm_bindgen::prelude::*;

// Default dlmalloc allocator (was: lol_alloc::FreeListAllocator).
// Swapped out because the bench's repeated alloc/dealloc cycles
// (10k allocations per row toggle × N iterations) fragmented the
// freelist and made each subsequent iteration measurably slower —
// 1k apply went from 9ms in iteration 1 to 67ms in iteration 7,
// and the slowdown was entirely in pure-Rust per-row work
// (`batched_repeat_enqueue_loop`), which only does HashMap lookups
// + Vec pushes + String/Box allocations. dlmalloc is larger code
// but has stable performance under churn. Keep this measurement
// branch swapped while we evaluate.
// #[cfg(target_arch = "wasm32")]
// #[global_allocator]
// static ALLOCATOR: lol_alloc::AssumeSingleThreaded<lol_alloc::FreeListAllocator> =
//     unsafe { lol_alloc::AssumeSingleThreaded::new(lol_alloc::FreeListAllocator::new()) };

// =============================================================================
// Theme — the same shape as arena/instrument.js's LIGHT/DARK
// =============================================================================

/// Theme fields the perf row + chrome actually read. Mirrors the
/// shared `LIGHT`/`DARK` exports in `arena/instrument.js` so every
/// variant agrees on the exact hex values.
///
/// Each field is a `Tokenized<Color>` — a `Token` reference with the
/// current theme's color as the fallback. Stylesheets close over these
/// directly; the resulting `StyleRules` carry the token name (not the
/// fallback) into the content key, so the same `(sheet, variants)`
/// produces the **same minted CSS class** under any theme. Theme swap
/// updates the `:root` variable values in place — no class
/// regeneration, no `className` mutation on any node.
#[derive(Clone)]
pub struct Theme {
    pub background: Tokenized<Color>,
    pub surface: Tokenized<Color>,
    pub surface_alt: Tokenized<Color>,
    pub text: Tokenized<Color>,
    pub border: Tokenized<Color>,
    pub primary: Tokenized<Color>,
    pub primary_text: Tokenized<Color>,
}

fn tok_color(name: &'static str, fallback: &str) -> Tokenized<Color> {
    Tokenized::token(name, Color(fallback.into()))
}

pub fn light() -> Theme {
    Theme {
        background: tok_color("color-background", "#f7f7fb"),
        surface: tok_color("color-surface", "#ffffff"),
        surface_alt: tok_color("color-surface-alt", "#eef0f7"),
        text: tok_color("color-text", "#1a1a1f"),
        border: tok_color("color-border", "#e4e6ef"),
        primary: tok_color("color-primary", "#5b6cff"),
        primary_text: tok_color("color-primary-text", "#ffffff"),
    }
}

pub fn dark() -> Theme {
    Theme {
        background: tok_color("color-background", "#0f1115"),
        surface: tok_color("color-surface", "#1a1d24"),
        surface_alt: tok_color("color-surface-alt", "#262a35"),
        text: tok_color("color-text", "#e8eaf0"),
        border: tok_color("color-border", "#2a2e3a"),
        primary: tok_color("color-primary", "#8b9aff"),
        primary_text: tok_color("color-primary-text", "#0f1115"),
    }
}

/// `ThemeTokens` impl: enumerate every theme color as a
/// `TokenEntry { name, value }` so the backend can install them on
/// `:root`. The names here are exactly the names embedded in the
/// `Tokenized::Token { name, .. }` references — keep these two lists
/// in sync.
impl ThemeTokens for Theme {
    fn tokens(&self) -> Vec<TokenEntry> {
        fn entry(t: &Tokenized<Color>) -> TokenEntry {
            // Tokenized::Token always — we never construct literals
            // for theme fields above, but guard against it anyway:
            // unwrap to the fallback if a literal slipped in.
            let name = t.name().expect("theme fields must be Tokenized::Token");
            TokenEntry {
                name,
                value: TokenValue::Color(t.value().clone()),
            }
        }
        vec![
            entry(&self.background),
            entry(&self.surface),
            entry(&self.surface_alt),
            entry(&self.text),
            entry(&self.border),
            entry(&self.primary),
            entry(&self.primary_text),
        ]
    }
}

// =============================================================================
// Stylesheets — match the dimensions/transitions every other arena variant uses
// =============================================================================

// Per-stylesheet token references. Names + fallbacks match the
// `Theme` struct above (which is what `install_theme` registers with
// the framework). Web backend turns these into `var(--name, fallback)`
// in the emitted CSS, so theme swap is just a `:root` variable update.

stylesheet! {
    pub Page<Theme> {
        base(_t) {
            background: Tokenized::token("color-background", Color("#f7f7fb".into())),
            color: Tokenized::token("color-text", Color("#1a1a1f".into())),
            padding: 32.0,
            gap: Length::Px(24.0),
            min_height: Length::pct(100.0),
        }
        transitions {
            background: 250ms EaseInOut,
            color: 250ms EaseInOut,
        }
    }
}

stylesheet! {
    pub Controls<Theme> {
        base(_t) {
            flex_direction: FlexDirection::Row,
            gap: Length::Px(16.0),
            align_items: AlignItems::Center,
            padding_vertical: 8.0,
            padding_horizontal: 16.0,
            border_radius: 10.0,
            border_width: 1.0,
            border_color: Tokenized::token("color-border", Color("#e4e6ef".into())),
            background: Tokenized::token("color-surface", Color("#ffffff".into())),
            color: Tokenized::token("color-text", Color("#1a1a1f".into())),
        }
        transitions {
            background: 250ms EaseInOut,
            border_color: 250ms EaseInOut,
            color: 250ms EaseInOut,
        }
    }
}

stylesheet! {
    pub PerfList<Theme> {
        base(_t) {
            // `flex_direction: Column` here serves two purposes: it
            // sets the layout axis for the row children, and it
            // triggers the framework's CSS-emit auto-promotion to
            // `display: flex` — so rows behave as flex children
            // (flex-shrink: 1 by default, which collapses each row
            // to its content height + padding). Without this, the
            // list would emit `display: block` and rows would
            // honor their declared `height: 36` literally, ending
            // up 53px tall — visibly larger than every other
            // arena variant's 34px rows.
            flex_direction: FlexDirection::Column,
            background: Tokenized::token("color-surface", Color("#ffffff".into())),
            border_radius: 10.0,
            border_width: 1.0,
            border_color: Tokenized::token("color-border", Color("#e4e6ef".into())),
            height: 500.0,
            overflow: Overflow::Hidden,
        }
        transitions {
            background: 250ms EaseInOut,
            border_color: 250ms EaseInOut,
        }
    }
}

// Stylesheet used by the reactive-style suite. The row's
// `background` is overridable via `.background(color_or_signal)`,
// which is what makes the per-row Effect reactive: passing a
// closure (or signal) here routes through `StyleSource::Reactive`
// in the walker and gives the row its own per-node Effect that
// re-fires on any signal the closure reads.
stylesheet! {
    pub RStyleRow<Theme> {
        base(_t) {
            padding_horizontal: 16.0,
            padding_vertical: 8.0,
            color: Tokenized::token("color-text", Color("#1a1a1f".into())),
            font_size: 13.0,
            height: 36.0,
            justify_content: JustifyContent::Center,
        }
        override background: Color
    }
}

stylesheet! {
    pub PerfRow<Theme> {
        base(_t) {
            padding_horizontal: 16.0,
            padding_vertical: 8.0,
            background: Tokenized::token("color-surface", Color("#ffffff".into())),
            color: Tokenized::token("color-text", Color("#1a1a1f".into())),
            border_bottom_width: 1.0,
            border_bottom_color: Tokenized::token("color-border", Color("#e4e6ef".into())),
            font_size: 13.0,
            height: 36.0,
            justify_content: JustifyContent::Center,
        }
        variant parity {
            #[default]
            even(_t) {}
            odd(_t) {
                background: Tokenized::token("color-surface-alt", Color("#eef0f7".into())),
            }
        }
        transitions {
            background: 250ms EaseInOut,
            color: 250ms EaseInOut,
            border_bottom_color: 250ms EaseInOut,
        }
    }
}

// =============================================================================
// App + lifecycle hooks
// =============================================================================

const DEFAULT_ROWS: usize = 1000;
const ROW_MAX: usize = 100_000;

thread_local! {
    /// Holds the render's `Owner` so the framework's signals / effects
    /// stay alive for the page's lifetime. Without this, the Owner
    /// would drop at the end of `start()` and the framework would
    /// tear down everything immediately.
    static OWNER: RefCell<Option<framework_core::Owner>> = const { RefCell::new(None) };

    /// Diagnostic-only: a second handle to the backend so we can
    /// inspect its per-node HashMaps from the arena_stats accessor.
    /// The framework's render path owns the primary reference; this
    /// is just an `Rc::clone` so we can peek without going through
    /// the framework.
    static BACKEND: RefCell<Option<Rc<RefCell<WebBackend>>>> = const { RefCell::new(None) };

    /// Tracks current theme so `toggle()` knows what to flip to.
    /// Lives at module scope (not inside the framework's reactive
    /// graph) because the JS side calls `toggle()` directly.
    static IS_DARK: RefCell<bool> = const { RefCell::new(false) };

    /// Reactive row count. The framework reads this inside a
    /// `match` in the screen body — flipping it from `set_rows()`
    /// causes the framework to drop the previous row scope and
    /// build a fresh tree at the new size, exactly the same path
    /// the perf screen in `examples/hello-world` uses.
    ///
    /// Initialized in `start()` once we know what value the JS side
    /// wants (from the URL's `?rows=` param). Default is 1000.
    static ROW_COUNT: RefCell<Option<Signal<usize>>> = const { RefCell::new(None) };

    /// Mode signal: 0 = rows (rebuild/toggle suites), 1 = tree
    /// (hierarchy suite). Changing this re-fires the top-level
    /// Switch in `app()` and swaps the rendered subtree.
    static MODE: RefCell<Option<Signal<u32>>> = const { RefCell::new(None) };

    /// Tree-version signal: bumped on every `setup_hierarchy` so
    /// the tree-mode Switch arm rebuilds with the new tree shape.
    /// The actual tree spec lives in `TREE_ROOT` below — the
    /// version signal is the reactivity trigger.
    static TREE_VERSION: RefCell<Option<Signal<u64>>> = const { RefCell::new(None) };
    static TREE_ROOT: RefCell<Option<Rc<NodeSpec>>> = const { RefCell::new(None) };

    /// Leaves read this every time they re-fire. Bumping it makes
    /// every leaf in the cohort re-render.
    static GLOBAL_COUNTER: RefCell<Option<Signal<u32>>> = const { RefCell::new(None) };

    /// Only the target leaf reads this. Bumping it makes only
    /// that one leaf re-render.
    static BRANCH_COUNTER: RefCell<Option<Signal<u32>>> = const { RefCell::new(None) };

    /// Set by `setup_hierarchy` to the id of the leaf chosen as
    /// the BRANCH-update target. Read by `build_leaf` to decide
    /// whether that specific Leaf subscribes to `BRANCH_COUNTER`
    /// in addition to `GLOBAL_COUNTER`.
    static TARGET_LEAF_ID: std::cell::Cell<u32> = const { std::cell::Cell::new(u32::MAX) };

    // --- granular suite (mode = 2) -------------------------------
    /// Per-row counter signals. Each row binds its own. Length =
    /// COUNTER_COUNT after `setup_counters`. Reset on every
    /// `setup_counters` call.
    static COUNTERS: RefCell<Vec<Signal<u32>>> = const { RefCell::new(Vec::new()) };
    /// Number of counter rows mounted. Drives the granular-mode
    /// `for` loop's range. Bumped on `setup_counters`; the inner
    /// match arm reads it to size the row list.
    static COUNTER_COUNT: RefCell<Option<Signal<usize>>> = const { RefCell::new(None) };

    // --- reactive-style suite (mode = 3) -------------------------
    /// Shared color signal — every reactive-style row's bg reads
    /// from this. Driven by `set_shared_color`. 0 → color A,
    /// 1 → color B. (Two-state so the verifier knows which RGB
    /// to look for.)
    static SHARED_COLOR: RefCell<Option<Signal<u32>>> = const { RefCell::new(None) };
    /// Per-row color signals. Each row's bg blends shared + point:
    /// the row's actual bg is determined by which of (shared OR
    /// point) was most recently bumped. Simpler implementation:
    /// the row reads BOTH signals and picks point if set, else
    /// shared. (See `build_reactive_style_row` for the resolution
    /// rule.)
    static POINT_COLORS: RefCell<Vec<Signal<u32>>> = const { RefCell::new(Vec::new()) };
    static RSTYLE_COUNT: RefCell<Option<Signal<usize>>> = const { RefCell::new(None) };
}

// =============================================================================
// Hierarchy suite: deterministic tree generation (matches benchmark/tree.js)
// =============================================================================

/// Recursive tree node. Built by `gen_tree_shape` from a seed +
/// target leaf count. Each variant (JS, Rust) must produce the
/// SAME shape for the same seed so cross-variant numbers are
/// comparable. The algorithm mirrors `benchmark/tree.js` exactly.
pub struct NodeSpec {
    pub id: u32,
    pub kind: NodeKind,
}

pub enum NodeKind {
    Leaf,
    Branch(Vec<Rc<NodeSpec>>),
}

/// Mulberry32 PRNG. Same algorithm as `benchmark/tree.js`'s
/// `mulberry32`. `Math.imul` in JS produces the lower 32 bits of
/// a 32-bit signed multiply; `u32::wrapping_mul` matches that
/// bit pattern.
struct Mulberry32 {
    state: u32,
}

impl Mulberry32 {
    fn new(seed: u32) -> Self {
        Self { state: seed }
    }
    fn next(&mut self) -> u32 {
        self.state = self.state.wrapping_add(0x6D2B79F5);
        let mut t = self.state;
        t = (t ^ (t >> 15)).wrapping_mul(t | 1);
        t = t ^ t.wrapping_add((t ^ (t >> 7)).wrapping_mul(t | 61));
        t ^ (t >> 14)
    }
}

/// Result of tree generation. Mirrors the JS `{ root, leaves,
/// targetLeaf, totalNodes }` shape; we only keep what the variant
/// actually uses (root + target leaf id).
struct TreeShape {
    root: Rc<NodeSpec>,
    target_leaf_id: u32,
}

fn gen_tree_shape(seed: u32, target_leaves: usize, max_depth: Option<u32>) -> TreeShape {
    // `max_depth` and the leaf-probability threshold (15%) must
    // match `benchmark/tree.js`'s `genTreeShape` exactly —
    // otherwise the JS and Rust variants produce different tree
    // shapes from the same seed. Auto-sizing formula:
    // log_2.55(target) + 2 headroom levels, floor 8. With leaf
    // probability 15% and average branching factor ~2.55, this
    // gives the tree enough room to actually hit `target` leaves
    // before the depth cap kicks in.
    let max_depth: u32 = max_depth.unwrap_or_else(|| {
        let t = target_leaves.max(1) as f64;
        let base = (t.ln() / 2.55_f64.ln()).ceil() as u32 + 2;
        base.max(8)
    });
    let mut rng = Mulberry32::new(seed);
    let mut next_id: u32 = 0;
    let mut leaf_ids: Vec<u32> = Vec::new();

    fn walk(
        rng: &mut Mulberry32,
        next_id: &mut u32,
        leaf_ids: &mut Vec<u32>,
        depth: u32,
        max_depth: u32,
        target: usize,
    ) -> Rc<NodeSpec> {
        let id = *next_id;
        *next_id += 1;
        let force_leaf = depth >= max_depth || leaf_ids.len() >= target;
        if force_leaf {
            leaf_ids.push(id);
            return Rc::new(NodeSpec { id, kind: NodeKind::Leaf });
        }
        let r = rng.next() % 100;
        if r < 15 {
            leaf_ids.push(id);
            return Rc::new(NodeSpec { id, kind: NodeKind::Leaf });
        }
        let n_children = 2 + (rng.next() % 3) as usize;
        let mut children = Vec::with_capacity(n_children);
        for _ in 0..n_children {
            children.push(walk(rng, next_id, leaf_ids, depth + 1, max_depth, target));
        }
        Rc::new(NodeSpec { id, kind: NodeKind::Branch(children) })
    }

    let root = walk(&mut rng, &mut next_id, &mut leaf_ids, 0, max_depth, target_leaves);
    let target_leaf_id = leaf_ids
        .get(leaf_ids.len() / 2)
        .copied()
        .unwrap_or(u32::MAX);
    TreeShape { root, target_leaf_id }
}

/// Build the Primitive for a leaf. Closes over `global` and, if
/// the leaf is the BRANCH target, `branch` — so only the target
/// subscribes to `branch`. Other leaves only subscribe to
/// `global`.
fn build_leaf(id: u32, target_id: u32, global: Signal<u32>, branch: Signal<u32>) -> Primitive {
    // `text_fmt!("template", args...)` constructs a
    // `TextSource::JsBinding`: per-fire fan-out happens entirely
    // on the backend side (web → JS reactive layer). Args in
    // `bind!(...)` are signals (subscribed to + interpolated per
    // fire); bare args are captured at construction time and
    // formatted into the template's static parts.
    if id == target_id {
        framework_core::text_fmt!(
            "leaf {}: g={} b={}",
            id,
            framework_core::bind!(global),
            framework_core::bind!(branch),
        )
        .into_primitive()
    } else {
        framework_core::text_fmt!(
            "leaf {}: g={}",
            id,
            framework_core::bind!(global),
        )
        .into_primitive()
    }
}

/// Recursively turn a NodeSpec into a Primitive tree. Branches
/// become `view(children)`; leaves become reactive `text(...)`.
fn build_tree_primitive(
    node: &NodeSpec,
    target_id: u32,
    global: Signal<u32>,
    branch: Signal<u32>,
) -> Primitive {
    match &node.kind {
        NodeKind::Leaf => build_leaf(node.id, target_id, global, branch),
        NodeKind::Branch(children) => {
            let kids: Vec<Primitive> = children
                .iter()
                .map(|c| build_tree_primitive(c, target_id, global, branch))
                .collect();
            view(kids).into_primitive()
        }
    }
}

fn app(initial_rows: usize) -> Primitive {
    install_theme(light());

    // Reactive row count + mode + hierarchy state. Stored in
    // thread_locals so the wasm-bindgen exports below can mutate
    // them from JS.
    let count = signal!(initial_rows);
    ROW_COUNT.with(|c| *c.borrow_mut() = Some(count));
    let mode = signal!(0u32);
    MODE.with(|c| *c.borrow_mut() = Some(mode));
    let tree_version = signal!(0u64);
    TREE_VERSION.with(|c| *c.borrow_mut() = Some(tree_version));
    let global_counter = signal!(0u32);
    GLOBAL_COUNTER.with(|c| *c.borrow_mut() = Some(global_counter));
    let branch_counter = signal!(0u32);
    BRANCH_COUNTER.with(|c| *c.borrow_mut() = Some(branch_counter));

    // Granular + reactive-style suite signals. Sized to 0 at boot —
    // each suite's `setup_*` call fills the per-row signal vectors and
    // bumps the count signal so the matching Switch arm builds.
    let counter_count = signal!(0usize);
    COUNTER_COUNT.with(|c| *c.borrow_mut() = Some(counter_count));
    let rstyle_count = signal!(0usize);
    RSTYLE_COUNT.with(|c| *c.borrow_mut() = Some(rstyle_count));
    let shared_color = signal!(0u32);
    SHARED_COLOR.with(|c| *c.borrow_mut() = Some(shared_color));

    // Register the two hierarchy-bench signals with the web
    // backend's JS-side reactive layer so signal writes flow into
    // JS for per-binding fan-out (instead of firing N Rust
    // Effects, one per leaf). Done once at startup; the leaves
    // themselves only need to declare the binding (no per-leaf
    // Effect setup). Wrapped in `untrack` so the stringifier
    // can't accidentally pick up a subscription if it fires
    // inside some outer effect's run.
    BACKEND.with(|s| {
        if let Some(b_rc) = s.borrow().as_ref() {
            let mut b = b_rc.borrow_mut();
            b.register_signal_for_js(global_counter.id(), move || {
                framework_core::untrack(|| global_counter.get()).to_string()
            });
            b.register_signal_for_js(branch_counter.id(), move || {
                framework_core::untrack(|| branch_counter.get()).to_string()
            });
        }
    });

    ui! {
        View(style = page_style()) {
            // Top-level Switch on `mode`. Flipping `mode` swaps
            // the entire subtree atomically. Branches: 0 = row
            // list (rebuild/toggle), 1 = hierarchy tree.
            match mode.get() {
                m => {
                    {
                        match *m {
                            0u32 => {
                                let n: usize = count.get();
                                ui! {
                                    ScrollView(style = perf_list_style()) {
                                        for i in 0..n {
                                            View(style = PerfRow().parity(if i % 2 == 0 {
                                                PerfRowParity::Even
                                            } else {
                                                PerfRowParity::Odd
                                            })) {
                                                Text { format!("Row #{}", i) }
                                            }
                                        }
                                    }
                                }
                            }
                            2u32 => {
                                // Granular suite — N rows, each binding its own
                                // counter signal via `text_fmt!` + `bind!`. The
                                // JS-side text bridge handles per-binding fan-out
                                // so a `bump_counter(i, v)` becomes one JS-side
                                // notifier call, not a fresh Rust Effect per row.
                                let n: usize = counter_count.get();
                                let sigs: Vec<Signal<u32>> =
                                    COUNTERS.with(|c| c.borrow().clone());
                                ui! {
                                    ScrollView(style = perf_list_style()) {
                                        for i in 0..n {
                                            View(style = PerfRow().parity(if i % 2 == 0 {
                                                PerfRowParity::Even
                                            } else {
                                                PerfRowParity::Odd
                                            })) {
                                                // `bind!(sigs[i])` subscribes via
                                                // the JS-side binding registry, so
                                                // each counter update fans out
                                                // through a single notifier — not
                                                // a Rust Effect per row.
                                                {
                                                    framework_core::text_fmt!(
                                                        "row {}: c={}",
                                                        i,
                                                        framework_core::bind!(sigs[i]),
                                                    )
                                                    .into_primitive()
                                                }
                                            }
                                        }
                                    }
                                }
                            }
                            3u32 => {
                                // Reactive-style suite — N rows whose
                                // `background` reads SHARED_COLOR + the row's
                                // own POINT_COLORS[i] signal inside a reactive
                                // style closure. Each row's `attach_style_reactive`
                                // Effect subscribes to whichever signals the
                                // closure read; a SHARED bump fans out to all
                                // N Effects, a POINT(i) bump fans out to just
                                // row i's. (This is the path the static-style
                                // rebuild bench skips — exercises the per-node
                                // style Effect path end-to-end.)
                                let n: usize = rstyle_count.get();
                                let points: Vec<Signal<u32>> =
                                    POINT_COLORS.with(|p| p.borrow().clone());
                                ui! {
                                    ScrollView(style = perf_list_style()) {
                                        for i in 0..n {
                                            // Capture per-row signal handle. The
                                            // outer style closure is moved into
                                            // the attach_style_reactive Effect.
                                            // We construct the StyleApplication
                                            // manually rather than going through
                                            // the `RStyleRow()` builder because
                                            // `IntoStyleSource for F: Fn() ->
                                            // StyleApplication` is what makes
                                            // the style reactive; the builder
                                            // returns its OWN `StyleSource` and
                                            // would lose the reactive routing.
                                            View(style = {
                                                let pt = points[i];
                                                let sh = shared_color;
                                                move || {
                                                    // Resolution: if the point
                                                    // signal is non-zero, use the
                                                    // point's choice (1 = A,
                                                    // 2 = B); otherwise the row
                                                    // follows the shared signal.
                                                    let p = pt.get();
                                                    let s = sh.get();
                                                    let idx = if p == 0 { s } else { p - 1 };
                                                    let color = if idx == 0 {
                                                        Color("#5b6cff".into())
                                                    } else {
                                                        Color("#ff5b6c".into())
                                                    };
                                                    framework_core::StyleApplication::new(
                                                        RStyleRow::sheet(),
                                                    )
                                                    .override_background(color)
                                                }
                                            }) {
                                                Text { format!("rstyle {}", i) }
                                            }
                                        }
                                    }
                                }
                            }
                            _ => {
                                // Hierarchy mode (default 1u32). Inner
                                // match on tree_version so changing
                                // seed/nodes rebuilds the whole tree.
                                ui! {
                                    match tree_version.get() {
                                        _v => {
                                            {
                                                let root = TREE_ROOT.with(|r| r.borrow().clone());
                                                let target_id = TARGET_LEAF_ID.with(|t| t.get());
                                                match root {
                                                    Some(root) => build_tree_primitive(
                                                        &root,
                                                        target_id,
                                                        global_counter,
                                                        branch_counter,
                                                    ),
                                                    None => view(Vec::new()).into_primitive(),
                                                }
                                            }
                                        }
                                    }
                                }
                            }
                        }
                    }
                }
            }
        }
    }
}

/// Boot the wasm side: mount the screen under `#app` with
/// `initial_rows` rows. Called once from the arena variant's
/// `<script type="module">` after the wasm is loaded.
#[wasm_bindgen]
pub fn start(initial_rows: usize) {
    console_error_panic_hook::set_once();
    // Register the web backend's wasm-bindgen-backed scheduler so the
    // framework can defer work to microtasks / rAF. Without this, the
    // first render panics the moment it hits a Switch primitive (or any
    // other path that calls `schedule_microtask`).
    backend_web::install_scheduler();
    // Register a TimeSource so `framework_core::debug::now_micros`
    // (used by `PhaseTimer` and the rest of the debug-stats
    // aggregator) reads `performance.now()` instead of returning 0.
    // Without this, every phase counter records duration 0 and the
    // profiling output is useless.
    backend_web::install_time_source();
    let rows = initial_rows.clamp(1, ROW_MAX);
    let backend = Rc::new(RefCell::new(WebBackend::new("#app")));
    // Opt the variant into the web backend's batched text-update
    // path. Without this call, `create_text_with_id` returns `None`
    // and the framework falls back to per-fire `update_text(node,
    // str)`. The opt-in costs +1 FFI per text-node create (one-time)
    // and saves O(N) FFI per fan-out for every reactive text effect.
    // Material on the hierarchy bench (2 k+ leaves subscribing to
    // one signal); neutral elsewhere.
    backend_web::install_text_batcher(&backend);
    BACKEND.with(|slot| *slot.borrow_mut() = Some(backend.clone()));
    let owner = framework_core::render(backend, app(rows));
    OWNER.with(|slot| *slot.borrow_mut() = Some(owner));
}


/// Flip the theme. Wrapped by the arena's `runToggle(...)` so the
/// click handler can measure the synchronous JS cost. Returns the
/// name of the now-active theme so the JS side can label the
/// readout correctly.
#[wasm_bindgen]
pub fn toggle() -> String {
    let mut name = String::new();
    IS_DARK.with(|d| {
        let mut is_dark = d.borrow_mut();
        *is_dark = !*is_dark;
        if *is_dark {
            set_theme(dark());
            name = "dark".to_string();
        } else {
            set_theme(light());
            name = "light".to_string();
        }
    });
    name
}

/// Set the theme by name ("light" or "dark"). Called from the
/// JS-side `setTheme` hook the toggle suite drives. Anything
/// other than "dark" is treated as light. Updates `IS_DARK` so
/// the unrelated `toggle()` (console diagnostic) stays in sync.
#[wasm_bindgen]
pub fn set_theme_by_name(name: &str) {
    let want_dark = name == "dark";
    IS_DARK.with(|d| {
        let mut is_dark = d.borrow_mut();
        *is_dark = want_dark;
    });
    if want_dark {
        set_theme(dark());
    } else {
        set_theme(light());
    }
}

/// Change the row count. Wrapped by the arena's `runToggle(...)`
/// so the resulting mount cost is measurable. Clamps to a sane
/// max (100k) so a stray huge value doesn't lock the browser.
///
/// IMPORTANT: writes `mode` unconditionally on every call. The
/// surrounding tree is structured as `match mode.get() { ... }`
/// (a reactive switch), with the `mode=0` arm body reading
/// `count.get()`. The framework's reactive-`match` lowering wraps
/// arm-body construction in `untrack(..)`, so reads inside the arm
/// body do NOT subscribe the Switch's Effect — only the
/// discriminant's reads do. That means a bare `count.set(n)` alone
/// is invisible to the Switch and the row list never rebuilds.
///
/// The fix is to also touch `mode` so the Switch's Effect re-fires.
/// `Signal::set` notifies subscribers unconditionally (no
/// value-equality skip), so `mode.set(0)` when mode is already 0
/// still triggers the rebuild — at which point the arm body reads
/// the latest `count.get()` and produces the new row list.
///
/// We could fix this at the framework level (e.g. by including
/// `count` in the discriminant), but it's much cheaper to just
/// keep the unconditional write here.
#[wasm_bindgen]
pub fn set_rows(n: usize) {
    let clamped = n.clamp(1, ROW_MAX);
    ROW_COUNT.with(|c| {
        if let Some(sig) = c.borrow().as_ref() {
            sig.set(clamped);
        }
    });
    // Unconditional — this is what actually triggers the rebuild.
    // Without it, the rebuild bench stays stuck on its initial
    // row count for every subsequent iteration (silent failure;
    // the bench reports apply times for set-rows calls that did
    // not actually mount any new DOM).
    MODE.with(|c| {
        if let Some(sig) = c.borrow().as_ref() {
            sig.set(0);
        }
    });
}

/// Mount a tree of `nodes` leaves for the hierarchy suite,
/// generated deterministically from `seed`. Bumps mode → 1
/// (tree) and the tree-version signal so the Switch arm rebuilds
/// with the new tree shape.
#[wasm_bindgen]
pub fn setup_hierarchy(seed: u32, nodes: u32, max_depth: u32) {
    // `max_depth = 0` from JS means "auto-size from `nodes`",
    // matching tree.js's `null`/`undefined` convention.
    let md = if max_depth == 0 { None } else { Some(max_depth) };
    let tree = gen_tree_shape(seed, nodes as usize, md);
    TARGET_LEAF_ID.with(|t| t.set(tree.target_leaf_id));
    TREE_ROOT.with(|r| *r.borrow_mut() = Some(tree.root));
    // Coalesce the mode flip + tree_version bump into one fan-out.
    // Without `batch`, MODE.set(1) and TREE_VERSION.update(...) each
    // re-fire the Switch arm's effect — so the tree builds TWICE
    // (once on mode flip, again on version bump), with the first
    // build's 100k+ Effects torn down before the second builds the
    // same tree fresh. Batching collapses to a single rebuild.
    framework_core::batch(|| {
        MODE.with(|c| {
            if let Some(sig) = c.borrow().as_ref() {
                sig.set(1);
            }
        });
        TREE_VERSION.with(|c| {
            if let Some(sig) = c.borrow().as_ref() {
                sig.update(|v| *v += 1);
            }
        });
    });
}

/// Bump the branch counter. Only the target leaf reads this
/// signal, so the framework's reactive graph fans out to exactly
/// one Effect — same shape that Vue/Svelte's fine-grained
/// reactivity produces.
#[wasm_bindgen]
pub fn branch_update(n: u32) {
    BRANCH_COUNTER.with(|c| {
        if let Some(sig) = c.borrow().as_ref() {
            sig.set(n);
        }
    });
}

/// Bump the global counter. EVERY leaf reads this signal, so
/// the framework's reactive graph fans out to N Effects.
#[wasm_bindgen]
pub fn global_update(n: u32) {
    GLOBAL_COUNTER.with(|c| {
        if let Some(sig) = c.borrow().as_ref() {
            sig.set(n);
        }
    });
}

// ----------------------------------------------------------------
// Granular suite hooks (mode = 2)
// ----------------------------------------------------------------

/// Mount `n` counter rows. Each row binds its own signal via the
/// JS-side text-binding registry, so a `bump_counter(i, v)` is one
/// JS notifier call (not a Rust Effect per row). Tearing down a
/// previous mount: we drop the existing signal Vec and re-create.
///
/// The mode + count writes are batched so the Switch arm rebuilds
/// exactly once. Without the batch, `MODE.set(2)` and
/// `COUNTER_COUNT.set(n)` would each re-fire the Switch effect and
/// the row list would build twice.
#[wasm_bindgen]
pub fn setup_counters(n: u32) {
    let n = n.clamp(1, ROW_MAX as u32) as usize;
    // Fresh signals each setup — old ones drop with the previous
    // scope when the Switch arm rebuilds.
    let sigs: Vec<Signal<u32>> = (0..n).map(|_| Signal::new(0u32)).collect();
    // Register each counter signal as a JS-side notifier so
    // `bump_counter` updates flow through the batched text bridge
    // instead of firing per-row Rust Effects.
    BACKEND.with(|s| {
        if let Some(b_rc) = s.borrow().as_ref() {
            let mut b = b_rc.borrow_mut();
            for sig in &sigs {
                let sig_for_notify = *sig;
                b.register_signal_for_js(sig.id(), move || {
                    framework_core::untrack(|| sig_for_notify.get()).to_string()
                });
            }
        }
    });
    COUNTERS.with(|c| *c.borrow_mut() = sigs);
    framework_core::batch(|| {
        MODE.with(|c| {
            if let Some(sig) = c.borrow().as_ref() {
                sig.set(2);
            }
        });
        COUNTER_COUNT.with(|c| {
            if let Some(sig) = c.borrow().as_ref() {
                sig.set(n);
            }
        });
    });
}

/// Bump one row's counter to `v`. Routes through the JS-side text
/// binding for that signal.
#[wasm_bindgen]
pub fn bump_counter(i: u32, v: u32) {
    COUNTERS.with(|c| {
        let c = c.borrow();
        if let Some(sig) = c.get(i as usize) {
            sig.set(v);
        }
    });
}

/// Bump every counter in `[start, end)` to `v` inside a single
/// reactive `batch` so subscriber fan-out coalesces (each subscriber
/// runs at most once even if multiple writes touch its deps).
#[wasm_bindgen]
pub fn bump_range(start: u32, end: u32, v: u32) {
    let s = start as usize;
    let e = (end as usize).min(COUNTERS.with(|c| c.borrow().len()));
    if s >= e {
        return;
    }
    framework_core::batch(|| {
        COUNTERS.with(|c| {
            let c = c.borrow();
            for sig in c.iter().skip(s).take(e - s) {
                sig.set(v);
            }
        });
    });
}

// ----------------------------------------------------------------
// Reactive-style suite hooks (mode = 3)
// ----------------------------------------------------------------

/// Mount `n` reactive-style rows. Each row's `background` reads
/// `SHARED_COLOR` and the row's own `POINT_COLORS[i]` inside a
/// reactive style closure — see the mode = 3 arm in `app()`.
#[wasm_bindgen]
pub fn setup_reactive_styles(n: u32) {
    let n = n.clamp(1, ROW_MAX as u32) as usize;
    let points: Vec<Signal<u32>> = (0..n).map(|_| Signal::new(0u32)).collect();
    POINT_COLORS.with(|p| *p.borrow_mut() = points);
    // Reset shared color to 0 (== color A) so the suite starts in
    // a known state on each setup.
    SHARED_COLOR.with(|c| {
        if let Some(sig) = c.borrow().as_ref() {
            sig.set(0);
        }
    });
    framework_core::batch(|| {
        MODE.with(|c| {
            if let Some(sig) = c.borrow().as_ref() {
                sig.set(3);
            }
        });
        RSTYLE_COUNT.with(|c| {
            if let Some(sig) = c.borrow().as_ref() {
                sig.set(n);
            }
        });
    });
}

/// Set the shared color. `name` is "A" or "B" — anything else is
/// treated as A. Fans out to every row's reactive style Effect.
#[wasm_bindgen]
pub fn set_shared_color(name: &str) {
    let v: u32 = if name == "B" { 1 } else { 0 };
    SHARED_COLOR.with(|c| {
        if let Some(sig) = c.borrow().as_ref() {
            sig.set(v);
        }
    });
}

/// Set row `i`'s point color. `name` is "A" / "B" / anything else
/// (anything else is treated as A). Non-zero point overrides
/// shared for that row. Encoding: 0 = follow shared, 1 = force A,
/// 2 = force B.
#[wasm_bindgen]
pub fn set_point_color(i: u32, name: &str) {
    let v: u32 = if name == "B" { 2 } else { 1 };
    POINT_COLORS.with(|p| {
        let p = p.borrow();
        if let Some(sig) = p.get(i as usize) {
            sig.set(v);
        }
    });
}

/// Convenience: how many rows the wasm is rendering right now.
/// Read via the signal so it reflects post-`set_rows` state.
#[wasm_bindgen]
pub fn row_count() -> usize {
    ROW_COUNT
        .with(|c| c.borrow().as_ref().map(|sig| sig.get()))
        .unwrap_or(DEFAULT_ROWS)
}

/// Diagnostic: return arena slot counts so JS can detect when the
/// rebuild loop is leaving state behind. Each field returns
/// `in_use` × 1_000_000 + `total` so we can shove the full snapshot
/// over the wasm boundary as a small struct of plain numbers without
/// inventing a wrapper type. Tooling-only — call `bench_stats_json`
/// for human-readable output.
#[wasm_bindgen]
pub fn bench_stats_json() -> String {
    let s = framework_core::arena_stats();
    let b = BACKEND.with(|slot| slot.borrow().as_ref().map(|rc| rc.borrow().debug_counts()));
    let backend_json = match b {
        Some(b) => format!(
            "{{\"node_ids\":{},\"dynamic\":{},\"state_listeners\":{},\"pregen\":{},\"pregen_by_ptr\":{},\"free_rule_indices\":{},\"next_node_id\":{}}}",
            b.node_ids, b.dynamic, b.state_listeners, b.pregen, b.pregen_by_ptr, b.free_rule_indices, b.next_node_id,
        ),
        None => "null".into(),
    };
    let phases_json = phase_counters_json();
    format!(
        "{{\"signals_in_use\":{},\"signals_total\":{},\"effects_in_use\":{},\"effects_total\":{},\"refs_in_use\":{},\"refs_total\":{},\"total_subscribers\":{},\"total_deps\":{},\"backend\":{},\"phases\":{}}}",
        s.signals_in_use, s.signals_total,
        s.effects_in_use, s.effects_total,
        s.refs_in_use, s.refs_total,
        s.total_subscribers, s.total_deps,
        backend_json,
        phases_json,
    )
}

/// Drain + serialize `framework_core::debug` phase counters as a
/// JSON object: `{ "phase_name": { "calls": N, "total_us": N,
/// "max_us": N, "avg_us": N }, … }`. Calling this also CLEARS the
/// counters so a subsequent `bench_stats_json` measures a fresh
/// window.
///
/// When `debug-stats` is OFF (the production-bench configuration),
/// returns `"null"` — phase counters don't exist in that build.
#[cfg(feature = "debug-stats")]
fn phase_counters_json() -> String {
    let counters = framework_core::debug::take_phase_counters();
    if counters.is_empty() {
        return "{}".into();
    }
    // Sort by total_us descending so the heaviest phase is first in
    // the JSON — readers scanning the output land on the hot spot
    // immediately.
    let mut entries: Vec<_> = counters.into_iter().collect();
    entries.sort_by(|a, b| b.1.total_us.cmp(&a.1.total_us));
    let mut out = String::from("{");
    for (i, (name, c)) in entries.iter().enumerate() {
        if i > 0 {
            out.push(',');
        }
        let avg = if c.call_count == 0 { 0 } else { c.total_us / c.call_count };
        out.push_str(&format!(
            "\"{}\":{{\"calls\":{},\"total_us\":{},\"max_us\":{},\"avg_us\":{}}}",
            name, c.call_count, c.total_us, c.max_us, avg
        ));
    }
    out.push('}');
    out
}

#[cfg(not(feature = "debug-stats"))]
fn phase_counters_json() -> String {
    "null".into()
}

/// Reset phase counters without dumping them — useful between
/// suite runs when you want to attribute time to just the
/// upcoming work. Pair with `bench_stats_json()` at the end of
/// the window to read the accumulated phase data.
///
/// No-op when `debug-stats` is OFF.
#[wasm_bindgen]
pub fn clear_phase_counters() {
    #[cfg(feature = "debug-stats")]
    framework_core::debug::clear_phase_counters();
}
