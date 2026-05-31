//! Host↔sidecar IPC for the split-process runtime-server dev server.
//!
//! ## Architecture
//!
//! The runtime-server dev host used to be a single binary that statically linked
//! the user's crate (`docs`) alongside the WebSocket server, mDNS
//! advertise, and the file-watch + rebuild loop. To load new user
//! code after an edit, the entire process had to `execve` itself.
//! Every reload tore down the TCP listening socket, which forced each
//! connected client (Android, iOS) to reconnect and replay a full
//! snapshot — incurring ~500ms of dead time per save.
//!
//! The split moves the user code into a *sidecar* child process. The
//! parent ("host") keeps WS listeners, mDNS, and the watcher alive
//! across rebuilds; only the sidecar restarts. Clients never see a
//! socket close, so the perceived hot-reload latency drops to roughly
//! the build time.
//!
//! ## Wire format on the IPC
//!
//! Each direction is a stream of length-prefixed JSON frames:
//!
//!   ```text
//!   [u32 LE length][JSON-encoded SidecarOut | SidecarIn]
//!   ```
//!
//! JSON is used (not bincode/postcard) because the same enums are
//! re-serialized verbatim onto the client WebSockets, and the WS
//! transport is JSON. One canonical encoding keeps the host's command
//! mirroring trivially zero-cost: bytes off the sidecar pipe become
//! bytes on the client WS without re-encoding.
//!
//! ## Lifecycle
//!
//! `Sidecar::spawn` starts the child and two background threads:
//!
//! - **reader thread**: parses frames off the child's stdout and
//!   invokes `on_message` with each `SidecarOut`.
//! - **writer thread**: drains an outbound channel and writes
//!   `SidecarIn` frames to the child's stdin.
//!
//! `Sidecar::send` is non-blocking; if the writer thread has died
//! (sidecar exited) the send silently drops. `Sidecar::kill` sends
//! SIGKILL and joins the threads. `Sidecar::restart` is the rebuild
//! entry point — kill + spawn under the same handle.

use serde::{Deserialize, Serialize};
use std::io::{Read, Write};
use std::process::{Child, ChildStdin, ChildStdout, Command as ProcCommand, Stdio};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc;
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;
use wire::{AppToDev, Command};

/// Frames going *from* the sidecar *to* the host. The sidecar runs
/// the user's reactive runtime and emits these whenever the walker
/// produces new wire commands — either at startup (initial snapshot)
/// or in response to an inbound `Event`.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "kind", content = "payload")]
pub enum SidecarOut {
    /// First frame the sidecar sends after spawn. Reports the
    /// sidecar's `dlsym("main")` runtime address so the host's
    /// hot-patch builder knows the ASLR slide. Sent before any
    /// session-scoped frame so the host always has a valid value
    /// cached before any file-change-triggered patch build kicks off.
    Hello {
        /// Result of `dlsym(RTLD_DEFAULT, "main")` inside the
        /// freshly-spawned sidecar process.
        aslr_reference: u64,
    },
    /// A batch of newly-produced wire commands for `session`. The host
    /// looks up that session's mirror recorder, appends each command
    /// via `push_external_command`, and broadcasts to connected
    /// clients attached to *that session only*.
    ///
    /// The session id always corresponds to a prior
    /// [`SidecarIn::CreateSession`] from the host. If the sidecar
    /// emits a `Commands` for an id the host never created, the host
    /// drops the batch and logs (likely a race during teardown).
    Commands {
        session: String,
        cmds: Vec<Command>,
    },
    /// Sidecar acknowledges that the session thread has spawned and
    /// finished its initial render. Mostly diagnostic — the host
    /// doesn't block on it.
    SessionReady { session: String },
    /// The session's scene was torn down (typically after a hot-patch
    /// rerender). Host drops the matching mirror's command log + scene
    /// before applying the following `Commands` frame, and broadcasts
    /// a fresh snapshot to every attached client. Always paired with a
    /// `Commands` frame holding the post-rerender state — emitted
    /// strictly *before* those commands so the host knows the next
    /// batch isn't a delta on the old log.
    SessionReset { session: String },
    /// Sidecar reports a session thread has exited (panic / orderly
    /// shutdown). Host removes the session's mirror and disconnects
    /// the clients attached to it (they'll typically reconnect and
    /// land on a fresh session).
    SessionEnded { session: String },
}

/// Frames going *from* the host *to* the sidecar. Carries the
/// per-session lifecycle plus the legacy event/patch payloads.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "kind", content = "payload")]
pub enum SidecarIn {
    /// Tell the sidecar to spin up a new author runtime thread under
    /// `session`. The sidecar spawns a thread, runs `render(app())`
    /// against a per-session `WireRecordingBackend`, and starts
    /// streaming back [`SidecarOut::Commands`] tagged with this
    /// session id. If the sidecar already has a thread under the
    /// same id it drops the message — host-side dedup is the
    /// authoritative gate so this is just a safety net.
    ///
    /// `viewport` carries the client's initial size, extracted from
    /// the client's `AppToDev::Hello`. It's bundled here (rather
    /// than sent as a separate `ViewportChanged` event afterwards)
    /// so the session thread can apply it BEFORE `mount(app)` runs
    /// — that way the user's `app()` (and any `effect!` it
    /// schedules with viewport-dependent math) sees the right size
    /// from frame zero. `None` means the client didn't report one;
    /// the recorder's `frame()` falls back to `None` and author
    /// code uses its hardcoded default.
    CreateSession {
        session: String,
        #[serde(default)]
        viewport: Option<wire::WireViewport>,
    },
    /// Tell the sidecar to shut down the named session's thread. The
    /// thread drops its `Owner` (firing teardown effects) and exits.
    /// The sidecar emits [`SidecarOut::SessionEnded`] once the thread
    /// is joined.
    CloseSession { session: String },
    /// Forward a client→app event to the named session's thread. The
    /// session thread dispatches through its local
    /// `WireRecordingBackend::dispatch_event` (or the equivalent for
    /// `ScreenReleased`, etc.). Events for an unknown session are
    /// dropped + logged.
    Event {
        session: String,
        event: AppToDev,
    },
    /// Install a hot-patch jump table. The host built this from a
    /// freshly-linked patch dylib; the sidecar dlopens it via
    /// `dev_hot::apply_patch` and any subsequent component
    /// dispatch lands in the patched body. The dylib is loaded once
    /// process-wide; every running session thread is then told to
    /// tear down its `Owner` and re-render so it picks up the new
    /// component bodies. JumpTable's PathBuf must be readable from
    /// the sidecar's filesystem (typically somewhere under
    /// `target/idealyst/.../patches/`).
    ApplyPatch {
        /// Serialized as JSON so the IPC frame stays a single
        /// `serde_json::to_vec` round-trip. The sidecar parses
        /// this back into `subsecond_types::JumpTable` and feeds
        /// it to `dev_hot::apply_patch`.
        table_json: String,
    },
}

/// Handle to a running sidecar. Owns the child process, the writer
/// channel, and the join handles for the I/O threads.
///
/// Inbound `SidecarOut` frames are buffered on the held `inbound_rx`
/// channel. The owner is expected to drain that channel from a thread
/// that owns the `WireRecordingBackend` (which is `!Send` due to its
/// `Rc<RefCell<…>>` storage) and forward the commands via
/// `WireRecordingBackend::push_external_command`. The split-process
/// server's main tick loop does exactly that.
pub struct Sidecar {
    /// `None` for test sidecars constructed via
    /// [`Self::for_test_with_channels`] — these don't own a real
    /// subprocess, just a pair of mpsc channels the test drives
    /// directly. The Drop / kill path branches on this.
    child: Option<Child>,
    /// Outbound channel: host pushes `SidecarIn`, writer thread
    /// drains it onto the child's stdin. Held as `Option` so
    /// [`Self::kill`] can `take()` and drop it — the writer thread
    /// blocks on `recv()` and only exits once every `Sender` to the
    /// channel is gone, so dropping this before `join()` is what
    /// lets `kill()` actually return.
    outbound_tx: Option<mpsc::Sender<SidecarIn>>,
    /// Inbound channel: reader thread pushes `SidecarOut` frames,
    /// owning thread drains via [`Self::drain_inbound`].
    inbound_rx: Mutex<mpsc::Receiver<SidecarOut>>,
    /// Sidecar's runtime `dlsym("main")` address. Populated when
    /// the host drains a [`SidecarOut::Hello`] frame; the
    /// hot-patch builder reads it to compute the ASLR slide for
    /// each patch. Zero until the first Hello arrives.
    aslr_reference: AtomicU64,
    reader_thread: Option<JoinHandle<()>>,
    writer_thread: Option<JoinHandle<()>>,
}

impl Sidecar {
    /// Spawn the sidecar binary and wire up I/O threads. Frames the
    /// child emits arrive on the internal inbound channel — the
    /// caller pumps them out via [`Self::drain_inbound`] on the
    /// thread that owns the recorder.
    pub fn spawn(program: &std::path::Path) -> std::io::Result<Self> {
        let mut child = ProcCommand::new(program)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::inherit())
            .spawn()?;
        let stdin = child.stdin.take().expect("sidecar stdin captured");
        let stdout = child.stdout.take().expect("sidecar stdout captured");

        let (outbound_tx, outbound_rx) = mpsc::channel::<SidecarIn>();
        let (inbound_tx, inbound_rx) = mpsc::channel::<SidecarOut>();

        let reader_thread = spawn_reader_thread(stdout, inbound_tx);
        let writer_thread = spawn_writer_thread(stdin, outbound_rx);

        Ok(Self {
            child: Some(child),
            outbound_tx: Some(outbound_tx),
            inbound_rx: Mutex::new(inbound_rx),
            aslr_reference: AtomicU64::new(0),
            reader_thread: Some(reader_thread),
            writer_thread: Some(writer_thread),
        })
    }

    /// Test-only constructor: build a `Sidecar` whose IPC is plain
    /// mpsc channels instead of a real child process's stdin/stdout.
    /// The returned `(sidecar, fake_in_rx, fake_out_tx)` lets a test
    /// drive both directions:
    ///
    /// - `fake_in_rx` receives every `SidecarIn` the host sends.
    /// - `fake_out_tx` is what the test uses to emit `SidecarOut`
    ///   frames *as if* they came from the real sidecar.
    ///
    /// Used in dev-server integration tests to verify per-session
    /// routing without compiling and spawning the generated sidecar
    /// binary.
    #[doc(hidden)]
    pub fn for_test_with_channels() -> (
        Self,
        mpsc::Receiver<SidecarIn>,
        mpsc::Sender<SidecarOut>,
    ) {
        let (outbound_tx, fake_in_rx) = mpsc::channel::<SidecarIn>();
        let (fake_out_tx, inbound_rx) = mpsc::channel::<SidecarOut>();
        let sidecar = Self {
            child: None,
            outbound_tx: Some(outbound_tx),
            inbound_rx: Mutex::new(inbound_rx),
            aslr_reference: AtomicU64::new(0),
            reader_thread: None,
            writer_thread: None,
        };
        (sidecar, fake_in_rx, fake_out_tx)
    }

    /// Cached ASLR reference reported by the sidecar's `Hello`
    /// frame. Returns 0 until the sidecar has sent its first
    /// frame; callers should treat 0 as "not yet ready".
    pub fn aslr_reference(&self) -> u64 {
        self.aslr_reference.load(Ordering::Relaxed)
    }

    /// Update the cached ASLR reference. Called from the host's
    /// inbound drain loop when a `Hello` frame arrives.
    pub fn set_aslr_reference(&self, addr: u64) {
        self.aslr_reference.store(addr, Ordering::Relaxed);
    }

    /// Queue an outbound frame. Returns immediately; delivery is best
    /// effort. If the sidecar has exited and the writer thread is
    /// gone, the send is dropped silently — the next rebuild will
    /// spawn a fresh sidecar and the event would have been stale
    /// anyway.
    pub fn send(&self, msg: SidecarIn) {
        if let Some(tx) = self.outbound_tx.as_ref() {
            let _ = tx.send(msg);
        }
    }


    /// Pull every `SidecarOut` frame that's arrived since the last
    /// call. Non-blocking; returns an empty vec if nothing is
    /// pending.
    pub fn drain_inbound(&self) -> Vec<SidecarOut> {
        let mut out = Vec::new();
        if let Ok(rx) = self.inbound_rx.lock() {
            while let Ok(msg) = rx.try_recv() {
                out.push(msg);
            }
        }
        out
    }

    /// Kill the child process. Joins the I/O threads (they exit
    /// once their respective pipe ends close + the outbound channel
    /// senders drop). Safe to call once.
    pub fn kill(&mut self) {
        // `kill` is idempotent on the child handle but errors if the
        // process already exited — ignore that case. Test-mode
        // sidecars have no child, so skip the signal.
        if let Some(child) = self.child.as_mut() {
            let _ = child.kill();
            let _ = child.wait();
        }
        // Drop the outbound `Sender` *before* joining the writer
        // thread: the writer thread is parked in `recv()`, and the
        // mpsc receiver only returns `Err` (i.e. the recv loop
        // exits) once every `Sender` to the channel is dropped.
        // Without this drop, `kill()` blocks the watcher thread
        // forever on `writer_thread.join()`. The reader thread is
        // not symmetrically affected — it parks in `read_frame` and
        // unblocks as soon as the child's stdout closes from the
        // SIGKILL above.
        self.outbound_tx.take();
        if let Some(t) = self.reader_thread.take() {
            let _ = t.join();
        }
        if let Some(t) = self.writer_thread.take() {
            let _ = t.join();
        }
    }

    /// Best-effort: synchronously dispatch every pending outbound
    /// `SidecarIn` to whichever channel the writer thread is draining
    /// onto, used in tests to wait for a `send()` to be visible on the
    /// fake receiver before asserting on it. For real sidecars this is
    /// a no-op (the writer thread does the work).
    #[cfg(test)]
    pub fn flush_for_test(&self) {
        // mpsc sends are already synchronous to the bounded queue; the
        // test side reads off the fake receiver directly. No work
        // needed — this is here as a hook in case we move to a buffered
        // transport later.
    }
}

impl Drop for Sidecar {
    fn drop(&mut self) {
        self.kill();
    }
}

/// A shared `Sidecar` slot. Holds the current sidecar (if any) behind
/// a `Mutex` so the rebuild thread and the event-forwarding path can
/// share access. `None` while a swap is in progress or before the
/// first spawn.
pub type SidecarSlot = Arc<Mutex<Option<Sidecar>>>;

/// Send + Sync mirror of the live session-id set. The serve loop
/// (single-threaded, owns the per-session
/// `WireRecordingBackend` mirrors) inserts on `accept_new` and
/// removes on `SessionEnded`. The watcher/respawn thread reads this
/// after spawning a fresh sidecar so it can replay `CreateSession`
/// for every active session — keeping the new sidecar in sync with
/// what the host believes is live.
///
/// Without this, a hot-patch fallback that takes the respawn ladder
/// would leave every existing session orphaned on the host (mirror
/// alive, but no author runtime emitting commands).
#[derive(Clone, Default)]
pub struct SessionTracker {
    /// `String → last-known viewport`. The viewport is updated
    /// whenever the host sees an `AppToDev::Hello` or `ViewportChanged`
    /// for that session; `replay_sessions_to_sidecar` reads it so a
    /// hot-patch respawn replays `CreateSession { viewport }` with
    /// the correct size, instead of falling back to None and making
    /// the next raf tick anchor at the welcome's hardcoded 393×800.
    inner: Arc<Mutex<std::collections::HashMap<String, Option<wire::WireViewport>>>>,
}

impl SessionTracker {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn insert(&self, id: &str) {
        if let Ok(mut g) = self.inner.lock() {
            g.entry(id.to_string()).or_insert(None);
        }
    }

    pub fn remove(&self, id: &str) {
        if let Ok(mut g) = self.inner.lock() {
            g.remove(id);
        }
    }

    /// Record / update the last-known viewport for `id`. Idempotent
    /// when called repeatedly with the same value.
    pub fn set_viewport(&self, id: &str, viewport: Option<wire::WireViewport>) {
        if let Ok(mut g) = self.inner.lock() {
            g.insert(id.to_string(), viewport);
        }
    }

    /// Snapshot of the current session set as `(id, viewport)` pairs.
    /// Used after sidecar respawn to replay `CreateSession` for each
    /// known session, with the viewport the client last reported.
    pub fn snapshot(&self) -> Vec<(String, Option<wire::WireViewport>)> {
        self.inner
            .lock()
            .map(|g| g.iter().map(|(k, v)| (k.clone(), *v)).collect())
            .unwrap_or_default()
    }
}

// ---------------------------------------------------------------------------
// I/O threads
// ---------------------------------------------------------------------------

fn spawn_reader_thread(
    mut stdout: ChildStdout,
    inbound_tx: mpsc::Sender<SidecarOut>,
) -> JoinHandle<()> {
    std::thread::spawn(move || loop {
        let frame = match read_frame::<SidecarOut, _>(&mut stdout) {
            Ok(f) => f,
            Err(e) if is_eof(&e) => {
                eprintln!("[host] sidecar stdout closed");
                break;
            }
            Err(e) => {
                eprintln!("[host] sidecar frame read error: {e}");
                break;
            }
        };
        if inbound_tx.send(frame).is_err() {
            // Receiver dropped — host is tearing the sidecar down.
            break;
        }
    })
}

fn spawn_writer_thread(
    mut stdin: ChildStdin,
    outbound_rx: mpsc::Receiver<SidecarIn>,
) -> JoinHandle<()> {
    std::thread::spawn(move || {
        while let Ok(msg) = outbound_rx.recv() {
            if let Err(e) = write_frame(&mut stdin, &msg) {
                eprintln!("[host] sidecar frame write error: {e}");
                break;
            }
        }
    })
}

// ---------------------------------------------------------------------------
// Frame I/O — re-exported so the sidecar's generated main can use the
// same helpers for the other end of the pipe.
// ---------------------------------------------------------------------------

/// Read one length-prefixed JSON frame. Returns `Err(UnexpectedEof)`
/// when the peer closes the pipe cleanly between frames — callers
/// detect that via [`is_eof`] and treat it as a clean exit.
pub fn read_frame<T, R>(reader: &mut R) -> std::io::Result<T>
where
    T: serde::de::DeserializeOwned,
    R: Read,
{
    let mut len_bytes = [0u8; 4];
    reader.read_exact(&mut len_bytes)?;
    let len = u32::from_le_bytes(len_bytes) as usize;
    let mut buf = vec![0u8; len];
    reader.read_exact(&mut buf)?;
    serde_json::from_slice(&buf).map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))
}

/// Write one length-prefixed JSON frame.
pub fn write_frame<T, W>(writer: &mut W, value: &T) -> std::io::Result<()>
where
    T: serde::Serialize,
    W: Write,
{
    let bytes = serde_json::to_vec(value)
        .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))?;
    let len = bytes.len() as u32;
    writer.write_all(&len.to_le_bytes())?;
    writer.write_all(&bytes)?;
    writer.flush()?;
    Ok(())
}

/// Returns true when the error indicates the peer closed the pipe
/// without sending a partial frame. Used by I/O loops to distinguish
/// "clean exit" from "transport-level failure".
pub fn is_eof(e: &std::io::Error) -> bool {
    matches!(
        e.kind(),
        std::io::ErrorKind::UnexpectedEof | std::io::ErrorKind::BrokenPipe
    )
}

// ---------------------------------------------------------------------------
// In-process sidecar runtime
// ---------------------------------------------------------------------------
//
// Compiled into the runtime-server *sidecar* binary — the worker that statically
// links the user's crate and runs N independent author runtimes
// (one per dev-host session) on dedicated threads. Pre-refactor this
// code was inlined into the build-orchestrator's `format!` template
// as ~310 lines of generated Rust; that meant every internal change
// to `SidecarIn`/`SidecarOut` shape immediately broke any project
// whose pinned framework rev predated the template change.
//
// Now the sidecar wrapper is a 4-line `fn main() { run(my_crate::app) }`.
// Internal refactors stop at this crate's boundary.

#[cfg(feature = "runtime-server")]
pub use runtime::run;

#[cfg(feature = "runtime-server")]
mod runtime {
    use super::{is_eof, read_frame, write_frame, SidecarIn, SidecarOut};
    use crate::WireRecordingBackend;
    use runtime_core::{mount, Owner, Element};
    use std::cell::RefCell;
    use std::collections::HashMap;
    use std::io::{stdin, stdout, BufReader, Write};
    use std::rc::Rc;
    use std::sync::mpsc;
    use std::sync::{Arc, Mutex};
    use std::thread::JoinHandle;

    /// Per-session control message dispatched from the sidecar's
    /// main thread into the session's owned thread. Each thread
    /// blocks on `recv()`; the main thread routes by session id.
    enum SessionMsg {
        /// Forward an app→dev event into this session's recorder.
        Event(wire::AppToDev),
        /// Hot-patch has been applied process-wide. Tear down this
        /// session's `Owner`, reset its scene log, and re-render to
        /// pick up patched component bodies.
        Rerender,
        /// Graceful shutdown — the host has closed the session. The
        /// thread drops its `Owner` (firing any teardown effects)
        /// and exits.
        Shutdown,
    }

    struct SessionHandle {
        tx: mpsc::Sender<SessionMsg>,
        join: JoinHandle<()>,
    }

    /// Entry point for the runtime-server sidecar process.
    ///
    /// Blocks until the host closes the stdin pipe (typically when the
    /// host process exits or SIGKILLs the sidecar). Spawns and joins
    /// one author-runtime thread per `SidecarIn::CreateSession` frame;
    /// fans `ApplyPatch` frames out to every live session so each one
    /// re-renders against the freshly-patched component bodies.
    ///
    /// `app` is the user crate's root constructor — exactly what the
    /// generated wrapper's `use {lib}::app;` referred to before this
    /// refactor. Passed as a function pointer so it's `Send + Sync +
    /// Copy + 'static` without any user-side adaptation.
    pub fn run(app: fn() -> Element) -> std::io::Result<()> {
        // Install a SIGSEGV/SIGBUS handler so silent dylib-call
        // crashes (from a hot-patched function jumping to a bad
        // address) print the faulting address before the process
        // dies. Without this the runtime-server log just stops mid-flow and
        // the user can't tell what blew up.
        crate::crash_handler::install();

        // Install the sidecar's `runtime_core::scheduling::Scheduler`
        // impl so author code using `raf_loop_scoped` / `after_ms` /
        // `after_animation_frame` actually fires. Without this, the
        // welcome example's planet orbits (and any other raf-driven
        // custom math) silently no-op because `raf_loop` returns an
        // inert handle. Process-global install; each session thread
        // stashes its registered closures in its own thread-local.
        crate::scheduler::install();

        // Report our `main` runtime address before anything else. The
        // host uses this to compute the ASLR slide for the symbol-
        // table diff in hot-patch builds. Doing it first keeps the
        // host's hot-patch builder usable from the very first
        // file-change event.
        let main_addr: u64 = unsafe {
            libc::dlsym(libc::RTLD_DEFAULT, b"main\0".as_ptr() as *const _) as u64
        };

        // Outbound stdout is shared across all session threads. A
        // `Mutex` is the simplest way to serialize length-prefixed
        // JSON frames without a dedicated writer thread — frame
        // writes are infrequent (per-event or per-tick) so contention
        // is minimal.
        let out = Arc::new(Mutex::new(stdout()));

        {
            let mut o = out.lock().expect("stdout lock");
            write_frame(
                &mut *o,
                &SidecarOut::Hello {
                    aslr_reference: main_addr,
                },
            )?;
            let _ = o.flush();
        }

        let mut sessions: HashMap<String, SessionHandle> = HashMap::new();

        let mut input = BufReader::new(stdin());
        loop {
            let msg: SidecarIn = match read_frame(&mut input) {
                Ok(f) => f,
                Err(e) if is_eof(&e) => {
                    eprintln!("[runtime-server-app] host pipe closed; exiting");
                    break;
                }
                Err(e) => {
                    eprintln!("[runtime-server-app] frame read error: {e} — exiting");
                    return Err(e);
                }
            };

            match msg {
                SidecarIn::CreateSession { session, viewport } => {
                    if sessions.contains_key(&session) {
                        eprintln!(
                            "[runtime-server-app] CreateSession({session}): already exists; ignoring"
                        );
                        continue;
                    }
                    let (tx, rx) = mpsc::channel::<SessionMsg>();
                    let out_clone = out.clone();
                    let session_for_thread = session.clone();
                    // 16 MB stack. The default 2 MB is too small for
                    // hot-patched welcome: the patched component bodies
                    // include large compiler-generated stack frames
                    // (welcome's `vignette` alone allocates ~38 KB),
                    // and `mount` → `app()` → component-via-subsecond-
                    // dispatch chains nest deep enough that the
                    // default stack overflows on the first re-render
                    // after a patch lands. Symptom is a SIGBUS at
                    // the *first instruction* of a small leaf
                    // function (typically `Cloned::next_unchecked`'s
                    // `stp x29, x30, [sp, #-0x10]!`) with sp pointing
                    // outside the mapped stack region — classic
                    // stack-overflow signature on macOS aarch64.
                    let join = std::thread::Builder::new()
                        .name(format!("aas-session-{session}"))
                        .stack_size(16 * 1024 * 1024)
                        .spawn(move || {
                            run_session_thread(
                                session_for_thread,
                                rx,
                                out_clone,
                                app,
                                viewport,
                            );
                        })
                        .expect("spawn session thread");
                    sessions.insert(session.clone(), SessionHandle { tx, join });
                    let mut o = out.lock().expect("stdout lock");
                    write_frame(
                        &mut *o,
                        &SidecarOut::SessionReady {
                            session: session.clone(),
                        },
                    )?;
                    let _ = o.flush();
                }
                SidecarIn::CloseSession { session } => {
                    let Some(handle) = sessions.remove(&session) else {
                        eprintln!("[runtime-server-app] CloseSession({session}): no such session");
                        continue;
                    };
                    let _ = handle.tx.send(SessionMsg::Shutdown);
                    drop(handle.tx);
                    if let Err(e) = handle.join.join() {
                        eprintln!("[runtime-server-app] session thread panicked: {:?}", e);
                    }
                    let mut o = out.lock().expect("stdout lock");
                    write_frame(&mut *o, &SidecarOut::SessionEnded { session })?;
                    let _ = o.flush();
                }
                SidecarIn::Event { session, event } => {
                    let Some(handle) = sessions.get(&session) else {
                        eprintln!(
                            "[runtime-server-app] Event for unknown session {session:?}; dropping"
                        );
                        continue;
                    };
                    if handle.tx.send(SessionMsg::Event(event)).is_err() {
                        eprintln!(
                            "[runtime-server-app] session {session:?} channel closed; pruning"
                        );
                        sessions.remove(&session);
                    }
                }
                SidecarIn::ApplyPatch { table_json } => {
                    match serde_json::from_str::<subsecond_types::JumpTable>(&table_json) {
                        Ok(table) => {
                            eprintln!(
                                "[runtime-server-app] applying patch ({} jump-table entries)",
                                table.map.len(),
                            );
                            match unsafe { dev_hot::apply_patch(table) } {
                                Ok(()) => {
                                    if sessions.is_empty() {
                                        // No clients connected → the patch is loaded
                                        // into the sidecar's address space but no
                                        // session thread will exercise the new code.
                                        // Visible symptom: the user's browser tab
                                        // (if any) keeps showing the OLD code's
                                        // output because there's nothing to drive a
                                        // re-render. Most common cause: the browser
                                        // tab is from a previous `idealyst dev --aas`
                                        // and never reconnected to this sidecar —
                                        // refreshing it should mint a new session
                                        // and pick up the patch immediately.
                                        eprintln!(
                                            "[runtime-server-app] patch applied but NO CLIENTS connected — \
                                             refresh your browser tab (Cmd+Shift+R) to mint a \
                                             new session and see the patched code"
                                        );
                                    } else {
                                        eprintln!(
                                            "[runtime-server-app] patch applied; notifying {} session(s) to re-render",
                                            sessions.len(),
                                        );
                                    }
                                    for (id, handle) in &sessions {
                                        if handle.tx.send(SessionMsg::Rerender).is_err() {
                                            eprintln!(
                                                "[runtime-server-app] session {id} unreachable during rerender fan-out"
                                            );
                                        }
                                    }
                                }
                                Err(e) => {
                                    eprintln!("[runtime-server-app] apply_patch failed: {e:?}");
                                }
                            }
                        }
                        Err(e) => {
                            eprintln!("[runtime-server-app] failed to parse JumpTable JSON: {e}");
                        }
                    }
                }
            }
        }

        // Best-effort shutdown of any sessions still running when the
        // host closes its pipe.
        for (_, handle) in sessions.drain() {
            let _ = handle.tx.send(SessionMsg::Shutdown);
            drop(handle.tx);
            let _ = handle.join.join();
        }

        Ok(())
    }

    /// Per-session worker. Owns its own `WireRecordingBackend` +
    /// `Owner`; drains `SessionMsg`s from the main thread's router.
    /// Every emitted command goes onto stdout tagged with this
    /// session's id.
    fn run_session_thread(
        session: String,
        rx: mpsc::Receiver<SessionMsg>,
        out: Arc<Mutex<std::io::Stdout>>,
        app: fn() -> Element,
        initial_viewport: Option<wire::WireViewport>,
    ) {
        let recorder = WireRecordingBackend::new();
        let backend_rc = Rc::new(RefCell::new(recorder.clone()));
        // Plant the viewport BEFORE `mount` runs. The user's `app()`
        // executes inside `mount`'s root scope and may immediately
        // schedule effects/timers that read `page_ref.with(|h|
        // h.frame())`; without setting the viewport first those
        // first reads see `None` and fall through to the welcome's
        // hardcoded 393×800 fallback. The matching `ViewportChanged`
        // event still updates it on subsequent resizes — see
        // `dispatch_app_to_dev`.
        if let Some(v) = initial_viewport {
            crate::set_session_viewport(v.width, v.height);
        }
        // `dev_hot::with_retry` wraps the mount in subsecond's
        // catch-unwind / auto-retry loop. This is the idiomatic
        // Dioxus pattern: when patched code reached via
        // `dev_hot::call(__Component_hot_impl, ...)` makes a
        // call against a stale function pointer, subsecond raises
        // `HotFnPanic`; without this outer `with_retry` boundary,
        // that panic kills the session thread, the host's IPC
        // channel never sees a clean error, and the user sees a
        // silently-frozen UI after the first hot-patch. With the
        // boundary, the call retries against the fresh jump table.
        // `dev_hot::with_retry` wraps the mount in subsecond's
        // catch-unwind / auto-retry loop. This is the idiomatic
        // Dioxus pattern: when patched code reached via
        // `dev_hot::call(__Component_hot_impl, ...)` makes a
        // call against a stale function pointer, subsecond raises
        // `HotFnPanic`; without this outer `with_retry` boundary,
        // that panic kills the session thread, the host's IPC
        // channel never sees a clean error, and the user sees a
        // silently-frozen UI after the first hot-patch. With the
        // boundary, the call retries against the fresh jump table.
        let backend_for_mount = backend_rc.clone();
        let mut owner: Option<Owner> = Some(dev_hot::with_retry(|| {
            mount(backend_for_mount.clone(), app)
        }));

        // Register the headless `"screenshot"` Robot-bridge verb for
        // this session. `mount` above started the auto-polling Robot
        // bridge on THIS thread (the `robot` feature is on via the
        // sidecar's `runtime-core/dev`); the custom-verb registry is
        // thread-local, so registration must happen here, on the same
        // thread. The handler snapshots this session's recorder and
        // rasterizes it via the headless wgpu renderer — letting Robot
        // / the MCP server screenshot the mocked app on demand.
        #[cfg(feature = "screenshot")]
        {
            let snap_recorder = recorder.clone();
            let size = initial_viewport
                .map(|v| (v.width.round() as u32, v.height.round() as u32))
                .unwrap_or((393, 800));
            headless_screenshot::register_screenshot_command(size, move || snap_recorder.snapshot());
        }

        let mut cursor = recorder.command_count();

        // Ship the initial render's snapshot up to the host.
        let initial = recorder.snapshot();
        if !initial.is_empty() {
            if let Ok(mut o) = out.lock() {
                let _ = write_frame(
                    &mut *o,
                    &SidecarOut::Commands {
                        session: session.clone(),
                        cmds: initial,
                    },
                );
                let _ = o.flush();
            }
        }

        // Animation cadence is **client-driven**: the client's native
        // raf fires `AppToDev::RequestFrame { dt_ms }`, which arrives
        // as a `SessionMsg::Event(AppToDev::RequestFrame ...)` and
        // routes through `dispatch_app_to_dev` → `recorder.tick_animations`.
        // No sidecar-self-paced timer is needed; the session thread
        // blocks on `recv()` between client requests, idling at zero
        // CPU when no client is asking for frames.
        while let Ok(msg) = rx.recv() {
            match msg {
                SessionMsg::Event(app_to_dev) => {
                    dispatch_app_to_dev(&recorder, app_to_dev);
                }
                SessionMsg::Rerender => {
                    // Timing instrumentation — measure where time
                    // actually goes during a hot-patch rerender so we
                    // can target the real bottleneck. Per-step
                    // elapsed reported in microseconds.
                    let t_total = std::time::Instant::now();
                    let t_drop = std::time::Instant::now();
                    let drop_result = std::panic::catch_unwind(
                        std::panic::AssertUnwindSafe(|| drop(owner.take())),
                    );
                    let drop_us = t_drop.elapsed().as_micros();
                    if let Err(e) = drop_result {
                        let msg = panic_payload_to_string(&e);
                        eprintln!(
                            "[runtime-server-app] {session}: PANIC during old-owner drop: {msg}"
                        );
                        return;
                    }
                    let t_reset = std::time::Instant::now();
                    recorder.reset_log_and_scene();
                    let reset_us = t_reset.elapsed().as_micros();
                    let t_mount = std::time::Instant::now();
                    let backend_for_mount = backend_rc.clone();
                    let mount_result = std::panic::catch_unwind(
                        std::panic::AssertUnwindSafe(|| {
                            dev_hot::with_retry(|| {
                                mount(backend_for_mount.clone(), app)
                            })
                        }),
                    );
                    let mount_us = t_mount.elapsed().as_micros();
                    let new_owner = match mount_result {
                        Ok(o) => o,
                        Err(e) => {
                            let msg = panic_payload_to_string(&e);
                            eprintln!(
                                "[runtime-server-app] {session}: PANIC during patched mount(app): {msg}"
                            );
                            return;
                        }
                    };
                    owner = Some(new_owner);
                    cursor = 0;
                    let cmd_count = recorder.command_count();
                    eprintln!(
                        "[runtime-server-app] {session}: rerender total={}us (drop={}us reset={}us mount={}us cmds={})",
                        t_total.elapsed().as_micros(), drop_us, reset_us, mount_us, cmd_count
                    );
                    if let Ok(mut o) = out.lock() {
                        let _ = write_frame(
                            &mut *o,
                            &SidecarOut::SessionReset {
                                session: session.clone(),
                            },
                        );
                        let _ = o.flush();
                    }
                }
                SessionMsg::Shutdown => {
                    eprintln!("[runtime-server-app] session {session} shutting down");
                    drop(owner);
                    return;
                }
            }

            let count_now = recorder.command_count();
            if count_now > cursor {
                let new_cmds = recorder.commands_since(cursor);
                cursor = count_now;
                if let Ok(mut o) = out.lock() {
                    let _ = write_frame(
                        &mut *o,
                        &SidecarOut::Commands {
                            session: session.clone(),
                            cmds: new_cmds,
                        },
                    );
                    let _ = o.flush();
                }
            }
        }
        drop(owner);
    }

    /// Mirror of the legacy `handle_app_msg` in
    /// `dev-server::transport`. The split moves this logic into the
    /// sidecar because the recorder here is the one with registered
    /// handler closures — the host's recorder is purely a transport
    /// mirror.
    /// Extract a printable message from a `catch_unwind` payload.
    /// Most Rust panics carry a `&'static str` or `String`; anything
    /// else gets a generic placeholder.
    fn panic_payload_to_string(e: &Box<dyn std::any::Any + Send>) -> String {
        if let Some(s) = e.downcast_ref::<&'static str>() {
            (*s).to_string()
        } else if let Some(s) = e.downcast_ref::<String>() {
            s.clone()
        } else {
            "<non-string panic payload>".to_string()
        }
    }

    fn dispatch_app_to_dev(recorder: &WireRecordingBackend, msg: wire::AppToDev) {
        use wire::AppToDev::*;
        match msg {
            Hello { viewport, .. } => {
                // Capture the client's initial viewport so the first
                // raf tick's planet-orbit math sees the right size.
                // Without this, welcome's `page_ref.with(|h| h.frame())`
                // returns None and the orbits anchor at the
                // hardcoded 393×800 fallback.
                if let Some(v) = viewport {
                    crate::set_session_viewport(v.width, v.height);
                }
            }
            ViewportChanged { width, height } => {
                crate::set_session_viewport(width, height);
            }
            Event { handler, args } => {
                let _ = recorder.dispatch_event(handler, args);
            }
            StateChanged { node, bit, on } => {
                let _ = recorder.dispatch_state(node, bit, on);
            }
            ColorSchemeChanged { scheme: _ } => {}
            ScreenReleased { scope } => {
                recorder.handle_screen_released(scope.0);
            }
            NavigatorDepthChanged { .. } => {}
            DrawerStateChanged { navigator, is_open } => {
                recorder.handle_drawer_state_changed(navigator, is_open);
            }
            TabSelected { navigator, index } => {
                recorder.handle_tab_selected(navigator, index);
            }
            VirtualizerMountItem { .. }
            | VirtualizerReleaseItem { .. }
            | VirtualizerMeasuredSize { .. } => {}
            RequestFrame { dt_ms } => {
                // Client-driven animation tick. Convert ms → Duration
                // and ask the recorder to advance its thread-local
                // animation clock. Any registered `AnimatedValue`
                // tick closures fire and produce `SetAnimated*`
                // commands on the recorder, which flush back to the
                // client via the session's normal command-drain at
                // the end of this iteration.
                //
                // Wrap in `catch_unwind` because a patched raf closure
                // (welcome's `coordinator::use_welcome` is the canonical
                // example) can carry a stale `HotFnPanic` from a child
                // `subsecond::call` against a pre-patch symbol. Without
                // the catch, the panic unwinds the whole session
                // thread, channel closes, host can't recover the
                // session — see subsecond's docs: "stale call sites
                // emit a safe panic that is automatically caught and
                // retried by the next call instance up the callstack."
                // We're that "next call up" here.
                let dt = std::time::Duration::from_millis(dt_ms as u64);
                if let Err(e) = std::panic::catch_unwind(
                    std::panic::AssertUnwindSafe(|| {
                        dev_hot::with_retry(|| recorder.tick_animations(dt))
                    }),
                ) {
                    let msg = panic_payload_to_string(&e);
                    eprintln!(
                        "[runtime-server-app] tick_animations panicked: {msg} — likely a stale \
                         hot-patch call site that didn't get caught by the inner \
                         `with_retry`; will retry on next RequestFrame"
                    );
                }
            }
            Error { message } => {
                eprintln!("[runtime-server-app] client reported error: {message}");
            }
        }
    }
}
