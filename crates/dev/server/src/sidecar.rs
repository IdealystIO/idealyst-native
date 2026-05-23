//! Host↔sidecar IPC for the split-process AAS dev server.
//!
//! ## Architecture
//!
//! The AAS dev host used to be a single binary that statically linked
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
    CreateSession { session: String },
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
    /// `framework_hot::apply_patch` and any subsequent component
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
        /// it to `framework_hot::apply_patch`.
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
    inner: Arc<Mutex<std::collections::HashSet<String>>>,
}

impl SessionTracker {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn insert(&self, id: &str) {
        if let Ok(mut g) = self.inner.lock() {
            g.insert(id.to_string());
        }
    }

    pub fn remove(&self, id: &str) {
        if let Ok(mut g) = self.inner.lock() {
            g.remove(id);
        }
    }

    /// Snapshot of the current session set. Used after sidecar
    /// respawn to replay `CreateSession` for each known session.
    pub fn snapshot(&self) -> Vec<String> {
        self.inner
            .lock()
            .map(|g| g.iter().cloned().collect())
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
