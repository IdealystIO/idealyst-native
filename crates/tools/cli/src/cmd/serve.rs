//! `idealyst serve` — minimal static-file HTTP server.
//!
//! Same as `idealyst dev --web` minus the rebuild-watch loop. Point
//! it at a directory that already contains `index.html` + `pkg/` and
//! it serves the files unchanged. Defaults to `dist/web`, which is
//! where `idealyst build --web` stages its bundle — so `idealyst
//! build --web && idealyst serve` Just Works from a project root.

use std::path::PathBuf;

use anyhow::{Context, Result};
use dev_http::serve_static;

#[derive(clap::Args, Debug)]
pub struct Args {
    /// Directory to serve. Defaults to `dist/web` (the output of
    /// `idealyst build --web`). Point this at any dir containing
    /// `index.html` + `pkg/`.
    #[arg(default_value = "dist/web")]
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
    if !args.dir.is_dir() {
        return Err(anyhow::anyhow!(
            "nothing to serve at {} — run `idealyst build --web` first \
             (or pass a directory: `idealyst serve <dir>`)",
            args.dir.display(),
        ))
        .context("serve");
    }
    serve_static(&args.host, args.port, &args.dir, None, None, None)
}

#[cfg(test)]
mod tests {
    //! `idealyst serve` defaults to the web bundle dir and refuses to
    //! bind a port when there's nothing there. The default must track
    //! `idealyst build --web`'s output (`<project>/dist/web`); if one
    //! moves without the other, `build --web && serve` silently serves
    //! the wrong (or an empty) tree.

    use super::*;
    use clap::Parser;

    #[derive(Parser)]
    struct TestCli {
        #[command(flatten)]
        args: Args,
    }

    #[test]
    fn default_dir_is_dist_web() {
        let cli = TestCli::parse_from(["serve"]);
        assert_eq!(
            cli.args.dir,
            PathBuf::from("dist/web"),
            "serve default must match `idealyst build --web` output dir",
        );
    }

    #[test]
    fn missing_dir_errors_before_binding() {
        let tmp = tempfile::tempdir().unwrap();
        let missing = tmp.path().join("dist/web");
        let err = run(Args {
            dir: missing.clone(),
            // Port 0 would let the OS pick a free one, but the guard
            // returns before any bind — so this never opens a socket.
            port: 0,
            host: "127.0.0.1".to_string(),
        })
        .unwrap_err();
        let msg = format!("{err:#}");
        assert!(
            msg.contains("idealyst build --web"),
            "missing-dir error should point at `idealyst build --web`, got: {msg}",
        );
    }
}
