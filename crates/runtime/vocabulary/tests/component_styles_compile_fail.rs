//! A component's axis values are typed per component: a value of one
//! component's axis must not compile against another component's slot or
//! restyle (a value shared across components would compile against a
//! component with no arm for it and draw nothing), and an app-defined key
//! must be a class-safe name. Regenerate the `.stderr` snapshots after an intentional message
//! change with `TRYBUILD=overwrite`.

#[test]
fn component_styles_reject_foreign_axis_values() {
    let t = trybuild::TestCases::new();
    t.compile_fail("tests/component_styles_ui/*.rs");
}
