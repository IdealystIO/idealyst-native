//! Running the Windows wrapper on a REAL Windows machine over SSH.
//!
//! Wine is a useful dev loop but not Windows: it lacks DirectComposition
//! surfaces, stubs Direct2D layers, and has crashes and CPU costs real
//! Windows doesn't (see `docs/windows.md`). With `IDEALYST_WINDOWS_REMOTE`
//! set to `user@host`, every launch path (`idealyst run windows`,
//! `idealyst dev --windows`, and so `idealyst test --parity …,windows`) runs
//! the `.exe` on that machine instead of under Wine, while everything else
//! (the build, the parity runner, the MCP tools) stays on this host.
//!
//! ## How the remote app looks local
//!
//! - **Bridge.** The app binds its robot bridge on the remote machine
//!   ([`REMOTE_BRIDGE_PORT`]); an SSH local forward exposes it on a free
//!   port here, and a registration file in `~/.idealyst/apps` names that
//!   port, the HOST project root, and the local ssh session's pid. That pid
//!   is a real process on this host, so the registry's PID-liveness pruning
//!   judges it correctly (unlike a Wine pid), and the parity runner and the
//!   MCP tools find the app unchanged.
//! - **Loopback services.** Env values that name `127.0.0.1:<port>` (the dev
//!   server, a runtime endpoint) get an SSH reverse forward, so the same URL
//!   reaches this host from the remote side.
//! - **A desktop.** Programs started from an SSH session have no desktop, so
//!   the app is started by a one-off scheduled task in the interactive
//!   session of the logged-in user ([`launch_script`]). That needs someone
//!   logged in to the machine's desktop.
//! - **Lifetime.** The returned `Child` is the local `ssh` session. The
//!   task's script watches the remote end of that session and stops the app
//!   when it goes away, so killing the child (Ctrl-C in `dev`, the parity
//!   runner's teardown) also ends the remote app. What it watches is the
//!   session's `sshd.exe`, not the session's shell: Windows' OpenSSH does not
//!   end the shells when a client disconnects — they are orphaned and keep
//!   running — but the per-session `sshd.exe` exits.
//!
//! Setup: an SSH server on the Windows machine with key login for this host
//! (`docs/windows.md`, "Parity and tests on a real Windows machine").

use std::net::TcpListener;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};

use anyhow::{Context, Result};

/// Where the app binds its bridge on the remote machine. Fixed rather than
/// ephemeral so the forward can be set up before the app starts; a stale
/// app holding it is stopped before each launch.
pub const REMOTE_BRIDGE_PORT: u16 = 9718;

/// How long the remote session waits for the app's process to appear. A
/// debug build of a 3D app is half a gigabyte, and Windows scans a new
/// executable before its first start, which took well past the 30 s the
/// launcher used to allow.
const START_TIMEOUT_SECS: u32 = 180;

/// Variables a remote launch drops: host-side paths and PIDs that mean
/// nothing on the remote machine, and the bridge port it sets itself.
const DROPPED: &[&str] = &[
    "IDEALYST_LAUNCHER_PID",
    "IDEALYST_BRIDGE_PORT_FILE",
    "IDEALYST_APPS_DIR",
    "IDEALYST_BRIDGE_PORT",
];

/// Variables taken from THIS process's environment when set, because the
/// launch paths rely on the app inheriting them (the parity runner pins the
/// window with `IDEALYST_WINDOW_SIZE` on the `idealyst dev` process).
const INHERITED: &[&str] = &[
    "IDEALYST_WINDOW_SIZE",
    "IDEALYST_TEST_DRIVER",
    "IDEALYST_WINDOWS_RENDERER",
    "RUST_BACKTRACE",
    "RUST_LOG",
];

/// The remote machine: `IDEALYST_WINDOWS_REMOTE=user@host`, with an optional
/// `IDEALYST_WINDOWS_REMOTE_KEY` naming the SSH key to use.
#[derive(Clone, Debug, PartialEq)]
pub struct Remote {
    pub dest: String,
    pub key: Option<PathBuf>,
}

impl Remote {
    pub fn from_env() -> Option<Self> {
        let dest = std::env::var("IDEALYST_WINDOWS_REMOTE").ok().filter(|d| !d.trim().is_empty())?;
        let key = std::env::var_os("IDEALYST_WINDOWS_REMOTE_KEY").filter(|k| !k.is_empty()).map(PathBuf::from);
        Some(Remote { dest: dest.trim().to_string(), key })
    }

    fn ssh_base(&self) -> Command {
        let mut cmd = Command::new("ssh");
        self.auth_args(&mut cmd);
        cmd
    }

    fn auth_args(&self, cmd: &mut Command) {
        if let Some(k) = &self.key {
            cmd.arg("-i").arg(k).args(["-o", "IdentitiesOnly=yes"]);
        }
        cmd.args(["-o", "BatchMode=yes", "-o", "ConnectTimeout=10"]);
    }

    /// Run one PowerShell script remotely and wait for it.
    fn run_ps(&self, script: &str) -> Result<String> {
        let out = self
            .ssh_base()
            .arg(&self.dest)
            .arg(encoded_command(script))
            .stdin(Stdio::null())
            .output()
            .with_context(|| format!("ssh to {}", self.dest))?;
        anyhow::ensure!(
            out.status.success(),
            "remote command on {} failed ({}): {}",
            self.dest,
            out.status,
            String::from_utf8_lossy(&out.stderr).trim()
        );
        Ok(String::from_utf8_lossy(&out.stdout).into_owned())
    }

    fn copy(&self, local: &Path, remote_dir: &str) -> Result<()> {
        let mut cmd = Command::new("scp");
        if let Some(k) = &self.key {
            cmd.arg("-i").arg(k).args(["-o", "IdentitiesOnly=yes"]);
        }
        cmd.args(["-q", "-r", "-o", "BatchMode=yes"]);
        let status = cmd
            .arg(local)
            .arg(format!("{}:{}", self.dest, remote_dir.replace('\\', "/")))
            .status()
            .with_context(|| format!("scp {} to {}", local.display(), self.dest))?;
        anyhow::ensure!(status.success(), "copying {} to {} failed ({status})", local.display(), self.dest);
        Ok(())
    }

    /// Copy `binary` (with its `assets/` folder and the SDK `runtime_files`
    /// it needs beside it — `build_windows::BuildArtifact::runtime_files`)
    /// over and start it on the remote desktop. Returns the local ssh
    /// session that keeps the forwards up and the app alive (see the module
    /// docs).
    pub fn launch(
        &self,
        binary: &Path,
        runtime_files: &[std::path::PathBuf],
        project_dir: &Path,
        env_vars: &[(String, String)],
    ) -> Result<Child> {
        let app = binary
            .file_stem()
            .and_then(|s| s.to_str())
            .context("binary has no file name")?
            .to_string();
        let exe = binary.file_name().and_then(|s| s.to_str()).expect("has a stem").to_string();
        let dir = remote_dir(&app);
        let task = task_name(&app);

        let local_env: Vec<(String, String)> =
            INHERITED.iter().filter_map(|k| std::env::var(k).ok().map(|v| (k.to_string(), v))).collect();
        let (env, reverse_ports) = remote_env(env_vars, &local_env, REMOTE_BRIDGE_PORT);

        eprintln!("[run-windows] remote: preparing {} on {}", dir, self.dest);
        self.run_ps(&prepare_script(&dir, &task, &app))?;
        self.copy(binary, &dir)?;
        if let Some(assets) = binary.parent().map(|p| p.join("assets")).filter(|p| p.is_dir()) {
            self.copy(&assets, &dir)?;
        }
        for file in runtime_files {
            self.copy(file, &dir)?;
        }
        let script = std::env::temp_dir().join(format!("idealyst-remote-launch-{}.ps1", std::process::id()));
        std::fs::write(&script, launch_script(&exe, &env)).context("writing the remote launch script")?;
        let copied = self.copy(&script, &format!("{dir}\\launch.ps1"));
        let _ = std::fs::remove_file(&script);
        copied?;

        let local_port = free_local_port()?;
        let mut cmd = Command::new("ssh");
        self.auth_args(&mut cmd);
        cmd.args(["-o", "ExitOnForwardFailure=yes", "-o", "ServerAliveInterval=5", "-o", "ServerAliveCountMax=3"]);
        cmd.arg("-L").arg(format!("{local_port}:127.0.0.1:{REMOTE_BRIDGE_PORT}"));
        for p in &reverse_ports {
            cmd.arg("-R").arg(format!("{p}:127.0.0.1:{p}"));
        }
        cmd.arg(&self.dest).arg(encoded_command(&session_script(&dir, &task, &app)));
        cmd.stdin(Stdio::null());
        let child = cmd.spawn().with_context(|| format!("starting the ssh session to {}", self.dest))?;
        eprintln!(
            "[run-windows] remote: {app} starting on {} (bridge {} → 127.0.0.1:{local_port})",
            self.dest, REMOTE_BRIDGE_PORT
        );

        let root = std::fs::canonicalize(project_dir).unwrap_or_else(|_| project_dir.to_path_buf());
        let body = registration_json(local_port, child.id(), &app, &root.to_string_lossy());
        if let Some(dir) = remote_apps_dir() {
            let _ = std::fs::create_dir_all(&dir);
            clear_registrations_for(&dir, &app);
            let _ = std::fs::write(dir.join(format!("{app}-{}.json", child.id())), &body);
        }
        // The project-scoped port file the dev CLI asked for, pointing at
        // the local end of the forward.
        if let Some((_, path)) = env_vars.iter().find(|(k, _)| k == "IDEALYST_BRIDGE_PORT_FILE") {
            if let Some(parent) = Path::new(path).parent() {
                let _ = std::fs::create_dir_all(parent);
            }
            let _ = std::fs::write(path, &body);
        }
        Ok(child)
    }
}

/// `C:\idealyst\<app>` on the remote machine.
fn remote_dir(app: &str) -> String {
    format!("C:\\idealyst\\{app}")
}

fn task_name(app: &str) -> String {
    format!("idealyst-{app}")
}

fn remote_apps_dir() -> Option<PathBuf> {
    let home = std::env::var_os("HOME")?;
    Some(PathBuf::from(home).join(".idealyst").join("apps"))
}

/// Drop earlier registrations of `app` (a remote machine runs one copy).
/// Matches on the `<app>-` file-name prefix the bridge also uses; the
/// Windows binary's stem (`<project>-windows`) is unique to this target.
fn clear_registrations_for(dir: &Path, app: &str) {
    let prefix = format!("{app}-");
    for entry in std::fs::read_dir(dir).into_iter().flatten().flatten() {
        let name = entry.file_name();
        let name = name.to_string_lossy();
        if name.starts_with(&prefix) && name.ends_with(".json") {
            let _ = std::fs::remove_file(entry.path());
        }
    }
}

fn free_local_port() -> Result<u16> {
    let l = TcpListener::bind("127.0.0.1:0").context("finding a free local port")?;
    Ok(l.local_addr()?.port())
}

/// The env the remote app runs with, and the loopback ports to reverse-
/// forward: `env_vars` minus [`DROPPED`], plus `inherited` (the launching
/// process's own [`INHERITED`] values, where `env_vars` doesn't set them),
/// plus the bridge port. A port is reverse-forwarded when a value names
/// `127.0.0.1:<port>` or `localhost:<port>`.
pub(crate) fn remote_env(
    env_vars: &[(String, String)],
    inherited: &[(String, String)],
    bridge_port: u16,
) -> (Vec<(String, String)>, Vec<u16>) {
    let mut env: Vec<(String, String)> =
        env_vars.iter().filter(|(k, _)| !DROPPED.contains(&k.as_str())).cloned().collect();
    for (k, v) in inherited {
        if !env.iter().any(|(ek, _)| ek == k) {
            env.push((k.clone(), v.clone()));
        }
    }
    env.push(("IDEALYST_BRIDGE_PORT".into(), bridge_port.to_string()));
    let mut ports: Vec<u16> = env.iter().flat_map(|(_, v)| loopback_ports(v)).collect();
    ports.sort_unstable();
    ports.dedup();
    (env, ports)
}

fn loopback_ports(value: &str) -> Vec<u16> {
    ["127.0.0.1:", "localhost:"]
        .iter()
        .flat_map(|host| {
            value.match_indices(host).filter_map(move |(i, _)| {
                let digits: String = value[i + host.len()..].chars().take_while(|c| c.is_ascii_digit()).collect();
                digits.parse::<u16>().ok()
            })
        })
        .collect()
}

/// A PowerShell single-quoted literal.
fn ps_quote(s: &str) -> String {
    format!("'{}'", s.replace('\'', "''"))
}

/// Stop a previous run of `app` and make its directory.
pub(crate) fn prepare_script(dir: &str, task: &str, app: &str) -> String {
    format!(
        "$ErrorActionPreference = 'Stop'\n\
         $ProgressPreference = 'SilentlyContinue'\n\
         Stop-ScheduledTask -TaskName {task} -ErrorAction SilentlyContinue\n\
         Get-Process -Name {app} -ErrorAction SilentlyContinue | Stop-Process -Force\n\
         Start-Sleep -Milliseconds 300\n\
         New-Item -ItemType Directory -Force -Path {dir} | Out-Null\n",
        task = ps_quote(task),
        app = ps_quote(app),
        dir = ps_quote(dir),
    )
}

/// The script the scheduled task runs on the remote desktop: set `env`,
/// start the app with its output in `app.log`, and stop it once the
/// watched process (the ssh session's shell, `-Watch`) is gone.
pub(crate) fn launch_script(exe: &str, env: &[(String, String)]) -> String {
    let mut s = String::from(
        "param([int]$Watch)\n\
         # Generated by run-windows (remote.rs). Runs in the desktop session.\n",
    );
    for (k, v) in env {
        s.push_str(&format!("${{env:{k}}} = {}\n", ps_quote(v)));
    }
    s.push_str(&format!(
        "$dir = Split-Path -Parent $MyInvocation.MyCommand.Path\n\
         $exe = Join-Path $dir {exe}\n\
         $p = Start-Process -FilePath $exe -WorkingDirectory $dir -PassThru \
         -RedirectStandardError (Join-Path $dir 'app.log') -RedirectStandardOutput (Join-Path $dir 'app.out.log')\n\
         while (-not $p.HasExited) {{\n\
         \x20   if (-not (Get-Process -Id $Watch -ErrorAction SilentlyContinue)) {{ Stop-Process -Id $p.Id -Force; break }}\n\
         \x20   Start-Sleep -Milliseconds 500\n\
         }}\n",
        exe = ps_quote(exe),
    ));
    s
}

/// The long-lived ssh session's script: start the task in the desktop
/// session with this shell's PID to watch, report whether the app came up,
/// then hold the session (and with it the forwards) open until the app
/// exits — so a foreground `idealyst run windows` returns when the window
/// is closed.
pub(crate) fn session_script(dir: &str, task: &str, app: &str) -> String {
    format!(
        "$ErrorActionPreference = 'Stop'\n\
         $ProgressPreference = 'SilentlyContinue'\n\
         # The session's sshd.exe: the first sshd.exe above this shell. It exits\n\
         # when the client disconnects; the shells under it do not.\n\
         $watch = $PID\n\
         $proc = Get-CimInstance Win32_Process -Filter ('ProcessId = ' + $PID)\n\
         while ($proc -and $proc.Name -ne 'sshd.exe') {{ $proc = Get-CimInstance Win32_Process -Filter ('ProcessId = ' + $proc.ParentProcessId) }}\n\
         if ($proc) {{ $watch = $proc.ProcessId }}\n\
         $launch = Join-Path {dir} 'launch.ps1'\n\
         $arg = '-NoProfile -ExecutionPolicy Bypass -WindowStyle Hidden -File \"' + $launch + '\" -Watch ' + $watch\n\
         $action = New-ScheduledTaskAction -Execute 'powershell.exe' -Argument $arg\n\
         $principal = New-ScheduledTaskPrincipal -UserId $env:USERNAME -LogonType Interactive\n\
         Register-ScheduledTask -TaskName {task} -Action $action -Principal $principal -Force | Out-Null\n\
         Start-ScheduledTask -TaskName {task}\n\
         # Wait for the process first: a large new .exe can take well over\n\
         # 30 s to start the first time (Windows scans it), and ending the\n\
         # session early makes the watcher stop the app as it appears.\n\
         $deadline = (Get-Date).AddSeconds({start_timeout})\n\
         while ((Get-Date) -lt $deadline -and -not (Get-Process -Name {app} -ErrorAction SilentlyContinue)) {{ Start-Sleep -Milliseconds 250 }}\n\
         if (-not (Get-Process -Name {app} -ErrorAction SilentlyContinue)) {{ 'idealyst-remote: the app did not start within {start_timeout} s (is a user logged in to the desktop? see ' + (Join-Path {dir} 'app.log') + ')'; exit 1 }}\n\
         $deadline = (Get-Date).AddSeconds(30)\n\
         while ((Get-Date) -lt $deadline -and -not (Get-NetTCPConnection -LocalPort {port} -State Listen -ErrorAction SilentlyContinue) -and (Get-Process -Name {app} -ErrorAction SilentlyContinue)) {{ Start-Sleep -Milliseconds 250 }}\n\
         if (Get-NetTCPConnection -LocalPort {port} -State Listen -ErrorAction SilentlyContinue) {{ 'idealyst-remote: app is up (robot bridge open)' }}\n\
         elseif (Get-Process -Name {app} -ErrorAction SilentlyContinue) {{ 'idealyst-remote: app is up (no robot bridge: built without the dev feature)' }}\n\
         else {{ 'idealyst-remote: the app exited during start-up (see ' + (Join-Path {dir} 'app.log') + ')'; exit 1 }}\n\
         Start-Sleep -Seconds 2\n\
         while (Get-Process -Name {app} -ErrorAction SilentlyContinue) {{ Start-Sleep -Seconds 1 }}\n\
         'idealyst-remote: the app exited'\n",
        dir = ps_quote(dir),
        task = ps_quote(task),
        app = ps_quote(app),
        port = REMOTE_BRIDGE_PORT,
        start_timeout = START_TIMEOUT_SECS,
    )
}

/// `powershell -EncodedCommand <base64 UTF-16LE>`: the script reaches the
/// remote shell with no quoting through ssh, cmd or PowerShell.
fn encoded_command(script: &str) -> String {
    let utf16: Vec<u8> = script.encode_utf16().flat_map(|u| u.to_le_bytes()).collect();
    format!("powershell -NoProfile -NonInteractive -EncodedCommand {}", base64(&utf16))
}

fn base64(bytes: &[u8]) -> String {
    const T: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::with_capacity(bytes.len().div_ceil(3) * 4);
    for chunk in bytes.chunks(3) {
        let n = (chunk[0] as u32) << 16 | (*chunk.get(1).unwrap_or(&0) as u32) << 8 | *chunk.get(2).unwrap_or(&0) as u32;
        for i in 0..4 {
            if i <= chunk.len() {
                out.push(T[(n >> (18 - 6 * i) & 63) as usize] as char);
            } else {
                out.push('=');
            }
        }
    }
    out
}

/// The registration file for the forwarded bridge — the bridge's own
/// format (`runtime_shared::robot::bridge::bridge_registration_json`), with
/// the local port and the ssh session's pid.
pub(crate) fn registration_json(port: u16, pid: u32, name: &str, project_root: &str) -> String {
    format!(
        "{{\"port\":{port},\"pid\":{pid},\"name\":{},\"bundle_id\":null,\"project_root\":{},\"platform\":\"windows\",\"proto\":1}}",
        serde_json::to_string(name).expect("string"),
        serde_json::to_string(project_root).expect("string"),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn kv(k: &str, v: &str) -> (String, String) {
        (k.to_string(), v.to_string())
    }

    #[test]
    fn remote_env_drops_host_only_values_and_sets_the_bridge_port() {
        let env_vars = vec![
            kv("IDEALYST_LAUNCHER_PID", "4242"),
            kv("IDEALYST_BRIDGE_PORT_FILE", "/Users/me/app/.idealyst/bridge.port"),
            kv("IDEALYST_BRIDGE_PORT", "7001"),
            kv("IDEALYST_SERVER_URL", "http://127.0.0.1:3000"),
        ];
        let inherited = vec![kv("IDEALYST_WINDOW_SIZE", "1280x800"), kv("IDEALYST_SERVER_URL", "ignored")];
        let (env, ports) = remote_env(&env_vars, &inherited, REMOTE_BRIDGE_PORT);
        let get = |k: &str| env.iter().find(|(ek, _)| ek == k).map(|(_, v)| v.as_str());
        assert_eq!(get("IDEALYST_LAUNCHER_PID"), None, "a host PID means nothing remotely");
        assert_eq!(get("IDEALYST_BRIDGE_PORT_FILE"), None, "a host path means nothing remotely");
        assert_eq!(get("IDEALYST_BRIDGE_PORT"), Some("9718"));
        assert_eq!(get("IDEALYST_WINDOW_SIZE"), Some("1280x800"), "the parity viewport reaches the app");
        assert_eq!(get("IDEALYST_SERVER_URL"), Some("http://127.0.0.1:3000"), "explicit values win");
        assert_eq!(ports, vec![3000], "loopback services are reverse-forwarded");
    }

    #[test]
    fn loopback_ports_are_found_in_urls() {
        assert_eq!(loopback_ports("ws://127.0.0.1:8123/x"), vec![8123]);
        assert_eq!(loopback_ports("http://localhost:3000"), vec![3000]);
        assert_eq!(loopback_ports("http://example.com:80"), Vec::<u16>::new());
        assert_eq!(loopback_ports("1280x800"), Vec::<u16>::new());
    }

    #[test]
    fn scripts_quote_values_for_powershell() {
        let s = launch_script("app-windows.exe", &[kv("IDEALYST_X", "it's here")]);
        assert!(s.contains("${env:IDEALYST_X} = 'it''s here'"), "{s}");
        assert!(s.contains("Join-Path $dir 'app-windows.exe'"));
        assert!(s.contains("Stop-Process -Id $p.Id"), "the app stops when the watched session goes away");
        let p = prepare_script(r"C:\idealyst\it's", "idealyst-a", "a");
        assert!(p.contains(r"'C:\idealyst\it''s'"));
    }

    /// A slow first start (Windows scanning a large new .exe) outlasted the
    /// session's 30 s wait; the session then ended and its watcher stopped
    /// the app as it appeared. The session must wait for the PROCESS with the
    /// long timeout before anything else, and fail loudly if it never comes.
    #[test]
    fn regression_session_waits_for_a_slow_first_start() {
        let s = session_script(r"C:\idealyst\a", "idealyst-a", "a");
        let wait = s.find("AddSeconds(180)").expect("the long start timeout");
        let bridge = s.find("-State Listen").expect("the bridge check");
        assert!(wait < bridge, "the process wait comes before the bridge wait");
        assert!(s.contains("did not start within 180 s"));
        assert!(s.contains("exit 1"), "a failed start ends the session with an error");
    }

    #[test]
    fn registration_matches_the_bridge_format() {
        let json = registration_json(50123, 77, "app-windows", "/Users/me/app");
        let v: serde_json::Value = serde_json::from_str(&json).unwrap();
        assert_eq!(v["port"], 50123);
        assert_eq!(v["pid"], 77);
        assert_eq!(v["project_root"], "/Users/me/app");
        assert_eq!(v["platform"], "windows");
        assert_eq!(v["proto"], 1);
    }

    #[test]
    fn encoded_command_is_utf16le_base64() {
        // "a" → 61 00 → "YQA="
        assert_eq!(encoded_command("a"), "powershell -NoProfile -NonInteractive -EncodedCommand YQA=");
        assert_eq!(base64(b"Man"), "TWFu");
        assert_eq!(base64(b"Ma"), "TWE=");
    }
}
