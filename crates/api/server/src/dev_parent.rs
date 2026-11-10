//! A server started by `idealyst dev` exits when the dev process is gone.
//!
//! The dev loop kills its server on every restart and at teardown, but a
//! dev process that is SIGKILLed — the kernel's OOM killer in a
//! memory-tight devcontainer is the usual one — runs no teardown at all.
//! Its server was re-parented to init and kept the app's port: the next
//! session could not bind it, and the browser kept talking to a stale
//! server whose output went into a pipe nobody read.
//!
//! So the dev loop gives the server a pipe as stdin, keeps the write end,
//! and sets [`ENV`] to `stdin`. [`watch_from_env`] reads that stdin on a
//! thread of its own; nothing is ever written to it, so the read returns
//! only at EOF — when the last write end closes, which the OS does when
//! the dev process dies, however it dies. The server then exits.
//!
//! A pipe rather than polling the dev process's pid: it needs no
//! platform code, it reaches through `cargo run` (and `idealyst
//! run-linked`) untouched because both pass stdin on, and a recycled pid
//! cannot keep it open.
//!
//! Only under [`ENV`]: a server under systemd or in a container commonly
//! has `/dev/null` (immediate EOF) or a closed stdin, and must not read
//! that as a reason to exit.

use std::io::{ErrorKind, Read, Write};

/// Set by `idealyst dev` in the environment of the server it spawns.
/// The one value is `stdin`: stdin is a pipe whose write end the dev
/// process holds.
pub const ENV: &str = "IDEALYST_DEV_PARENT";

/// Start the watch when [`ENV`] asks for it. Once per process, however
/// many routers it builds.
pub(crate) fn watch_from_env() {
    if !watches(std::env::var(ENV).ok().as_deref()) {
        return;
    }
    static STARTED: std::sync::Once = std::sync::Once::new();
    STARTED.call_once(|| {
        let spawned = std::thread::Builder::new()
            .name("idealyst-dev-parent".into())
            .spawn(|| {
                wait_for_eof(std::io::stdin().lock());
                // Not `eprintln!`: the dev process held the read end of
                // stderr's pipe too, so with it gone this write fails with
                // EPIPE, and `eprintln!` panics on a failed write — ending
                // this thread before the exit below.
                let _ = writeln!(
                    std::io::stderr(),
                    "[server] the `idealyst dev` session that started this server is gone; exiting",
                );
                std::process::exit(0);
            });
        if let Err(e) = spawned {
            let _ = writeln!(std::io::stderr(), "[server] could not watch the dev session: {e}");
        }
    });
}

/// Whether [`ENV`]'s value asks for the watch.
fn watches(value: Option<&str>) -> bool {
    value.is_some_and(|v| v.trim() == "stdin")
}

/// Read `pipe` until it reports EOF, or an error that means it never
/// will deliver anything again. Anything written is discarded.
pub(crate) fn wait_for_eof(mut pipe: impl Read) {
    let mut buf = [0u8; 64];
    loop {
        match pipe.read(&mut buf) {
            Ok(0) => return,
            Ok(_) => {}
            Err(e) if e.kind() == ErrorKind::Interrupted => {}
            Err(_) => return,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_the_stdin_value_starts_the_watch() {
        assert!(watches(Some("stdin")));
        assert!(watches(Some(" stdin\n")));
        assert!(!watches(None));
        assert!(!watches(Some("")));
        assert!(!watches(Some("1")));
    }

    #[test]
    fn the_wait_ends_when_the_last_writer_closes() {
        let (reader, mut writer) = std::io::pipe().unwrap();
        let (done_tx, done_rx) = std::sync::mpsc::channel();
        std::thread::spawn(move || {
            wait_for_eof(reader);
            done_tx.send(()).unwrap();
        });
        // Bytes do not end it: only EOF does.
        writer.write_all(b"noise").unwrap();
        assert!(done_rx.recv_timeout(std::time::Duration::from_millis(200)).is_err());
        drop(writer);
        done_rx
            .recv_timeout(std::time::Duration::from_secs(5))
            .expect("the wait returns at EOF");
    }
}
