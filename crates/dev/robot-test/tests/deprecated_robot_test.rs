//! `#[robot_test]`, the former name of `#[idea_test]`: it still expands
//! (arguments included), and the caller is warned it is deprecated.

#[test]
fn robot_test_expands_like_idea_test_and_warns_deprecated() {
    let t = trybuild::TestCases::new();
    t.pass("tests/ui/robot_test_still_expands.rs");
    t.compile_fail("tests/ui/robot_test_is_deprecated.rs");
}
