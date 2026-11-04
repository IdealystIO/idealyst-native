//! The vocabulary's per-build emission macros — the `macro_rules!` that
//! `#[component]`, `#[props]` and `ui!` expand into, defined per the
//! vocabulary's own features/cfgs so the author's crate declares none.
//!
//! What is pinned is that the side a build DROPS is never expanded: the
//! tokens are skipped as one tree, so a `compile_error!` there compiles.
//! That is the whole point of routing the inspector probes, the remote
//! bundle stub and the call-site set through these macros — the compiler
//! never type-checks code the build can't run.

use runtime_vocabulary::glue::{BuildElement, Element};

#[derive(Default)]
struct Probe {
    built_with: std::rc::Rc<std::cell::RefCell<Option<&'static [&'static str]>>>,
}

impl BuildElement for Probe {
    fn build(self) -> Element {
        runtime_scene::fragment(Vec::new())
    }

    fn build_set(self, set: &'static [&'static str]) -> Element {
        *self.built_with.borrow_mut() = Some(set);
        self.build()
    }
}

/// A `ui!` component call site: the props stay outside the macro, the set
/// reaches `build_set` unchanged.
#[test]
fn a_call_site_hands_its_set_to_build_set() {
    let seen = std::rc::Rc::default();
    let _ = runtime_vocabulary::glue::__site(
        Probe { built_with: std::rc::Rc::clone(&seen) },
        runtime_vocabulary::__remote_site!(["label", "children"]),
    );
    assert_eq!(*seen.borrow(), Some(&["label", "children"][..]));
}

/// The one-macro form older `runtime-macros` emit still expands.
#[test]
fn the_older_whole_call_form_still_expands() {
    let seen = std::rc::Rc::default();
    let _ = runtime_vocabulary::__remote_site!(Probe { built_with: std::rc::Rc::clone(&seen) }, ["a"]);
    assert_eq!(*seen.borrow(), Some(&["a"][..]));
}

/// Outside a bundle build a component body is the app side, handed back
/// as a block; the bundle stub is never expanded.
#[test]
fn a_component_body_keeps_the_app_side_without_expanding_the_stub() {
    let v: i32 = runtime_vocabulary::__remote_guest_split! {
        bundle: { ::core::compile_error!("the bundle stub must not expand outside a bundle build") }
        app_expr: { let x = 20; x + 1 }
    };
    assert_eq!(v, 21);
}

/// The item-position form splices the kept side's contents.
mod item_position {
    runtime_vocabulary::__remote_guest_split! {
        bundle: { ::core::compile_error!("the bundle side must not expand outside a bundle build"); }
        app: { pub fn kept() -> u8 { 7 } }
    }

    #[test]
    fn splices_the_app_side() {
        assert_eq!(kept(), 7);
    }
}

/// Outside a bundle build an imported component's overrides are nothing: the
/// trait's own `build_set` still runs, and the arguments never expand.
#[derive(Default)]
struct NoOverrides;

impl BuildElement for NoOverrides {
    fn build(self) -> Element {
        runtime_scene::fragment(Vec::new())
    }
    runtime_vocabulary::__remote_build_set!("NoOverrides", ::core::compile_error!("must not expand"));
}

#[test]
fn an_imported_components_overrides_are_nothing_outside_a_bundle() {
    let _ = NoOverrides.build_set(&["label"]);
}

/// A sheet with nothing to set is its class in a preminted build, without its
/// rules ever being built, and the live sheet otherwise.
#[test]
fn a_plain_sheet_is_its_class_or_its_sheet() {
    use runtime_vocabulary::glue::{StyleProp, StyleSheet};
    fn unreachable_sheet() -> std::rc::Rc<StyleSheet> {
        panic!("a preminted build must not build the sheet")
    }
    match runtime_vocabulary::__plain_sheet_prop(true, "iy-1", unreachable_sheet) {
        StyleProp::Preminted { class, inline: None, overrides: None } => assert_eq!(class, "iy-1"),
        _ => panic!("expected the preminted class"),
    }
    fn live_sheet() -> std::rc::Rc<StyleSheet> {
        std::rc::Rc::new(StyleSheet::new(|_| Default::default()))
    }
    assert!(matches!(
        runtime_vocabulary::__plain_sheet_prop(false, "iy-1", live_sheet),
        StyleProp::Sheet(_)
    ));
}

/// In a robot build the registration bracket is the real one: the props
/// block becomes the closure `__inspect_component` calls. (The non-robot
/// side — probes dropped unexpanded — is pinned in
/// `runtime-core/tests/emission_macros_non_robot.rs`, a test graph without
/// the `robot` feature.)
#[cfg(feature = "robot")]
#[test]
fn a_robot_build_registers_a_prop_list() {
    let label = String::from("x");
    let reg = runtime_vocabulary::__inspect_component!("Probe", [label: "String"]);
    let el: Element = reg.finish(runtime_scene::fragment(Vec::new()));
    drop(el);
}

/// The form `runtime-macros` 2.0 and older emit.
#[cfg(feature = "robot")]
#[test]
fn a_robot_build_registers_with_the_props_block() {
    let reg = runtime_vocabulary::__inspect_component!("Probe", ::core::file!(), ::core::line!(), {
        ::std::vec::Vec::new()
    });
    let el: Element = reg.finish(runtime_scene::fragment(Vec::new()));
    drop(el);
}
