//! Serve the spike's bundles live: rebuild on every source edit, hand the
//! newest build to whoever asks.
//!
//! ```sh
//! cargo run --release -p stream-spike --bin stream-serve   # port 7878
//! ```
//!
//! - `GET /bundle.wasm` — `spike-guest` (model A), from `GUEST_SOURCES`.
//! - `GET /remote.wasm` — `spike-remoteguest`, the bridged RemoteCounter,
//!   from `REMOTE_GUEST_SOURCES` (edit `spike/components/src/lib.rs`).
//! - `GET /showcase.wasm` — the showcase's bundle
//!   (`crates/streaming/showcase/app/src/lib.rs`).
//! - `GET /example.wasm` — the single-file example's bundle
//!   (`crates/streaming/example/app/src/main.rs`).
//!
//! Each answers with its latest successful build and an `X-Bundle-Version`
//! counter. When the newest sources fail to compile it
//! answers 500 with the compiler output, so the app can show the error and
//! keep running what it has.
//!
//! Change detection is an mtime poll (250 ms) over each bundle's listed
//! sources and every file its last build compiled (cargo's dep-info), plus a
//! check on every request — so a refresh right after a save waits for that
//! save's build instead of racing the poller. Builds hold the state lock,
//! which is what makes a request wait.

use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant, SystemTime};

use stream_spike::guest_build::{
    bundle_sources, guest_build_command, guest_wasm_path, EXAMPLE_SOURCES, GUEST_SOURCES, REMOTE_GUEST_SOURCES, SHOWCASE_SOURCES,
};

/// One served bundle.
struct State {
    package: &'static str,
    /// The wasm file's name: the package's `[lib] name`.
    artifact: &'static str,
    sources: &'static [&'static str],
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
    /// The listed sources, plus every file the last build compiled (its
    /// dep-info): an edit to a library the bundle uses rebuilds it too.
    fn sources_mtime(&self) -> Option<SystemTime> {
        let listed = self.sources.iter().map(|p| self.crate_dir.join(p));
        let compiled = bundle_sources(&self.target_dir, self.artifact);
        listed.chain(compiled).filter_map(|p| newest_mtime(&p)).max()
    }

    fn build_if_stale(&mut self) {
        let stamp = self.sources_mtime();
        if stamp.is_some() && stamp == self.built_from {
            return;
        }
        eprintln!("[stream-serve] sources changed — building {}…", self.package);
        let t = Instant::now();
        let out = guest_build_command("cargo", &self.crate_dir, &self.target_dir, self.package).output();
        self.built_from = stamp;
        match out {
            Ok(out) if out.status.success() => match std::fs::read(guest_wasm_path(&self.target_dir, self.artifact)) {
                Ok(wasm) => {
                    self.version += 1;
                    eprintln!(
                        "[stream-serve] {} v{} ready: {} bytes in {:.1}s",
                        self.package,
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
                eprintln!("[stream-serve] {} build FAILED (still serving v{}):\n{errors}", self.package, self.version);
                self.error = Some(errors);
            }
            Err(e) => self.error = Some(format!("could not run cargo: {e}")),
        }
    }
}

fn respond(stream: &mut TcpStream, served: &[(&str, Arc<Mutex<State>>)]) -> std::io::Result<()> {
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

    let Some((_, state)) = served.iter().find(|(p, _)| *p == path) else {
        return write!(stream, "HTTP/1.1 404 Not Found\r\nContent-Length: 0\r\nConnection: close\r\n\r\n");
    };
    let mut s = state.lock().unwrap();
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
    let served: Arc<Vec<(&str, Arc<Mutex<State>>)>> = Arc::new(
        [
            ("/bundle.wasm", "spike-guest", "spike_guest", GUEST_SOURCES),
            ("/remote.wasm", "spike-remoteguest", "spike_remoteguest", REMOTE_GUEST_SOURCES),
            ("/example.wasm", "remote-example-bundle", "remote_example", EXAMPLE_SOURCES),
            ("/showcase.wasm", "remote-showcase-bundle", "remote_showcase", SHOWCASE_SOURCES),
        ]
            .into_iter()
            .map(|(path, package, artifact, sources)| {
                let state = State {
                    package,
                    artifact,
                    sources,
                    crate_dir: crate_dir.clone(),
                    target_dir: target_dir.clone(),
                    built_from: None,
                    wasm: None,
                    version: 0,
                    error: None,
                };
                (path, Arc::new(Mutex::new(state)))
            })
            .collect(),
    );
    for (_, state) in served.iter() {
        state.lock().unwrap().build_if_stale();
    }

    let watcher = served.clone();
    std::thread::spawn(move || loop {
        std::thread::sleep(Duration::from_millis(250));
        for (_, state) in watcher.iter() {
            state.lock().unwrap().build_if_stale();
        }
    });

    let listener = TcpListener::bind(("127.0.0.1", port)).unwrap_or_else(|e| panic!("bind 127.0.0.1:{port}: {e}"));
    eprintln!(
        "[stream-serve] serving http://127.0.0.1:{port}/bundle.wasm (edit spike/guest/src) and \
         /remote.wasm (edit spike/components/src) — refresh the demo to load"
    );
    for stream in listener.incoming() {
        let Ok(mut stream) = stream else { continue };
        let served = served.clone();
        std::thread::spawn(move || {
            if let Err(e) = respond(&mut stream, &served) {
                eprintln!("[stream-serve] request failed: {e}");
            }
        });
    }
}
