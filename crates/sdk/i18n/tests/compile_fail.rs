//! The strong-typing headline, proven: each case in `tests/ui/` must fail
//! to compile with the expected diagnostic. Regenerate the `.stderr`
//! snapshots after intentional message changes with `TRYBUILD=overwrite`.

#[test]
fn compile_fail() {
    let t = trybuild::TestCases::new();
    t.compile_fail("tests/ui/*.rs");
}
