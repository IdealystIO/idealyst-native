//! Regression test for wasm-split data-segment pruning.
//!
//! `Box<dyn Trait>` dispatch reads the impl's vtable — a static byte
//! blob in the wasm data segment laid out as
//! `[drop_fn, size, align, method_0, method_1, …]`. The pruning pass in
//! `wasm-split-cli` zeroes data symbols it can't prove are reachable;
//! a vtable is never named by a direct `call` instruction (dispatch
//! goes through `call_indirect`), so the conservative heuristic is at
//! risk of clearing it. When that happens the first dispatched method
//! traps with `RuntimeError: null function` or jumps to whatever
//! function index landed in slot 0 of the (now-zeroed) table.
//!
//! This app builds many `Box<dyn Trait>` values from several traits
//! and several impl types, dispatches every method, and renders the
//! results into the DOM. If pruning corrupted any vtable, either:
//!
//! - the page panics on mount (visible in the devtools console), or
//! - the rendered strings differ from the expected output below.
//!
//! Expected output (smoke check):
//!
//! ```text
//! greet: hello hola bonjour
//! count: 1,2,4,16
//! shape: rectangle:3x4=12 circle:r=5,A≈78
//! ```
//!
//! The traits exercise three vtable shapes:
//!
//! - `Greet` — single by-ref method returning an owned String (the
//!   common trait-object shape; vtable carries one method pointer).
//! - `Counter` — by-`&mut self` method that mutates internal state
//!   (verifies the vtable's drop_fn slot survives — `Box<dyn Trait>`
//!   calls it on drop).
//! - `Shape` — multi-method trait (forces a wider vtable so any
//!   trailing-byte zeroing is visible).
//!
//! The `Vec<Box<dyn Trait>>` indirection defeats devirtualization —
//! the compiler can't see the concrete type at the call site, so the
//! indirect dispatch through the vtable is preserved.

use idea_ui::{install_idea_theme, light_theme, Stack, StackGap, Typography};
use runtime_core::{ui, Element};

// ---- Trait 1: simple by-ref dispatch ---------------------------------------

trait Greet {
    fn say(&self) -> String;
}

struct English;
impl Greet for English {
    fn say(&self) -> String { "hello".to_string() }
}

struct Spanish;
impl Greet for Spanish {
    fn say(&self) -> String { "hola".to_string() }
}

struct French;
impl Greet for French {
    fn say(&self) -> String { "bonjour".to_string() }
}

// ---- Trait 2: by-&mut self (touches vtable drop_fn on Drop) ----------------

trait Counter {
    fn next(&mut self) -> u32;
}

struct Linear { n: u32 }
impl Counter for Linear {
    fn next(&mut self) -> u32 { self.n += 1; self.n }
}

struct Power { n: u32 }
impl Counter for Power {
    fn next(&mut self) -> u32 { self.n = self.n.saturating_mul(2); self.n }
}

// ---- Trait 3: multi-method (wider vtable) ----------------------------------

trait Shape {
    fn name(&self) -> &'static str;
    fn area(&self) -> f64;
    fn describe(&self) -> String;
}

struct Rectangle { w: f64, h: f64 }
impl Shape for Rectangle {
    fn name(&self) -> &'static str { "rectangle" }
    fn area(&self) -> f64 { self.w * self.h }
    fn describe(&self) -> String {
        format!("{}:{}x{}={}", self.name(), self.w as u32, self.h as u32, self.area() as u32)
    }
}

struct Circle { r: f64 }
impl Shape for Circle {
    fn name(&self) -> &'static str { "circle" }
    fn area(&self) -> f64 { std::f64::consts::PI * self.r * self.r }
    fn describe(&self) -> String {
        format!("{}:r={},A\u{2248}{}", self.name(), self.r as u32, self.area().round() as u32)
    }
}

pub fn app() -> Element {
    install_idea_theme(light_theme());

    let greeters: Vec<Box<dyn Greet>> =
        vec![Box::new(English), Box::new(Spanish), Box::new(French)];
    let greet_line: String = greeters
        .iter()
        .map(|g| g.say())
        .collect::<Vec<_>>()
        .join(" ");

    // Counter uses &mut self, so we mutate in place and collect.
    let mut counters: Vec<Box<dyn Counter>> = vec![
        Box::new(Linear { n: 0 }),
        Box::new(Linear { n: 1 }),
        Box::new(Power { n: 1 }),
        Box::new(Power { n: 4 }),
    ];
    let count_line: String = counters
        .iter_mut()
        .map(|c| c.next().to_string())
        .collect::<Vec<_>>()
        .join(",");
    drop(counters); // exercises the vtable drop_fn slot

    let shapes: Vec<Box<dyn Shape>> = vec![
        Box::new(Rectangle { w: 3.0, h: 4.0 }),
        Box::new(Circle { r: 5.0 }),
    ];
    let shape_line: String = shapes
        .iter()
        .map(|s| s.describe())
        .collect::<Vec<_>>()
        .join(" ");

    ui! {
        view {
            Stack(gap = StackGap::Md) {
                Typography(content = "vtable-dispatch regression test".to_string())
                Typography(content = format!("greet: {}", greet_line))
                Typography(content = format!("count: {}", count_line))
                Typography(content = format!("shape: {}", shape_line))
            }
        }
    }
}

pub fn register_extensions<B: runtime_core::Backend>(_backend: &mut B) {}
