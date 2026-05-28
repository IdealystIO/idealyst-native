import { loadSide } from "./loader.mjs";
import { readFileSync } from "node:fs";

const dir = "target/wasm32-unknown-unknown/release/";
const main = await WebAssembly.instantiate(await WebAssembly.compile(readFileSync(dir + "dynlink_main.wasm")), {});
main.exports.__wasm_call_ctors?.();

const sideMod = await WebAssembly.compile(readFileSync(dir + "dynlink_side.wasm"));
const { side, unresolved } = await loadSide(main, sideMod);

console.log("side imports total :", WebAssembly.Module.imports(sideMod).length);
console.log("unresolved         :", unresolved.length, unresolved.slice(0, 6));

// --- proof 1: a static (DYNLINK_COUNTER) shared across modules ---
console.log("main_bump()  =", main.exports.main_bump());   // 1
console.log("side_bump()  =", side.exports.side_bump());    // shared 1 + 10 = 11
console.log("main_read()  =", main.exports.main_read());    // 11
const sharedOk = main.exports.main_read() === 11;

// --- proof 2: real reactive framework code RUNS in the side, on main's arena/heap ---
let frameworkOk = false, sideSig = "n/a";
try {
  main.exports.main_signal();                 // init main's reactive arena first
  sideSig = side.exports.side_signal();        // runtime-core code executing in the SIDE module
  frameworkOk = sideSig === 7;
} catch (e) { sideSig = "threw: " + e.message; }

console.log("side_signal() =", sideSig, "(real runtime-core reactive code in the side)");
console.log(sharedOk ? "PASS ✓ shared static across modules" : "FAIL shared static");
console.log(frameworkOk ? "PASS ✓ runtime-core runs in the side via the loader"
                        : "framework-in-side: " + sideSig);
