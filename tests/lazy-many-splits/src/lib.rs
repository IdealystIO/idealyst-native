//! Regression test for wasm-split call-graph classification with MANY
//! split points and duplicate mangled names.
//!
//! # The bug this guards against
//!
//! LLVM under `opt-level=z` emits distinct functions that share one
//! mangled name (small alloc/core/hashbrown monomorphizations — the
//! idea-ui-docs release module carried 42 such names, including
//! `alloc::fmt::format`). `wasm-split-cli` used to correlate the
//! relocation-bearing pre-bindgen module with the bindgened module via
//! name-keyed `HashMap<String, FunctionId>` maps, so same-named copies
//! silently collided: call-graph edges landed on an arbitrary copy, the
//! other copy was classified chunk-only, and `emit` gutted it from the
//! main bundle even though a main-resident fmt vtable (a function
//! pointer in a DATA segment) still pointed at its table slot. First
//! main-side `format!` at boot then trapped with `RuntimeError:
//! function signature mismatch` — before any chunk was even requested.
//!
//! One or two split points never triggered it (`tests/lazy-chunk-handoff`,
//! `tests/lazy-payload-split` stayed green); the idea-ui-docs site with
//! dozens of `#[component(lazy)]` pages hit it reliably. This fixture
//! recreates that shape deterministically small:
//!
//! - 30 `#[component(lazy)]` page components, each `format!`-heavy with
//!   varied argument types so the module breeds plenty of small fmt/alloc
//!   monomorphizations (the duplicate-name candidates).
//! - A static fn-pointer catalog (`CATALOG`) dispatching the pages — the
//!   idea-ui-docs `routes::CATALOG` shape: main reaches the shims only
//!   through data-segment function pointers (`call_indirect`), never a
//!   direct call.
//! - Main-side `format!` + `HashMap` work at boot, so main itself calls
//!   through fmt vtables before any chunk loads.
//!
//! The browser check's marker ("chunk page #0 mounted") is rendered from
//! inside page 0's chunk: seeing it proves boot survived main-side
//! formatting AND the fn-pointer dispatch AND the chunk handoff.
//!
//! # Why this can't be a tighter test
//!
//! WHICH copy wins a name collision is HashMap-iteration-order luck, so
//! whether the loser is a main-critical function is probabilistic — this
//! fixture carries the same duplicate names (30+ in its release module)
//! but happened to survive the pre-fix splitter, while idea-ui-docs
//! failed reliably. A synthetic unit test isn't practical either: the
//! splitter consumes LLVM's linking + reloc custom sections, which
//! neither `wat` nor walrus can author, and duplicate-name emission
//! can't be forced from source. The deterministic guard is instead IN
//! the splitter: `emit_main_module` refuses to gut a function that a
//! main-reachable data symbol still points at, so any classification
//! regression fails the build loudly instead of shipping a main.wasm
//! that traps at boot. This fixture keeps the many-split fn-pointer
//! shape exercised end-to-end on top of that.

use std::collections::HashMap;
use std::rc::Rc;

use idea_ui::{
    install_idea_theme, light_theme, tone, variant, Badge, Button, Stack, StackGap, ToneRef,
    Typography,
};
use runtime_core::primitives::lazy::LazyError;
use runtime_core::{component, rx, signal, ui, Element, Signal};

/// One catalog row — the idea-ui-docs `Entry` shape: main only ever
/// reaches `body` through this data-stored function pointer.
pub struct Entry {
    pub name: &'static str,
    pub body: fn() -> Element,
}

fn chunk_loading() -> Element {
    ui! { Typography(content = "loading page chunk\u{2026}".to_string()) }
}

fn chunk_error(e: &LazyError) -> Element {
    ui! { Typography(content = format!("chunk failed: {}", e.message())) }
}

/// One `#[component(lazy)]` split point + fn-pointer shim per page. The
/// bodies deliberately format a spread of argument types (unsigned,
/// signed, float, tuple Debug, width/precision specs) so release
/// codegen instantiates the same small fmt/alloc shims the docs site's
/// pages did — the duplicate-mangled-name candidates the splitter must
/// keep classified as main-reachable.
macro_rules! lazy_pages {
    ($( ($Page:ident, $shim:ident, $seed:literal) ),+ $(,)?) => {
        $(
            #[component(lazy, retryable)]
            fn $Page() -> Element {
                let seed: u32 = $seed;
                let mut lines: Vec<String> = Vec::new();
                lines.push(format!("{} #{seed}", stringify!($Page)));
                lines.push(format!("{:?}", (seed as i64 - 7, seed as f64 / 3.0)));
                lines.push(format!("{:>8.3}", seed as f64 * 1.5));
                lines.push(format!("{:#06x}", seed * 2654435761u32 % 65536));
                let mut counts: HashMap<String, u32> = HashMap::new();
                for line in &lines {
                    counts.insert(line.clone(), line.len() as u32);
                }
                let total: u32 = counts.values().sum();
                // A real idea-ui subtree per page (button + reactive text +
                // badge), like the docs pages: each chunk's reachable graph
                // then claims the same component/fmt/alloc machinery main
                // uses, which is what put duplicate-named copies on both
                // sides of the split classification.
                let count: Signal<u32> = signal(seed);
                let inc: Rc<dyn Fn()> = Rc::new(move || count.update(|n| n + 1));
                let page_tone: ToneRef = match seed % 3 {
                    0 => tone::Primary.into(),
                    1 => tone::Success.into(),
                    _ => tone::Warning.into(),
                };
                ui! {
                    view {
                        Stack(gap = StackGap::Sm) {
                            Typography(content = format!("chunk page #{seed} mounted"))
                            Typography(content = format!("{} fmt bytes: {total}", lines.join(" | ")))
                            Typography(content = rx!(format!("count = {}", count.get())))
                            Badge(label = format!("seed {seed}"), tone = page_tone.clone())
                            Button(
                                label = format!("Increment page {seed}"),
                                tone = page_tone,
                                variant = variant::Soft,
                                on_click = inc,
                            )
                        }
                    }
                }
            }

            pub fn $shim() -> Element {
                ui! {
                    $Page(
                        loading = || chunk_loading(),
                        error = |e: &LazyError| chunk_error(e),
                    )
                }
            }
        )+

        pub static CATALOG: &[Entry] = &[
            $( Entry { name: stringify!($shim), body: $shim } ),+
        ];
    };
}

lazy_pages! {
    (Page00, page00, 0),
    (Page01, page01, 1),
    (Page02, page02, 2),
    (Page03, page03, 3),
    (Page04, page04, 4),
    (Page05, page05, 5),
    (Page06, page06, 6),
    (Page07, page07, 7),
    (Page08, page08, 8),
    (Page09, page09, 9),
    (Page10, page10, 10),
    (Page11, page11, 11),
    (Page12, page12, 12),
    (Page13, page13, 13),
    (Page14, page14, 14),
    (Page15, page15, 15),
    (Page16, page16, 16),
    (Page17, page17, 17),
    (Page18, page18, 18),
    (Page19, page19, 19),
    (Page20, page20, 20),
    (Page21, page21, 21),
    (Page22, page22, 22),
    (Page23, page23, 23),
    (Page24, page24, 24),
    (Page25, page25, 25),
    (Page26, page26, 26),
    (Page27, page27, 27),
    (Page28, page28, 28),
    (Page29, page29, 29),
}

pub fn app() -> Element {
    install_idea_theme(light_theme());

    // Main-side formatting + HashMap traffic at boot, BEFORE any chunk
    // loads — the docs trap fired inside exactly this kind of main-only
    // `format!` (navigator `match_prefix`) when the fmt vtable's target
    // had been gutted.
    let mut index: HashMap<&'static str, usize> = HashMap::new();
    for (i, entry) in CATALOG.iter().enumerate() {
        index.insert(entry.name, i);
    }
    let banner = format!(
        "lazy-many-splits: {} pages registered, page00 at {:?}",
        CATALOG.len(),
        index.get("page00")
    );

    // Dispatch the first page through the data-stored fn pointer
    // (`call_indirect`) — never a direct call, like the docs catalog.
    let first = (CATALOG[0].body)();

    ui! {
        view {
            Stack(gap = StackGap::Lg) {
                Typography(
                    content = banner,
                    kind = idea_ui::typography_kind::H2,
                )
                first
            }
        }
    }
}

// SDK-handler registration seam, invoked by the CLI-generated wrapper
// after `runtime_vocabulary::register_builtins`. Registry-generic over
// the scene `Host` so one seam serves every backend. This fixture
// registers no third-party scene handlers.
pub fn register_scene_extensions<H: runtime_scene::Host>(
    _registry: &mut runtime_scene::Registry<H>,
) {
}
