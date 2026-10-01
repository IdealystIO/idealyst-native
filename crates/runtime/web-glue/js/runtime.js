// web-glue JS runtime — the `G` object every glue snippet closes over.
//
// This file is NOT loaded by a <script> tag and is not known to the build
// tool. It travels INSIDE the linked wasm as the `runtime` record of the
// `__idealyst_glue` custom section (see `src/record.rs`); the build pass
// (`build_web::own_glue`) pastes it into `pkg/<lib>.js` as the body of a
// function and binds its return value to `G`. So the runtime is versioned
// with the crate, and the build tool only knows the framing + the entry
// points below (`attach`, `lazyAttach`, `module`, and the optional
// `entry`) — never the heap layout. That is what ends the CLI/crate version lock-step wasm-bindgen
// imposes.
//
// Invariants (each one has a test in the headless-browser E2E,
// `crates/tools/build/web/tests/own_glue_e2e.rs`):
//
// * MEMORY VIEWS ARE NEVER HELD ACROSS A CALL INTO WASM. Growing wasm
//   memory detaches every existing ArrayBuffer view (its byteLength drops
//   to 0). Any call into wasm — `__glue_alloc` / `__glue_realloc` above
//   all — may grow it. Every read/write goes through `G.u8()` / `G.u32()`,
//   which re-create the view when it has been detached. `retStr` is the
//   canonical case: it allocates FIRST and only then asks for the view it
//   writes through, and asks again after every realloc.
// * Handle 0 is `undefined` and handle 1 is `null`, permanently.
//   `add(undefined)` returns 0 and `add(null)` returns 1; dropping or
//   cloning either is a no-op, so Rust never allocates a slot for them
//   (`JsValue::UNDEFINED` / `JsValue::NULL` are constants).
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
const heap = [undefined, null];
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
let finalizer = null;

// Workers (`web_glue::worker`). The generated JS that embeds this runtime
// reports where it was loaded from (`G.entry`); a Worker re-imports that
// URL to instantiate the same module. `entryModule` hands back the
// compiled `WebAssembly.Module` when the loader kept it (own mode), so a
// worker skips the fetch + compile.
let entryUrl = null;
let entryModule = null;
let workerBlobUrl = null;

// The bootstrap every worker runs (a module worker from a blob URL). The
// first message carries the entry URL, the module (or undefined) and the
// table index of the Rust fn to run. `__glueWorkerInit` is exported by the
// generated JS in both modes (own: `initSync` / `init`; hybrid: through
// wasm-bindgen's `init`) and resolves to the raw exports. Any failure is
// rethrown from a task so it reaches the parent as the Worker's `error`
// event — an async rejection inside a worker never would.
const WORKER_BOOT = `self.onmessage = (e) => {
  self.onmessage = null;
  const { url, module, entry } = e.data;
  import(url)
    .then((m) => {
      if (typeof m.__glueWorkerInit !== "function") {
        throw new Error("web-glue: " + url + " has no __glueWorkerInit export (built by a tool without worker support)");
      }
      return m.__glueWorkerInit(module);
    })
    .then((ex) => ex.__glue_worker_start(entry))
    .catch((err) => setTimeout(() => { throw err; }));
};
`;

// Callback flags — must match `callback::FLAG_*` in Rust.
const ONCE = 1;
const SILENT = 2;
const ARGS = 4;

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
  // The generated JS reports its own URL (`import.meta.url`) and, when it
  // keeps one, a thunk for the compiled module. Optional: a bundle built
  // without it simply cannot spawn workers (`spawnWorker` says so).
  entry(url, module) {
    entryUrl = url;
    entryModule = typeof module === "function" ? module : null;
  },
  // The instance's raw exports (memory, the function table, …) — what the
  // hot-patch loader grows and instantiates a patch against.
  exports() {
    return attached();
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
    if (v === null) return 1;
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
    if (i <= 1) return;
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
    if ((i >>> 0) <= 1) return i >>> 0;
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
  // JS → Rust: UTF-8-encode `s` straight into a buffer Rust allocates
  // (and then owns), and write `[ptr, len]` into the two u32s at `out`.
  //
  // No intermediate `TextEncoder.encode` array: the buffer is allocated at
  // `s.length` bytes — exact for ASCII, the common case (CSS values,
  // attribute names, ids) — and ASCII is copied in with `charCodeAt`. At
  // the first non-ASCII code unit the buffer is grown to the worst case
  // for the rest (3 UTF-8 bytes per UTF-16 unit; a surrogate pair is 2
  // units → 4 bytes) and `encodeInto` writes the remainder in place, then
  // the buffer shrinks to what was written, so Rust adopts it with
  // capacity == length. Measured in V8 on short CSS-value strings: ~21 ns
  // against ~176 ns for encode + `set`. `__glue_alloc` / `__glue_realloc`
  // may GROW MEMORY: the view is (re)taken after each of them, never held
  // across one.
  retStr(s, out) {
    const n = s.length;
    let ptr = 0;
    let len = 0;
    if (n !== 0) {
      let cap = n;
      ptr = attached().__glue_alloc(cap) >>> 0;
      const view = G.u8();
      for (; len < n; len++) {
        const c = s.charCodeAt(len);
        if (c > 0x7f) break;
        view[ptr + len] = c;
      }
      if (len !== n) {
        const rest = s.slice(len);
        const need = len + rest.length * 3;
        ptr = ex.__glue_realloc(ptr, cap, need) >>> 0;
        cap = need;
        len += enc.encodeInto(rest, G.u8().subarray(ptr + len, ptr + cap)).written;
        if (len !== cap) ptr = ex.__glue_realloc(ptr, cap, len) >>> 0;
      }
    }
    const w = G.u32();
    const o = (out >>> 0) >>> 2;
    w[o] = ptr;
    w[o + 1] = len;
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
      const a = st.flags & ARGS ? G.add(Array.from(arguments)) : G.add(arg);
      const code = attached().__glue_invoke(st.id, a) >>> 0;
      if (code === 1) throw new Error(`web-glue: callback #${st.id} is no longer registered`);
      if (code === 2) throw new Error(`web-glue: callback #${st.id} invoked recursively`);
      // 3 + the return value's handle (Rust gave up ownership of it).
      return G.take(code - 3);
    };
    fnState.set(f, st);
    return G.add(f);
  },
  // `Closure::into_js_value`: the JS garbage collector owns the Rust
  // closure from here on. When the function is collected, release its
  // registry entry. (A registry that was never created — no
  // FinalizationRegistry in this engine — degrades to keeping the entry
  // for the page's lifetime, the pre-2021 wasm-bindgen behaviour.)
  gcOwn(h) {
    if (finalizer === null && typeof FinalizationRegistry === "function") {
      finalizer = new FinalizationRegistry((id) => {
        if (ex !== null || lazy !== null) attached().__glue_release(id);
      });
    }
    const st = fnState.get(G.get(h));
    if (finalizer !== null && st !== undefined) finalizer.register(G.get(h), st.id);
  },
  revokeFn(h) {
    const f = G.take(h);
    const st = fnState.get(f);
    if (st !== undefined) st.dead = true;
  },
  queueMicrotask() {
    queueMicrotask(runMicrotasks);
  },

  // ---- workers ---------------------------------------------------------

  // A module Worker that instantiates this same wasm module (sharing no
  // memory) and runs the Rust `fn()` at table index `entry` in it. Function
  // table indices agree because both instances come from the same module
  // bytes. The worker is NOT listening for messages until that fn installs
  // a listener — the caller waits for a message from it before posting.
  spawnWorker(entry) {
    if (entryUrl === null) {
      throw new Error(
        "web-glue: this bundle's JS does not report its module URL, so it cannot start a worker " +
          "(it was generated by a build tool that predates worker support — rebuild it)",
      );
    }
    if (workerBlobUrl === null) {
      workerBlobUrl = URL.createObjectURL(new Blob([WORKER_BOOT], { type: "text/javascript" }));
    }
    const w = new Worker(workerBlobUrl, { type: "module" });
    w.postMessage({ url: entryUrl, module: entryModule === null ? undefined : entryModule(), entry: entry >>> 0 });
    return G.add(w);
  },
};

return G;
