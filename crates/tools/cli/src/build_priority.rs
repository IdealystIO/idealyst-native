//! CPU priority for builds that should yield to others.
//!
//! In a full-stack `idealyst dev` session one save can start the web
//! bundle's build (or hot patch) and the server's `cargo build` in the same
//! second. At equal priority they split the cores, and the web side — the
//! one the user is watching — gets slower. The server's build runs at a
//! lower priority instead: it still uses every core nothing else wants (so
//! alone it is as fast as before, with full `-j`), and yields the cores the
//! web build asks for.
//!
//! The mechanism is the OS scheduler's, inherited by the whole build tree:
//!
//! - **unix**: `nice(n)` in the child between fork and exec. Cargo, rustc
//!   and the linker it spawns inherit the niceness. On Linux with
//!   `CONFIG_SCHED_AUTOGROUP`, niceness only weighs threads within one
//!   autogroup (session); the web and server builds are both children of
//!   this process, so they share one and the niceness applies between them.
//! - **Windows**: `BELOW_NORMAL_PRIORITY_CLASS` at creation. A process
//!   created without a priority-class flag by a below-normal parent gets
//!   below-normal itself, so rustc inherits it too.
//!
//! Only the child is lowered: the CLI process and every other command it
//! spawns (the web build included) keep their priority.
//!
//! `IDEALYST_SERVER_BUILD_NICE=<n>` sets the niceness increment (1–19,
//! default [`DEFAULT_SERVER_BUILD_NICE`]); `0` turns the lowering off.

use std::process::Command;

/// The env var that sets (or, with `0`, disables) the server build's
/// niceness increment.
pub const SERVER_BUILD_NICE_ENV: &str = "IDEALYST_SERVER_BUILD_NICE";

/// The default niceness increment for the dev server's build. 10 is the
/// conventional "background work" step (`nice` with no `-n`): low enough
/// that a normal-priority build wins contended cores, high enough above
/// the 19 floor that the user can still renice it further.
pub const DEFAULT_SERVER_BUILD_NICE: i32 = 10;

/// The largest niceness increment unix accepts.
const MAX_NICE: i32 = 19;

/// The priority a spawned build runs at.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BuildPriority {
    /// The spawning process's own priority.
    Normal,
    /// Lowered by `nice` (unix: the increment; Windows: any value means
    /// `BELOW_NORMAL_PRIORITY_CLASS`).
    Lowered { nice: i32 },
}

impl BuildPriority {
    /// The dev server build's priority, from [`SERVER_BUILD_NICE_ENV`].
    pub fn server_build() -> Self {
        Self::server_build_from(std::env::var(SERVER_BUILD_NICE_ENV).ok().as_deref())
    }

    /// The priority for a value of [`SERVER_BUILD_NICE_ENV`]: unset or
    /// unparsable → the default, `0` (or negative) → [`Normal`](Self::Normal),
    /// above 19 → 19. A negative value never raises priority — that needs
    /// privileges the CLI does not have, and is not what the knob is for.
    pub fn server_build_from(value: Option<&str>) -> Self {
        let nice = match value.map(str::trim) {
            None | Some("") => DEFAULT_SERVER_BUILD_NICE,
            Some(v) => match v.parse::<i32>() {
                Ok(n) => n,
                Err(_) => {
                    crate::dlog!(
                        "dev server",
                        "{SERVER_BUILD_NICE_ENV}={v:?} is not a number; using {DEFAULT_SERVER_BUILD_NICE}",
                    );
                    DEFAULT_SERVER_BUILD_NICE
                }
            },
        };
        if nice <= 0 {
            Self::Normal
        } else {
            Self::Lowered { nice: nice.min(MAX_NICE) }
        }
    }

    /// Make `cmd` spawn at this priority. Touches only the child: the
    /// calling process's own priority is unchanged.
    pub fn apply(self, cmd: &mut Command) {
        let Self::Lowered { nice } = self else { return };
        #[cfg(unix)]
        {
            use std::os::unix::process::CommandExt;
            // SAFETY: pre_exec runs in the child after fork, before exec;
            // `nice` is a single syscall with no allocation or locking.
            // Its -1 return is ambiguous (a legitimate new niceness of -1)
            // and a failure only leaves the build at normal priority, so
            // the result is ignored.
            unsafe {
                cmd.pre_exec(move || {
                    let _ = libc::nice(nice);
                    Ok(())
                });
            }
        }
        #[cfg(windows)]
        {
            use std::os::windows::process::CommandExt;
            const BELOW_NORMAL_PRIORITY_CLASS: u32 = 0x0000_4000;
            let _ = nice;
            cmd.creation_flags(BELOW_NORMAL_PRIORITY_CLASS);
        }
        #[cfg(not(any(unix, windows)))]
        let _ = (nice, cmd);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[cfg(unix)]
    use super::test_support::{niceness_of, spawned_niceness};

    #[test]
    fn server_build_priority_defaults_to_nice_10() {
        assert_eq!(BuildPriority::server_build_from(None), BuildPriority::Lowered { nice: 10 });
        assert_eq!(BuildPriority::server_build_from(Some("")), BuildPriority::Lowered { nice: 10 });
        assert_eq!(BuildPriority::server_build_from(Some("junk")), BuildPriority::Lowered { nice: 10 });
    }

    #[test]
    fn server_build_priority_zero_or_negative_disables() {
        assert_eq!(BuildPriority::server_build_from(Some("0")), BuildPriority::Normal);
        assert_eq!(BuildPriority::server_build_from(Some("-5")), BuildPriority::Normal);
    }

    #[test]
    fn server_build_priority_takes_the_value_clamped_to_19() {
        assert_eq!(BuildPriority::server_build_from(Some(" 5 ")), BuildPriority::Lowered { nice: 5 });
        assert_eq!(BuildPriority::server_build_from(Some("40")), BuildPriority::Lowered { nice: 19 });
    }

    #[cfg(unix)]
    #[test]
    fn lowered_priority_nices_the_child_and_only_the_child() {
        let own = niceness_of(std::process::id());
        let mut lowered = Command::new("sleep");
        lowered.arg("30");
        BuildPriority::Lowered { nice: 10 }.apply(&mut lowered);
        assert_eq!(spawned_niceness(lowered), (own + 10).min(MAX_NICE));
        // The CLI itself, and a command spawned without `apply` after it —
        // the web build's shape — keep the parent's niceness.
        assert_eq!(niceness_of(std::process::id()), own);
        let mut plain = Command::new("sleep");
        plain.arg("30");
        assert_eq!(spawned_niceness(plain), own);
    }

    #[cfg(unix)]
    #[test]
    fn normal_priority_leaves_the_child_alone() {
        let own = niceness_of(std::process::id());
        let mut cmd = Command::new("sleep");
        cmd.arg("30");
        BuildPriority::Normal.apply(&mut cmd);
        assert_eq!(spawned_niceness(cmd), own);
    }
}

/// Probes for tests that check a spawned build's priority.
#[cfg(all(test, unix))]
pub(crate) mod test_support {
    use std::process::Command;

    /// The niceness `ps` reports for `pid`.
    pub fn niceness_of(pid: u32) -> i32 {
        let out = Command::new("ps")
            .args(["-o", "nice=", "-p", &pid.to_string()])
            .output()
            .expect("run ps");
        String::from_utf8_lossy(&out.stdout)
            .trim()
            .parse()
            .unwrap_or_else(|_| panic!("ps -o nice= for {pid}: {:?}", String::from_utf8_lossy(&out.stdout)))
    }

    /// Spawn `cmd` (a long `sleep`), read its niceness, and reap it.
    pub fn spawned_niceness(mut cmd: Command) -> i32 {
        let mut child = cmd.spawn().expect("spawn the probe child");
        let nice = niceness_of(child.id());
        let _ = child.kill();
        let _ = child.wait();
        nice
    }
}
