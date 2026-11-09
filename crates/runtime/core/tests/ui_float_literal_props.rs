//! An unsuffixed float literal passed to an `f32` prop must type-check
//! without rustc's `float_literal_f32_fallback` (rust-lang/rust#154024).
//!
//! The component call sites lower each prop to `field: (value).into()`.
//! For `inset = 16.0` into an `f32` field that leaves the literal's type
//! to inference: `f32: From<{float}>` holds only for `f32`, so rustc picks
//! `f32` through a fallback it is phasing out (a future hard error). The
//! macros lower an unsuffixed float literal to a suffixed one picked by
//! the field's type instead (`macros-expand/src/float_lit.rs`); the
//! `deny` below turns any remaining fallback into a compile failure of
//! this file. Covered sites: the `ui!` and `jsx!` call sites, an inline
//! `#[prop(default = …)]`, and `#[component(default(field = …))]`.
//!
//! The values are read back too: an `f64`-capable field must keep taking
//! the literal as `f64` (no precision lost through `f32`), and `f32` /
//! `Reactive<f32>` fields must receive the literal's value.

#![deny(float_literal_f32_fallback)]
#![allow(unknown_lints)]

use std::cell::RefCell;

use runtime_core::{component, jsx, props, ui, Element, Reactive};

thread_local! {
    static SEEN: RefCell<Vec<String>> = const { RefCell::new(Vec::new()) };
}

fn record(s: String) {
    SEEN.with(|v| v.borrow_mut().push(s));
}

fn take() -> Vec<String> {
    SEEN.with(|v| std::mem::take(&mut *v.borrow_mut()))
}

#[component]
fn Inset(
    #[prop(static)] inset: f32,
    #[prop(static)] wide: f64,
    #[prop(static)] maybe: Option<f32>,
    scale: f32,
    #[prop(default = 2.5)] ratio: f32,
    #[prop(static, default = -0.25)] offset: f32,
) -> Element {
    record(format!(
        "inset={inset} wide={wide} maybe={maybe:?} scale={} ratio={} offset={offset}",
        scale.get(),
        ratio.get()
    ));
    ui! { view() }
}

#[props]
#[derive(Default)]
pub struct GapProps {
    pub gap: Reactive<f32>,
}

#[component(default(gap = 4.5))]
fn Gap(props: &GapProps) -> Element {
    record(format!("gap={}", props.gap.get()));
    ui! { view() }
}

#[test]
fn ui_float_literal_lands_in_f32_f64_and_reactive_props() {
    let _ = ui! {
        Inset(inset = 16.0, wide = 0.1, maybe = 3.5, scale = -1.5)
    };
    assert_eq!(
        take(),
        vec!["inset=16 wide=0.1 maybe=Some(3.5) scale=-1.5 ratio=2.5 offset=-0.25".to_string()]
    );
}

#[test]
fn jsx_float_literal_lands_in_f32_prop() {
    let _ = jsx! { <Inset inset={16.0} wide={0.1} scale={2.0} ratio={0.5} /> };
    assert_eq!(
        take(),
        vec!["inset=16 wide=0.1 maybe=None scale=2 ratio=0.5 offset=-0.25".to_string()]
    );
}

#[test]
fn component_default_float_literal_lands_in_reactive_f32() {
    let _ = ui! { Gap() };
    let _ = ui! { Gap(gap = 1.25) };
    assert_eq!(take(), vec!["gap=4.5".to_string(), "gap=1.25".to_string()]);
}
