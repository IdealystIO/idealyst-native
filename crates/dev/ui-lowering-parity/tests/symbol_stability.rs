//! Adding a component must not rename its neighbours' functions.
//!
//! A hot patch pairs functions with the running program by symbol name, and
//! an `impl`'s symbol carries its position in the module. So this compiles a
//! real module three ways with the hot-patch emission on, under v0 mangling,
//! and compares what `nm` reports. Only the compiler's own numbering can
//! answer this: no token-level test sees it.
//!
//! The fixture's explicit-props struct hand-writes `Default`: a `#[derive]`
//! impl lands at module level no macro of ours can scope, so its symbol moves
//! and the patch pipeline pairs it by rename instead.

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::{Mutex, MutexGuard};

/// The tests share one fixture crate and its target directory.
static FIXTURE: Mutex<()> = Mutex::new(());

fn lock_fixture() -> MutexGuard<'static, ()> {
    FIXTURE.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
}

const BASE: &str = r#"
#![allow(dead_code)]
use runtime_macros::{component, props, stylesheet, ui};
use runtime_vocabulary::glue::Element;

//@AHEAD

stylesheet! {
    pub Card<()> {
        base(_t) { padding: 8 }
        variant size {
            #[default]
            sm(_t) { padding: 2 }
            lg(_t) { padding: 4 }
        }
    }
}

#[component]
pub fn Inline(label: String, #[prop(default = 3)] count: i32) -> Element {
    ui! { text { "{label} {count}" } }
}

#[component]
pub fn Screen() -> Element {
    ui! { view { Inline(label = "x") } }
}

#[props]
pub struct ExplicitProps {
    pub title: String,
}

impl Default for ExplicitProps {
    fn default() -> Self {
        Self { title: Default::default() }
    }
}

#[component]
pub fn Explicit(props: &ExplicitProps) -> Element {
    let _ = props;
    ui! { view {} }
}

/// The control: an impl no macro scoped, which DOES move when one is added
/// ahead of it. If this stops renaming, the comparison has stopped working.
pub mod control {
    //@CONTROL
    pub struct Unscoped;
    impl Unscoped {
        #[inline(never)]
        pub fn control_method(&self) -> u32 {
            7
        }
    }
}

//@BEHIND
"#;

const NEW_COMPONENT: &str = r#"
#[component]
pub fn Added(label: String) -> Element {
    ui! { text { "{label}" } }
}
"#;

const NEW_SHEET_AND_COMPONENTS: &str = r#"
stylesheet! {
    pub Panel<()> {
        base(_t) { padding: 1 }
        variant tone {
            #[default]
            plain(_t) { padding: 2 }
        }
    }
}

#[component]
pub fn AddedInline(label: String) -> Element {
    ui! { text { "{label}" } }
}

#[component]
pub fn AddedScreen() -> Element {
    ui! { view {} }
}
"#;

const NEW_HAND_WRITTEN_IMPL: &str = r#"
#[component]
pub fn Appended(label: String) -> Element {
    ui! { text { "{label}" } }
}

pub struct Thing;

impl Default for Thing {
    fn default() -> Self {
        Thing
    }
}
"#;

const CONTROL_IMPL_AHEAD: &str = r#"
    pub struct Ahead;
    impl Ahead {
        #[inline(never)]
        pub fn ahead_method(&self) -> u32 {
            9
        }
    }
"#;

fn repo_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).ancestors().nth(3).expect("crates/dev/<crate>").to_path_buf()
}

/// The fixture crate: its own workspace, the repo's lockfile, so the framework
/// compiles with exactly `hot-reload` and `ui-overlay` turned on and no network.
fn fixture_dir() -> PathBuf {
    let dir = Path::new(env!("CARGO_TARGET_TMPDIR")).join("symbol-stability");
    std::fs::create_dir_all(dir.join("src")).unwrap();
    let root = repo_root();
    let path = |rel: &str| root.join(rel).display().to_string();
    std::fs::write(
        dir.join("Cargo.toml"),
        format!(
            r#"[package]
name = "symbol-probe"
version = "0.0.0"
edition = "2021"

[workspace]

[dependencies]
runtime-core = {{ path = "{core}", features = ["hot-reload", "ui-overlay"] }}
runtime-macros = {{ path = "{macros}" }}
runtime-vocabulary = {{ path = "{vocabulary}" }}
"#,
            core = path("crates/runtime/core"),
            macros = path("crates/runtime/macros"),
            vocabulary = path("crates/runtime/vocabulary"),
        ),
    )
    .unwrap();
    std::fs::copy(root.join("Cargo.lock"), dir.join("Cargo.lock")).unwrap();
    dir
}

/// Every v0 symbol this source defines.
fn symbols(dir: &Path, source: &str) -> BTreeSet<String> {
    std::fs::write(dir.join("src/lib.rs"), source).unwrap();
    let target = dir.join("target");
    let deps = target.join("debug/deps");
    if let Ok(entries) = std::fs::read_dir(&deps) {
        for e in entries.flatten() {
            let name = e.file_name();
            let name = name.to_string_lossy();
            if name.starts_with("symbol_probe") && name.ends_with(".o") {
                std::fs::remove_file(e.path()).unwrap();
            }
        }
    }
    let out = Command::new(env!("CARGO"))
        .current_dir(dir)
        .env("CARGO_TARGET_DIR", &target)
        .env("CARGO_INCREMENTAL", "0")
        .env_remove("RUSTFLAGS")
        // The fixture sits outside the repo, so cargo would not find the
        // repo's config, which declares the `idealyst` registry.
        .arg("--config")
        .arg(repo_root().join(".cargo/config.toml"))
        .args(["rustc", "--lib", "--offline", "--message-format=short", "--"])
        .args(["-Csymbol-mangling-version=v0", "-Ccodegen-units=1", "--emit=obj,metadata"])
        .output()
        .expect("run cargo");
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(out.status.success(), "the fixture did not compile:\n{stderr}");
    // A generated `const` must not warn in a downstream crate: the fixture
    // stands in for one, and its only allow is `dead_code`.
    let warnings: Vec<&str> = stderr.lines().filter(|l| l.starts_with("src/lib.rs") && l.contains("warning")).collect();
    assert!(warnings.is_empty(), "the expansion warns downstream:\n{}", warnings.join("\n"));

    let object = std::fs::read_dir(&deps)
        .unwrap()
        .flatten()
        .map(|e| e.path())
        .find(|p| {
            let name = p.file_name().unwrap().to_string_lossy().into_owned();
            name.starts_with("symbol_probe") && name.ends_with(".o")
        })
        .expect("--emit=obj wrote an object");
    let nm = Command::new("nm").arg(&object).output().expect("run nm");
    assert!(nm.status.success());
    String::from_utf8_lossy(&nm.stdout)
        .lines()
        .filter_map(|line| {
            let mut parts = line.split_whitespace().rev();
            let name = parts.next()?;
            // Defined text symbols only: what a patch pairs by name.
            matches!(parts.next()?, "T" | "t").then_some(name)?;
            let name = name.strip_prefix('_').unwrap_or(name);
            name.starts_with("_R").then(|| name.to_string())
        })
        .collect()
}

fn fill(ahead: &str, control: &str, behind: &str) -> String {
    BASE.replace("//@AHEAD", ahead).replace("//@CONTROL", control).replace("//@BEHIND", behind)
}

/// What `added` renamed: base symbols it no longer has, leaving out the
/// control module, which is expected to move.
fn renamed(base: &BTreeSet<String>, added: &BTreeSet<String>) -> Vec<String> {
    base.difference(added).filter(|s| !s.contains("7control")).cloned().collect()
}

#[test]
fn regression_adding_a_component_keeps_every_neighbours_symbol() {
    let _fixture = lock_fixture();
    let dir = fixture_dir();
    let base = symbols(&dir, &fill("", "", ""));

    // Sanity: the fixture really does emit the impl symbols under test.
    for needle in ["__component_scope_Inline", "__component_scope_Explicit", "__props_scope_ExplicitProps", "__sheet_scope_Card"] {
        assert!(base.iter().any(|s| s.contains(needle)), "no symbol under {needle} in:\n{base:#?}");
    }

    let front = symbols(&dir, &fill(NEW_SHEET_AND_COMPONENTS, CONTROL_IMPL_AHEAD, ""));
    let moved = renamed(&base, &front);
    assert!(moved.is_empty(), "a sheet and components added ahead renamed:\n{moved:#?}");

    // The comparison can see a rename: the unscoped impl moved.
    let control = |set: &BTreeSet<String>| {
        set.iter().filter(|s| s.contains("control_method")).cloned().collect::<BTreeSet<_>>()
    };
    assert_eq!(control(&base).len(), 1, "{base:#?}");
    assert_ne!(control(&base), control(&front), "an impl added ahead of the control must rename it");
}

/// Hand-written impls are numbered ahead of macro-expanded ones, so without a
/// scope this one renames every generated impl in the module, wherever in the
/// file it is written.
#[test]
fn regression_a_hand_written_impl_at_end_of_file_keeps_every_neighbours_symbol() {
    let _fixture = lock_fixture();
    let dir = fixture_dir();
    let base = symbols(&dir, &fill("", "", ""));
    let appended = symbols(&dir, &fill("", "", NEW_HAND_WRITTEN_IMPL));
    let moved = renamed(&base, &appended);
    assert!(moved.is_empty(), "a component and hand-written impl appended renamed:\n{moved:#?}");
}

/// A new component alone, ahead of the rest: the most common addition.
#[test]
fn regression_a_component_inserted_ahead_keeps_every_neighbours_symbol() {
    let _fixture = lock_fixture();
    let dir = fixture_dir();
    let base = symbols(&dir, &fill("", "", ""));
    let front = symbols(&dir, &fill(NEW_COMPONENT, "", ""));
    let moved = renamed(&base, &front);
    assert!(moved.is_empty(), "a component added ahead renamed:\n{moved:#?}");
}
