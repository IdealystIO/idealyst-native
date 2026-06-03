package io.idealyst.runtime

/**
 * `Runnable` that re-polls a cooperative-async-executor task on the main
 * `Looper`. Holds only the task's `Long id`; `run` forwards into Rust via
 * `nativePoll(id)`, which calls the executor's `poll_task(id)` on the main
 * thread.
 *
 * # Why this exists separately from `RustScheduledRunnable`
 *
 * The async executor's `TaskWaker` may fire on ANY thread (a Camera2 /
 * network callback off the main thread). It marshals a re-poll onto the main
 * looper by constructing one of these on the waking thread and calling
 * `Handler.post(this)` — `Handler.post` is thread-safe, so this is the
 * cross-thread → main hop.
 *
 * It can NOT reuse `RustScheduledRunnable`, whose Rust side keys closures in
 * a THREAD-LOCAL registry: a runnable built on the background waker thread
 * would register in that thread's registry, and `nativeInvoke` (running back
 * on main) would look up the id in the MAIN thread's registry and find
 * nothing. This class instead carries only the `id` — a plain value that is
 * meaningful on any thread — and `nativePoll` does the `poll_task(id)` lookup
 * only once it's back on main, inside the executor's main-thread `TASKS`.
 *
 * One-shot: a single `post` runs `doFrame`-style once; the executor re-posts
 * a fresh instance on the next wake. No cancellation path is needed — a task
 * that completed makes `poll_task(id)` a no-op.
 */
class RustAsyncPoll(private val id: Long) : Runnable {
    override fun run() {
        nativePoll(id)
    }

    private external fun nativePoll(id: Long)
}
