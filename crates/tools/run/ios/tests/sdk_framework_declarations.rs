//! Every Apple framework an SDK crate links on iOS must also be declared in
//! that crate's `[package.metadata.idealyst.ios].frameworks`.
//!
//! # The bug this prevents
//!
//! A `#[link(name = "Network", kind = "framework")]` attribute is honoured
//! when *rustc* runs the final link (a macOS binary, a test executable). The
//! iOS app is different: the Rust code ships as a staticlib, and the final
//! link is done by Xcode (`xcodebuild` on the device path) or `swiftc` (the
//! simulator path). A staticlib carries no link directives, so the framework
//! request is dropped, and the link fails with undefined symbols
//! (`_nw_path_monitor_*` for the `connectivity` SDK) — or, for a framework
//! used only through the Obj-C runtime, the class is missing at launch.
//!
//! The iOS run tool links exactly the frameworks it collects from
//! `[package.metadata.idealyst.ios].frameworks` across the app's dependency
//! closure (see `run_ios::frameworks`), plus a fixed base set. So the
//! declaration is the only thing that reaches the iOS linker. This test scans
//! every crate under `crates/sdk/` for `#[link(..., kind = "framework")]`
//! attributes that are compiled on iOS — following `mod` declarations and
//! evaluating `#[cfg]`s for an iOS device and simulator target — and fails
//! for any framework the crate's metadata doesn't declare.
//!
//! The framework's own crates get the same scan: every crate in the closure
//! every iOS app links (`runtime-core`, `runtime-shared`,
//! `backend-ios-mobile`, resolved through `cargo metadata`) must have its
//! iOS framework links covered by the run tool's base set.

use std::collections::BTreeSet;
use std::fs;
use std::path::{Path, PathBuf};

use syn::punctuated::Punctuated;
use syn::{Attribute, Expr, ExprLit, Item, Lit, Meta, Token};

/// The two iOS compilation targets the app is linked for. The `cfg`
/// evaluation runs once per flavour and the results are unioned, so a
/// framework linked only on the device (the `camera` SDK routes the
/// simulator to a synthetic backend) still counts.
#[derive(Clone, Copy)]
enum Flavour {
    Device,
    Simulator,
}

/// Evaluate a `#[cfg(...)]` predicate for an iOS target.
///
/// Features count as enabled: an app can turn any of them on, and a link
/// behind a feature is still a link the iOS build must satisfy. An unknown
/// key panics so a new kind of predicate can't silently be evaluated wrong.
fn eval_cfg(meta: &Meta, flavour: Flavour) -> bool {
    match meta {
        Meta::Path(path) => {
            let name = path_name(path);
            match name.as_str() {
                "unix" | "debug_assertions" | "panic" => true,
                "windows" | "test" | "doc" | "doctest" | "miri" => false,
                // Set only by `idealyst build remote` for wasm32 stream
                // bundles (crates/tools/build/remote), never for an iOS app.
                "idealyst_stream_guest" => false,
                // Set only by the web / SSR build tools
                // (crates/tools/build/{web,ssr}); the iOS build never passes them.
                "idealyst_premint"
                | "idealyst_premint_only"
                | "idealyst_premint_report"
                | "idealyst_premint_dump" => false,
                other => panic!("unknown cfg flag `{other}` — teach eval_cfg about it"),
            }
        }
        Meta::NameValue(nv) => {
            let key = path_name(&nv.path);
            let Expr::Lit(ExprLit { lit: Lit::Str(value), .. }) = &nv.value else {
                panic!("cfg `{key}` with a non-string value");
            };
            let value = value.value();
            match key.as_str() {
                "target_os" => value == "ios",
                "target_vendor" => value == "apple",
                "target_family" => value == "unix",
                "target_arch" => value == "aarch64",
                "target_pointer_width" => value == "64",
                "target_endian" => value == "little",
                "target_env" => match flavour {
                    Flavour::Device => value.is_empty(),
                    Flavour::Simulator => value == "sim" || value.is_empty(),
                },
                "target_abi" => match flavour {
                    Flavour::Device => value.is_empty(),
                    Flavour::Simulator => value == "sim",
                },
                "feature" => true,
                "panic" => value == "unwind",
                other => panic!("unknown cfg key `{other}` — teach eval_cfg about it"),
            }
        }
        Meta::List(list) => {
            let name = path_name(&list.path);
            let nested: Vec<Meta> = list
                .parse_args_with(Punctuated::<Meta, Token![,]>::parse_terminated)
                .unwrap_or_else(|e| panic!("parse cfg `{name}(...)`: {e}"))
                .into_iter()
                .collect();
            match name.as_str() {
                "any" => nested.iter().any(|m| eval_cfg(m, flavour)),
                "all" => nested.iter().all(|m| eval_cfg(m, flavour)),
                "not" => {
                    assert_eq!(nested.len(), 1, "cfg not(...) takes one predicate");
                    !eval_cfg(&nested[0], flavour)
                }
                other => panic!("unknown cfg combinator `{other}`"),
            }
        }
    }
}

fn path_name(path: &syn::Path) -> String {
    path.segments
        .iter()
        .map(|s| s.ident.to_string())
        .collect::<Vec<_>>()
        .join("::")
}

/// An item's attributes as the compiler sees them for this flavour:
/// `#[cfg_attr(pred, a, b)]` becomes `a, b` when `pred` holds and vanishes
/// otherwise (the `screen-recorder` SDK picks its `imp` file this way).
fn effective_attrs(attrs: &[Attribute], flavour: Flavour) -> Vec<Meta> {
    let mut out = Vec::new();
    for attr in attrs {
        expand_meta(attr.meta.clone(), flavour, &mut out);
    }
    out
}

fn expand_meta(meta: Meta, flavour: Flavour, out: &mut Vec<Meta>) {
    match &meta {
        Meta::List(list) if list.path.is_ident("cfg_attr") => {
            let mut parts = list
                .parse_args_with(Punctuated::<Meta, Token![,]>::parse_terminated)
                .expect("parse #[cfg_attr(...)]")
                .into_iter();
            let pred = parts.next().expect("cfg_attr predicate");
            if eval_cfg(&pred, flavour) {
                for m in parts {
                    expand_meta(m, flavour, out);
                }
            }
        }
        _ => out.push(meta),
    }
}

/// Are all of an item's `#[cfg]`s true for this flavour?
fn cfg_enabled(attrs: &[Meta], flavour: Flavour) -> bool {
    attrs.iter().all(|m| match m {
        Meta::List(list) if list.path.is_ident("cfg") => {
            let pred: Meta = list.parse_args().expect("parse #[cfg(...)]");
            eval_cfg(&pred, flavour)
        }
        _ => true,
    })
}

/// `#[path = "..."]` on a `mod` item, if any.
fn path_attr(attrs: &[Meta]) -> Option<String> {
    attrs.iter().find_map(|m| match m {
        Meta::NameValue(nv) if nv.path.is_ident("path") => match &nv.value {
            Expr::Lit(ExprLit { lit: Lit::Str(s), .. }) => Some(s.value()),
            _ => panic!("malformed #[path]"),
        },
        _ => None,
    })
}

/// The framework name of a `#[link(name = "X", kind = "framework")]`, if the
/// attribute is one.
fn link_framework(meta: &Meta) -> Option<String> {
    let Meta::List(list) = meta else { return None };
    if !list.path.is_ident("link") {
        return None;
    }
    let args = list
        .parse_args_with(Punctuated::<Meta, Token![,]>::parse_terminated)
        .expect("parse #[link(...)]");
    let mut name = None;
    let mut is_framework = false;
    for arg in args {
        let Meta::NameValue(nv) = arg else { continue };
        let Expr::Lit(ExprLit { lit: Lit::Str(s), .. }) = &nv.value else { continue };
        if nv.path.is_ident("name") {
            name = Some(s.value());
        } else if nv.path.is_ident("kind") {
            is_framework = s.value() == "framework";
        }
    }
    if is_framework { name } else { None }
}

/// Where a source file's out-of-line child modules live: the file's own
/// directory for `lib.rs` / `main.rs` / `mod.rs`, else `<dir>/<stem>/`.
fn child_dir(file: &Path) -> PathBuf {
    let dir = file.parent().expect("source file has a parent").to_path_buf();
    match file.file_stem().and_then(|s| s.to_str()) {
        Some("lib" | "main" | "mod") => dir,
        Some(stem) => dir.join(stem),
        None => dir,
    }
}

/// Walk `items` (from `file`, with out-of-line children resolved under
/// `mod_dir`) and collect the frameworks linked by enabled `extern` blocks.
fn collect_items(
    items: &[Item],
    file: &Path,
    mod_dir: &Path,
    flavour: Flavour,
    out: &mut BTreeSet<String>,
) {
    for item in items {
        match item {
            Item::ForeignMod(fm) => {
                let attrs = effective_attrs(&fm.attrs, flavour);
                if cfg_enabled(&attrs, flavour) {
                    out.extend(attrs.iter().filter_map(link_framework));
                }
            }
            Item::Mod(m) => {
                let attrs = effective_attrs(&m.attrs, flavour);
                if !cfg_enabled(&attrs, flavour) {
                    continue;
                }
                let explicit = path_attr(&attrs);
                match &m.content {
                    Some((_, inner)) => {
                        let dir = match &explicit {
                            Some(p) => mod_dir.join(p),
                            None => mod_dir.join(m.ident.to_string()),
                        };
                        collect_items(inner, file, &dir, flavour, out);
                    }
                    None => {
                        let child = match &explicit {
                            // Out-of-line `#[path]` resolves against the
                            // declaring file's directory.
                            Some(p) => file.parent().unwrap().join(p),
                            None => {
                                let name = m.ident.to_string();
                                let flat = mod_dir.join(format!("{name}.rs"));
                                if flat.is_file() {
                                    flat
                                } else {
                                    mod_dir.join(name).join("mod.rs")
                                }
                            }
                        };
                        collect_file(&child, flavour, out);
                    }
                }
            }
            _ => {}
        }
    }
}

fn collect_file(file: &Path, flavour: Flavour, out: &mut BTreeSet<String>) {
    let src = fs::read_to_string(file)
        .unwrap_or_else(|e| panic!("read {}: {e}", file.display()));
    let parsed =
        syn::parse_file(&src).unwrap_or_else(|e| panic!("parse {}: {e}", file.display()));
    collect_items(&parsed.items, file, &child_dir(file), flavour, out);
}

/// Every framework a crate links when compiled for iOS (device ∪ simulator).
fn ios_linked_frameworks(lib_rs: &Path) -> BTreeSet<String> {
    let mut out = BTreeSet::new();
    for flavour in [Flavour::Device, Flavour::Simulator] {
        collect_file(lib_rs, flavour, &mut out);
    }
    out
}

/// `[package.metadata.idealyst.ios].frameworks` from a crate's manifest.
fn declared_ios_frameworks(manifest: &toml::Value) -> BTreeSet<String> {
    manifest
        .get("package")
        .and_then(|p| p.get("metadata"))
        .and_then(|m| m.get("idealyst"))
        .and_then(|i| i.get("ios"))
        .and_then(|i| i.get("frameworks"))
        .and_then(|f| f.as_array())
        .map(|a| a.iter().filter_map(|v| v.as_str().map(String::from)).collect())
        .unwrap_or_default()
}

/// Every `Cargo.toml` under `dir`, skipping build output and vendored trees.
fn find_manifests(dir: &Path, out: &mut Vec<PathBuf>) {
    let Ok(entries) = fs::read_dir(dir) else { return };
    for entry in entries.flatten() {
        let path = entry.path();
        let name = entry.file_name();
        let name = name.to_string_lossy();
        if path.is_dir() {
            if matches!(name.as_ref(), "target" | "node_modules" | ".git") {
                continue;
            }
            find_manifests(&path, out);
        } else if name == "Cargo.toml" {
            out.push(path);
        }
    }
}

fn repo_root() -> PathBuf {
    // crates/tools/run/ios → repo root.
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .ancestors()
        .nth(4)
        .expect("repo root above crates/tools/run/ios")
        .to_path_buf()
}

/// Names the iOS build links regardless of any declaration (the run tool's
/// base set — `BASE` in `src/frameworks.rs`).
fn base_frameworks() -> BTreeSet<String> {
    run_ios::frameworks::collect_ios_frameworks(Path::new("/nonexistent"))
        .expect("base set")
        .into_iter()
        .map(|f| f.name)
        .collect()
}

#[test]
fn regression_sdk_framework_links_are_declared_for_ios() {
    let sdk_root = repo_root().join("crates").join("sdk");
    let mut manifests = Vec::new();
    find_manifests(&sdk_root, &mut manifests);
    manifests.sort();
    assert!(!manifests.is_empty(), "no SDK crates found under {}", sdk_root.display());

    let base = base_frameworks();
    let mut scanned = 0;
    let mut problems = Vec::new();
    for manifest_path in &manifests {
        let text = fs::read_to_string(manifest_path).unwrap();
        let manifest: toml::Value = toml::from_str(&text)
            .unwrap_or_else(|e| panic!("parse {}: {e}", manifest_path.display()));
        if manifest.get("package").is_none() {
            continue; // a workspace root, not a crate
        }
        let crate_dir = manifest_path.parent().unwrap();
        let lib_rs = manifest
            .get("lib")
            .and_then(|l| l.get("path"))
            .and_then(|p| p.as_str())
            .map(|p| crate_dir.join(p))
            .unwrap_or_else(|| crate_dir.join("src").join("lib.rs"));
        if !lib_rs.is_file() {
            continue; // binary-only crate: rustc links it, directives survive
        }
        scanned += 1;
        let declared = declared_ios_frameworks(&manifest);
        let missing: Vec<String> = ios_linked_frameworks(&lib_rs)
            .into_iter()
            .filter(|fw| !base.contains(fw) && !declared.contains(fw))
            .collect();
        if !missing.is_empty() {
            let rel = manifest_path.strip_prefix(repo_root()).unwrap_or(manifest_path);
            problems.push(format!(
                "{}: links {missing:?} on iOS but [package.metadata.idealyst.ios].frameworks \
                 doesn't declare them",
                rel.display()
            ));
        }
    }
    assert!(scanned > 10, "scanned only {scanned} SDK crates — is the walk broken?");
    assert!(
        problems.is_empty(),
        "an iOS app links SDK code as a staticlib, so `#[link(kind = \"framework\")]` is \
         lost; declare the framework in the crate's metadata instead:\n  {}",
        problems.join("\n  ")
    );
}

/// The scanner itself: `connectivity` (the crate that shipped the bug) must
/// be seen to link Network on iOS, so a broken walk can't pass vacuously.
#[test]
fn scanner_sees_connectivity_link_network_on_ios() {
    let lib_rs = repo_root().join("crates/sdk/client/connectivity/src/lib.rs");
    let linked = ios_linked_frameworks(&lib_rs);
    assert!(linked.contains("Network"), "linked on iOS: {linked:?}");
}

/// `cfg` evaluation: a macOS-only module's links are not iOS links, and the
/// simulator/device split is honoured.
#[test]
fn cfg_evaluation_targets_ios() {
    let parse = |s: &str| -> Meta { syn::parse_str(s).unwrap() };
    let macos_only = parse(r#"all(target_os = "macos", not(target_arch = "wasm32"))"#);
    assert!(!eval_cfg(&macos_only, Flavour::Device));
    let apple = parse(r#"any(target_os = "ios", target_os = "macos")"#);
    assert!(eval_cfg(&apple, Flavour::Device));
    let sim = parse(r#"all(target_os = "ios", target_abi = "sim")"#);
    assert!(eval_cfg(&sim, Flavour::Simulator));
    assert!(!eval_cfg(&sim, Flavour::Device));
    assert!(!eval_cfg(&parse("test"), Flavour::Device));
}

/// The crates every iOS app links, whatever SDKs it uses: the dependencies of
/// the generated build wrapper (`crates/tools/build/ios`: `runtime-core`,
/// `runtime-shared`, and `backend-ios-mobile` on iOS).
const BASE_CLOSURE_ROOTS: &[&str] = &["runtime-core", "runtime-shared", "backend-ios-mobile"];

/// The workspace crates in the base closure, resolved by `cargo metadata` for
/// the iOS device target with every feature on (an app can enable any of
/// them, and a feature-gated link is still a link). Only crates that end up
/// in the staticlib count: proc-macros and build-only deps run on the host.
/// Returns `(name, Cargo.toml path, lib.rs path)`.
fn base_closure_workspace_crates() -> Vec<(String, PathBuf, PathBuf)> {
    let cargo = std::env::var("CARGO").unwrap_or_else(|_| "cargo".into());
    let output = std::process::Command::new(cargo)
        .args([
            "metadata",
            "--format-version",
            "1",
            "--filter-platform",
            "aarch64-apple-ios",
            "--all-features",
        ])
        .current_dir(repo_root())
        .output()
        .expect("spawn cargo metadata");
    assert!(
        output.status.success(),
        "cargo metadata failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let meta: serde_json::Value = serde_json::from_slice(&output.stdout).expect("metadata JSON");

    let packages = meta["packages"].as_array().expect("packages");
    let by_id: std::collections::HashMap<&str, &serde_json::Value> = packages
        .iter()
        .map(|p| (p["id"].as_str().unwrap(), p))
        .collect();
    let nodes: std::collections::HashMap<&str, &serde_json::Value> = meta["resolve"]["nodes"]
        .as_array()
        .expect("resolve nodes")
        .iter()
        .map(|n| (n["id"].as_str().unwrap(), n))
        .collect();
    let is_workspace = |p: &serde_json::Value| p["source"].is_null();
    let lib_target = |p: &serde_json::Value| -> Option<PathBuf> {
        p["targets"].as_array()?.iter().find_map(|t| {
            let kinds: Vec<&str> =
                t["kind"].as_array()?.iter().filter_map(|k| k.as_str()).collect();
            let linked = kinds.iter().any(|k| matches!(*k, "lib" | "rlib" | "staticlib"));
            linked.then(|| PathBuf::from(t["src_path"].as_str().unwrap()))
        })
    };

    let mut stack: Vec<&str> = BASE_CLOSURE_ROOTS
        .iter()
        .map(|root| {
            packages
                .iter()
                .find(|p| is_workspace(p) && p["name"] == *root)
                .unwrap_or_else(|| panic!("workspace crate `{root}` not found"))["id"]
                .as_str()
                .unwrap()
        })
        .collect();
    let mut seen = std::collections::HashSet::new();
    let mut out = Vec::new();
    while let Some(id) = stack.pop() {
        if !seen.insert(id) {
            continue;
        }
        let pkg = by_id[id];
        if !is_workspace(pkg) {
            continue; // registry crates: not the framework's own code
        }
        let Some(lib_rs) = lib_target(pkg) else {
            continue; // proc-macro / binary-only: never in the staticlib
        };
        out.push((
            pkg["name"].as_str().unwrap().to_string(),
            PathBuf::from(pkg["manifest_path"].as_str().unwrap()),
            lib_rs,
        ));
        for dep in nodes[id]["deps"].as_array().into_iter().flatten() {
            let normal = dep["dep_kinds"]
                .as_array()
                .into_iter()
                .flatten()
                .any(|k| k["kind"].is_null());
            if normal {
                stack.push(dep["pkg"].as_str().unwrap());
            }
        }
    }
    out.sort();
    out
}

/// Every Apple framework the framework's OWN crates link on iOS must be in the
/// run tool's base set.
///
/// # The bug this prevents
///
/// `backend-apple-core` links CoreText (font registration) and CoreFoundation
/// (`CFRelease`, the pre-commit run-loop observer) through `#[link(kind =
/// "framework")]`, which the staticlib drops (see the module docs). The base
/// set was UIKit/Foundation/CoreGraphics/QuartzCore only, so those symbols
/// resolved solely because the Swift host's `import UIKit` autolinks UIKit's
/// transitive module imports — an accident of the host, not a guarantee.
///
/// A framework crate may instead declare the framework in its own
/// `[package.metadata.idealyst.ios].frameworks`, which the run tool also
/// collects across the closure; either satisfies the check.
#[test]
fn regression_framework_crate_links_are_in_ios_base_set() {
    let crates = base_closure_workspace_crates();
    let names: Vec<&str> = crates.iter().map(|(n, _, _)| n.as_str()).collect();
    for expected in ["backend-ios-mobile", "backend-ios-core", "backend-apple-core", "runtime-core"] {
        assert!(names.contains(&expected), "{expected} missing from base closure: {names:?}");
    }

    let base = base_frameworks();
    let mut problems = Vec::new();
    for (name, manifest_path, lib_rs) in &crates {
        let manifest: toml::Value =
            toml::from_str(&fs::read_to_string(manifest_path).unwrap()).unwrap();
        let declared = declared_ios_frameworks(&manifest);
        let missing: Vec<String> = ios_linked_frameworks(lib_rs)
            .into_iter()
            .filter(|fw| !base.contains(fw) && !declared.contains(fw))
            .collect();
        if !missing.is_empty() {
            problems.push(format!("{name}: links {missing:?} on iOS"));
        }
    }
    assert!(
        problems.is_empty(),
        "the iOS app links the framework's crates as a staticlib, so \
         `#[link(kind = \"framework\")]` is lost; add these to BASE in \
         crates/tools/run/ios/src/frameworks.rs:\n  {}",
        problems.join("\n  ")
    );
}

/// The scanner sees `backend-apple-core`'s C-API links on iOS, so the check
/// above can't pass vacuously.
#[test]
fn scanner_sees_apple_core_coretext_and_corefoundation_on_ios() {
    let lib_rs = repo_root().join("crates/backend/apple/core/src/lib.rs");
    let linked = ios_linked_frameworks(&lib_rs);
    for fw in ["CoreText", "CoreFoundation", "CoreGraphics"] {
        assert!(linked.contains(fw), "{fw} not seen; linked on iOS: {linked:?}");
    }
}
