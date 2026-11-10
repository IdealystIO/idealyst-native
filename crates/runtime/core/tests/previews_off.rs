//! Without the `previews` feature — every dev and release build — nothing
//! preview-related compiles:
//!
//! - a `#[prop_control]` impl is removed: this file names
//!   `runtime_core::previews::PropControl`, which does not exist in these
//!   builds, so it compiles only because the attribute dropped the impl;
//! - `#[prop(example = …)]` is accepted and changes nothing;
//! - with `catalog` on but `previews` off (what `idealyst dev` builds), a
//!   the catalog registers no preview symbol.
//!
//!   cargo test -p runtime-core --test previews_off
//!   cargo test -p runtime-core --features catalog --test previews_off

#![cfg(not(feature = "previews"))]

use std::cell::RefCell;

use runtime_core::{component, prop_control, props, ui, Element};

thread_local! {
    static SEEN: RefCell<Vec<String>> = const { RefCell::new(Vec::new()) };
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Rating(pub u8);

#[prop_control]
impl runtime_core::previews::PropControl for Rating {
    fn control() -> runtime_core::previews::Control {
        unreachable!("never compiled without `previews`")
    }
    fn to_value(&self) -> runtime_core::previews::ControlValue {
        unreachable!()
    }
    fn from_value(_: &runtime_core::previews::ControlValue) -> Option<Self> {
        unreachable!()
    }
}

#[component]
fn Labeled(#[prop(example = "Example")] label: String, rating: Rating) -> Element {
    SEEN.with(|s| s.borrow_mut().push(format!("{}:{:?}", label.get(), rating.get())));
    ui! { view() }
}

#[props]
#[derive(Default)]
pub struct TitledProps {
    #[prop(static, example = "Example")]
    pub title: String,
}

#[component]
fn Titled(props: TitledProps) -> Element {
    SEEN.with(|s| s.borrow_mut().push(props.title.clone()));
    ui! { view() }
}

#[test]
fn example_is_inert_and_prop_control_impls_vanish() {
    let _ = ui! { Labeled() };
    let _ = ui! { Titled() };
    // `static` still applies alongside `example`: `title` stays a bare
    // `String`, not `Reactive<String>`.
    let _: String = TitledProps::default().title;
    SEEN.with(|s| {
        assert_eq!(*s.borrow(), [":Rating(0)".to_string(), String::new()], "defaults, not examples");
    });
}

#[cfg(feature = "catalog")]
#[test]
fn catalog_without_previews_records_no_symbol() {
    assert!(
        runtime_core::__mcp::entries().any(|e| e.name == "Labeled" && e.module_path == module_path!()),
        "Labeled is in the catalog"
    );
    assert_eq!(runtime_core::__mcp::previews().count(), 0, "no preview is registered without `previews`");
}

/// `#[derive(PropControl)]` expands to nothing here: the trait it would
/// implement does not exist in this build.
#[derive(Clone, Copy, Debug, Default, PartialEq, runtime_core::PropControl)]
pub enum Expand {
    #[default]
    Single,
    Multi,
}

#[test]
fn derived_prop_control_vanishes() {
    assert_eq!(Expand::default(), Expand::Single);
    assert_ne!(Expand::Multi, Expand::Single);
}
