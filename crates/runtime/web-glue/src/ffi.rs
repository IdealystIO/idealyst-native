//! web-glue's own imports — declared with the same `import!` any crate
//! uses, so the runtime has no privileged channel the build pass must
//! know about.
//!
//! Off wasm32 these resolve to [`crate::mock`], a Rust stand-in for the JS
//! heap, so the Rust-side logic (handle accounting, the callback registry,
//! the executor, string ownership) is unit-testable with `cargo test`. The
//! JS-side behaviour is covered by the headless-browser E2E.

#[cfg(not(target_arch = "wasm32"))]
pub(crate) use crate::mock::*;

#[cfg(target_arch = "wasm32")]
crate::import! {
    pub(crate) fn drop_ref(h: u32) = "(h) => G.drop(h)";
    pub(crate) fn clone_ref(h: u32) -> u32 = "(h) => G.clone(h)";
    pub(crate) fn live_js() -> u32 = "() => G.live";

    pub(crate) fn str_new(p: usize, l: usize) -> u32 = "(p, l) => G.add(G.str(p, l))";
    pub(crate) fn num_new(n: f64) -> u32 = "(n) => G.add(n)";
    pub(crate) fn bool_new(b: u32) -> u32 = "(b) => G.add(b !== 0)";
    pub(crate) fn global() -> u32 = "() => G.add(globalThis)";

    // 0 undefined, 1 null, 2 boolean, 3 number, 4 string, 5 object,
    // 6 function, 7 symbol, 8 bigint.
    pub(crate) fn type_of(h: u32) -> u32 =
        "(h) => { const v = G.get(h); if (v === null) return 1; \
          switch (typeof v) { case 'undefined': return 0; case 'boolean': return 2; \
          case 'number': return 3; case 'string': return 4; case 'function': return 6; \
          case 'symbol': return 7; case 'bigint': return 8; default: return 5; } }";
    pub(crate) fn num_get(h: u32) -> f64 = "(h) => +G.get(h)";
    pub(crate) fn truthy(h: u32) -> u32 = "(h) => G.get(h) ? 1 : 0";
    pub(crate) fn str_get(h: u32, out: usize) -> u32 =
        "(h, o) => { const v = G.get(h); if (typeof v !== 'string') return 0; G.retStr(v, o); return 1; }";
    pub(crate) fn strict_eq(a: u32, b: u32) -> u32 = "(a, b) => G.get(a) === G.get(b) ? 1 : 0";

    #[catch]
    pub(crate) fn get(h: u32, p: usize, l: usize) -> u32 = "(h, p, l) => G.add(G.get(h)[G.str(p, l)])";
    #[catch]
    pub(crate) fn set(h: u32, p: usize, l: usize, v: u32) = "(h, p, l, v) => { G.get(h)[G.str(p, l)] = G.get(v); }";
    #[catch]
    pub(crate) fn call_method(h: u32, p: usize, l: usize, args: usize, argc: usize) -> u32 =
        "(h, p, l, a, n) => { const o = G.get(h); return G.add(o[G.str(p, l)](...G.args(a, n))); }";
    #[catch]
    pub(crate) fn call(f: u32, this: u32, args: usize, argc: usize) -> u32 =
        "(f, t, a, n) => G.add(G.get(f).apply(G.get(t), G.args(a, n)))";
    #[catch]
    pub(crate) fn construct(f: u32, args: usize, argc: usize) -> u32 =
        "(f, a, n) => G.add(Reflect.construct(G.get(f), G.args(a, n)))";
    #[catch]
    pub(crate) fn to_string(h: u32, out: usize) = "(h, o) => G.retStr(String(G.get(h)), o)";
    pub(crate) fn error_message(h: u32, out: usize) =
        "(h, o) => { const e = G.get(h); G.retStr(e instanceof Error ? `${e.name}: ${e.message}` : String(e), o); }";

    pub(crate) fn make_fn(id: u32, flags: u32) -> u32 = "(i, f) => G.fn(i, f)";
    pub(crate) fn revoke_fn(h: u32) = "(h) => G.revokeFn(h)";
    pub(crate) fn gc_own_fn(h: u32) = "(h) => G.gcOwn(h)";
    pub(crate) fn queue_microtask() = "() => G.queueMicrotask()";
    pub(crate) fn promise_then(p: u32, ok: u32, err: u32) =
        "(p, a, b) => { G.get(p).then(G.get(a), G.get(b)); }";
    pub(crate) fn promise_resolve(v: u32) -> u32 = "(v) => G.add(Promise.resolve(G.get(v)))";
}
