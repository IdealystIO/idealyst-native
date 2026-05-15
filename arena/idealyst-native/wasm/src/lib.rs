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
//! spirit to `examples/hello`'s PerfRow / PerfList stylesheets so
//! the comparison is apples-to-apples with the other arena
//! variants (which mirror those same dimensions).

use backend_web::WebBackend;
use framework_core::{
    install_theme, set_theme, signal, stylesheet, ui, AlignItems, Color, FlexDirection,
    JustifyContent, Length, Overflow, Primitive, Signal,
};
use std::cell::RefCell;
use std::rc::Rc;
use wasm_bindgen::prelude::*;

#[cfg(target_arch = "wasm32")]
#[global_allocator]
static ALLOCATOR: lol_alloc::AssumeSingleThreaded<lol_alloc::FreeListAllocator> =
    unsafe { lol_alloc::AssumeSingleThreaded::new(lol_alloc::FreeListAllocator::new()) };

// =============================================================================
// Theme — the same shape as arena/instrument.js's LIGHT/DARK
// =============================================================================

/// Theme fields the perf row + chrome actually read. Mirrors the
/// shared `LIGHT`/`DARK` exports in `arena/instrument.js` so every
/// variant agrees on the exact hex values.
#[derive(Clone)]
pub struct Theme {
    pub background: String,
    pub surface: String,
    pub surface_alt: String,
    pub text: String,
    pub border: String,
    pub primary: String,
    pub primary_text: String,
}

pub fn light() -> Theme {
    Theme {
        background: "#f7f7fb".into(),
        surface: "#ffffff".into(),
        surface_alt: "#eef0f7".into(),
        text: "#1a1a1f".into(),
        border: "#e4e6ef".into(),
        primary: "#5b6cff".into(),
        primary_text: "#ffffff".into(),
    }
}

pub fn dark() -> Theme {
    Theme {
        background: "#0f1115".into(),
        surface: "#1a1d24".into(),
        surface_alt: "#262a35".into(),
        text: "#e8eaf0".into(),
        border: "#2a2e3a".into(),
        primary: "#8b9aff".into(),
        primary_text: "#0f1115".into(),
    }
}

// =============================================================================
// Stylesheets — match the dimensions/transitions every other arena variant uses
// =============================================================================

stylesheet! {
    pub Page<Theme> {
        base(t) {
            background: Color(t.background.clone()),
            color: Color(t.text.clone()),
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
        base(t) {
            flex_direction: FlexDirection::Row,
            gap: Length::Px(16.0),
            align_items: AlignItems::Center,
            padding_vertical: 8.0,
            padding_horizontal: 16.0,
            border_radius: 10.0,
            border_width: 1.0,
            border_color: Color(t.border.clone()),
            background: Color(t.surface.clone()),
            color: Color(t.text.clone()),
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
        base(t) {
            background: Color(t.surface.clone()),
            border_radius: 10.0,
            border_width: 1.0,
            border_color: Color(t.border.clone()),
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
        base(t) {
            padding_horizontal: 16.0,
            padding_vertical: 8.0,
            background: Color(t.surface.clone()),
            color: Color(t.text.clone()),
            border_bottom_width: 1.0,
            border_bottom_color: Color(t.border.clone()),
            font_size: 13.0,
            height: 36.0,
            justify_content: JustifyContent::Center,
        }
        variant parity {
            #[default]
            even(_t) {}
            odd(t) {
                background: Color(t.surface_alt.clone()),
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
    /// the perf screen in `examples/hello` uses.
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
            View(style = controls_style()) {
                // No button or stats here — the arena's index.html
                // hosts both as static HTML. We just need the
                // chrome that participates in the transition.
            }
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
/// inventing a wrapper type. Tooling-only — call `arena_stats_json`
/// for human-readable output.
#[wasm_bindgen]
pub fn arena_stats_json() -> String {
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
