// Proves the lazy-UI mechanic over the dynamic link: the SIDE module
// constructs a runtime_core::Primitive (View > Text) on the shared heap;
// MAIN mounts it through the real walker against a counting backend.
//
// This is the core risk in EVERY dynamic-split fork — once "side builds
// UI, main renders it" works, the rest is build plumbing.
import { loadSide } from "./loader.mjs";
import { readFileSync } from "node:fs";

const dir = "target/wasm32-unknown-unknown/release/";
const main = await WebAssembly.instantiate(
  await WebAssembly.compile(readFileSync(dir + "dynlink_main.wasm")),
  {},
);
main.exports.__wasm_call_ctors?.();
// Touch main's reactive system so its statics (ARENA, …) are initialized
// before the side allocates a Primitive that the walker will read.
main.exports.main_signal();

const sideMod = await WebAssembly.compile(readFileSync(dir + "dynlink_side.wasm"));
const { side, unresolved } = await loadSide(main, sideMod);
console.log("unresolved imports :", unresolved.length, unresolved.slice(0, 6));

// 1) side builds a Primitive on the shared heap → raw pointer
const ptr = side.exports.side_make_view();
console.log("side_make_view() ptr =", ptr);

// 2) main's walker mounts it; result encodes texts*1000 + text bytes
const result = main.exports.main_render_side(ptr);
const texts = Math.trunc(result / 1000);
const textBytes = result % 1000;

const expected = "hello from side #7";
console.log(`main_render_side() => texts=${texts} text_bytes=${textBytes}`);

const ok = texts === 1 && textBytes === expected.length;
console.log(
  ok
    ? `PASS ✓ side-built View>Text mounted by main's walker (1 text, ${expected.length} bytes = "${expected}")`
    : `FAIL expected texts=1 text_bytes=${expected.length}, got texts=${texts} text_bytes=${textBytes}`,
);
process.exit(ok ? 0 : 1);
