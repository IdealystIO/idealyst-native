// Loads a wasm-bindgen PIC side module against a plain main module, merging
// the side's bindgen glue imports with main-resolved GOT/env, then runs a
// side function that calls a web API (console.log) through shared memory.
import { readFileSync } from "node:fs";
import * as sideGlue from "./bgout/bgside.js";

const dir = "bgtarget/wasm32-unknown-unknown/release/";
const mainMod = await WebAssembly.compile(readFileSync(dir + "bgmain.wasm"));
const sideMod = await WebAssembly.compile(readFileSync("bgout/bgside_bg.wasm"));

// main owns memory + heap + the std symbols the side imports
const main = await WebAssembly.instantiate(mainMod, {});
main.exports.__wasm_call_ctors?.();
main.exports.main_touch();
const ex = main.exports, mem = ex.memory, table = ex.__indirect_function_table;
const g = (v, mut) => new WebAssembly.Global({ value: "i32", mutable: mut }, v);

// bindgen import functions from the side's own glue (uses the shared memory)
const glueImports = sideGlue.__wbg_get_imports(mem);

const REGION = 8 * 1024 * 1024;
const memoryBase = ex.host_reserve(REGION);
const stackTop = memoryBase + REGION - 16;

const imports = {};
const deferredFuncs = []; // side-defined GOT.func -> back-patch to side's own table post-instantiate
for (const imp of WebAssembly.Module.imports(sideMod)) {
  const ns = (imports[imp.module] ??= {});
  const { name } = imp;
  // bindgen namespaces come straight from the side's glue
  if (imp.module === "./bgside_bg.js" || imp.module === "__wbindgen_placeholder__") {
    ns[name] = glueImports[imp.module]?.[name] ?? (() => { throw new Error("bindgen import missing " + name); });
    continue;
  }
  if (imp.module === "env" && name === "memory") { ns.memory = mem; continue; }
  if (name === "__indirect_function_table") { ns[name] = table; continue; }
  if (name === "__memory_base") { ns[name] = g(memoryBase, false); continue; }
  if (name === "__table_base") { continue; } // set after GOT.func slots
  if (name === "__stack_pointer") { ns[name] = g(stackTop, true); continue; }
  if (imp.module === "GOT.mem") {
    const a = ex[name];
    ns[name] = a instanceof WebAssembly.Global ? g(a.value, true) : g(0, true);
    continue;
  }
  if (imp.module === "GOT.func") {
    const fn = ex[name];
    if (typeof fn === "function") { const i = table.length; table.grow(1); table.set(i, fn); ns[name] = g(i, true); }
    else { const gl = g(0, true); ns[name] = gl; deferredFuncs.push([name, gl]); } // side-defined: back-patch later
    continue;
  }
  ns[name] = g(0, true);
}
// reserve the side's own function region after GOT.func slots
const tableBase = table.length; table.grow(2048);
(imports.env ??= {}).__table_base = g(tableBase, false);

const side = await WebAssembly.instantiate(sideMod, imports);
// back-patch side-defined GOT.func to the side's own functions BEFORE start/relocs
for (const [name, gl] of deferredFuncs) {
  const fn = side.exports[name];
  if (typeof fn === "function") { const i = table.length; table.grow(1); table.set(i, fn); gl.value = i; }
}
console.log("deferred (side-defined) GOT.func back-patched:", deferredFuncs.length);

sideGlue.__wbg_finalize_init(side, sideMod);   // wires glue's wasm ref + runs __wbindgen_start

console.log(">>> calling side_hello(42) — bindgen web API call from the dynamically-linked side:");
sideGlue.side_hello(42);
console.log("<<< returned without error");
