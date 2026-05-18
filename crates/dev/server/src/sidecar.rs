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
    /// A batch of newly-produced wire commands. The host appends each
    /// to its `WireRecordingBackend` via `push_external_command` and
    /// broadcasts to connected clients.
    Commands(Vec<Command>),
}

/// Frames going *from* the host *to* the sidecar. Today this is just
/// event forwarding — when an app sends `AppToDev::EventOccurred` over
/// the WebSocket, the host needs to deliver that to the sidecar so
/// the registered closure fires and signals update.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "kind", content = "payload")]
pub enum SidecarIn {
    /// Forward a client→app event. The sidecar dispatches it through
    /// its local `WireRecordingBackend::dispatch_event` (or the
    /// equivalent for `ScreenReleased`, etc.).
    Event(AppToDev),
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
    child: Child,
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
            child,
            outbound_tx: Some(outbound_tx),
            inbound_rx: Mutex::new(inbound_rx),
            reader_thread: Some(reader_thread),
            writer_thread: Some(writer_thread),
        })
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
        // process already exited — ignore that case.
        let _ = self.child.kill();
        let _ = self.child.wait();
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
