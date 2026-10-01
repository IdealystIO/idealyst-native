//! Native backend: run the job on a `std::thread` and deliver the result back
//! through a oneshot channel.
//!
//! There is no Web Worker off-web, so [`run`] simply spawns a thread, calls the
//! job's function pointer there, and resolves the awaited future when the thread
//! sends its result. (A thread *pool* is a later optimization; one thread per
//! call is fine for the fallback + tests.)

use crate::{Handle, OffloadError};

/// Run `handle`'s job with `arg` on a background thread and await the result.
///
/// `T: Clone` because the argument is moved onto the worker thread (mirroring the
/// web backend, which serializes it across the worker boundary).
pub async fn run<T, R>(handle: Handle<T, R>, arg: &T) -> Result<R, OffloadError>
where
    T: Clone + Send + 'static,
    R: Send + 'static,
{
    let input = arg.clone();
    let f = handle.f;
    let (tx, rx) = futures_channel::oneshot::channel();
    std::thread::spawn(move || {
        let out = f(input);
        // Receiver gone (caller's future dropped) → nothing to deliver to.
        let _ = tx.send(out);
    });
    rx.await.map_err(|_| OffloadError::Canceled)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn add_one(x: u64) -> u64 {
        x + 1
    }

    fn thread_id_string(_: ()) -> String {
        format!("{:?}", std::thread::current().id())
    }

    #[test]
    fn runs_job_and_returns_result() {
        let out = pollster::block_on(run(crate::handle!(add_one), &41u64)).unwrap();
        assert_eq!(out, 42);
    }

    #[test]
    fn runs_off_the_calling_thread() {
        let main_id = format!("{:?}", std::thread::current().id());
        let worker_id = pollster::block_on(run(crate::handle!(thread_id_string), &())).unwrap();
        assert_ne!(main_id, worker_id, "the job must run on a different thread");
    }

    /// A job that panics resolves to `Canceled` (its thread drops the
    /// sender) — the same answer the web backend gives — instead of hanging.
    #[test]
    fn a_panicking_job_is_canceled_not_hung() {
        fn boom(_: ()) -> u8 {
            panic!("deliberate panic in an offload job");
        }
        let r = pollster::block_on(run(crate::handle!(boom), &()));
        assert!(matches!(r, Err(OffloadError::Canceled)), "{r:?}");
    }
}
