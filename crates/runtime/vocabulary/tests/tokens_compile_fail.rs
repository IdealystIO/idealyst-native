//! `#[derive(Tokens)]` must REJECT, at compile time, every way an app's
//! theme can be wrong: a misspelled accessor, a theme function that skips
//! a token, a field that is not a token, installing a shape, two fields
//! that kebab to one token name, and a non-colour host background. Each
//! of these was previously a string that compiled and rendered its
//! fallback forever. Regenerate the `.stderr` snapshots after an
//! intentional message change with `TRYBUILD=overwrite`.

#[test]
fn derive_tokens_rejects_invalid_themes() {
    let t = trybuild::TestCases::new();
    t.compile_fail("tests/tokens_ui/*.rs");
}
