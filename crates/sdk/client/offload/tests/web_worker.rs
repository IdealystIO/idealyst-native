//! offload's web backend in a real browser: jobs run in Web Workers that
//! instantiate this same test module (`web_glue::worker`), with postcard
//! arguments and results.
//!
//! Run with `cargo test -p offload --target wasm32-unknown-unknown` (the
//! workspace runner supplies the glue JS; a matching chromedriver via
//! `CHROMEDRIVER`).

#![cfg(target_arch = "wasm32")]

use serde::{Deserialize, Serialize};
use wasm_bindgen_test::{wasm_bindgen_test, wasm_bindgen_test_configure};

wasm_bindgen_test_configure!(run_in_browser);

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
struct Req {
    label: String,
    values: Vec<u32>,
}

#[derive(Debug, PartialEq, Serialize, Deserialize)]
struct Out {
    label: String,
    sum: u64,
    in_worker: bool,
}

#[offload::job]
fn summarize(req: Req) -> Out {
    Out {
        label: req.label.to_uppercase(),
        sum: req.values.iter().map(|&v| v as u64).sum(),
        in_worker: web_glue::worker::in_worker(),
    }
}

#[offload::job]
fn panics(_: u32) -> u32 {
    panic!("deliberate panic in an offload job");
}

/// A trap that is not a Rust panic (no panic hook runs): the worker's
/// `error` event is the only signal.
#[offload::job]
fn traps(_: u32) -> u32 {
    core::arch::wasm32::unreachable()
}

/// Busy-waits `ms` milliseconds; returns when it ran (Date.now, ms) so the
/// test can see whether jobs overlapped.
#[offload::job]
fn spin(ms: u32) -> (f64, f64) {
    let start = web_glue::dom::date_now();
    while web_glue::dom::date_now() - start < ms as f64 {}
    (start, web_glue::dom::date_now())
}

#[wasm_bindgen_test]
async fn a_job_runs_in_a_worker_and_returns_its_result() {
    let req = Req { label: "héllo".into(), values: (1..=1000).collect() };
    let out = offload::run(offload::handle!(summarize), &req).await.expect("result");
    assert_eq!(out, Out { label: "HÉLLO".into(), sum: 500_500, in_worker: true });
    assert!(!web_glue::worker::in_worker(), "the caller is on the page");
}

#[wasm_bindgen_test]
async fn a_large_payload_round_trips() {
    let req = Req { label: "big".into(), values: vec![7; 1 << 20] };
    let out = offload::run(offload::handle!(summarize), &req).await.expect("result");
    assert_eq!(out.sum, 7 * (1 << 20));
}

/// Under wasmworker a panicking job never answered — the future hung.
#[wasm_bindgen_test]
async fn regression_a_panicking_job_is_canceled_not_hung() {
    let r = offload::run(offload::handle!(panics), &1).await;
    assert!(matches!(r, Err(offload::OffloadError::Canceled)), "{r:?}");
    // The pool replaced the dead worker: the next job still runs.
    let out = offload::run(offload::handle!(summarize), &Req { label: "after".into(), values: vec![1, 2] })
        .await
        .expect("a job after the panic");
    assert_eq!(out.sum, 3);
}

#[wasm_bindgen_test]
async fn a_trapping_job_is_canceled_not_hung() {
    let r = offload::run(offload::handle!(traps), &1).await;
    assert!(matches!(r, Err(offload::OffloadError::Canceled)), "{r:?}");
    let out = offload::run(offload::handle!(summarize), &Req { label: "x".into(), values: vec![5] })
        .await
        .expect("a job after the trap");
    assert_eq!(out.sum, 5);
}

#[wasm_bindgen_test]
async fn concurrent_jobs_run_in_parallel_workers() {
    let cores = web_glue::worker::hardware_concurrency().unwrap_or(1);
    if cores < 2 {
        return; // one core reported: the pool is one worker, nothing to overlap
    }
    let n = cores.min(4);
    let jobs = (0..n).map(|_| offload::run(offload::handle!(spin), &300));
    let spans: Vec<(f64, f64)> =
        futures::future::join_all(jobs).await.into_iter().map(|r| r.expect("spin")).collect();
    assert_eq!(spans.len(), n as usize);
    // Every pair overlaps: all ran at once, not one after another.
    let latest_start = spans.iter().map(|s| s.0).fold(f64::MIN, f64::max);
    let earliest_end = spans.iter().map(|s| s.1).fold(f64::MAX, f64::min);
    assert!(latest_start < earliest_end, "jobs did not overlap: {spans:?}");
    // And results are routed to the right callers.
    let outs = futures::future::join_all((0..8u32).map(|i| async move {
        let req = Req { label: format!("j{i}"), values: vec![i; 10] };
        offload::run(offload::handle!(summarize), &req).await.expect("result")
    }))
    .await;
    for (i, out) in outs.iter().enumerate() {
        assert_eq!(out.label, format!("J{i}"));
        assert_eq!(out.sum, 10 * i as u64);
    }
}
