// web-glue JS runtime — the `G` object every glue snippet closes over.
//
// This file is NOT loaded by a <script> tag and is not known to the build
// tool. It travels INSIDE the linked wasm as the `runtime` record of the
// `__idealyst_glue` custom section (see `src/record.rs`); the build pass
// (`build_web::own_glue`) pastes it into `pkg/<lib>.js` as the body of a
// function and binds its return value to `G`. So the runtime is versioned
// with the crate, and the build tool only knows the framing + the three
// entry points below (`attach`, `lazyAttach`, `module`) — never the heap
// layout. That is what ends the CLI/crate version lock-step wasm-bindgen
// imposes.
//
// Invariants (each one has a test in the headless-browser E2E,
// `crates/tools/build/web/tests/own_glue_e2e.rs`):
//
// * MEMORY VIEWS ARE NEVER HELD ACROSS A CALL INTO WASM. Growing wasm
//   memory detaches every existing ArrayBuffer view (its byteLength drops
//   to 0). Any call into wasm — `__glue_alloc` above all — may grow it.
//   Every read/write goes through `G.u8()` / `G.u32()`, which re-create
//   the view when it has been detached. `retStr` is the canonical case:
//   it allocates FIRST and only then asks for the view it writes through.
// * Handle 0 is `undefined`, permanently. `add(undefined)` returns 0,
//   `drop(0)` is a no-op, so Rust never allocates a slot for undefined.
// * A released slot holds the FREE sentinel until reused; touching it is
//   a thrown Error naming the handle — a Rust-side double release or
//   use-after-release is loud, never a silent read of someone else's
//   object. (Slots are reused LIFO, so a stale index CAN alias a newer
//   object once reused; the Rust side makes that unrepresentable by
//   owning every handle through an RAII `JsValue`.)
// * A JS function minted for a Rust callback checks its own `dead` flag
//   before entering wasm. Rust revokes it when the owning `Closure` drops,
//   so a stale call is a thrown Error, not a call into a freed closure.
//   Promise reactions are minted SILENT instead: dropping a `JsFuture`
//   before its promise settles must not turn the settle into an
//   uncaught error.
// * wasm i32 values arrive in JS SIGNED. Pointers and lengths above 2 GiB
//   would be negative, so every pointer/length is `>>> 0`'d before use.

"use strict";

const FREE = Symbol("web-glue:free");
const heap = [undefined];
const freeList = [];
let live = 0;

let ex = null;
let mem = null;
let lazy = null;
let errSlot = 0;
let u8 = new Uint8Array(0);
let u32 = new Uint32Array(0);

const dec = new TextDecoder("utf-8", { ignoreBOM: true, fatal: true });
const enc = new TextEncoder();
const fnState = new WeakMap();
const modules = new Map();

// Callback flags — must match `callback::FLAG_*` in Rust.
const ONCE = 1;
const SILENT = 2;

function attached() {
  if (ex === null) {
    if (lazy === null) {
      throw new Error("web-glue: a glue function ran before the wasm module was attached");
    }
    G.attach(lazy());
  }
  return ex;
}

function runMicrotasks() {
  attached().__glue_microtask();
}

const G = {
  // ---- entry points the build pass calls -------------------------------

  // Own mode: the generated loader calls this with the instance's exports
  // before running constructors or `main`.
  attach(exports) {
    ex = exports;
    mem = exports.memory;
    u8 = new Uint8Array(0);
    u32 = new Uint32Array(0);
    errSlot = exports.__glue_err_slot() >>> 0;
  },
  // Hybrid mode: wasm-bindgen owns instantiation, so the exports are
  // fetched on first use (`initSync()` returns them once instantiated).
  lazyAttach(thunk) {
    lazy = thunk;
  },
  // A crate's JS module (a `js_module!` record): evaluated on first `m()`.
  module(name, factory) {
    if (!modules.has(name)) modules.set(name, { factory, done: false, value: undefined });
  },
  m(name) {
    const rec = modules.get(name);
    if (rec === undefined) throw new Error(`web-glue: no JS module "${name}" in this bundle`);
    if (!rec.done) {
      rec.value = rec.factory(G);
      rec.done = true;
    }
    return rec.value;
  },

  // ---- handles ---------------------------------------------------------

  add(v) {
    if (v === undefined) return 0;
    let i;
    if (freeList.length !== 0) {
      i = freeList.pop();
      heap[i] = v;
    } else {
      i = heap.length;
      heap.push(v);
    }
    live++;
    return i;
  },
  get(i) {
    i >>>= 0;
    const v = heap[i];
    if (v === FREE || i >= heap.length) {
      throw new Error(`web-glue: handle ${i} used after release`);
    }
    return v;
  },
  drop(i) {
    i >>>= 0;
    if (i === 0) return;
    if (i >= heap.length || heap[i] === FREE) {
      throw new Error(`web-glue: handle ${i} released twice`);
    }
    heap[i] = FREE;
    freeList.push(i);
    live--;
  },
  take(i) {
    const v = G.get(i);
    G.drop(i);
    return v;
  },
  clone(i) {
    return G.add(G.get(i));
  },
  get live() {
    return live;
  },
  // `n` borrowed handles laid out as u32s at `ptr` (Rust keeps ownership).
  args(ptr, n) {
    const w = G.u32();
    const base = (ptr >>> 0) >>> 2;
    const out = new Array(n >>> 0);
    for (let k = 0; k < out.length; k++) out[k] = G.get(w[base + k]);
    return out;
  },

  // ---- memory ----------------------------------------------------------

  u8() {
    attached();
    if (u8.byteLength === 0) u8 = new Uint8Array(mem.buffer);
    return u8;
  },
  u32() {
    attached();
    if (u32.byteLength === 0) u32 = new Uint32Array(mem.buffer);
    return u32;
  },

  // ---- strings ---------------------------------------------------------

  // Rust → JS: borrow `len` UTF-8 bytes at `ptr`.
  str(ptr, len) {
    ptr >>>= 0;
    len >>>= 0;
    return len === 0 ? "" : dec.decode(G.u8().subarray(ptr, ptr + len));
  },
  // JS → Rust: encode, copy into a buffer Rust allocates (and then owns),
  // and write `[ptr, len]` into the two u32s at `out`.
  retStr(s, out) {
    const bytes = enc.encode(s);
    let ptr = 0;
    if (bytes.length !== 0) {
      // May grow memory. The view is requested AFTER this call on purpose.
      ptr = attached().__glue_alloc(bytes.length) >>> 0;
      G.u8().set(bytes, ptr);
    }
    const w = G.u32();
    const o = (out >>> 0) >>> 2;
    w[o] = ptr;
    w[o + 1] = bytes.length;
  },

  // ---- exceptions ------------------------------------------------------

  // Wraps a `#[catch]` snippet. A throw is parked in the Rust-owned error
  // slot ([flag, handle]) and the import returns 0; the Rust wrapper
  // checks the flag after every catching call.
  catching(f) {
    return function () {
      try {
        return f.apply(this, arguments);
      } catch (e) {
        const w = G.u32();
        const o = errSlot >>> 2;
        w[o + 1] = G.add(e);
        w[o] = 1;
        return 0;
      }
    };
  },

  // ---- callbacks -------------------------------------------------------

  fn(id, flags) {
    const st = { id: id >>> 0, flags, dead: false };
    const f = function (arg) {
      if (st.dead) {
        if (st.flags & SILENT) return;
        throw new Error(`web-glue: callback #${st.id} called after its Rust owner dropped it`);
      }
      if (st.flags & ONCE) st.dead = true;
      const code = attached().__glue_invoke(st.id, G.add(arg));
      if (code === 1) throw new Error(`web-glue: callback #${st.id} is no longer registered`);
      if (code === 2) throw new Error(`web-glue: callback #${st.id} invoked recursively`);
    };
    fnState.set(f, st);
    return G.add(f);
  },
  revokeFn(h) {
    const f = G.take(h);
    const st = fnState.get(f);
    if (st !== undefined) st.dead = true;
  },
  queueMicrotask() {
    queueMicrotask(runMicrotasks);
  },
};

return G;
