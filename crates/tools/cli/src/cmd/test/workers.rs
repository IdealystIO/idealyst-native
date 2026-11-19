//! `idealyst test --workers N`: run a web suite on N copies of the app at
//! once, each test on whichever copy is free.
//!
//! A worker is one copy of the app a test can have to itself: its own port,
//! its own robot relay, its own browser (its own cookies: they are scoped by
//! host, not port, so pages of one browser would share a sign-in) and, for
//! a full-stack project, its own server process — which is what lets each
//! worker have its own database (`{worker}` in an `--env` value).
//!
//! The app builds once. Worker 0 is the ordinary `idealyst dev` session the
//! runner always launches; it writes a [`WorkerTemplate`] saying how its
//! page is served. The runner starts workers 1… from that, without a
//! second build:
//!
//! - full-stack: the session's own server command again on the worker's
//!   port, pointed at the worker's relay and serving a copy of the staged
//!   bundle whose `index.html` names the worker's relay and environment
//!   ([`stage_worker_bundle`]; the other files are hard links);
//! - static web: `dev_http` serving the project on the worker's port, its
//!   relay spliced in, as the session serves worker 0.
//!
//! Each worker's relay is its own (one per worker, as one per session), so
//! a test sees exactly the apps it sees in a single-worker run — a page it
//! reloads joins its own relay as the newest app, never another worker's.
//!
//! Every test then runs as its own test-binary process (`--exact <name>`)
//! against a free worker ([`run_queue`]), and the results go into the run's
//! one report.

use super::{
    engines, kill_tree, own_group, read_not_applicable, read_test_log, serve_browser_control, suite_env,
    wait_for_web_server, wait_until_ready, BrowserSlot, Outcome, RunResult, Size, TestStatus,
};
use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::collections::VecDeque;
use std::io::Write as _;
use std::net::SocketAddr;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

/// The variable every test and hook gets: the number of the worker it runs
/// on, from 0. A run without `--workers` is worker 0.
pub const WORKER_ENV: &str = "IDEALYST_TEST_WORKER";

/// What `{worker}` in an `--env` value becomes: the worker's number.
pub const WORKER_PLACEHOLDER: &str = "{worker}";

/// Where the session writes its [`WorkerTemplate`], in its state directory.
pub const TEMPLATE_FILE: &str = "test-workers.json";

/// How a session launched by `idealyst test` serves its page: what the
/// runner needs to serve the same app again on another port.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "shape", rename_all = "snake_case")]
pub enum WorkerTemplate {
    /// The project's own server serves the staged bundle.
    Server {
        /// The command the session started the server with.
        program: PathBuf,
        args: Vec<String>,
        cwd: Option<PathBuf>,
        /// The variables the session set on it (a worker overrides the
        /// port, bundle and relay). The rest it inherits, as the session's
        /// own server does.
        env: Vec<(String, String)>,
        /// The staged bundle (`WEB_DIST`).
        web_dist: PathBuf,
        /// The relay URL the staged `index.html` names, if any.
        relay_url: Option<String>,
        /// The page-environment `<script>`s the staged `index.html` holds
        /// (test driver, `--app-env`).
        page_env: String,
    },
    /// `dev_http` serves the project directory.
    Static {
        root: PathBuf,
        /// The served `<head>` before the relay and page-environment tags.
        head_base: String,
        preload_fonts: Vec<String>,
        overlay_roots: Vec<PathBuf>,
        fallback_index: String,
    },
}

impl WorkerTemplate {
    /// Write it to `<state_dir>/test-workers.json`.
    pub fn write(&self, state_dir: &Path) -> Result<()> {
        std::fs::create_dir_all(state_dir)?;
        let path = state_dir.join(TEMPLATE_FILE);
        std::fs::write(&path, serde_json::to_vec_pretty(self)?)
            .with_context(|| format!("writing {}", path.display()))
    }

    /// The template a session wrote, waiting up to `timeout` for it.
    pub fn read(state_dir: &Path, timeout: Duration) -> Result<WorkerTemplate> {
        let path = state_dir.join(TEMPLATE_FILE);
        let deadline = Instant::now() + timeout;
        loop {
            if let Ok(bytes) = std::fs::read(&path) {
                return serde_json::from_slice(&bytes).with_context(|| format!("reading {}", path.display()));
            }
            anyhow::ensure!(
                Instant::now() < deadline,
                "the dev session wrote no {} — it says how to serve the app again for the other workers",
                path.display()
            );
            std::thread::sleep(Duration::from_millis(200));
        }
    }

    /// The template of a server command the session is about to spawn.
    pub fn server(cmd: &Command, web_dist: &Path, relay_url: Option<String>, page_env: String) -> WorkerTemplate {
        WorkerTemplate::Server {
            program: PathBuf::from(cmd.get_program()),
            args: cmd.get_args().map(|a| a.to_string_lossy().into_owned()).collect(),
            cwd: cmd.get_current_dir().map(Path::to_path_buf),
            env: cmd
                .get_envs()
                .filter_map(|(k, v)| Some((k.to_string_lossy().into_owned(), v?.to_string_lossy().into_owned())))
                .collect(),
            web_dist: web_dist.to_path_buf(),
            relay_url,
            page_env,
        }
    }
}

/// `env` for worker `worker`: every `{worker}` in a value replaced with its
/// number, and [`WORKER_ENV`] set.
pub(crate) fn for_worker(env: &[(String, String)], worker: usize) -> Vec<(String, String)> {
    let mut out: Vec<(String, String)> = env
        .iter()
        .filter(|(k, _)| k != WORKER_ENV)
        .map(|(k, v)| (k.clone(), v.replace(WORKER_PLACEHOLDER, &worker.to_string())))
        .collect();
    out.push((WORKER_ENV.to_string(), worker.to_string()));
    out
}

/// The page-environment `<script>`s a session's page gets (the session's
/// own `page_env_scripts` builds them the same way): the test-driver flag
/// and the app environment.
pub fn page_env_html(test_driven: bool, env: &[(String, String)]) -> String {
    let mut out = String::new();
    if test_driven {
        out.push_str(build_web::test_driver_script_tag());
    }
    out.push_str(&build_web::app_env_script_tag(env));
    out
}

/// Copy the session's staged bundle `from` into `to` for one worker: every
/// file a hard link (a copy where the filesystem can't link), and
/// `index.html` written afresh with the worker's relay and page
/// environment in place of the session's (`replace`: (session's, worker's)
/// pairs of exact strings). A replacement that is not in the file is an
/// error: the page would dial worker 0's relay, or read its environment.
pub(super) fn stage_worker_bundle(from: &Path, to: &Path, replace: &[(String, String)]) -> Result<()> {
    let _ = std::fs::remove_dir_all(to);
    link_tree(from, to)?;
    let index = to.join("index.html");
    let mut html = std::fs::read_to_string(from.join("index.html"))
        .with_context(|| format!("reading {}", from.join("index.html").display()))?;
    for (old, new) in replace.iter().filter(|(old, new)| old != new && !old.is_empty()) {
        anyhow::ensure!(html.contains(old.as_str()), "the staged index.html holds no {old:?} to give a worker its own");
        html = html.replace(old.as_str(), new);
    }
    // A link to the session's file: replace it, never write through it.
    let _ = std::fs::remove_file(&index);
    std::fs::write(&index, html).with_context(|| format!("writing {}", index.display()))
}

fn link_tree(from: &Path, to: &Path) -> Result<()> {
    std::fs::create_dir_all(to).with_context(|| format!("creating {}", to.display()))?;
    for entry in std::fs::read_dir(from).with_context(|| format!("reading {}", from.display()))? {
        let entry = entry?;
        let (src, dst) = (entry.path(), to.join(entry.file_name()));
        if entry.file_type()?.is_dir() {
            link_tree(&src, &dst)?;
        } else if std::fs::hard_link(&src, &dst).is_err() {
            std::fs::copy(&src, &dst).with_context(|| format!("copying {}", src.display()))?;
        }
    }
    Ok(())
}

/// Run `names` on `workers` workers: each worker takes the next name as
/// soon as it is free, so a slow test holds up only its own worker. Returns
/// each name's result, in `names`' order.
pub(super) fn run_queue<T: Send>(names: &[String], workers: usize, run: impl Fn(usize, &str) -> T + Sync) -> Vec<T> {
    let queue: Mutex<VecDeque<usize>> = Mutex::new((0..names.len()).collect());
    let results: Mutex<Vec<Option<T>>> = Mutex::new((0..names.len()).map(|_| None).collect());
    std::thread::scope(|s| {
        for worker in 0..workers.max(1) {
            let (queue, results, run) = (&queue, &results, &run);
            s.spawn(move || loop {
                let Some(i) = queue.lock().unwrap().pop_front() else { break };
                let r = run(worker, &names[i]);
                results.lock().unwrap()[i] = Some(r);
            });
        }
    });
    results.into_inner().unwrap().into_iter().map(|r| r.expect("every name ran")).collect()
}

/// One worker of a run: where its app answers, and what was started for it.
pub(super) struct Worker {
    pub index: usize,
    /// The port its page is served on.
    pub port: u16,
    /// Its relay's bridge (the TCP side tests connect to).
    pub bridge: SocketAddr,
    /// Its environment ([`for_worker`] of the run's).
    pub env: Vec<(String, String)>,
    /// Where its browser profile lives (its relay looks here).
    pub profiles: PathBuf,
    /// Its browser, swapped when a size changes engine.
    pub browser: BrowserSlot,
    /// Its browser-control address for the tests (`set_viewport`).
    pub control: Option<SocketAddr>,
    /// Workers 1…: the relay and server the runner started for it, held so
    /// they stop when the worker is dropped (the server first).
    pub _server: Option<ServerChild>,
    pub _relay: Option<robot_relay::RelayHandle>,
}

/// A worker's server process, killed (with what it started) on drop. Its
/// stdin is held open: a server on the framework's router exits when it
/// reads EOF there (`server::dev_parent`), so it goes with the runner.
pub(super) struct ServerChild(Child);

impl Drop for ServerChild {
    fn drop(&mut self) {
        kill_tree(&mut self.0);
    }
}

/// Where worker `index`'s files go: its browser profile, bundle copy and
/// server log.
pub(super) fn worker_dir(state_dir: &Path, index: usize) -> PathBuf {
    state_dir.join("workers").join(format!("w{index}"))
}

/// Start worker `index` (1…) from the session's template: a relay, then the
/// app served on `port` (a server process, or `dev_http` in a thread).
pub(super) fn start_extra(
    template: &WorkerTemplate,
    index: usize,
    port: u16,
    env: Vec<(String, String)>,
    state_dir: &Path,
) -> Result<Worker> {
    let dir = worker_dir(state_dir, index);
    std::fs::create_dir_all(&dir).with_context(|| format!("creating {}", dir.display()))?;
    let relay = robot_relay::start(robot_relay::RelayConfig {
        // The runner hands tests the bridge address directly; no
        // registration, so nothing else ever discovers this app.
        register: false,
        screenshot_dir: Some(state_dir.join("screenshots")),
        // The relay finds its tab (screenshots, cookies, `load_url`) among
        // the browsers profiled here: this worker's alone.
        cdp_profiles_dir: Some(dir.clone()),
        under_test: true,
        ..Default::default()
    })
    .with_context(|| format!("starting worker {index}'s robot relay"))?;
    let relay_ws = relay.ws_addr;
    let relay_url = format!("ws://{relay_ws}");
    let test_driven = true;
    let server = match template {
        WorkerTemplate::Server { program, args, cwd, env: session_env, web_dist, relay_url: session_relay, page_env } => {
            let bundle = dir.join("web");
            let mut replace = vec![(page_env.clone(), page_env_html(test_driven, &env))];
            if let Some(url) = session_relay {
                replace.push((build_web::robot_relay_script_tag(url), build_web::robot_relay_script_tag(&relay_url)));
            }
            stage_worker_bundle(web_dist, &bundle, &replace)
                .with_context(|| format!("staging worker {index}'s copy of {}", web_dist.display()))?;
            let log = dir.join("server.log");
            let file = std::fs::File::create(&log).with_context(|| format!("creating {}", log.display()))?;
            let mut cmd = Command::new(program);
            cmd.args(args);
            if let Some(cwd) = cwd {
                cmd.current_dir(cwd);
            }
            cmd.envs(session_env.iter().map(|(k, v)| (k, v)))
                .envs(env.iter().map(|(k, v)| (k, v)))
                .env("PORT", port.to_string())
                .env("WEB_DIST", &bundle)
                .env(crate::cmd::dev::DEV_RELAY_ENV, &relay_url)
                .stdin(Stdio::piped())
                .stdout(file.try_clone()?)
                .stderr(file);
            let child = own_group(&mut cmd).spawn().with_context(|| format!("starting worker {index}'s server"))?;
            let child = ServerChild(child);
            anyhow::ensure!(
                wait_for_web_server(port, Duration::from_secs(120)),
                "worker {index}'s server never answered on port {port} — see {}",
                log.display()
            );
            Some(child)
        }
        WorkerTemplate::Static { root, head_base, preload_fonts, overlay_roots, fallback_index } => {
            let mut head = head_base.clone();
            for tag in [build_web::robot_relay_script_tag(&relay_url), page_env_html(test_driven, &env)] {
                head.push('\n');
                head.push_str(&tag);
            }
            let reload = dev_http::ReloadContext { signal: dev_reload::ReloadSignal::new(), relay: Some(relay_ws) };
            let preload = (!preload_fonts.is_empty())
                .then(|| dev_http::PreloadContext { font_paths: preload_fonts.clone() });
            let overlay = (!overlay_roots.is_empty()).then(|| dev_http::OverlayContext { roots: overlay_roots.clone() });
            let (root, fallback) = (root.clone(), fallback_index.clone());
            // Serves until the runner exits.
            std::thread::spawn(move || {
                if let Err(e) = dev_http::serve_static(
                    "127.0.0.1",
                    port,
                    &root,
                    Some(reload),
                    None,
                    preload,
                    overlay,
                    Some(dev_http::HeadInjectionContext { html: head }),
                    Some(fallback),
                    false,
                ) {
                    note!("worker {index}: serving the page on port {port} failed: {e:#}");
                }
            });
            anyhow::ensure!(
                wait_for_web_server(port, Duration::from_secs(30)),
                "worker {index}'s page server never answered on port {port}"
            );
            None
        }
    };
    Ok(Worker {
        index,
        port,
        bridge: relay.tcp_addr,
        env,
        profiles: dir,
        browser: Arc::new(Mutex::new(None)),
        control: None,
        _server: server,
        _relay: Some(relay),
    })
}

/// The tests a run executes, by name, each with whether it is `#[ignore]`d
/// (a platform-bound one, which needs `--include-ignored`).
pub(super) type Planned = Vec<(String, bool)>;

/// Run one size on every worker: bring each worker's page to `size`, wait
/// for each app, then run `planned` through the queue. One [`RunResult`]
/// for the size, every test's result in it. Each test's line (and a
/// failure's output) goes to the terminal, or to `suite_log` when a
/// `--jobs` run gives one.
#[allow(clippy::too_many_arguments)]
pub(super) fn run_size(
    workers: &mut [Worker],
    size: &Size,
    planned: &Planned,
    test_bin: &Path,
    dir: &Path,
    tag: &str,
    suite_log: Option<&Path>,
) -> RunResult {
    let prepared = (|| -> Result<()> {
        for w in workers.iter_mut() {
            let url = format!("http://127.0.0.1:{}/", w.port);
            if let Some(e) = &size.emulation {
                let mut slot = w.browser.lock().unwrap();
                if slot.as_ref().map(|b| b.engine()) != Some(size.engine) {
                    *slot = None;
                    *slot = Some(engines::launch(size.engine, &url, &w.profiles)?);
                }
                let b = slot.as_mut().expect("launched above");
                b.show(e).with_context(|| format!("worker {}: see {}", w.index, b.log().display()))?;
            }
            if w.control.is_none() {
                w.control = Some(serve_browser_control(Arc::downgrade(&w.browser))?);
            }
        }
        for w in workers.iter() {
            wait_until_ready(w.bridge, Duration::from_secs(240))
                .with_context(|| format!("worker {}'s app did not become ready", w.index))?;
        }
        Ok(())
    })();
    if let Err(e) = prepared {
        note!("{}: {e:#}", size.label);
        return RunResult::new(size.label.clone(), Outcome::Error(format!("{e:#}")));
    }
    note!("running `{}` at {} on {} workers…\n", planned.len(), size.label, workers.len());

    let names: Vec<String> = planned.iter().map(|(n, _)| n.clone()).collect();
    let ignored = |name: &str| planned.iter().any(|(n, i)| n == name && *i);
    let out_lock = Mutex::new(());
    let scratch = std::env::temp_dir().join(format!("idealyst-test-{}-{tag}", std::process::id()));
    let _ = std::fs::create_dir_all(&scratch);
    let na_files: Vec<PathBuf> = workers.iter().map(|w| scratch.join(format!("na-w{}.jsonl", w.index))).collect();
    for f in &na_files {
        let _ = std::fs::remove_file(f);
    }
    let shared: Vec<(usize, u16, SocketAddr, Vec<(String, String)>, Option<SocketAddr>, engines::Engine)> = workers
        .iter()
        .map(|w| (w.index, w.port, w.bridge, w.env.clone(), w.control, size.engine))
        .collect();
    let statuses = run_queue(&names, shared.len(), |slot, name| {
        let (index, port, bridge, env, control, engine) = &shared[slot];
        let log = scratch.join(format!("log-w{index}-{}.txt", sanitize(name)));
        let output = scratch.join(format!("out-w{index}-{}.txt", sanitize(name)));
        let _ = std::fs::remove_file(&log);
        let started = Instant::now();
        let ran = (|| -> Result<std::process::ExitStatus> {
            let file = std::fs::File::create(&output)?;
            let mut cmd = Command::new(test_bin);
            cmd.current_dir(dir)
                .env("CARGO_MANIFEST_DIR", dir)
                .envs(suite_env(size, *port, env))
                .env("IDEALYST_ROBOT_BRIDGE", bridge.to_string())
                .env(robot_test::RESULTS_ENV, &na_files[slot]);
            // Workers are web-only.
            cmd.envs(super::runner_env(*control, true, *engine, "web"));
            cmd.args(["--exact", name]);
            if ignored(name) {
                cmd.arg("--include-ignored");
            }
            cmd.args(["--test-threads=1", "--nocapture"]).arg("--logfile").arg(&log);
            cmd.stdout(file.try_clone()?).stderr(file);
            Ok(cmd.status()?)
        })();
        let status = read_test_log(&log)
            .into_iter()
            .find(|(n, _)| n == name)
            .map(|(_, s)| s)
            // No line: the binary never got to the test (it crashed, or
            // could not start) — a failure, never a quiet pass.
            .unwrap_or(TestStatus::Failed);
        let text = std::fs::read_to_string(&output).unwrap_or_default();
        let _guard = out_lock.lock().unwrap();
        let verdict = match (&ran, status) {
            (Err(e), _) => format!("could not run: {e:#}"),
            (_, TestStatus::Ok) => "ok".into(),
            (_, TestStatus::Failed) => "FAILED".into(),
            (_, TestStatus::Ignored) => "ignored".into(),
        };
        let mut out: Box<dyn std::io::Write> = match suite_log
            .and_then(|p| std::fs::OpenOptions::new().create(true).append(true).open(p).ok())
        {
            Some(f) => Box::new(f),
            None => Box::new(std::io::stdout().lock()),
        };
        let _ = writeln!(out, "── worker {index}: {name} … {verdict} ({:.1}s)", started.elapsed().as_secs_f64());
        if status != TestStatus::Ok {
            let _ = out.write_all(text.as_bytes());
        }
        let _ = std::fs::remove_file(&output);
        let _ = std::fs::remove_file(&log);
        status
    });
    let tests: Vec<(String, TestStatus)> = names.into_iter().zip(statuses).collect();
    let not_applicable = na_files.iter().flat_map(|f| read_not_applicable(f)).collect();
    let _ = std::fs::remove_dir_all(&scratch);
    let failed = tests.iter().any(|(_, s)| *s == TestStatus::Failed);
    RunResult {
        label: size.label.clone(),
        outcome: if failed { Outcome::Failed(Some(101)) } else { Outcome::Passed },
        tests,
        not_applicable,
    }
}

/// A test name as a file-name part.
fn sanitize(name: &str) -> String {
    name.chars().map(|c| if c.is_ascii_alphanumeric() || c == '_' { c } else { '-' }).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pairs(p: &[(&str, &str)]) -> Vec<(String, String)> {
        p.iter().map(|(k, v)| (k.to_string(), v.to_string())).collect()
    }

    /// `{worker}` becomes the worker's number in every value, and the
    /// worker's number is always set — a single-worker run is worker 0.
    #[test]
    fn each_worker_gets_its_number_in_the_environment() {
        let env = pairs(&[("DATABASE_URL", "postgres://db/app_robot_{worker}"), ("PLAIN", "x"), (WORKER_ENV, "9")]);
        let w2 = for_worker(&env, 2);
        assert_eq!(
            w2,
            pairs(&[("DATABASE_URL", "postgres://db/app_robot_2"), ("PLAIN", "x"), (WORKER_ENV, "2")]),
            "a stale worker number is replaced, not doubled"
        );
        assert_eq!(for_worker(&[], 0), pairs(&[(WORKER_ENV, "0")]));
    }

    /// Every name runs exactly once, every worker takes some when there is
    /// enough to go round, and the results come back in the names' order.
    #[test]
    fn the_queue_runs_each_test_once_on_whichever_worker_is_free() {
        let names: Vec<String> = (0..12).map(|i| format!("t{i}")).collect();
        let ran: Mutex<Vec<(usize, String)>> = Mutex::new(Vec::new());
        let out = run_queue(&names, 3, |w, name| {
            // A slow first test: the other workers take the rest meanwhile.
            if name == "t0" {
                std::thread::sleep(Duration::from_millis(150));
            } else {
                std::thread::sleep(Duration::from_millis(10));
            }
            ran.lock().unwrap().push((w, name.to_string()));
            format!("{name}@{w}")
        });
        let ran = ran.into_inner().unwrap();
        let mut seen: Vec<&str> = ran.iter().map(|(_, n)| n.as_str()).collect();
        seen.sort();
        let mut want: Vec<&str> = names.iter().map(String::as_str).collect();
        want.sort();
        assert_eq!(seen, want, "each once");
        let workers: std::collections::HashSet<usize> = ran.iter().map(|(w, _)| *w).collect();
        assert_eq!(workers.len(), 3, "every worker took tests: {ran:?}");
        let t0_worker = ran.iter().find(|(_, n)| n == "t0").unwrap().0;
        assert_eq!(ran.iter().filter(|(w, _)| *w == t0_worker).count(), 1, "the slow test held up only its own worker: {ran:?}");
        for (i, r) in out.iter().enumerate() {
            assert!(r.starts_with(&format!("t{i}@")), "in order: {out:?}");
        }
    }

    /// A worker's bundle is the session's, linked, except an `index.html` of
    /// its own that names the worker's relay and environment; the session's
    /// file is untouched.
    #[test]
    fn a_workers_bundle_names_its_own_relay_and_environment() {
        let root = tempfile::tempdir().unwrap();
        let (from, to) = (root.path().join("web"), root.path().join("w1"));
        std::fs::create_dir_all(from.join("pkg")).unwrap();
        let session_relay = build_web::robot_relay_script_tag("ws://127.0.0.1:1000");
        let session_env = page_env_html(true, &pairs(&[("DB", "app_0"), (WORKER_ENV, "0")]));
        let index = format!("<html><head>{session_relay}{session_env}</head><body></body></html>");
        std::fs::write(from.join("index.html"), &index).unwrap();
        std::fs::write(from.join("pkg/app.wasm"), b"wasm").unwrap();

        let worker_relay = build_web::robot_relay_script_tag("ws://127.0.0.1:2000");
        let worker_env = page_env_html(true, &pairs(&[("DB", "app_1"), (WORKER_ENV, "1")]));
        stage_worker_bundle(
            &from,
            &to,
            &[(session_env.clone(), worker_env.clone()), (session_relay.clone(), worker_relay.clone())],
        )
        .unwrap();
        let staged = std::fs::read_to_string(to.join("index.html")).unwrap();
        assert!(staged.contains(&worker_relay) && staged.contains(&worker_env), "{staged}");
        assert!(!staged.contains("1000") && !staged.contains("app_0"), "{staged}");
        assert_eq!(std::fs::read(to.join("pkg/app.wasm")).unwrap(), b"wasm");
        assert_eq!(std::fs::read_to_string(from.join("index.html")).unwrap(), index, "the session's own is untouched");

        // A tag that is not there: the page would dial worker 0's relay.
        let e = stage_worker_bundle(&from, &to, &[("<script>nope</script>".into(), "x".into())]).unwrap_err();
        assert!(e.to_string().contains("holds no"), "{e}");
    }

    #[test]
    fn the_template_round_trips_through_its_file() {
        let state = tempfile::tempdir().unwrap();
        let mut cmd = Command::new("cargo");
        cmd.args(["run", "--bin", "server"]).env("PORT", "3000").env("IDEALYST_DEV_PARENT", "stdin");
        let t = WorkerTemplate::server(&cmd, Path::new("/x/web"), Some("ws://127.0.0.1:1".into()), "<script></script>".into());
        t.write(state.path()).unwrap();
        assert_eq!(WorkerTemplate::read(state.path(), Duration::from_secs(1)).unwrap(), t);
        let WorkerTemplate::Server { args, env, .. } = &t else { panic!() };
        assert_eq!(args, &["run", "--bin", "server"]);
        assert!(env.contains(&("IDEALYST_DEV_PARENT".into(), "stdin".into())), "{env:?}");
        let missing = tempfile::tempdir().unwrap();
        assert!(WorkerTemplate::read(missing.path(), Duration::from_millis(10)).is_err());
    }
}
