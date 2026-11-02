//! Behaviour of the `makeLoad` split-module loader (`src/__wasm_split.js`),
//! run under node against a mocked `fetch` / `WebAssembly` / `sessionStorage`
//! / `location`.
//!
//! The bug these cover: a deploy replaced the build a tab was running and
//! removed its `pkg/` files; the tab then opened a lazy screen for the first
//! time, the chunk fetch 404'd, and the whole app panicked. The loader must
//! (a) retry a transient failure, (b) reload the page once — and only once —
//! when the running build's chunk is gone, and (c) fetch again on a later
//! attempt instead of caching the failure.
//!
//! Skipped (with a note) when `node` is not installed, like the other
//! node-driven tests in this crate.

use std::process::Command;
use std::sync::atomic::{AtomicUsize, Ordering};

fn node_available() -> bool {
    Command::new("node").arg("--version").output().map(|o| o.status.success()).unwrap_or(false)
}

/// Runs `scenario` (the body of an async function with `makeLoad`, `env`
/// and `load(fn)` in scope) and returns its stdout.
fn run(scenario: &str) -> String {
    static NEXT: AtomicUsize = AtomicUsize::new(0);
    let dir = std::env::temp_dir().join(format!(
        "make-load-js-{}-{}",
        std::process::id(),
        NEXT.fetch_add(1, Ordering::Relaxed)
    ));
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("loader.mjs"), wasm_carve::MAKE_LOAD_JS).unwrap();
    let harness = format!(
        r#"
const env = {{ fetches: [], responses: [], reloads: 0, storage: new Map(), storageThrows: false }};
globalThis.fetch = async (url) => {{
  env.fetches.push(url);
  const r = env.responses.shift();
  if (r === "network") throw new TypeError("Failed to fetch");
  return {{ ok: r >= 200 && r < 300, status: r, url }};
}};
globalThis.setTimeout = (f) => {{ queueMicrotask(f); return 0; }};
globalThis.sessionStorage = {{
  getItem: (k) => {{ if (env.storageThrows) throw new Error("denied"); return env.storage.has(k) ? env.storage.get(k) : null; }},
  setItem: (k, v) => {{ if (env.storageThrows) throw new Error("denied"); env.storage.set(k, v); }},
}};
globalThis.location = {{ reload: () => {{ env.reloads++; }} }};
WebAssembly.instantiateStreaming = async (response) => ({{
  instance: {{ exports: {{ ["from_" + response.url]: () => {{}} }} }},
}});
console.error = () => {{}};
console.warn = () => {{}};

const signals = [];
const table = {{ get: () => (_data, ok) => signals.push(ok) }};
const initSync = () => ({{ memory: {{}}, __indirect_function_table: table }});
const {{ makeLoad }} = await import("./loader.mjs");
// One Rust-side attempt: resolves with the signalled outcome.
async function load(fn) {{
  const before = signals.length;
  await fn(1, 0);
  if (signals.length !== before + 1) throw new Error("expected exactly one signal, got " + (signals.length - before));
  return signals[signals.length - 1];
}}
const out = await (async () => {{ {scenario} }})();
process.stdout.write(JSON.stringify(out));
"#
    );
    let path = dir.join("harness.mjs");
    std::fs::write(&path, harness).unwrap();
    let out = Command::new("node").arg(&path).output().unwrap();
    let _ = std::fs::remove_dir_all(&dir);
    assert!(out.status.success(), "node failed: {}", String::from_utf8_lossy(&out.stderr));
    String::from_utf8(out.stdout).unwrap()
}

macro_rules! require_node {
    () => {
        if !node_available() {
            eprintln!("skipping: node not installed");
            return;
        }
    };
}

#[test]
fn transient_failures_are_retried_before_failing() {
    require_node!();
    let out = run(r#"
        env.responses = ["network", 503, 200];
        const fn = makeLoad("./m.wasm", [], {}, initSync);
        return [await load(fn), env.fetches.length, env.reloads];
    "#);
    assert_eq!(out, "[true,3,0]");
}

#[test]
fn transient_failures_give_up_after_the_retry_schedule() {
    require_node!();
    let out = run(r#"
        env.responses = [503, 503, 503, 503];
        const fn = makeLoad("./m.wasm", [], {}, initSync);
        return [await load(fn), env.fetches.length, env.reloads];
    "#);
    assert_eq!(out, "[false,3,0]");
}

#[test]
fn regression_stale_build_chunk_404_reloads_once_not_in_a_loop() {
    require_node!();
    let out = run(r#"
        env.responses = [404, 404];
        const fn = makeLoad("./module_7___lazy_body.9c5e.wasm", [], {}, initSync);
        const first = await load(fn);
        const second = await load(fn);
        // A 404 is not transient: one fetch per attempt, no retries.
        return [first, second, env.fetches.length, env.reloads];
    "#);
    assert_eq!(out, "[false,false,2,1]");
}

#[test]
fn blocked_session_storage_never_reloads() {
    require_node!();
    let out = run(r#"
        env.storageThrows = true;
        env.responses = [404];
        const fn = makeLoad("./m.wasm", [], {}, initSync);
        return [await load(fn), env.reloads];
    "#);
    assert_eq!(out, "[false,0]");
}

#[test]
fn regression_failed_load_is_fetched_again_on_the_next_attempt() {
    require_node!();
    let out = run(r#"
        env.responses = [403, 200];
        const fused = {};
        const fn = makeLoad("./m.wasm", [], fused, initSync);
        const first = await load(fn);
        const second = await load(fn);
        // Once loaded, later attempts signal success without fetching.
        const third = await load(fn);
        return [first, second, third, env.fetches.length, typeof fused["from_./m.wasm"]];
    "#);
    assert_eq!(out, r#"[false,true,true,2,"function"]"#);
}

#[test]
fn a_failed_dependency_fails_the_dependent_and_is_shared() {
    require_node!();
    let out = run(r#"
        env.responses = [404, 200, 200, 200];
        const fused = {};
        const chunk = makeLoad("./chunk_0.wasm", [], fused, initSync);
        const a = makeLoad("./a.wasm", [chunk], fused, initSync);
        const b = makeLoad("./b.wasm", [chunk], fused, initSync);
        const failed = await load(a);
        // The chunk now loads once and both dependents reuse it.
        const okA = await load(a);
        const okB = await load(b);
        return [failed, okA, okB, env.fetches];
    "#);
    assert_eq!(out, r#"[false,true,true,["./chunk_0.wasm","./chunk_0.wasm","./a.wasm","./b.wasm"]]"#);
}
