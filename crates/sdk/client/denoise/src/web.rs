//! Web backend (wasm32): no threads, so run the same [`Engine`] inline on the
//! main thread inside the subscribe callback.
//!
//! `AudioStream` subscribe callbacks fire on the single wasm thread, so the
//! engine lives in an `Rc<RefCell<…>>` and `feed` is called directly — no
//! channel, no worker. Functionally identical output to the native path; the
//! only difference is that one DeepFilterNet inference per ~10 ms frame runs on
//! the main thread. If profiling shows jank, the path forward is an
//! AudioWorklet hosting the wasm engine (a larger lift, not in this cut).
//!
//! `start` is `async` to match the native signature (and because the model
//! bytes are typically fetched before this is called); the build itself is
//! synchronous CPU work on the single wasm thread.

use std::cell::RefCell;
use std::rc::Rc;

use media_stream::{AudioFrame, AudioStream, AudioSubscription, AudioWriter};

use crate::engine::{Config, Engine, Weights};
use crate::DenoiseError;

pub(crate) struct Handle {
    // Held so the callback keeps firing; dropped with the output stream.
    _sub: AudioSubscription,
    // The engine outlives `start` via the subscription closure's clone; this
    // field keeps the original alive and pins ownership to the handle.
    _engine: Rc<RefCell<Engine>>,
}

pub(crate) async fn start(
    input: &AudioStream,
    writer: AudioWriter,
    cfg: Config,
    weights: Weights,
) -> Result<Handle, DenoiseError> {
    // Build synchronously (no threads on wasm); model-init errors propagate.
    let engine = Rc::new(RefCell::new(Engine::new(writer, cfg, weights)?));

    let e = engine.clone();
    let sub = input.subscribe(move |f: &AudioFrame| {
        e.borrow_mut().feed(f.samples, f.sample_rate, f.channels);
    });

    Ok(Handle { _sub: sub, _engine: engine })
}
