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
//! * [`regression_an_impl_inserted_ahead_of_another_reaches_the_new_body_through_the_old_name`]:
//!   a new `impl` ahead of an existing one renumbers it (`{impl#0}` →
//!   `{impl#1}`). Left under the new name, the app's base code keeps
//!   calling the OLD body of the method the same save edited — the test
//!   shows the stale value, then that the verified patch pairs the two;
//! * [`a_renumbered_method_nothing_edited_behaves_as_before`]: an item
//!   that was only renumbered runs as it did;
//! * [`a_renumbered_method_pairs_in_an_untrimmed_patch`]: the same pairing
//!   with no baseline, where every function is redirected;
//! * [`a_held_fn_pointer_reaches_the_edited_closure_of_a_renumbered_method`]:
//!   a closure the page already holds a pointer to follows its method;
//! * [`a_group_that_grew_is_refused`]: two functions of one path where the
//!   base had one pair with nothing, so the patch is refused;
//! * [`a_pair_with_another_signature_is_refused`]: what the signature
//!   check is for;
//! * [`regression_an_added_fn_with_a_static_is_refused`]: new writable
//!   state would get a fresh copy in every later patch;
//! * [`an_appended_unused_pub_fn_patches_and_changes_nothing`]: the
//!   study's "shape" edit — a `pub fn` appended and called by nothing —
//!   patches, and the program behaves as before;
//! * [`an_unused_new_module_patches_with_nothing_to_redirect`]: new code
//!   that changes nothing the program runs is a patch with an empty jump
//!   table, not a failed pairing.
//!
//! The same fixture also pins one seeding regression, for body edits and
//! additive ones alike:
//! [`regression_a_carried_crate_whose_seed_was_overtaken_still_trims`].
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

#[inline(never)]
pub fn scoped() -> u64 {
    #[inline(never)]
    fn local() -> u64 {
        3
    }
    std::hint::black_box(local as fn() -> u64)()
}

/// In a codegen unit of its own, so its call to `total` crosses objects:
/// the object that holds it names `total` by an import.
pub mod far {
    #[inline(never)]
    pub fn total_of(row: &super::Row) -> u32 {
        row.total()
    }
}

pub struct Timer;

impl Timer {
    #[inline(never)]
    pub fn callback() -> fn(u64) -> u64 {
        |x| x + 1000
    }
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

static HELD: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);

/// Keeps a fn pointer, as a mounted tree keeps a callback across a patch.
#[no_mangle]
pub extern "C" fn hold() {
    let held = probe_lib::Timer::callback();
    HELD.store(held as usize, std::sync::atomic::Ordering::SeqCst);
}

#[no_mangle]
pub extern "C" fn held() -> u64 {
    let raw = HELD.load(std::sync::atomic::Ordering::SeqCst);
    let held: fn(u64) -> u64 = unsafe { std::mem::transmute(raw) };
    held(1)
}

/// Keeps `scoped` in the program without a caller the jump table reaches.
#[no_mangle]
pub extern "C" fn scoped() -> u64 {
    probe_lib::scoped()
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
  base.hold();
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
  console.log('held=' + base.held());
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

/// A carried crate whose background seed was overtaken by a save — the
/// sources moved while it ran, so it delivered no baseline — used to
/// leave every patch of the base untrimmed: the plan needs a baseline for
/// every crate it carries. The patch's own replay of that crate compiled
/// the base's sources, so it now becomes the baseline, and the patch
/// redirects only what changed: `__Probe_hot_impl` (calls the edited
/// `lib_value`), not `__Other_hot_impl`.
#[test]
#[ignore = "compiles two crates for wasm32 and runs them in node; run with --ignored"]
fn regression_a_carried_crate_whose_seed_was_overtaken_still_trims() {
    use std::sync::atomic::{AtomicBool, Ordering};
    let base = Base::build("seed-overtaken");
    let slots = build_web::hotpatch_wasm::base_slots(&std::fs::read(&base.served).unwrap()).unwrap();
    let slot = |end: &str| *slots.iter().find(|(k, _)| k.ends_with(end)).unwrap_or_else(|| panic!("no slot {end}")).1 as u64;
    let (probe_slot, other_slot) = (slot("5probe16___Probe_hot_impl"), slot("5probe16___Other_hot_impl"));

    // While the seeds run, the app's sources read as moving.
    let seeding = Arc::new(AtomicBool::new(true));
    let file = base.dir.join("lib/src/lib.rs");
    let busy = seeding.clone();
    let probe: SourceProbe = Arc::new(move |name: &str| {
        if name == "probe" {
            return Some(if busy.load(Ordering::SeqCst) { "moving" } else { "base" }.to_string());
        }
        let pristine = std::fs::read_to_string(&file).unwrap() == LIB_RS;
        Some(if pristine { "base" } else { "edited" }.to_string())
    });
    let builder = WasmPatchBuilder::new(
        &base.served,
        Some(&base.aliases),
        Some(&base.data),
        &base.captures,
        "probe",
        base.dir.join("patches"),
    )
    .unwrap()
    .with_source_probe(probe);
    builder.seed_all(vec![
        PatchCrate::carried("probe_lib", Some("base".into())),
        PatchCrate::carried("probe", Some("base".into())),
    ]);
    builder.wait_for_seeds();
    seeding.store(false, Ordering::SeqCst);

    let patch = base
        .patch(&builder, &LIB_RS.replace(BODY_MARK, "    7"), false)
        .unwrap_or_else(|e| panic!("the patch did not build:\n{e:#}"));
    assert!(patch.jump_table.map.contains_key(&probe_slot), "{:?}", patch.jump_table.map);
    assert!(
        !patch.jump_table.map.contains_key(&other_slot),
        "an unchanged function was redirected: the patch was not trimmed ({} entries)",
        patch.jump_table.map.len()
    );
    let out = base.run(&patch);
    assert!(out.contains("probe=7\n") && out.contains("other=20\n"), "{out}");
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

/// A new type with an impl, ahead of `impl Row`: it renumbers `Row`'s.
const IMPL_AHEAD: &str =
    "pub struct Fresh;\n\nimpl Fresh {\n    #[inline(never)]\n    pub fn fresh(&self) -> u32 {\n        9\n    }\n}\n\nimpl Row {";

/// The base's slot for the function whose symbol ends in `end`.
fn slot_of(base: &Base, end: &str) -> u64 {
    let slots = build_web::hotpatch_wasm::base_slots(&std::fs::read(&base.served).unwrap()).unwrap();
    *slots.iter().find(|(k, _)| k.ends_with(end)).unwrap_or_else(|| panic!("no slot {end}")).1 as u64
}

#[test]
#[ignore = "compiles two crates for wasm32 and runs them in node; run with --ignored"]
fn regression_an_impl_inserted_ahead_of_another_reaches_the_new_body_through_the_old_name() {
    let base = Base::build("additive-impl-ahead");
    let builder = base.seeded_builder();
    // A new impl ahead of `impl Row`, and an edit to `Row::total` in the
    // same save.
    let source = LIB_RS.replace("impl Row {", IMPL_AHEAD).replace("self.a + 1", "self.a + 50");

    // Unverified, the patch builds — and is wrong. `total` compiled to a
    // new name, so the app's base code, which calls it directly by the
    // OLD name, is not redirected and keeps the old body: 20, where the
    // source now says 510.
    let unverified = base.patch(&builder, &source, false).unwrap_or_else(|e| panic!("{e:#}"));
    let out = base.run(&unverified);
    assert!(out.contains("other=20\n"), "expected the stale value the check exists to prevent:\n{out}");

    let patch = base.patch(&builder, &source, true).unwrap_or_else(|e| panic!("the patch did not build:\n{e:#}"));
    assert!(verified(&patch));
    let out = base.run(&patch);
    assert!(out.contains("other=510\n"), "the base did not reach the new body through the old name:\n{out}");
    assert!(out.contains("probe=2\n"), "{out}");

    // A later patch reuses the replay's objects, and pairs them as before.
    let again = base.patch(&builder, &source, true).unwrap_or_else(|e| panic!("{e:#}"));
    let out = base.run(&again);
    assert!(out.contains("other=510\n"), "{out}");
}

/// The patch stays trimmed: `__Probe_hot_impl` reaches nothing that
/// differs. (`total` itself may: a codegen unit that gained a function
/// names its anonymous data differently, which is more redirected than
/// the minimum, never less.)
#[test]
#[ignore = "compiles two crates for wasm32 and runs them in node; run with --ignored"]
fn a_renumbered_method_nothing_edited_behaves_as_before() {
    let base = Base::build("additive-renumbered-unedited");
    let builder = base.seeded_builder();
    let probe_slot = slot_of(&base, "5probe16___Probe_hot_impl");
    let source = LIB_RS.replace("impl Row {", IMPL_AHEAD);
    let patch = base.patch(&builder, &source, true).unwrap_or_else(|e| panic!("the patch did not build:\n{e:#}"));
    assert!(!patch.jump_table.map.contains_key(&probe_slot), "{:?}", patch.jump_table.map);
    let out = base.run(&patch);
    assert!(out.contains("other=20\n") && out.contains("probe=2\n") && out.contains("held=1001\n"), "{out}");
}

#[test]
#[ignore = "compiles two crates for wasm32 and runs them in node; run with --ignored"]
fn a_renumbered_method_pairs_in_an_untrimmed_patch() {
    let base = Base::build("additive-renumbered-untrimmed");
    let builder = WasmPatchBuilder::new(
        &base.served,
        Some(&base.aliases),
        Some(&base.data),
        &base.captures,
        "probe",
        base.dir.join("patches"),
    )
    .unwrap();
    let source = LIB_RS.replace("impl Row {", IMPL_AHEAD).replace("self.a + 1", "self.a + 50");
    let patch = base.patch(&builder, &source, true).unwrap_or_else(|e| panic!("the patch did not build:\n{e:#}"));
    let out = base.run(&patch);
    assert!(out.contains("other=510\n") && out.contains("probe=2\n"), "{out}");
}

/// A closure coerced to a fn pointer is a `call_once` instantiation naming
/// the closure: that instantiation's slot in the base is what the pointer
/// holds, and both names moved with the impl.
#[test]
#[ignore = "compiles two crates for wasm32 and runs them in node; run with --ignored"]
fn a_held_fn_pointer_reaches_the_edited_closure_of_a_renumbered_method() {
    let base = Base::build("additive-held-closure");
    let builder = base.seeded_builder();
    let source = LIB_RS.replace("impl Timer {", "pub struct Fresh;\n\nimpl Fresh {\n    pub fn fresh(&self) -> u32 {\n        9\n    }\n}\n\nimpl Timer {").replace("x + 1000", "x + 2000");
    let patch = base.patch(&builder, &source, true).unwrap_or_else(|e| panic!("the patch did not build:\n{e:#}"));
    let out = base.run(&patch);
    assert!(out.contains("held=2001\n"), "the held pointer kept the old closure:\n{out}");
}

/// `local` keeps its name, and a second function of its path appears:
/// which of the two a base caller meant is nothing the path can say.
#[test]
#[ignore = "compiles two crates for wasm32 and runs them in node; run with --ignored"]
fn a_group_that_grew_is_refused() {
    let base = Base::build("additive-grown");
    let builder = base.seeded_builder();
    let source = LIB_RS.replace(
        "std::hint::black_box(local as fn() -> u64)()",
        "let second = {\n        #[inline(never)]\n        fn local() -> u64 {\n            4\n        }\n        std::hint::black_box(local as fn() -> u64)()\n    };\n    second + std::hint::black_box(local as fn() -> u64)()",
    );
    let err = match base.patch(&builder, &source, true) {
        Ok(p) => panic!("a second function of one path must refuse the patch (built {:?})", p.path),
        Err(e) => format!("{e:#}"),
    };
    assert!(err.contains("cannot be patched in") && err.contains("local"), "{err}");
}

/// `local` stops being compiled, and a new function of its path takes one
/// argument: they are one group of one on each side, and not one function.
#[test]
#[ignore = "compiles two crates for wasm32 and runs them in node; run with --ignored"]
fn a_pair_with_another_signature_is_refused() {
    let base = Base::build("additive-paired-signature");
    let builder = base.seeded_builder();
    let source = LIB_RS.replace(
        "std::hint::black_box(local as fn() -> u64)()",
        "#[allow(unused)]\n    {\n        #[inline(never)]\n        fn local(n: u64) -> u64 {\n            n\n        }\n        std::hint::black_box(local as fn(u64) -> u64)(4)\n    }",
    );
    let err = match base.patch(&builder, &source, true) {
        Ok(p) => panic!("a pair whose signatures differ must refuse the patch (built {:?})", p.path),
        Err(e) => format!("{e:#}"),
    };
    assert!(err.contains("signature"), "{err}");
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

/// A component ahead of a neighbour, as `#[component]` and `#[props]`
/// expand under hot-reload: their impls in a const named after the
/// component (`ui-lowering-parity`'s `symbol_stability` compiles the real
/// macros and pins that), and a `#[derive]` no macro can scope. The derive
/// lands at module level ahead of `impl Row`, renumbering it, in the save
/// that also edits `Row::total`.
#[test]
#[ignore = "compiles two crates for wasm32 and runs them in node; run with --ignored"]
fn a_component_added_ahead_of_a_neighbour_patches() {
    let base = Base::build("additive-component-ahead");
    let builder = base.seeded_builder();
    let component = "#[derive(Default, Clone)]\npub struct GaugeProps {\n    pub level: u32,\n}\n\n#[allow(non_upper_case_globals)]\nconst __Gauge_scope: () = {\n    impl GaugeProps {\n        #[inline(never)]\n        pub fn level(&self) -> u32 {\n            self.level + 1\n        }\n    }\n};\n\n#[inline(never)]\npub fn Gauge(props: &GaugeProps) -> u32 {\n    props.level()\n}\n\nimpl Row {";
    let source = LIB_RS.replace("impl Row {", component).replace("self.a + 1", "self.a + 50");
    let patch = base.patch(&builder, &source, true).unwrap_or_else(|e| panic!("the patch did not build:\n{e:#}"));
    assert!(verified(&patch));
    let out = base.run(&patch);
    assert!(out.contains("other=510\n") && out.contains("probe=2\n"), "{out}");
}

/// A new FILE: a `mod` line and a call in existing code, and everything
/// else in a file the base never compiled. It has no neighbours to
/// renumber, so nothing is paired, and the app's base code reaches it
/// through the edited function.
#[test]
#[ignore = "compiles two crates for wasm32 and runs them in node; run with --ignored"]
fn a_new_file_module_called_from_an_existing_body_runs() {
    let base = Base::build("additive-new-file");
    let builder = base.seeded_builder();
    std::fs::write(
        base.dir.join("lib/src/extra.rs"),
        "use super::*;\n\npub struct Fresh {\n    pub n: u64,\n}\n\nimpl Fresh {\n    #[inline(never)]\n    pub fn doubled(&self) -> u64 {\n        self.n * 2\n    }\n}\n\nimpl Clone for Fresh {\n    fn clone(&self) -> Self {\n        Fresh { n: self.n + lib_seed() }\n    }\n}\n",
    )
    .unwrap();
    let source = format!("mod extra;\n{}\nfn lib_seed() -> u64 {{\n    10\n}}\n", LIB_RS.replace(BODY_MARK, "    extra::Fresh { n: 11 }.clone().doubled()"));
    let patch = base.patch(&builder, &source, true).unwrap_or_else(|e| panic!("the patch did not build:\n{e:#}"));
    assert!(verified(&patch));
    let out = base.run(&patch);
    assert!(out.contains("probe=42\n"), "the base did not reach the new file's code:\n{out}");
    assert!(out.contains("other=20\n") && out.contains("held=1001\n"), "{out}");
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
