//! Drive a real crate through the whole wasm hot-patch pipeline and
//! check the artifact a browser would be handed.
//!
//! Every other test in this area works on modules assembled by hand,
//! which is right for pinning one decision but cannot catch the thing
//! that actually breaks a hot patch: a mismatch between what rustc,
//! wasm-ld and wasm-bindgen really emit and what the passes assume. This
//! test compiles Rust, links a PIC patch against the linked base, and
//! asserts on the bytes.
//!
//! What it proves, in order:
//!
//! * the base keeps its `__*_hot_impl` reachable through
//!   `__indirect_function_table` after wasm-bindgen's dead-code pass —
//!   which it only does because `prepare_base_module` rooted it;
//! * a patch linked from the same crate's objects carries an element
//!   segment offset by the imported `__table_base`;
//! * every import that patch carries can be resolved against the base;
//! * the jump table pairs the two, with an absolute key and a relative
//!   value.
//!
//! # Why it is `#[ignore]`d
//!
//! It needs the `wasm32-unknown-unknown` target, a network-warm cargo
//! registry, and `wasm-bindgen` on `PATH` at a version matching the
//! `wasm-bindgen` crate it compiles. Run it deliberately:
//!
//! ```text
//! cargo test -p build-web --test wasm_patch_roundtrip -- --ignored --nocapture
//! ```
//!
//! A second test, [`a_library_crates_patch_carries_its_dependents`],
//! does the same for a TWO-crate workspace through the builder the dev
//! loop uses (`WasmPatchBuilder::build_crates`), replaying the captured
//! invocations of a library crate and the app that depends on it.

use std::path::{Path, PathBuf};
use std::process::Command;

use build_web::hotpatch_aliases;
use build_web::hotpatch_base::prepare_base_module;
use build_web::hotpatch_patch::{resolve_against_base, BaseIndex};
use build_web::hotpatch_wasm::build_jump_table;

/// The fixture crate. Small, but every shape the pipeline has to handle
/// is in here: a private `__*_hot_impl` reached through a `fn` pointer
/// (so it gets a table slot), a `static` (so the patch needs a data
/// reference), and a call into JS (so wasm-bindgen synthesizes an
/// intrinsic import the patch must be re-pointed at).
const LIB_RS: &str = r#"
use wasm_bindgen::prelude::*;

static GREETING: &str = "hello from the base";

#[wasm_bindgen]
extern "C" {
    #[wasm_bindgen(js_namespace = console)]
    fn log(s: &str);
}

#[doc(hidden)]
#[inline(never)]
fn __Probe_hot_impl(a: u32) -> u32 {
    log(GREETING);
    // Formatting BOTH spellings of one machine type is what puts an
    // aliased symbol in the base. `usize` is `u32` on wasm32, so the
    // precompiled `core` rlib defines `<usize as Display>::fmt` and
    // `<u32 as Display>::fmt` as one function with two symbols — and the
    // name section keeps only one of the names. A patch asking for the
    // other one is the failure `hotpatch_aliases` exists to fix.
    log(&format!("{} {}", a, a as usize));
    a * MULTIPLIER
}

const MULTIPLIER: u32 = 7;

#[wasm_bindgen]
pub fn start() -> u32 {
    let f: fn(u32) -> u32 = __Probe_hot_impl;
    f(6)
}
"#;

const CARGO_TOML: &str = r#"
[package]
name = "roundtrip_probe"
version = "0.0.0"
edition = "2021"

[lib]
crate-type = ["cdylib"]

[dependencies]
wasm-bindgen = "0.2.128"

[profile.dev]
opt-level = 0

[workspace]
"#;

#[test]
#[ignore = "compiles a crate for wasm32 and shells out to wasm-bindgen; run with --ignored"]
fn a_patch_built_from_a_real_crate_pairs_with_its_base() {
    let Some(tools) = Tools::find() else {
        panic!(
            "this test needs the wasm32-unknown-unknown target and `wasm-bindgen` on PATH; \
             install with `rustup target add wasm32-unknown-unknown` and \
             `cargo install wasm-bindgen-cli`"
        );
    };

    let dir = scratch_dir("wasm_patch_roundtrip");
    std::fs::create_dir_all(dir.join("src")).unwrap();
    std::fs::write(dir.join("Cargo.toml"), CARGO_TOML).unwrap();
    std::fs::write(dir.join("src/lib.rs"), LIB_RS).unwrap();

    // ── 1. The base build ────────────────────────────────────────────
    //
    // The link args the pipeline passes. Spelled out AND checked against
    // the shipped decision: spelling them out is what makes this test
    // readable, checking is what stops it quietly proving something about
    // a flag set the build no longer uses.
    const BASE_LINK_ARGS: [&str; 8] = [
        "--no-gc-sections",
        "--export-table",
        "--export-memory",
        "--growable-table",
        "--emit-relocs",
        "--export=__stack_pointer",
        "--export=__heap_base",
        "--export=__data_end",
    ];
    let shipped = build_web::hot_patch_link_args();
    for flag in BASE_LINK_ARGS {
        assert!(
            shipped.iter().any(|a| a == flag),
            "the build no longer passes {flag}: {shipped:?}"
        );
    }
    assert_eq!(
        shipped.len(),
        BASE_LINK_ARGS.len(),
        "the build grew a link arg this test does not exercise: {shipped:?}"
    );

    let link_args = BASE_LINK_ARGS.map(|a| format!("-Clink-arg={a}")).join(" ");

    run(
        Command::new("cargo")
            .current_dir(&dir)
            .args(["build", "--target", "wasm32-unknown-unknown"])
            .env("RUSTFLAGS", &link_args),
        "the base build",
    );
    let linked = dir.join("target/wasm32-unknown-unknown/debug/roundtrip_probe.wasm");

    // ── 2. Root every function, then let wasm-bindgen run ────────────
    let (prepared, _census) = prepare_base_module(&std::fs::read(&linked).unwrap()).unwrap();
    let prepared_path = dir.join("base.prepared.wasm");
    std::fs::write(&prepared_path, &prepared).unwrap();

    run(
        Command::new(&tools.wasm_bindgen)
            .args(["--target", "web"])
            .args(["--keep-lld-exports", "--keep-debug", "--no-demangle"])
            .args(["--out-name", "probe"])
            .arg("--out-dir")
            .arg(dir.join("pkg"))
            .arg(&prepared_path),
        "wasm-bindgen over the prepared base",
    );
    let served = std::fs::read(dir.join("pkg/probe_bg.wasm")).unwrap();

    // The claim the whole base-prep step exists for: wasm-bindgen's
    // dead-code pass keeps a private function only because the element
    // segment roots it. Without `prepare_base_module` this is gone, and
    // with it there is a table slot to redirect.
    // The alias map has to be read from the LINKED module: walrus and
    // wasm-bindgen both renumber functions, and the `linking` section
    // travels with the stale indices.
    let aliases = hotpatch_aliases::read_from_linked(&std::fs::read(&linked).unwrap()).unwrap();
    assert!(
        !aliases.is_empty(),
        "no symbol aliases read from the linked base — either `--emit-relocs` is gone from \
         the link args or the `linking` section stopped carrying a symbol table"
    );
    let display_alias = aliases
        .iter()
        // v0 mangling: `j` is usize, `m` is u32. Whichever spelling the
        // name section kept, the other has to be its alias.
        .find(|(alias, canonical)| {
            let pair = [alias.as_str(), canonical.as_str()];
            pair.iter().any(|n| n.contains("3impjNtB9_7Display3fmt"))
                && pair.iter().any(|n| n.contains("3impmNtB9_7Display3fmt"))
        })
        .map(|(a, c)| (a.clone(), c.clone()))
        .expect(
            "core's `<usize as Display>::fmt` / `<u32 as Display>::fmt` are one function with \
             two symbols on wasm32; if that pair is gone the fixture stopped formatting both",
        );

    let base = BaseIndex::of(&served, &aliases).expect("indexing the served base");
    let hot_impl = base
        .ifunc
        .keys()
        .find(|k| k.contains("Probe_hot_impl"))
        .unwrap_or_else(|| {
            panic!(
                "the base kept no table slot for __Probe_hot_impl — \
                 wasm-bindgen's gc took it, so nothing can be patched. \
                 {} symbols in the table.",
                base.ifunc.len()
            )
        })
        .clone();
    assert!(
        base.ifunc.len() > 100,
        "only {} functions reachable through the table — prepare_base_module did not run",
        base.ifunc.len()
    );

    // ── 3. The edit, and the patch built from it ─────────────────────
    std::fs::write(
        dir.join("src/lib.rs"),
        LIB_RS.replace("const MULTIPLIER: u32 = 7;", "const MULTIPLIER: u32 = 9;"),
    )
    .unwrap();

    run(
        Command::new("cargo")
            .current_dir(&dir)
            .args(["rustc", "--target", "wasm32-unknown-unknown", "--"])
            .args(["--emit=obj", "-Crelocation-model=pic"]),
        "the patch codegen",
    );
    let object = dir.join("target/wasm32-unknown-unknown/debug/deps/roundtrip_probe.o");
    assert!(
        object.is_file(),
        "rustc emitted no merged object at {}",
        object.display()
    );

    let patch_path = dir.join("patch.linked.wasm");
    run(
        Command::new(&tools.wasm_ld)
            .args(build_web::hotpatch_build::PATCH_LINK_ARGS)
            .arg("-o")
            .arg(&patch_path)
            .arg(&object),
        "the patch link",
    );
    // The same link WITH DWARF, to show `--strip-debug` changes nothing
    // the pairing sees.
    let with_dwarf_path = dir.join("patch.with-dwarf.wasm");
    run(
        Command::new(&tools.wasm_ld)
            .args(
                build_web::hotpatch_build::PATCH_LINK_ARGS
                    .iter()
                    .filter(|a| **a != "--strip-debug"),
            )
            .arg("-o")
            .arg(&with_dwarf_path)
            .arg(&object),
        "the patch link, with DWARF",
    );

    // ── 4. Resolve and pair ──────────────────────────────────────────
    let raw = std::fs::read(&patch_path).unwrap();
    let resolved = resolve_against_base(&raw, &base).unwrap_or_else(|e| {
        panic!(
            "the patch's imports could not be resolved against the base, so a save would fall \
             back to a rebuild:\n{e:#}"
        )
    });

    // The regression, stated as a difference: index the same base
    // WITHOUT the alias map and the very same patch is refused, naming
    // the symbol the name section does not know it by. This is the state
    // the tier shipped in before `hotpatch_aliases` — every ordinary body
    // edit fell back to a rebuild over `<usize as Display>::fmt`.
    let (alias_name, canonical_name) = &display_alias;
    let blind = BaseIndex::of(&served, &Default::default()).unwrap();
    assert!(
        blind.ifunc.contains_key(canonical_name),
        "the canonical spelling must be in the table either way: {canonical_name}"
    );
    assert!(
        !blind.ifunc.contains_key(alias_name),
        "the alias is supposed to be invisible without the map: {alias_name}"
    );
    assert_eq!(
        base.ifunc.get(alias_name),
        base.ifunc.get(canonical_name),
        "with the map, both spellings have to resolve to the one slot they share"
    );
    let refused = resolve_against_base(&raw, &blind);
    assert!(
        refused.is_err(),
        "without the alias map this patch should have been refused — if it resolves, the \
         fixture no longer references an aliased symbol and this test proves nothing"
    );

    let table = build_jump_table(&served, &resolved, &aliases).unwrap();
    let with_dwarf = resolve_against_base(&std::fs::read(&with_dwarf_path).unwrap(), &base).unwrap();
    assert_eq!(
        build_jump_table(&served, &with_dwarf, &aliases).unwrap(),
        table,
        "linking the patch --strip-debug changed the jump table"
    );
    assert!(
        std::fs::metadata(&patch_path).unwrap().len() < std::fs::metadata(&with_dwarf_path).unwrap().len(),
        "--strip-debug removed nothing from the linked patch"
    );
    assert!(
        !table.is_empty(),
        "the jump table redirects nothing — the patch and the base did not pair"
    );
    assert!(
        table.ifunc_count >= 1,
        "the patch claims no table slots, so apply_patch would grow by zero"
    );

    // The key is the base's ABSOLUTE slot; the value is relative to the
    // patch's own segment, which `apply_patch` rebases by the grow's
    // return. A value at or above the base's table size would mean the
    // relative/absolute asymmetry got inverted somewhere.
    let base_slot = base.ifunc[&hot_impl];
    let paired = table
        .map
        .get(&(base_slot as u64))
        .unwrap_or_else(|| panic!("no entry for the base's slot {base_slot}: {:?}", table.map));
    assert!(
        *paired < table.ifunc_count as u64,
        "the patch-side value {paired} is not an index into the patch's own {} slots — \
         it looks like an absolute index, which apply_patch would rebase a second time",
        table.ifunc_count,
    );

    // The served patch is stripped of its `name` and DWARF sections
    // AFTER pairing. The table paired above must still describe it: same
    // element segment in the same order, same slot count, same imports,
    // and a module that validates.
    let stripped = build_web::hotpatch_patch::strip_debug_sections(&resolved).unwrap();
    assert!(
        stripped.len() < resolved.len(),
        "stripping removed nothing ({} bytes)",
        resolved.len()
    );
    wasmparser::Validator::new()
        .validate_all(&stripped)
        .expect("the stripped patch must still validate");
    let segment = |wasm: &[u8]| -> Vec<u32> {
        let m = walrus::Module::from_buffer(wasm).unwrap();
        m.elements
            .iter()
            .flat_map(|e| match &e.items {
                walrus::ElementItems::Functions(ids) => ids.iter().map(|id| id.index() as u32).collect(),
                _ => Vec::new(),
            })
            .collect()
    };
    assert_eq!(segment(&resolved), segment(&stripped), "the element segment moved");
    assert_eq!(
        build_web::hotpatch_wasm::table_slot_count(&stripped).unwrap(),
        table.ifunc_count,
        "the stripped patch claims a different number of table slots"
    );

    // Finally, the artifact has to be something the runtime can actually
    // instantiate: one import namespace, `env`, and nothing else.
    let module = walrus::Module::from_buffer(&resolved).unwrap();
    let foreign: Vec<String> = module
        .imports
        .iter()
        .filter(|i| i.module != "env")
        .map(|i| format!("{}.{}", i.module, i.name))
        .collect();
    assert!(
        foreign.is_empty(),
        "subsecond::apply_patch provides only an `env` namespace, so instantiation would \
         throw on: {foreign:?}"
    );
}

// ── Two crates: a library of the workspace and the app using it ──────

const WS_CARGO_TOML: &str = r#"
[workspace]
members = ["shared", "app"]
resolver = "2"

[profile.dev]
opt-level = 0
"#;

const SHARED_CARGO_TOML: &str = r#"
[package]
name = "rt_shared"
version = "0.0.0"
edition = "2021"
"#;

/// The library. `shared_value` is a plain function the APP calls
/// directly — the call a patch of this crate alone could never reach —
/// and `__SharedCard_hot_impl` is reached through a `fn` pointer, the way
/// a `#[component]` body is.
const SHARED_RS: &str = r#"
pub fn shared_value(n: u32) -> u32 {
    n + 11
}

#[doc(hidden)]
#[inline(never)]
fn __SharedCard_hot_impl(n: u32) -> u32 {
    shared_value(n) * 2
}

pub fn shared_card(n: u32) -> u32 {
    let f: fn(u32) -> u32 = __SharedCard_hot_impl;
    f(n)
}
"#;

const APP_CARGO_TOML: &str = r#"
[package]
name = "rt_app"
version = "0.0.0"
edition = "2021"

[lib]
crate-type = ["cdylib"]

[dependencies]
wasm-bindgen = "0.2.128"
rt_shared = { path = "../shared" }
"#;

const APP_RS: &str = r#"
use wasm_bindgen::prelude::*;

#[doc(hidden)]
#[inline(never)]
fn __Root_hot_impl(n: u32) -> u32 {
    rt_shared::shared_value(n) + rt_shared::shared_card(n)
}

#[wasm_bindgen]
pub fn start() -> u32 {
    let f: fn(u32) -> u32 = __Root_hot_impl;
    f(1)
}
"#;

/// A `RUSTC_WRAPPER` that records each invocation — argv NUL-separated,
/// cwd, env NUL-separated — into its own directory under `$CAPTURE_RAW`,
/// then runs rustc. The `idealyst` binary does this for real; the test
/// cannot depend on the CLI, so it records the same three things and
/// writes them in the replay's format itself.
const WRAPPER_SH: &str = r#"#!/bin/sh
d="$CAPTURE_RAW/$$"
mkdir -p "$d"
printf '%s\0' "$@" > "$d/args"
pwd > "$d/cwd"
env -0 > "$d/env"
exec "$@"
"#;

#[test]
#[ignore = "compiles a two-crate workspace for wasm32 and shells out to wasm-bindgen; run with --ignored"]
fn a_library_crates_patch_carries_its_dependents() {
    use build_runtime_server::hotpatch::replay::CapturedInvocation;
    use build_web::hotpatch_build::{PatchCrate, WasmPatchBuilder};
    use std::os::unix::fs::PermissionsExt;

    let Some(tools) = Tools::find() else {
        panic!("this test needs the wasm32-unknown-unknown target and `wasm-bindgen` on PATH");
    };

    let dir = scratch_dir("wasm_patch_roundtrip_ws");
    for sub in ["shared/src", "app/src"] {
        std::fs::create_dir_all(dir.join(sub)).unwrap();
    }
    std::fs::write(dir.join("Cargo.toml"), WS_CARGO_TOML).unwrap();
    std::fs::write(dir.join("shared/Cargo.toml"), SHARED_CARGO_TOML).unwrap();
    std::fs::write(dir.join("shared/src/lib.rs"), SHARED_RS).unwrap();
    std::fs::write(dir.join("app/Cargo.toml"), APP_CARGO_TOML).unwrap();
    std::fs::write(dir.join("app/src/lib.rs"), APP_RS).unwrap();
    let wrapper = dir.join("capture.sh");
    std::fs::write(&wrapper, WRAPPER_SH).unwrap();
    std::fs::set_permissions(&wrapper, std::fs::Permissions::from_mode(0o755)).unwrap();
    let raw = dir.join("raw-captures");
    let _ = std::fs::remove_dir_all(&raw);
    std::fs::create_dir_all(&raw).unwrap();

    // ── 1. The base build, every rustc invocation recorded ───────────
    //
    // `touch` both sources so cargo compiles both crates (and the wrapper
    // records both) even when the scratch dir is warm from a last run.
    for f in ["shared/src/lib.rs", "app/src/lib.rs"] {
        let p = dir.join(f);
        let text = std::fs::read(&p).unwrap();
        std::fs::write(&p, text).unwrap();
    }
    let link_args = build_web::hot_patch_link_args()
        .iter()
        .map(|a| format!("-Clink-arg={a}"))
        .collect::<Vec<_>>()
        .join(" ");
    run(
        Command::new("cargo")
            .current_dir(&dir)
            .args(["build", "-p", "rt_app", "--target", "wasm32-unknown-unknown"])
            .env("RUSTFLAGS", &link_args)
            .env("RUSTC_WRAPPER", &wrapper)
            .env("CAPTURE_RAW", &raw),
        "the two-crate base build",
    );
    let captures = dir.join("idealyst-hotpatch/captures");
    let _ = std::fs::remove_dir_all(&captures);
    std::fs::create_dir_all(&captures).unwrap();
    for entry in std::fs::read_dir(&raw).unwrap().flatten() {
        let split0 = |p: PathBuf| -> Vec<String> {
            std::fs::read(p)
                .unwrap()
                .split(|b| *b == 0)
                .filter(|s| !s.is_empty())
                .map(|s| String::from_utf8_lossy(s).into_owned())
                .collect()
        };
        let argv = split0(entry.path().join("args"));
        let Some(at) = argv.iter().position(|a| a == "--crate-name") else { continue };
        let name = argv[at + 1].clone();
        if !["rt_shared", "rt_app"].contains(&name.as_str())
            || !argv.iter().any(|a| a.starts_with("--emit=") && a.contains("link"))
        {
            continue;
        }
        let kind = argv
            .iter()
            .position(|a| a == "--crate-type")
            .map(|i| argv[i + 1].clone())
            .unwrap_or_else(|| "lib".into());
        let capture = CapturedInvocation {
            rustc: argv[0].clone(),
            args: argv[1..].to_vec(),
            cwd: std::fs::read_to_string(entry.path().join("cwd")).unwrap().trim().to_string(),
            env: split0(entry.path().join("env"))
                .into_iter()
                .filter_map(|kv| kv.split_once('=').map(|(k, v)| (k.to_string(), v.to_string())))
                .collect(),
        };
        std::fs::write(
            captures.join(format!("{name}.{kind}.json")),
            serde_json::to_string(&capture).unwrap(),
        )
        .unwrap();
    }
    for want in ["rt_shared.lib.json", "rt_app.cdylib.json"] {
        assert!(captures.join(want).is_file(), "no capture {want} recorded by the base build");
    }

    // ── 2. Prepare, bindgen, index ───────────────────────────────────
    let linked = dir.join("target/wasm32-unknown-unknown/debug/rt_app.wasm");
    let linked_bytes = std::fs::read(&linked).unwrap();
    let aliases = hotpatch_aliases::read_from_linked(&linked_bytes).unwrap();
    let alias_path = dir.join("rt_app.aliases.tsv");
    hotpatch_aliases::write(&alias_path, &aliases).unwrap();
    let (prepared, _) = prepare_base_module(&linked_bytes).unwrap();
    let prepared_path = dir.join("base.prepared.wasm");
    std::fs::write(&prepared_path, &prepared).unwrap();
    run(
        Command::new(&tools.wasm_bindgen)
            .args(["--target", "web", "--keep-lld-exports", "--keep-debug", "--no-demangle"])
            .args(["--out-name", "app"])
            .arg("--out-dir")
            .arg(dir.join("pkg"))
            .arg(&prepared_path),
        "wasm-bindgen over the prepared two-crate base",
    );
    let served_path = dir.join("pkg/app_bg.wasm");
    let served = std::fs::read(&served_path).unwrap();
    let base = BaseIndex::of(&served, &aliases).unwrap();
    let slot = |needle: &str| -> u64 {
        *base
            .ifunc
            .iter()
            .find(|(k, _)| k.contains(needle))
            .unwrap_or_else(|| panic!("the base has no table slot for {needle}"))
            .1 as u64
    };
    let root_slot = slot("__Root_hot_impl");
    let card_slot = slot("__SharedCard_hot_impl");

    // ── 3. A body edit in the LIBRARY ────────────────────────────────
    std::fs::write(dir.join("shared/src/lib.rs"), SHARED_RS.replace("n + 11", "n + 13")).unwrap();
    let builder = WasmPatchBuilder::new(
        &served_path,
        Some(&alias_path),
        &captures,
        "rt_app",
        dir.join("patches"),
    )
    .unwrap();

    // The library alone: its own component slot pairs, but nothing the
    // app defines is in the patch — so the app's base code, which calls
    // `shared_value` DIRECTLY, would keep calling the old body.
    let alone = builder.build_crates(&[PatchCrate::new("rt_shared", None)]).unwrap();
    assert!(alone.jump_table.map.contains_key(&card_slot), "{:?}", alone.jump_table.map);
    assert!(
        !alone.jump_table.map.contains_key(&root_slot),
        "a patch of the library alone cannot redirect the app's code"
    );

    // The library and its dependent, one module: both crates' slots pair,
    // and the app's copy of its caller is in the patch too.
    let both = builder
        .build_crates(&[PatchCrate::new("rt_shared", None), PatchCrate::new("rt_app", None)])
        .unwrap_or_else(|e| panic!("the two-crate patch did not build:\n{e:#}"));
    assert!(both.jump_table.map.contains_key(&card_slot), "{:?}", both.jump_table.map);
    assert!(both.jump_table.map.contains_key(&root_slot), "{:?}", both.jump_table.map);
    assert_eq!(
        both.crates.iter().map(|(n, _)| n.as_str()).collect::<Vec<_>>(),
        vec!["rt_shared", "rt_app"]
    );

    // The patched app's call to `shared_value` resolves INSIDE the patch,
    // to the new body, rather than being imported from the base.
    let named = std::fs::read(captures.parent().unwrap().join("last-patch.named.wasm")).unwrap();
    let module = walrus::Module::from_buffer(&named).unwrap();
    let defines = |needle: &str| {
        module.funcs.iter().any(|f| {
            matches!(f.kind, walrus::FunctionKind::Local(_))
                && f.name.as_deref().is_some_and(|n| n.contains(needle))
        })
    };
    assert!(defines("12shared_value"), "the patch does not define the library's new shared_value");
    assert!(defines("__Root_hot_impl"), "the patch does not carry the app's caller");
    assert!(
        !module.imports.iter().any(|i| i.name.contains("12shared_value")),
        "the patch imports shared_value from the base, i.e. the OLD body"
    );
}

struct Tools {
    wasm_bindgen: PathBuf,
    wasm_ld: PathBuf,
}

impl Tools {
    fn find() -> Option<Self> {
        let sysroot = Command::new("rustc")
            .args(["--print", "sysroot"])
            .output()
            .ok()?;
        let sysroot = PathBuf::from(String::from_utf8(sysroot.stdout).ok()?.trim());
        if !sysroot.join("lib/rustlib/wasm32-unknown-unknown").is_dir() {
            return None;
        }
        let host = String::from_utf8(Command::new("rustc").arg("-vV").output().ok()?.stdout)
            .ok()?
            .lines()
            .find_map(|l| l.strip_prefix("host: ").map(str::to_string))?;
        let wasm_ld = sysroot
            .join("lib/rustlib")
            .join(host)
            .join("bin/gcc-ld/wasm-ld");
        if !wasm_ld.is_file() {
            return None;
        }
        let which = Command::new("which").arg("wasm-bindgen").output().ok()?;
        if !which.status.success() {
            return None;
        }
        Some(Self {
            wasm_bindgen: PathBuf::from(String::from_utf8(which.stdout).ok()?.trim()),
            wasm_ld,
        })
    }
}

fn run(cmd: &mut Command, what: &str) {
    let output = cmd.output().unwrap_or_else(|e| panic!("spawn {what}: {e}"));
    if !output.status.success() {
        panic!(
            "{what} failed ({})\n--- stderr ---\n{}\n--- stdout ---\n{}",
            output.status,
            String::from_utf8_lossy(&output.stderr),
            String::from_utf8_lossy(&output.stdout),
        );
    }
}

/// A stable directory rather than a fresh temp one, so the cargo build
/// inside it is incremental across runs. A first run is a minute; later
/// ones are seconds.
fn scratch_dir(name: &str) -> PathBuf {
    let dir = Path::new(env!("CARGO_TARGET_TMPDIR")).join(name);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}
