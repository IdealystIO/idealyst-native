//! Manual smoke test for the Win32 host: open a window and mount the
//! `welcome` example app in it.
//!
//! ```text
//! cargo run -p host-win32 --example smoke
//! ```
//!
//! On Windows this opens a titled, resizable window running the real
//! framework mount loop (Text + Button primitives render as HWNDs;
//! other primitives show "not yet implemented" placeholders until the
//! backend matures). On other targets the host's stub returns a
//! non-zero exit code immediately.

fn main() {
    let opts = host_win32::RunOptions {
        title: "Idealyst — Win32 host smoke".to_string(),
        width: 1024,
        height: 768,
    };
    std::process::exit(host_win32::run(opts, welcome::app));
}
