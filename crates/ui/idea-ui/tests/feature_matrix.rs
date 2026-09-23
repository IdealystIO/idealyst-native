//! The feature combinations a consumer can produce must all compile.
//!
//! This is the closest reachable regression test for a class of bug a
//! unit test cannot see: an item gated on one feature used from an item
//! gated on another. The one that motivated it: the `Table` recipe
//! imports `crate::Table`, which is behind the `table` feature, while
//! `recipe!` self-gates only on `catalog`. A consumer with
//! `default-features = false` running `idealyst dev` (whose `dev` feature
//! turns `catalog` on) could not compile idea-ui at all.
//!
//! It shells out to cargo because features are resolved per build, not
//! per test. It reuses the ambient target dir, so a warm run is seconds.

use std::process::Command;

fn check(args: &[&str]) {
    let cargo = std::env::var("CARGO").unwrap_or_else(|_| "cargo".into());
    let manifest = concat!(env!("CARGO_MANIFEST_DIR"), "/Cargo.toml");
    let status = Command::new(cargo)
        .args(["check", "--manifest-path", manifest, "--lib"])
        .args(args)
        .status()
        .expect("cargo runs");
    assert!(status.success(), "`cargo check {}` failed", args.join(" "));
}

#[test]
fn regression_catalog_without_table_compiles() {
    check(&["--no-default-features", "--features", "catalog"]);
}

#[test]
fn no_default_features_compiles() {
    check(&["--no-default-features"]);
}
