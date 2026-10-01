//! Serve `spike-guest` as a live bundle: rebuild on every source edit, hand
//! the newest build to whoever asks.
//!
//! ```sh
//! cargo run --release -p stream-spike --bin stream-serve   # port 7878
//! ```
//!
//! `GET /bundle.wasm` answers with the latest successful build and an
//! `X-Bundle-Version` counter. When the newest sources fail to compile it
//! answers 500 with the compiler output, so the app can show the error and
//! keep running what it has.
//!
//! Change detection is an mtime poll (250 ms) over `GUEST_SOURCES`, plus a
//! check on every request — so a refresh right after a save waits for that
//! save's build instead of racing the poller. Builds hold the state lock,
//! which is what makes a request wait.

use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant, SystemTime};

use stream_spike::guest_build::{guest_build_command, guest_wasm_path, GUEST_SOURCES};

struct State {
    crate_dir: PathBuf,
    target_dir: PathBuf,
    /// Newest source mtime the current `wasm`/`error` reflects.
    built_from: Option<SystemTime>,
    wasm: Option<Vec<u8>>,
    version: u64,
    error: Option<String>,
}

fn newest_mtime(path: &Path) -> Option<SystemTime> {
    let meta = std::fs::metadata(path).ok()?;
    let mut newest = meta.modified().ok()?;
    if meta.is_dir() {
        for entry in std::fs::read_dir(path).ok()?.flatten() {
            if let Some(t) = newest_mtime(&entry.path()) {
                newest = newest.max(t);
            }
        }
    }
    Some(newest)
}

impl State {
    fn sources_mtime(&self) -> Option<SystemTime> {
        GUEST_SOURCES.iter().filter_map(|p| newest_mtime(&self.crate_dir.join(p))).max()
    }

    fn build_if_stale(&mut self) {
        let stamp = self.sources_mtime();
        if stamp.is_some() && stamp == self.built_from {
            return;
        }
        eprintln!("[stream-serve] sources changed — building spike-guest…");
        let t = Instant::now();
        let out = guest_build_command("cargo", &self.crate_dir, &self.target_dir, "spike-guest").output();
        self.built_from = stamp;
        match out {
            Ok(out) if out.status.success() => match std::fs::read(guest_wasm_path(&self.target_dir, "spike-guest")) {
                Ok(wasm) => {
                    self.version += 1;
                    eprintln!(
                        "[stream-serve] v{} ready: {} bytes in {:.1}s",
                        self.version,
                        wasm.len(),
                        t.elapsed().as_secs_f64()
                    );
                    self.wasm = Some(wasm);
                    self.error = None;
                }
                Err(e) => self.error = Some(format!("build succeeded but the wasm is unreadable: {e}")),
            },
            Ok(out) => {
                let stderr = String::from_utf8_lossy(&out.stderr);
                // The interesting part of a failed build is its errors, not
                // the "Compiling …" preamble.
                let errors: String = stderr
                    .lines()
                    .skip_while(|l| !l.starts_with("error"))
                    .take(40)
                    .collect::<Vec<_>>()
                    .join("\n");
                eprintln!("[stream-serve] build FAILED (still serving v{}):\n{errors}", self.version);
                self.error = Some(errors);
            }
            Err(e) => self.error = Some(format!("could not run cargo: {e}")),
        }
    }
}

fn respond(stream: &mut TcpStream, state: &Mutex<State>) -> std::io::Result<()> {
    // Read just the request head; the only request is a body-less GET.
    let mut head = Vec::new();
    let mut byte = [0u8; 1];
    while !head.ends_with(b"\r\n\r\n") {
        if stream.read(&mut byte)? == 0 {
            return Ok(());
        }
        head.push(byte[0]);
    }
    let request = String::from_utf8_lossy(&head);
    let path = request.split_whitespace().nth(1).unwrap_or("/");

    let mut s = state.lock().unwrap();
    if path != "/bundle.wasm" {
        return write!(stream, "HTTP/1.1 404 Not Found\r\nContent-Length: 0\r\nConnection: close\r\n\r\n");
    }
    s.build_if_stale();
    match (&s.error, &s.wasm) {
        (Some(err), _) => {
            let body = format!("bundle build failed:\n{err}");
            write!(
                stream,
                "HTTP/1.1 500 Internal Server Error\r\nContent-Type: text/plain\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                body.len()
            )
        }
        (None, Some(wasm)) => {
            write!(
                stream,
                "HTTP/1.1 200 OK\r\nContent-Type: application/wasm\r\nContent-Length: {}\r\nX-Bundle-Version: {}\r\nConnection: close\r\n\r\n",
                wasm.len(),
                s.version
            )?;
            stream.write_all(wasm)
        }
        (None, None) => write!(stream, "HTTP/1.1 503 Service Unavailable\r\nContent-Length: 0\r\nConnection: close\r\n\r\n"),
    }
}

fn main() {
    let port: u16 = std::env::var("STREAM_PORT").ok().and_then(|p| p.parse().ok()).unwrap_or(7878);
    let crate_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    // Its own target dir, so it never contends with the developer's builds.
    let target_dir = crate_dir.join("../../../target/stream-serve");
    let state = Arc::new(Mutex::new(State {
        crate_dir,
        target_dir,
        built_from: None,
        wasm: None,
        version: 0,
        error: None,
    }));
    state.lock().unwrap().build_if_stale();

    let watcher = state.clone();
    std::thread::spawn(move || loop {
        std::thread::sleep(Duration::from_millis(250));
        watcher.lock().unwrap().build_if_stale();
    });

    let listener = TcpListener::bind(("127.0.0.1", port)).unwrap_or_else(|e| panic!("bind 127.0.0.1:{port}: {e}"));
    eprintln!("[stream-serve] serving http://127.0.0.1:{port}/bundle.wasm — edit crates/streaming/spike/guest/src and refresh");
    for stream in listener.incoming() {
        let Ok(mut stream) = stream else { continue };
        let state = state.clone();
        std::thread::spawn(move || {
            if let Err(e) = respond(&mut stream, &state) {
                eprintln!("[stream-serve] request failed: {e}");
            }
        });
    }
}
