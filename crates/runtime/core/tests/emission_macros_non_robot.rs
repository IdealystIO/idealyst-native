//! Outside a robot build, `#[component]`'s inspector registration
//! (`runtime_vocabulary::__inspect_component!`) and `#[props]`'s
//! `InspectProps` impl (`__inspect_enabled!`) are dropped UNEXPANDED: the
//! compiler never type-checks probes nothing will run. A `compile_error!`
//! in the dropped side compiling is the proof. This lives here because
//! `runtime-vocabulary`'s own test graph always has `robot` on.

#![cfg(not(feature = "robot"))]

use runtime_core::{Element, IntoElement};

#[test]
fn inspector_probes_are_not_expanded_outside_robot_builds() {
    let reg = runtime_vocabulary::__inspect_component!("Probe", ::core::file!(), ::core::line!(), {
        ::core::compile_error!("the props probes must not expand outside a robot build")
    });
    let el: Element = runtime_core::view(Vec::new()).into_element();
    // The bracket is the identity.
    let _: Element = reg.finish(el);
}

runtime_vocabulary::__inspect_enabled! {{
    ::core::compile_error!("an InspectProps impl must not expand outside a robot build");
}}
