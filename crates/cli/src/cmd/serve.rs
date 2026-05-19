//! `idealyst serve` — minimal static-file HTTP server.
//!
//! Same as `idealyst dev --web` minus the rebuild-watch loop, the
//! livereload polling, AAS, and any platform-specific build step.
//! Just point it at a directory and it serves the files. The point
//! is to drop in for `python3 -m http.server` when you want to load
//! an already-built wasm bundle without spinning up the dev pipeline.

use std::path::PathBuf;

use anyhow::Result;
use dev_http::serve_static;

#[derive(clap::Args, Debug)]
pub struct Args {
    /// Directory to serve. Defaults to the current directory.
    /// Typically you'd point this at the docs example or whatever
    /// dir contains your `index.html` + `pkg/`.
    #[arg(default_value = ".")]
    pub dir: PathBuf,

    /// HTTP port.
    #[arg(long, default_value_t = 8080)]
    pub port: u16,

    /// Interface to bind. `127.0.0.1` for loopback only;
    /// `0.0.0.0` to expose to the LAN (useful for testing the same
    /// bundle on a phone over Wi-Fi).
    #[arg(long, default_value = "0.0.0.0")]
    pub host: String,
}

pub fn run(args: Args) -> Result<()> {
    serve_static(&args.host, args.port, &args.dir, None, None)
}
