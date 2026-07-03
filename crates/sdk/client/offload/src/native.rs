//! Native backend: run the job on a `std::thread` and deliver the result back
//! through a oneshot channel.
//!
//! There is no Web Worker off-web, so [`run`] simply spawns a thread, calls the
//! job's function pointer there, and resolves the awaited future when the thread
//! sends its result. (A thread *pool* is a later optimization; one thread per
//! call is fine for the fallback + tests.)

use crate::OffloadError;

/// A typed handle to an offload job — its name (for debugging/parity with the
/// web backend) and the function pointer to invoke. Built by the
/// [`handle!`](crate::handle) macro; mirrors `wasmworker::func::WebWorkerFn` so
/// the call site is identical across platforms.
pub struct Handle<T, R> {
    name: &'static str,
    f: fn(T) -> R,
}

impl<T, R> Handle<T, R> {
    /// Construct a handle from a job's name + function pointer. Prefer the
    /// [`handle!`](crate::handle) macro, which fills in the name via `stringify!`.
    #[doc(hidden)]
    pub fn new_unchecked(name: &'static str, f: fn(T) -> R) -> Self {
        Self { name, f }
    }

    /// The job's source name (the identifier passed to `handle!`).
    pub fn name(&self) -> &'static str {
        self.name
    }
}

impl<T, R> Clone for Handle<T, R> {
    fn clone(&self) -> Self {
        Self { name: self.name, f: self.f }
    }
}
impl<T, R> Copy for Handle<T, R> {}

/// Build a typed [`Handle`] from a `#[offload::job]` function. Mirrors
/// `wasmworker::webworker!` so `offload::handle!(my_job)` works on every target.
#[macro_export]
macro_rules! handle {
    ($f:path) => {
        $crate::Handle::new_unchecked(::core::stringify!($f), $f)
    };
}

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
}
