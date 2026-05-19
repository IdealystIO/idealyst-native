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
    signal, stylesheet, ui, AlignItems, Color, FlexDirection, JustifyContent, Length, Overflow,
    Primitive, Signal, TokenEntry, TokenValue, Tokenized,
};
use framework_theme::{install_theme, set_theme, ThemeTokens};
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
}

fn app(initial_rows: usize) -> Primitive {
    install_theme(light());

    // Reactive row count. The `match` below subscribes — changing
    // the signal rebuilds the list subtree from scratch.
    let count = signal!(initial_rows);
    ROW_COUNT.with(|c| *c.borrow_mut() = Some(count));

    ui! {
        View(style = page_style()) {
            // No controls chrome — the runner (in arena/index.html)
            // hosts every interactive control as static HTML
            // outside the iframe, and the variant page focuses on
            // the row list itself. We previously rendered an empty
            // `View(style = controls_style())` here as a transition
            // anchor; with the runner the box-with-no-children
            // read as a stray empty card above the list.
            //
            // Reactive `match` on count: changing the signal
            // re-fires the surrounding effect, drops the previous
            // ScrollView's scope (freeing every row's effect), and
            // builds a fresh subtree at the new size. The framework
            // does this through `Primitive::Switch` — same path the
            // example's perf screen uses.
            match count.get() {
                n => {
                    {
                        let n: usize = *n;
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
    let rows = initial_rows.clamp(1, ROW_MAX);
    let backend = Rc::new(RefCell::new(WebBackend::new("#app")));
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
#[wasm_bindgen]
pub fn set_rows(n: usize) {
    let clamped = n.clamp(1, ROW_MAX);
    ROW_COUNT.with(|c| {
        if let Some(sig) = c.borrow().as_ref() {
            sig.set(clamped);
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
    format!(
        "{{\"signals_in_use\":{},\"signals_total\":{},\"effects_in_use\":{},\"effects_total\":{},\"refs_in_use\":{},\"refs_total\":{},\"total_subscribers\":{},\"total_deps\":{},\"backend\":{}}}",
        s.signals_in_use, s.signals_total,
        s.effects_in_use, s.effects_total,
        s.refs_in_use, s.refs_total,
        s.total_subscribers, s.total_deps,
        backend_json,
    )
}
