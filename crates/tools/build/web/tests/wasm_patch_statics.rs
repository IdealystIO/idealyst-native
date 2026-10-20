//! A wasm hot patch runs against the RUNNING module's statics.
//!
//! The bug this guards: a patch re-emits the edited crate, so every
//! `static` / `thread_local!` that crate defines was defined a second
//! time inside the patch, in the patch's own slice of memory. Code the
//! patch redirected wrote one copy and the base code still running read
//! the other. CrewForge's kiosk stored the report sign-in from a patched
//! screen while its request headers, built by unpatched code, read "Not
//! signed in".
//!
//! This compiles a real crate for wasm32, prepares its base module the
//! way an own-mode dev build does, builds a patch for a body edit through
//! `WasmPatchBuilder` — the builder the dev loop uses — and then runs
//! both in Node: the base writes its statics, a loader that follows the
//! patch's import plan exactly as `backend_web::hot_patch` does
//! instantiates the patch, and the patched function must read what the
//! base wrote:
//!
//! * a module-level writable `static` (the sign-in case);
//! * a `static` declared inside a function body;
//! * a `const`-initialized `thread_local!` and a lazily initialized one
//!   (plain statics on `wasm32-unknown-unknown`);
//! * a writable `static` the base does not have gets storage of its own,
//!   and an immutable `static` keeps a fresh address per patch, which
//!   `stylesheet!`'s cache key relies on.
//!
//! `#[ignore]`d like its siblings: it needs the `wasm32-unknown-unknown`
//! target and `node` on `PATH`. Run it deliberately:
//!
//! ```text
//! cargo test -p build-web --test wasm_patch_statics -- --ignored --nocapture
//! ```

use std::path::{Path, PathBuf};
use std::process::Command;

use build_runtime_server::hotpatch::replay::CapturedInvocation;
use build_web::hotpatch_aliases;
use build_web::hotpatch_base::{prepare_base_module, Flavor};
use build_web::hotpatch_build::{PatchCrate, WasmPatchBuilder};

const LIB_RS: &str = r#"
#![allow(non_snake_case)]
use std::cell::Cell;
use std::sync::atomic::{AtomicU32, Ordering};

pub static SESSION: AtomicU32 = AtomicU32::new(0);
static KEY: u8 = 0;

thread_local! {
    static TOKEN: Cell<u32> = const { Cell::new(0) };
    static LAZY: Cell<u32> = Cell::new(0);
}

#[inline(never)]
fn visits() -> u32 {
    static VISITS: AtomicU32 = AtomicU32::new(0);
    VISITS.fetch_add(1, Ordering::SeqCst) + 1
}

#[doc(hidden)]
#[inline(never)]
fn __Probe_hot_impl() -> u64 {
    // BODY
    0
}

#[doc(hidden)]
#[inline(never)]
fn __Key_hot_impl() -> usize {
    &KEY as *const u8 as usize
}

#[no_mangle]
pub extern "C" fn init() {
    SESSION.store(42, Ordering::SeqCst);
    TOKEN.with(|t| t.set(7));
    LAZY.with(|t| t.set(5));
    visits();
    visits();
}

// Through `fn` pointers, as `#[component]`'s split reaches its inner
// half: that is what gives the two `__*_hot_impl` table slots in the base
// and in the patch. `black_box`, or rustc turns the pointer back into a
// direct call and neither module takes the address.
#[no_mangle]
pub extern "C" fn probe() -> u64 {
    std::hint::black_box(__Probe_hot_impl as fn() -> u64)()
}

#[no_mangle]
pub extern "C" fn key() -> usize {
    std::hint::black_box(__Key_hot_impl as fn() -> usize)()
}
"#;

/// The edit: a body-only change to `__Probe_hot_impl`. `FRESH` is a
/// writable static the base never had.
const PATCHED_BODY: &str = r#"
    static FRESH: AtomicU32 = AtomicU32::new(900);
    let fresh = FRESH.fetch_add(1, Ordering::SeqCst) as u64;
    SESSION.load(Ordering::SeqCst) as u64 * 1_000_000_000
        + TOKEN.with(|t| t.get()) as u64 * 10_000_000
        + LAZY.with(|t| t.get()) as u64 * 1_000_000
        + visits() as u64 * 1_000
        + fresh"#;

/// The page's loader (`backend_web::hot_patch::load`), in Node: reserve
/// memory and table, supply every import as the plan says, instantiate,
/// run the relocation thunks. Then call the patch's functions through the
/// slots the jump table names.
const LOADER_JS: &str = r#"
const fs = require('fs');
const [basePath, patchPath, probeRel, keyRel, ifuncCount] = process.argv.slice(2);
(async () => {
  const base = (await WebAssembly.instantiate(fs.readFileSync(basePath), {})).instance.exports;
  base.init();
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
  const table = base.__indirect_function_table;
  const probe = table.get(tableBase + Number(probeRel));
  const key = table.get(tableBase + Number(keyRel));
  console.log('read=' + probe());
  console.log('read-again=' + probe());
  console.log('key-fresh=' + (key() !== base.key()));
})().catch(e => { console.log('error=' + e.message); process.exit(1); });
"#;

fn scratch(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("idealyst-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

fn rustc() -> String {
    let out = Command::new("rustc").args(["--print", "sysroot"]).output().expect("run rustc");
    let sysroot = String::from_utf8(out.stdout).unwrap();
    Path::new(sysroot.trim()).join("bin/rustc").display().to_string()
}

#[test]
#[ignore = "compiles a crate for wasm32 and runs it in node; run with --ignored"]
fn regression_a_wasm_patch_reads_the_running_modules_statics() {
    let dir = scratch("wasm-patch-statics");
    std::fs::create_dir_all(dir.join("src")).unwrap();
    std::fs::create_dir_all(dir.join("deps")).unwrap();
    std::fs::write(dir.join("src/lib.rs"), LIB_RS).unwrap();

    // ── 1. The base build, with the link flags a hot-patch build uses ─
    let mut args: Vec<String> = [
        "--crate-name",
        "probe",
        "--edition=2021",
        "src/lib.rs",
        "--crate-type",
        "cdylib",
        "--emit=link",
        "--target",
        "wasm32-unknown-unknown",
        "-C",
        "opt-level=0",
        "-C",
        "debuginfo=0",
        "-C",
        "metadata=wasmpatchstatics",
        "--out-dir",
    ]
    .iter()
    .map(|s| s.to_string())
    .collect();
    args.push(dir.join("deps").display().to_string());
    for a in build_web::hot_patch_link_args() {
        args.push(format!("-Clink-arg={a}"));
    }
    let rustc = rustc();
    let status = Command::new(&rustc).args(&args).current_dir(&dir).status().expect("run rustc");
    assert!(status.success(), "the base build failed (is the wasm32-unknown-unknown target installed?)");
    let captures = dir.join("captures");
    std::fs::create_dir_all(&captures).unwrap();
    let capture = CapturedInvocation { rustc, args, cwd: dir.display().to_string(), env: Vec::new() };
    std::fs::write(captures.join("probe.cdylib.json"), serde_json::to_vec(&capture).unwrap()).unwrap();

    // ── 2. Prepare and index the base, as an own-mode dev build does ──
    let linked = std::fs::read(dir.join("deps/probe.wasm")).unwrap();
    let aliases = hotpatch_aliases::read_from_linked(&linked).unwrap();
    let alias_path = dir.join("probe.aliases.tsv");
    hotpatch_aliases::write(&alias_path, &aliases).unwrap();
    let data = hotpatch_aliases::read_data_symbols_from_linked(&linked).unwrap();
    let data_path = dir.join("probe.datasyms.tsv");
    hotpatch_aliases::write_data_symbols(&data_path, &data).unwrap();
    let (prepared, _) = prepare_base_module(&linked, Flavor::Own).unwrap();
    let served = dir.join("probe.served.wasm");
    std::fs::write(&served, &prepared).unwrap();
    // The slots the jump table is keyed by. By the v0 symbol's END: shims
    // instantiated over the function (`<fn() as FnOnce>::call_once`)
    // carry its name too.
    let slots = build_web::hotpatch_wasm::base_slots(&prepared).unwrap();
    let slot = |ident: &str| -> u64 {
        *slots.iter().find(|(k, _)| k.ends_with(ident)).unwrap_or_else(|| panic!("no slot for {ident}")).1 as u64
    };
    let (probe_slot, key_slot) = (slot("5probe16___Probe_hot_impl"), slot("5probe14___Key_hot_impl"));

    // ── 3. The body edit, through the dev loop's builder ─────────────
    std::fs::write(dir.join("src/lib.rs"), LIB_RS.replace("    // BODY\n    0", PATCHED_BODY)).unwrap();
    let builder =
        WasmPatchBuilder::new(&served, Some(&alias_path), Some(&data_path), &captures, "probe", dir.join("patches"))
            .unwrap();
    let patch = builder
        .build_crates(&[PatchCrate::new("probe", None)])
        .unwrap_or_else(|e| panic!("the patch did not build:\n{e:#}"));
    let rel = |s: u64| *patch.jump_table.map.get(&s).unwrap_or_else(|| panic!("slot {s} not in the jump table"));

    // ── 4. Run both ───────────────────────────────────────────────────
    let loader = dir.join("load.cjs");
    std::fs::write(&loader, LOADER_JS).unwrap();
    let out = Command::new("node")
        .arg(&loader)
        .arg(&served)
        .arg(&patch.path)
        .arg(rel(probe_slot).to_string())
        .arg(rel(key_slot).to_string())
        .arg(patch.jump_table.ifunc_count.to_string())
        .output()
        .expect("run node");
    let stdout = String::from_utf8_lossy(&out.stdout).into_owned();
    let stderr = String::from_utf8_lossy(&out.stderr);

    // SESSION 42, TOKEN 7, LAZY 5, VISITS 2 → 3 on this call, FRESH 900.
    assert!(
        stdout.contains("read=42075003900\n"),
        "the patched function did not read the running module's statics \
         (expected read=42075003900: SESSION 42, TOKEN 7, LAZY 5, visits 3, fresh 900):\n{stdout}{stderr}"
    );
    assert!(stdout.contains("read-again=42075004901\n"), "a second call did not advance the statics:\n{stdout}");
    // An immutable static is content, not state: the patch keeps its own
    // copy. `stylesheet!` keys its cache by exactly such an address.
    assert!(stdout.contains("key-fresh=true\n"), "an immutable static was bound to the base's copy:\n{stdout}");

    let _ = std::fs::remove_dir_all(&dir);
}
