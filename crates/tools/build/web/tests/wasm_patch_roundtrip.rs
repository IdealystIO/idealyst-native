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
            .args([
                "--no-entry",
                "--allow-undefined",
                "--import-memory",
                "--import-table",
                "--growable-table",
                "--pie",
                "--experimental-pic",
                "--no-demangle",
                "--no-gc-sections",
            ])
            .arg("-o")
            .arg(&patch_path)
            .arg(&object),
        "the patch link",
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
