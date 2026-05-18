//! Roku side-loader.
//!
//! Build pipeline produces `<project>/dist/roku.zip` already; this
//! crate just POSTs that zip to a Roku device in developer mode.
//! Pattern matches the well-known `curl --digest ... /plugin_install`
//! workflow that ships with the Roku SDK docs — we shell out to
//! `curl` rather than pull in a full HTTP-client + digest-auth
//! stack, since curl is a one-line install on every dev OS and the
//! upload is a one-shot operation.
//!
//! Also exposes [`tail_console`] which connects to the BrightScript
//! debug console on TCP port 8085 and forwards lines to stdout —
//! handy for catching `?` (print) output and crash dumps after a
//! launch.

use std::io::{BufRead, BufReader};
use std::net::TcpStream;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::Duration;

use anyhow::{anyhow, Context, Result};

#[derive(Clone, Debug)]
pub struct RunOptions {
    /// Device IP (`192.168.x.y`). Required — no Bonjour discovery
    /// for Roku; the device's dev-mode setup screen prints its IP.
    pub device_ip: String,
    /// Developer password set via the dev-mode enable sequence on
    /// the device (Home×3, Up×2, Right, Left, Right, Left, Right).
    pub password: String,
    /// Whether to stream the BrightScript debug console (port 8085)
    /// to stdout after the install. The console runs until Ctrl-C.
    pub console: bool,
    /// Pre-built channel zip — what `build_roku::build` writes to
    /// `<project>/dist/roku.zip`.
    pub zip_path: PathBuf,
}

#[derive(Debug)]
pub struct RunArtifact {
    /// Path to the channel zip uploaded to the device.
    pub zip_path: PathBuf,
}

pub fn run(opts: RunOptions) -> Result<RunArtifact> {
    if !opts.zip_path.is_file() {
        return Err(anyhow!(
            "channel zip {} doesn't exist — run `idealyst build roku` first",
            opts.zip_path.display()
        ));
    }

    upload_to_device(&opts.device_ip, &opts.password, &opts.zip_path)?;

    if opts.console {
        eprintln!(
            "[run-roku] streaming console at {}:8085 (Ctrl-C to stop)",
            opts.device_ip
        );
        tail_console(&opts.device_ip)?;
    }

    Ok(RunArtifact {
        zip_path: opts.zip_path,
    })
}

// ---------------------------------------------------------------------------
// Device upload — shell to `curl --digest`
// ---------------------------------------------------------------------------

fn upload_to_device(ip: &str, password: &str, zip: &Path) -> Result<()> {
    // Verify curl is available with a friendlier error than the
    // raw process-spawn failure.
    if Command::new("curl").arg("--version").output().is_err() {
        return Err(anyhow!(
            "couldn't find `curl` on PATH — install it (every dev OS ships it; \
             on Windows 10+ it's built in) or use the Roku web UI \
             at http://{}/ to upload {} manually",
            ip, zip.display()
        ));
    }

    let url = format!("http://{}/plugin_install", ip);
    let cred = format!("rokudev:{}", password);
    let archive_arg = format!("archive=@{}", zip.display());

    eprintln!("[run-roku] uploading {} to {}", zip.display(), url);
    let output = Command::new("curl")
        .args(["-s", "-S", "--digest", "-u"])
        .arg(&cred)
        .args(["-F", "mysubmit=Install"])
        .args(["-F", &archive_arg])
        .arg(&url)
        .output()
        .context("invoking curl")?;

    if !output.status.success() {
        return Err(anyhow!(
            "curl exited with status {} when uploading to {}:\n{}",
            output.status,
            url,
            String::from_utf8_lossy(&output.stderr)
        ));
    }

    let body = String::from_utf8_lossy(&output.stdout);
    // Roku's response is HTML. Cheap text-match against known
    // success/failure substrings — bulletproof parsing would
    // require an HTML parser we don't otherwise need.
    if body.contains("Install Success") {
        eprintln!("[run-roku] install succeeded");
    } else if body.contains("Identical to previous") {
        eprintln!("[run-roku] install succeeded (identical to previous package)");
    } else if body.contains("Failure") || body.contains("failed") {
        return Err(anyhow!(
            "Roku rejected the upload. Response body:\n{}",
            body
        ));
    } else {
        // Unknown response — pass through so the user can diagnose.
        eprintln!("[run-roku] device response:\n{}", body);
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// Debug console tail
// ---------------------------------------------------------------------------

pub fn tail_console(ip: &str) -> Result<()> {
    // 8085 is the Roku BrightScript debugger. Plain TCP, no auth —
    // anyone on the LAN can read it, which matters for shared dev
    // networks but not for our purposes.
    let addr = format!("{}:8085", ip);
    let stream = TcpStream::connect_timeout(
        &addr.parse().with_context(|| format!("parsing {}", addr))?,
        Duration::from_secs(5),
    )
    .with_context(|| format!("connecting to {}", addr))?;

    // Buffered line iterator with a generous capacity — Roku
    // sometimes emits long stack traces on crash.
    let reader = BufReader::with_capacity(8 * 1024, stream);
    for line in reader.lines() {
        match line {
            Ok(l) => println!("{}", l),
            Err(e) => {
                eprintln!("[run-roku] console stream error: {}", e);
                break;
            }
        }
    }
    Ok(())
}

