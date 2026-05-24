//! `idealyst serve` — minimal static-file HTTP server.
//!
//! Same as `idealyst dev --web` minus the rebuild-watch loop. Point
//! it at a directory that already contains `index.html` + `pkg/` and
//! it serves the files unchanged.

use std::path::PathBuf;

use anyhow::Result;
use dev_http::serve_static;

#[derive(clap::Args, Debug)]
pub struct Args {
    /// Directory to serve. Defaults to the current directory.
    /// Point this at the dir containing `index.html` + `pkg/`.
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
