//! The decision for a full-stack project's SERVER: patch the running
//! process in place, or rebuild and respawn it.
//!
//! The server is patched by redirecting the running binary's functions to
//! a patch's copies (`build_runtime_server::hotpatch::detour`, whose docs
//! cover the mechanism). Only the edited crates are re-emitted: a
//! dependent's unchanged code calls the edited crate directly, and the
//! redirect sends that call to the new body. So the question per saved
//! file is narrower than the web's, and needs no archive: the session
//! keeps the sources the running binary was built from, and each save is
//! compared with those.
//!
//! A file's save is a patch when, and only when:
//!
//! 1. its SHAPE is unchanged ([`runtime_macros_parse::shape_of`]): no
//!    signature, type, item, attribute or static moved — a patch can
//!    change code, never a layout;
//! 2. no `const` changed its value: a value is copied into every use, in
//!    other crates and in code a redirect never reaches;
//! 3. nothing a redirect cannot carry changed
//!    ([`runtime_macros_parse::detour_facts`]): a generic, `#[inline]`,
//!    `const fn`, `async fn` or `impl Trait` body, a closure or `async`
//!    block, an item declared in a body, `fn main`, an unmangled
//!    function.
//!
//! Anything else — a new or deleted file, a file that does not parse, a
//! non-Rust file, a manifest — rebuilds. "Cannot tell" always rebuilds:
//! a wrong patch corrupts a running process, a needless rebuild only
//! costs time.

use std::collections::{BTreeMap, BTreeSet};

/// One saved file, as the server's session sees it.
#[derive(Debug, Clone)]
pub struct NativeSave<'a> {
    /// The package that owns it.
    pub package: &'a str,
    /// For messages: the path as the session shows it.
    pub display: &'a str,
    /// Whether it is Rust source of the package's library or binary
    /// (`src/**.rs`, not `build.rs`, a test or an example).
    pub source: bool,
    /// The contents the running binary was built from, `None` when the
    /// file is new.
    pub before: Option<&'a str>,
    /// The contents now, `None` when the file was deleted.
    pub after: Option<&'a str>,
}

/// What to do with a save.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum NativeDecision {
    /// No file the running binary was built from changed.
    Unchanged,
    /// Re-emit these packages and patch the running server.
    Patch { packages: BTreeSet<String> },
    Rebuild(NativeReason),
}

/// Why a save rebuilds, naming the file.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum NativeReason {
    /// Not Rust source of a library or binary: a manifest, `build.rs`, an
    /// asset, a test.
    NotSource { file: String },
    Added { file: String },
    Removed { file: String },
    Unparsable { file: String },
    Shape { file: String },
    ConstValue { file: String, name: String },
    Body { file: String, item: String },
}

impl std::fmt::Display for NativeReason {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            NativeReason::NotSource { file } => write!(f, "{file} is not a source file of the server's crates"),
            NativeReason::Added { file } => write!(f, "{file} is a new file"),
            NativeReason::Removed { file } => write!(f, "{file} was deleted"),
            NativeReason::Unparsable { file } => write!(f, "{file} does not parse"),
            NativeReason::Shape { file } => {
                write!(f, "{file} changed outside function bodies (a signature, a type, an item or a static)")
            }
            NativeReason::ConstValue { file, name } => {
                write!(f, "{file} changed the value of `const {name}`, which is copied into every use")
            }
            NativeReason::Body { file, item } => write!(
                f,
                "{file} changed {item}, which a running server keeps or copies where a patch does not \
                 reach (a generic, inline, const, async or impl Trait body, a closure or async block, \
                 an item in a body, `main`, or an unmangled fn)"
            ),
        }
    }
}

/// Decide a save. See the module docs for the rules.
pub fn decide_native(saves: &[NativeSave<'_>]) -> NativeDecision {
    let mut packages = BTreeSet::new();
    for s in saves {
        if s.before == s.after {
            continue;
        }
        let file = s.display.to_string();
        if !s.source {
            return NativeDecision::Rebuild(NativeReason::NotSource { file });
        }
        let (before, after) = match (s.before, s.after) {
            (Some(b), Some(a)) => (b, a),
            (None, _) => return NativeDecision::Rebuild(NativeReason::Added { file }),
            (_, None) => return NativeDecision::Rebuild(NativeReason::Removed { file }),
        };
        if let Some(reason) = file_reason(&file, before, after) {
            return NativeDecision::Rebuild(reason);
        }
        packages.insert(s.package.to_string());
    }
    if packages.is_empty() {
        NativeDecision::Unchanged
    } else {
        NativeDecision::Patch { packages }
    }
}

fn file_reason(file: &str, before: &str, after: &str) -> Option<NativeReason> {
    let unparsable = || NativeReason::Unparsable { file: file.to_string() };
    let (Some(shape_before), Some(shape_after)) =
        (runtime_macros_parse::shape_of(before), runtime_macros_parse::shape_of(after))
    else {
        return Some(unparsable());
    };
    if shape_before != shape_after {
        return Some(NativeReason::Shape { file: file.to_string() });
    }
    let (Some(consts_before), Some(consts_after)) =
        (runtime_macros_parse::const_facts(before), runtime_macros_parse::const_facts(after))
    else {
        return Some(unparsable());
    };
    if let Some(name) = first_difference(&consts_before.values, &consts_after.values) {
        let name = consts_before.values.get(&name).or(consts_after.values.get(&name)).map(|v| v.name.clone()).unwrap_or(name);
        return Some(NativeReason::ConstValue { file: file.to_string(), name });
    }
    let (Some(facts_before), Some(facts_after)) =
        (runtime_macros_parse::detour_facts(before), runtime_macros_parse::detour_facts(after))
    else {
        return Some(unparsable());
    };
    first_difference(&facts_before, &facts_after).map(|item| NativeReason::Body { file: file.to_string(), item })
}

/// The first key whose value differs, or that only one side has.
fn first_difference<V: PartialEq>(a: &BTreeMap<String, V>, b: &BTreeMap<String, V>) -> Option<String> {
    a.iter()
        .find(|(k, v)| b.get(*k) != Some(*v))
        .map(|(k, _)| k.clone())
        .or_else(|| b.keys().find(|k| !a.contains_key(*k)).cloned())
}

#[cfg(test)]
mod tests {
    use super::*;

    const BASE: &str = r#"
use std::sync::atomic::{AtomicU64, Ordering};

pub static HITS: AtomicU64 = AtomicU64::new(0);
pub const LIMIT: u32 = 10;

pub fn variance_label(n: i64) -> String {
    HITS.fetch_add(1, Ordering::Relaxed);
    if n > 0 { format!("+{n}") } else { format!("{n}") }
}

pub async fn load(id: u32) -> u32 {
    id + 1
}

pub fn router() -> Vec<Box<dyn Fn() -> u32>> {
    vec![Box::new(|| 1)]
}

pub fn first<T: Clone>(v: &[T]) -> T {
    v[0].clone()
}
"#;

    fn decide(before: &str, after: &str) -> NativeDecision {
        decide_native(&[NativeSave {
            package: "core",
            display: "core/src/lib.rs",
            source: true,
            before: Some(before),
            after: Some(after),
        }])
    }

    fn patch() -> NativeDecision {
        NativeDecision::Patch { packages: BTreeSet::from(["core".to_string()]) }
    }

    /// The proposal's own edit: a statement in a plain function.
    #[test]
    fn a_statement_in_a_plain_function_patches() {
        let edited = BASE.replace(
            "HITS.fetch_add(1, Ordering::Relaxed);",
            "let _ = ::std::hint::black_box(1u64);\n    HITS.fetch_add(1, Ordering::Relaxed);",
        );
        assert_eq!(decide(BASE, &edited), patch());
    }

    #[test]
    fn identical_contents_are_unchanged() {
        assert_eq!(decide(BASE, BASE), NativeDecision::Unchanged);
    }

    #[test]
    fn a_signature_change_rebuilds() {
        let edited = BASE.replace("variance_label(n: i64)", "variance_label(n: i32)");
        assert!(matches!(decide(BASE, &edited), NativeDecision::Rebuild(NativeReason::Shape { .. })));
    }

    #[test]
    fn a_const_value_change_rebuilds() {
        let edited = BASE.replace("LIMIT: u32 = 10", "LIMIT: u32 = 11");
        assert_eq!(
            decide(BASE, &edited),
            NativeDecision::Rebuild(NativeReason::ConstValue { file: "core/src/lib.rs".into(), name: "LIMIT".into() })
        );
    }

    /// The future's layout comes from its body, and a caller in another
    /// crate (or a future already in flight) holds it at the old size.
    #[test]
    fn an_async_body_edit_rebuilds() {
        let edited = BASE.replace("id + 1", "id + 2");
        assert!(matches!(decide(BASE, &edited), NativeDecision::Rebuild(NativeReason::Body { item, .. }) if item == "fn load"));
    }

    /// A closure built at startup — a route handler — keeps its old body.
    #[test]
    fn a_closure_body_edit_rebuilds() {
        let edited = BASE.replace("Box::new(|| 1)", "Box::new(|| 2)");
        assert!(matches!(
            decide(BASE, &edited),
            NativeDecision::Rebuild(NativeReason::Body { item, .. }) if item == "closure #1 in fn router"
        ));
    }

    #[test]
    fn a_generic_body_edit_rebuilds() {
        let edited = BASE.replace("v[0].clone()", "v[v.len() - 1].clone()");
        assert!(matches!(decide(BASE, &edited), NativeDecision::Rebuild(NativeReason::Body { item, .. }) if item == "fn first"));
    }

    #[test]
    fn new_deleted_unparsable_and_non_source_files_rebuild() {
        let save = |source, before, after| NativeSave { package: "core", display: "f", source, before, after };
        assert!(matches!(decide_native(&[save(true, None, Some(BASE))]), NativeDecision::Rebuild(NativeReason::Added { .. })));
        assert!(matches!(decide_native(&[save(true, Some(BASE), None)]), NativeDecision::Rebuild(NativeReason::Removed { .. })));
        assert!(matches!(
            decide_native(&[save(true, Some(BASE), Some("fn broken( {"))]),
            NativeDecision::Rebuild(NativeReason::Unparsable { .. })
        ));
        assert!(matches!(
            decide_native(&[save(false, Some("a"), Some("b"))]),
            NativeDecision::Rebuild(NativeReason::NotSource { .. })
        ));
    }

    /// Every edited package is re-emitted; a rebuild anywhere wins.
    #[test]
    fn a_save_across_packages_patches_all_or_rebuilds() {
        let edited = BASE.replace("format!(\"{n}\")", "format!(\"{n} \")");
        let a = NativeSave { package: "core", display: "core/src/lib.rs", source: true, before: Some(BASE), after: Some(&edited) };
        let b = NativeSave { package: "api", display: "api/src/lib.rs", source: true, before: Some(BASE), after: Some(&edited) };
        assert_eq!(
            decide_native(&[a.clone(), b.clone()]),
            NativeDecision::Patch { packages: BTreeSet::from(["api".to_string(), "core".to_string()]) }
        );
        let shape = BASE.replace("pub fn first", "pub fn first_one");
        let c = NativeSave { after: Some(&shape), ..b };
        assert!(matches!(decide_native(&[a, c]), NativeDecision::Rebuild(_)));
    }
}
