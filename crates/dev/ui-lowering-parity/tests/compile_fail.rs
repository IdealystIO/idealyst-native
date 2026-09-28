//! `ui!` must REJECT a prop a primitive does not accept.
//!
//! Every case in `tests/ui/` used to compile: the primitive emitters
//! read a hand-picked handful of props and dropped the rest in silence,
//! so a typo (`on_tuch`), a prop the primitive never had (`view(label =
//! …)`), or a duplicate compiled and did nothing. Each must now fail with
//! a diagnostic spanned on the prop NAME that lists what the primitive
//! does accept. Regenerate the `.stderr` snapshots after an intentional
//! message change with `TRYBUILD=overwrite`.

#[test]
fn regression_ui_primitive_unknown_prop_is_a_compile_error() {
    let t = trybuild::TestCases::new();
    t.compile_fail("tests/ui/*.rs");
}

/// `stylesheet!` sheets that MUST compile clean: a `///` doc comment and
/// lint/cfg attributes on the declared sheet (Wave-42 / #2), and a
/// `<ThemeType>` imported only to name the sheet's vocabulary counts as
/// used even when no block reads the binding (#3).
#[test]
fn regression_stylesheet_accepts_docs_and_uses_its_theme_import() {
    let t = trybuild::TestCases::new();
    t.pass("tests/pass/*.rs");
}
