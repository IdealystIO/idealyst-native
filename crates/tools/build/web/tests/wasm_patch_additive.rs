//! An ADDITIVE hot patch — a save that adds items to a LIBRARY crate —
//! through the real pipeline: two crates compiled for wasm32, the patch
//! verified against the base module, linked, and run in Node with its
//! jump table applied the way the page applies it.
//!
//! The library `probe_lib` is what the saves edit; the app `probe`
//! depends on it and is carried by every patch, as a dependent is in the
//! dev loop. That is the layout where a renamed symbol does damage: the
//! app's base code calls the library by symbol name, and nothing
//! redirects a call to a name the patch no longer defines.
//!
//! What each test proves:
//!
//! * [`an_added_helper_called_from_an_existing_body_runs`]: a new fn and
//!   the edited body that calls it patch, and the page computes the new
//!   value — the everyday additive save;
//! * [`a_fn_inserted_between_existing_items_patches`]: placement alone
//!   does not refuse a patch; only a renamed symbol does;
//! * [`regression_an_impl_inserted_ahead_of_another_is_refused`]: a new
//!   `impl` ahead of an existing one renumbers it (`{impl#0}` →
//!   `{impl#1}`). Unverified, that patch builds and the app's base code
//!   keeps calling the OLD body of the method the same save edited — the
//!   test shows the stale value, then that verification refuses it;
//! * [`regression_an_added_fn_with_a_static_is_refused`]: new writable
//!   state would get a fresh copy in every later patch;
//! * [`an_appended_unused_pub_fn_patches_and_changes_nothing`]: the
//!   study's "shape" edit — a `pub fn` appended and called by nothing —
//!   patches, and the program behaves as before;
//! * [`an_unused_new_module_patches_with_nothing_to_redirect`]: new code
//!   that changes nothing the program runs is a patch with an empty jump
//!   table, not a failed pairing.
//!
//! `#[ignore]`d like its siblings: it needs the `wasm32-unknown-unknown`
//! target and `node` on `PATH`.
//!
//! ```text
//! cargo test -p build-web --test wasm_patch_additive -- --ignored --nocapture
//! ```

use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::Arc;

use build_runtime_server::hotpatch::replay::CapturedInvocation;
use build_web::hotpatch_aliases;
use build_web::hotpatch_base::{prepare_base_module, Flavor};
use build_web::hotpatch_build::{BuiltPatch, PatchCrate, SourceProbe, WasmPatchBuilder};

/// The library the saves edit.
const LIB_RS: &str = r#"
pub struct Row {
    pub a: u32,
}

impl Row {
    #[inline(never)]
    pub fn total(&self) -> u32 {
        self.a + 1
    }
}

#[inline(never)]
pub fn lib_value() -> u64 {
    // BODY
    2
}
"#;

const BODY_MARK: &str = "    // BODY\n    2";

/// The app. Each `__*_hot_impl` is reached through a `fn` pointer, as
/// `#[component]`'s split reaches its inner half, so it has a table slot
/// in the base and in a patch; each calls the library DIRECTLY.
const APP_RS: &str = r#"
#![allow(non_snake_case)]

#[doc(hidden)]
#[inline(never)]
fn __Probe_hot_impl() -> u64 {
    probe_lib::lib_value()
}

#[doc(hidden)]
#[inline(never)]
fn __Other_hot_impl() -> u64 {
    std::hint::black_box(probe_lib::Row { a: 1 }).total() as u64 * 10
}

#[no_mangle]
pub extern "C" fn probe() -> u64 {
    std::hint::black_box(__Probe_hot_impl as fn() -> u64)()
}

#[no_mangle]
pub extern "C" fn other() -> u64 {
    std::hint::black_box(__Other_hot_impl as fn() -> u64)()
}
"#;

/// The page's loader, as `wasm_patch_statics` runs it: memory and table
/// reserved, every import supplied as the plan says, relocation thunks
/// run, the jump table committed. Then the BASE's exports are called, so
/// what runs is what the page would run after the patch.
const LOADER_JS: &str = r#"
const fs = require('fs');
const [basePath, patchPath, jumps, ifuncCount] = process.argv.slice(2);
(async () => {
  const base = (await WebAssembly.instantiate(fs.readFileSync(basePath), {})).instance.exports;
  console.log('base-probe=' + base.probe());
  console.log('base-other=' + base.other());
  const module = new WebAssembly.Module(fs.readFileSync(patchPath));
  const plan = new DataView(WebAssembly.Module.customSections(module, 'idealyst.hotpatch')[0]);
  let at = 0;
  const u32 = () => { const v = plan.getUint32(at, true); at += 4; return v; };
  if (u32() !== 3) throw new Error('plan version');
  const hasSize = u32(), size = u32(); u32();
  const count = u32();
  const PAGE = 65536;
  const data = hasSize ? size : fs.statSync(patchPath).size;
  const prior = base.memory.grow(Math.ceil(data / PAGE) + 1);
  const memoryBase = (prior + 1) * PAGE;
  const tableBase = base.__indirect_function_table.grow(Number(ifuncCount));
  const g = (v, m) => new WebAssembly.Global({ value: 'i32', mutable: m }, v | 0);
  const env = Object.assign({}, base, { __memory_base: g(memoryBase, false), __table_base: g(tableBase, false) });
  const imports = { env };
  const descriptors = WebAssembly.Module.imports(module);
  if (descriptors.length !== count) throw new Error('plan/import count');
  for (const d of descriptors) {
    const tag = plan.getUint8(at); at += 1;
    let value;
    switch (tag) {
      case 0: if (!(d.name in env)) throw new Error('missing env.' + d.name); continue;
      case 1: value = base.__indirect_function_table.get(u32()); break;
      case 2: value = g(u32(), true); break;
      case 3: value = g(u32(), false); break;
      case 4: { const n = u32(); at += n; value = () => { throw new Error('trap ' + d.name); }; break; }
      case 6: value = g(u32() - memoryBase, false); break;
      default: throw new Error('plan tag ' + tag + ' for ' + d.module + '.' + d.name);
    }
    (imports[d.module] ||= {})[d.name] = value;
  }
  const patch = (await WebAssembly.instantiate(module, imports)).exports;
  for (const thunk of ['__wasm_apply_data_relocs', '__wasm_apply_global_relocs', '__wasm_call_ctors']) {
    if (typeof patch[thunk] === 'function') patch[thunk]();
  }
  // What `dev_hot`'s commit does on the page: every redirected base slot
  // now holds the patch's function.
  const table = base.__indirect_function_table;
  for (const pair of jumps.split(',').filter(Boolean)) {
    const [slot, rel] = pair.split(':').map(Number);
    table.set(slot, table.get(tableBase + rel));
  }
  console.log('probe=' + base.probe());
  console.log('other=' + base.other());
})().catch(e => { console.log('error=' + e.message); process.exit(1); });
"#;

/// A built base, ready for patches.
struct Base {
    dir: PathBuf,
    served: PathBuf,
    aliases: PathBuf,
    data: PathBuf,
    captures: PathBuf,
}

fn rustc() -> String {
    let out = Command::new("rustc").args(["--print", "sysroot"]).output().expect("run rustc");
    let sysroot = String::from_utf8(out.stdout).unwrap();
    Path::new(sysroot.trim()).join("bin/rustc").display().to_string()
}

impl Base {
    /// Compile the library and the app the way a hot-patch base build
    /// does, record both invocations, and prepare the linked app the way
    /// an own-mode dev build does.
    fn build(name: &str) -> Base {
        let dir = std::env::temp_dir().join(format!("idealyst-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        for sub in ["lib/src", "app/src", "deps", "captures"] {
            std::fs::create_dir_all(dir.join(sub)).unwrap();
        }
        std::fs::write(dir.join("lib/src/lib.rs"), LIB_RS).unwrap();
        std::fs::write(dir.join("app/src/lib.rs"), APP_RS).unwrap();
        let deps = dir.join("deps").display().to_string();
        let common = |crate_name: &str, src: &str, kind: &str, metadata: &str| -> Vec<String> {
            let mut a: Vec<String> = [
                "--crate-name", crate_name, "--edition=2021", src, "--crate-type", kind, "--target",
                "wasm32-unknown-unknown", "-C", "opt-level=0", "-C", "debuginfo=0", "-C", metadata,
                "--out-dir", &deps, "-L", &format!("dependency={deps}"),
            ]
            .iter()
            .map(|s| s.to_string())
            .collect();
            a.push(if kind == "rlib" { "--emit=dep-info,metadata,link" } else { "--emit=link" }.into());
            a
        };
        let lib_args = common("probe_lib", "lib/src/lib.rs", "rlib", "metadata=additivelib");
        let mut app_args = common("probe", "app/src/lib.rs", "cdylib", "metadata=additiveapp");
        app_args.push("--extern".into());
        app_args.push(format!("probe_lib={deps}/libprobe_lib.rlib"));
        for a in build_web::hot_patch_link_args() {
            app_args.push(format!("-Clink-arg={a}"));
        }
        let rustc = rustc();
        for (args, file) in [(&lib_args, "probe_lib.rlib.json"), (&app_args, "probe.cdylib.json")] {
            let status = Command::new(&rustc).args(args).current_dir(&dir).status().expect("run rustc");
            assert!(status.success(), "the base build failed (is the wasm32-unknown-unknown target installed?)");
            let capture =
                CapturedInvocation { rustc: rustc.clone(), args: args.clone(), cwd: dir.display().to_string(), env: Vec::new() };
            std::fs::write(dir.join("captures").join(file), serde_json::to_vec(&capture).unwrap()).unwrap();
        }

        let linked = std::fs::read(dir.join("deps/probe.wasm")).unwrap();
        let aliases = dir.join("probe.aliases.tsv");
        hotpatch_aliases::write(&aliases, &hotpatch_aliases::read_from_linked(&linked).unwrap()).unwrap();
        let data = dir.join("probe.datasyms.tsv");
        hotpatch_aliases::write_data_symbols(&data, &hotpatch_aliases::read_data_symbols_from_linked(&linked).unwrap())
            .unwrap();
        let (prepared, _) = prepare_base_module(&linked, Flavor::Own).unwrap();
        let served = dir.join("probe.served.wasm");
        std::fs::write(&served, &prepared).unwrap();
        Base { captures: dir.join("captures"), dir, served, aliases, data }
    }

    /// A builder for this base whose background seed replayed the BASE
    /// sources of both crates — the state a dev session reaches a few
    /// seconds after a rebuild — so patches are trimmed to what differs.
    fn seeded_builder(&self) -> WasmPatchBuilder {
        let file = self.dir.join("lib/src/lib.rs");
        let probe: SourceProbe = Arc::new(move |name: &str| {
            let base = name != "probe_lib" || std::fs::read_to_string(&file).unwrap() == LIB_RS;
            Some(if base { "base" } else { "edited" }.to_string())
        });
        let builder = WasmPatchBuilder::new(
            &self.served,
            Some(&self.aliases),
            Some(&self.data),
            &self.captures,
            "probe",
            self.dir.join("patches"),
        )
        .unwrap()
        .with_source_probe(probe);
        builder.seed_all(vec![
            PatchCrate::carried("probe_lib", Some("base".into())),
            PatchCrate::carried("probe", Some("base".into())),
        ]);
        builder.wait_for_seeds();
        builder
    }

    /// Save `source` as the library, then build the patch the dev loop
    /// builds for it: the library edited, the app carried.
    fn patch(&self, builder: &WasmPatchBuilder, source: &str, additive: bool) -> anyhow::Result<BuiltPatch> {
        std::fs::write(self.dir.join("lib/src/lib.rs"), source).unwrap();
        builder.build_crates(&[
            PatchCrate::new("probe_lib", Some("edited".into())).with_additions(additive),
            PatchCrate::carried("probe", Some("base".into())),
        ])
    }

    /// Run the base, apply `patch` as the page does, call the base's
    /// exports.
    fn run(&self, patch: &BuiltPatch) -> String {
        let jumps: Vec<String> = patch.jump_table.map.iter().map(|(k, v)| format!("{k}:{v}")).collect();
        let loader = self.dir.join("load.cjs");
        std::fs::write(&loader, LOADER_JS).unwrap();
        let out = Command::new("node")
            .arg(&loader)
            .arg(&self.served)
            .arg(&patch.path)
            .arg(jumps.join(","))
            .arg(patch.jump_table.ifunc_count.to_string())
            .output()
            .expect("run node");
        format!("{}{}", String::from_utf8_lossy(&out.stdout), String::from_utf8_lossy(&out.stderr))
    }
}

impl Drop for Base {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}

fn verified(patch: &BuiltPatch) -> bool {
    patch.timings.iter().any(|(s, _)| *s == "verify-additive")
}

#[test]
#[ignore = "compiles two crates for wasm32 and runs them in node; run with --ignored"]
fn an_added_helper_called_from_an_existing_body_runs() {
    let base = Base::build("additive-helper");
    let builder = base.seeded_builder();
    let source = LIB_RS.replace(BODY_MARK, "    fresh_helper(6)")
        + "\n#[inline(never)]\nfn fresh_helper(n: u64) -> u64 {\n    n * 7 + 3\n}\n";
    let patch =
        base.patch(&builder, &source, true).unwrap_or_else(|e| panic!("the additive patch did not build:\n{e:#}"));
    assert!(verified(&patch), "the additive patch was not verified: {:?}", patch.timings);
    let out = base.run(&patch);
    assert!(out.contains("base-probe=2\n") && out.contains("base-other=20\n"), "{out}");
    assert!(out.contains("probe=45\n"), "the patched body did not call the new helper:\n{out}");
    assert!(out.contains("other=20\n"), "{out}");
}

#[test]
#[ignore = "compiles two crates for wasm32 and runs them in node; run with --ignored"]
fn a_fn_inserted_between_existing_items_patches() {
    let base = Base::build("additive-between");
    let builder = base.seeded_builder();
    let source = LIB_RS
        .replace(BODY_MARK, "    between(5)")
        .replace("impl Row {", "#[inline(never)]\nfn between(n: u64) -> u64 {\n    n + 100\n}\n\nimpl Row {");
    let patch = base.patch(&builder, &source, true).unwrap_or_else(|e| panic!("the patch did not build:\n{e:#}"));
    let out = base.run(&patch);
    assert!(out.contains("probe=105\n"), "{out}");
}

#[test]
#[ignore = "compiles two crates for wasm32 and runs them in node; run with --ignored"]
fn regression_an_impl_inserted_ahead_of_another_is_refused() {
    let base = Base::build("additive-impl-ahead");
    let builder = base.seeded_builder();
    // A new type with an impl ahead of `impl Row`, and an edit to
    // `Row::total` in the same save.
    let source = LIB_RS
        .replace(
            "impl Row {",
            "pub struct Fresh;\n\nimpl Fresh {\n    #[inline(never)]\n    pub fn fresh(&self) -> u32 {\n        9\n    }\n}\n\nimpl Row {",
        )
        .replace("self.a + 1", "self.a + 50");

    // Unverified, the patch builds — and is wrong. `total` compiled to a
    // new name, so the app's base code, which calls it directly by the
    // OLD name, is not redirected and keeps the old body: 20, where the
    // source now says 510.
    let unverified = base.patch(&builder, &source, false).unwrap_or_else(|e| panic!("{e:#}"));
    let out = base.run(&unverified);
    assert!(out.contains("other=20\n"), "expected the stale value the check exists to prevent:\n{out}");

    let err = match base.patch(&builder, &source, true) {
        Ok(p) => panic!("an impl ahead of `impl Row` renumbers it; the patch must be refused (built {:?})", p.path),
        Err(e) => format!("{e:#}"),
    };
    assert!(err.contains("cannot be patched in") && err.contains("Row"), "{err}");
}

#[test]
#[ignore = "compiles two crates for wasm32 and runs them in node; run with --ignored"]
fn regression_an_added_fn_with_a_static_is_refused() {
    let base = Base::build("additive-static");
    let builder = base.seeded_builder();
    let source = LIB_RS.replace(BODY_MARK, "    counter() as u64")
        + "\n#[inline(never)]\nfn counter() -> u32 {\n    \
           static N: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);\n    \
           N.fetch_add(1, std::sync::atomic::Ordering::SeqCst)\n}\n";
    let err = match base.patch(&builder, &source, true) {
        Ok(_) => panic!("a new writable static must refuse the patch"),
        Err(e) => format!("{e:#}"),
    };
    assert!(err.contains("writable static"), "{err}");
}

/// The study's "shape" edit: a `pub fn` appended to the file, called by
/// nothing. It patches, and the running program computes what it did.
/// (It may still redirect a function or two of the same codegen unit,
/// whose compiled bytes can shift when the unit gains a function — more
/// than the minimum, never less.)
#[test]
#[ignore = "compiles two crates for wasm32 and runs them in node; run with --ignored"]
fn an_appended_unused_pub_fn_patches_and_changes_nothing() {
    let base = Base::build("additive-appended");
    let builder = base.seeded_builder();
    let source = format!("{LIB_RS}\npub fn shape_probe_unused(n: u32) -> u32 {{\n    n ^ 0x5a\n}}\n");
    let patch = base.patch(&builder, &source, true).unwrap_or_else(|e| panic!("the patch did not build:\n{e:#}"));
    assert!(verified(&patch));
    let out = base.run(&patch);
    assert!(out.contains("probe=2\n") && out.contains("other=20\n"), "{out}");
}

/// New code nothing calls, in a codegen unit of its own: nothing the
/// running program calls differs, so the patch redirects nothing — an
/// empty jump table, not a failed pairing.
#[test]
#[ignore = "compiles two crates for wasm32 and runs them in node; run with --ignored"]
fn an_unused_new_module_patches_with_nothing_to_redirect() {
    let base = Base::build("additive-unused");
    let builder = base.seeded_builder();
    let source = format!("{LIB_RS}\npub mod extra {{\n    pub fn unused(n: u32) -> u32 {{\n        n ^ 0x5a\n    }}\n}}\n");
    let patch = base.patch(&builder, &source, true).unwrap_or_else(|e| panic!("the patch did not build:\n{e:#}"));
    assert!(verified(&patch));
    assert!(
        patch.jump_table.map.is_empty(),
        "nothing running changed, so nothing is redirected: {:?}",
        patch.jump_table.map
    );
}
