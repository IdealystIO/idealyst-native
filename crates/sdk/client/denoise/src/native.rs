//! Native backend (macOS / iOS / Android / desktop): run the [`Engine`] on a
//! dedicated processing thread so DeepFilterNet inference never blocks the
//! audio thread.
//!
//! Topology mirrors `media-writer`'s encoder thread: the input subscription
//! (fires on the producer's audio thread) copies each chunk onto an `mpsc`
//! channel; the worker thread owns the [`Engine`] and the [`AudioWriter`] and
//! pumps `recv -> engine.feed`. Dropping the [`Handle`] drops the subscription
//! (closing the channel) and then joins the worker.
//!
//! `DfTract` is **not `Send`**, so the [`Engine`] is built *inside* the worker
//! thread and never crosses a thread boundary. `start` is `async`: it awaits a
//! one-shot carrying the worker's model-init result, so a load failure surfaces
//! to the caller without ever blocking the caller's thread.

use std::sync::mpsc;
use std::thread::JoinHandle;

use futures_channel::oneshot;
use media_stream::{AudioFrame, AudioStream, AudioSubscription, AudioWriter};

use crate::engine::{Config, Engine, Weights};
use crate::DenoiseError;

/// One pushed chunk: owned samples + its format. Owned because the
/// `AudioFrame::samples` slice is borrowed only for the callback's duration.
type Chunk = (Vec<f32>, u32, u16);

pub(crate) struct Handle {
    sub: Option<AudioSubscription>,
    join: Option<JoinHandle<()>>,
}

pub(crate) async fn start(
    input: &AudioStream,
    writer: AudioWriter,
    cfg: Config,
    weights: Weights,
) -> Result<Handle, DenoiseError> {
    let (chunk_tx, chunk_rx) = mpsc::channel::<Chunk>();
    // One-shot: the worker reports whether the model built. Awaited below so
    // init failure propagates to the caller without blocking a thread.
    let (init_tx, init_rx) = oneshot::channel::<Result<(), DenoiseError>>();

    let join = std::thread::Builder::new()
        .name("denoise".into())
        .spawn(move || {
            // Build the (`!Send`) engine here so `DfTract` lives only on this
            // thread. Report the init outcome, then pump chunks until the
            // channel closes (subscription dropped on `Handle` teardown).
            let mut engine = match Engine::new(writer, cfg, weights) {
                Ok(e) => {
                    let _ = init_tx.send(Ok(()));
                    e
                }
                Err(err) => {
                    let _ = init_tx.send(Err(err));
                    return;
                }
            };
            while let Ok((samples, sr, ch)) = chunk_rx.recv() {
                engine.feed(&samples, sr, ch);
            }
        })
        .map_err(|e| DenoiseError::Spawn(e.to_string()))?;

    // Subscribe BEFORE awaiting init so early chunks queue on the channel (the
    // worker drains them once the model is ready) — and so the `&input` borrow
    // isn't held across the await point.
    let sub = input.subscribe(move |f: &AudioFrame| {
        let _ = chunk_tx.send((f.samples.to_vec(), f.sample_rate, f.channels));
    });

    // Await the one-time model build.
    match init_rx.await {
        Ok(Ok(())) => {}
        Ok(Err(err)) => {
            drop(sub);
            let _ = join.join();
            return Err(err);
        }
        Err(_canceled) => {
            drop(sub);
            let _ = join.join();
            return Err(DenoiseError::Spawn("denoise worker exited during init".into()));
        }
    }

    Ok(Handle { sub: Some(sub), join: Some(join) })
}

impl Drop for Handle {
    fn drop(&mut self) {
        // Drop the subscription FIRST: that drops the capture closure (and its
        // `Sender`), closing the channel so the worker's `recv()` returns `Err`
        // and the loop exits. Only then can the join complete — joining before
        // dropping the sender would hang forever.
        self.sub.take();
        if let Some(join) = self.join.take() {
            let _ = join.join();
        }
    }
}
