//! Per-project `dev.toml` — local dev-mode configuration that
//! `idealyst dev` reads at startup.
//!
//! Lives at `<project>/dev.toml`. Optional — absence is fine, every
//! field has a default. Today's surface is small (just bridge_port);
//! the file exists so future dev knobs have a home that doesn't
//! pollute `Cargo.toml`'s metadata table.

use std::path::Path;

use anyhow::Result;
use serde::Deserialize;

/// `[package.metadata.idealyst.dev]`: the build settings of this project's
/// `idealyst dev`, so a plain `idealyst dev crates/app` needs no build flags.
///
/// In `Cargo.toml`, not `dev.toml`: the settings choose a build's cache and
/// belong to the project and its other developers, where `dev.toml` holds one
/// machine's ports. Every key is the flag of its name; unknown keys are an
/// error, because a misspelled one otherwise means a different build with no
/// word about it. Absent table: exactly the flags and their defaults.
///
/// ```toml
/// [package.metadata.idealyst.dev]
/// local = true
/// features = ["beta"]
/// debuginfo = "line-tables"
/// dev_opt = "optimized"
/// experimental_build = false
/// ```
///
/// A workspace's `[workspace.metadata.idealyst.dev]` is the default for every
/// member, and a member's own table overrides it key by key (see [`Self::load`]).
#[derive(Debug, Default, Clone, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ProjectBuildConfig {
    pub local: Option<bool>,
    pub features: Option<Vec<String>>,
    /// `--debuginfo`: `line-tables` (or cargo's `line-tables-only`), `full`, `none`.
    pub debuginfo: Option<String>,
    /// `--dev-opt`: `optimized`, `fast`.
    pub dev_opt: Option<String>,
    pub premint: Option<bool>,
    pub premint_only: Option<bool>,
    pub premint_report: Option<bool>,
    pub split: Option<bool>,
    pub no_robot: Option<bool>,
    pub experimental_build: Option<bool>,
}

impl ProjectBuildConfig {
    /// The build `<dir>` declares: `[workspace.metadata.idealyst.dev]` from its
    /// workspace root, with `<dir>/Cargo.toml`'s `[package.metadata.idealyst.dev]`
    /// over it key by key. A key the member sets replaces the workspace's whole
    /// value, so `features` does not add up. `None` when neither table exists or
    /// there is no manifest to read (the commands that need one report that
    /// themselves).
    pub fn load(dir: &Path) -> Result<Option<Self>> {
        let Ok(text) = std::fs::read_to_string(dir.join("Cargo.toml")) else { return Ok(None) };
        let own = Self::table_in(&text, "package")?;
        let shared = match workspace_manifest(dir, &text) {
            Some(root) => Self::table_in(&root, "workspace")?,
            None => None,
        };
        let merged = match (shared, own) {
            (None, None) => return Ok(None),
            (Some(table), None) | (None, Some(table)) => table,
            (Some(mut shared), Some(own)) => {
                shared.extend(own);
                shared
            }
        };
        // Both sides passed `table_in`, so this cannot name a key neither has.
        Ok(Some(toml::Value::Table(merged).try_into()?))
    }

    /// `[<section>.metadata.idealyst.dev]`, checked: an unknown key is an error
    /// that names the table it is in. `None` for no table. A manifest that does
    /// not parse is cargo's to report, in its words.
    fn table_in(manifest: &str, section: &str) -> Result<Option<toml::Table>> {
        let Ok(value) = toml::from_str::<toml::Value>(manifest) else { return Ok(None) };
        let table = value.get(section).and_then(|p| p.get("metadata")).and_then(|m| m.get("idealyst")).and_then(|i| i.get("dev"));
        let Some(table) = table else { return Ok(None) };
        table
            .clone()
            .try_into::<Self>()
            .map_err(|e| anyhow::anyhow!("[{section}.metadata.idealyst.dev]: {e}"))?;
        Ok(table.as_table().cloned())
    }

    #[cfg(test)]
    fn parse(manifest: &str) -> Result<Option<Self>> {
        Ok(Self::table_in(manifest, "package")?.map(|t| toml::Value::Table(t).try_into()).transpose()?)
    }
}

/// The text of the manifest of the workspace `dir` belongs to, found as cargo
/// finds it: `package.workspace` if the manifest names one, else the nearest
/// ancestor with a `[workspace]` table, which may be `dir` itself. `None` for
/// a package outside any workspace, or one the root lists in `exclude`.
fn workspace_manifest(dir: &Path, manifest: &str) -> Option<String> {
    let value: toml::Value = toml::from_str(manifest).ok()?;
    if value.get("workspace").is_some() {
        return Some(manifest.to_string());
    }
    let dir = std::fs::canonicalize(dir).ok()?;
    if let Some(path) = value.get("package").and_then(|p| p.get("workspace")).and_then(|w| w.as_str()) {
        return std::fs::read_to_string(dir.join(path).join("Cargo.toml")).ok();
    }
    for root in dir.ancestors().skip(1) {
        let Ok(text) = std::fs::read_to_string(root.join("Cargo.toml")) else { continue };
        let Ok(parsed) = toml::from_str::<toml::Value>(&text) else { continue };
        let Some(workspace) = parsed.get("workspace") else { continue };
        let relative = dir.strip_prefix(root).ok()?;
        let excluded = workspace
            .get("exclude")
            .and_then(|e| e.as_array())
            .is_some_and(|list| list.iter().filter_map(|e| e.as_str()).any(|e| relative.starts_with(e)));
        return (!excluded).then_some(text);
    }
    None
}

#[derive(Debug, Default, Deserialize)]
pub struct DevConfig {
    /// Optional fixed port for the Robot bridge. When set, the
    /// running app tries to bind exactly this port (and falls back
    /// to ephemeral with a warning if it's already taken). When
    /// unset, the bridge always picks ephemeral. Pin this only if
    /// an external tool needs a stable target — for normal Claude
    /// workflows the ephemeral + `.idealyst/bridge.port` discovery
    /// path is preferable.
    #[serde(default)]
    pub bridge_port: Option<u16>,

    /// A fixed port for the dev session's page stream (livereload,
    /// overlay and hot patches, the build badge) in a FULL-STACK session,
    /// bound on every interface inside a container. Overridden by
    /// `idealyst dev --stream-port`.
    ///
    /// Only needed when the project's server does not proxy
    /// `/__idealyst/*` itself (servers built on the framework's
    /// `server::router()` do, and the page then uses its own origin).
    /// Without either, the stream is on a random loopback port, which a
    /// devcontainer does not forward: `idealyst configure devcontainer`
    /// forwards this one, labelled.
    ///
    /// ```toml
    /// stream_port = 4777
    /// ```
    #[serde(default)]
    pub stream_port: Option<u16>,

    /// A fixed port for the dev session's Robot relay — the TCP bridge the
    /// MCP server and the Inspector connect to — bound on every interface
    /// inside a container. Overridden by `idealyst dev --robot-port`.
    /// Not `bridge_port`, which pins the bridge a NATIVE app hosts itself
    /// when no relay runs.
    ///
    /// Without it the relay's port is random and on loopback: fine when
    /// the MCP server runs where `idealyst dev` does, out of reach from a
    /// devcontainer's host. With it, the session also writes
    /// `<project>/.idealyst/robot.json`, which an MCP server on the host
    /// finds through the shared project directory, and `idealyst configure
    /// devcontainer` forwards the port, labelled.
    ///
    /// ```toml
    /// robot_port = 4778
    /// ```
    #[serde(default)]
    pub robot_port: Option<u16>,

    /// Local job-queue backend for `idealyst dev` / `idealyst worker`. Absent
    /// means the in-process `memory` backend: `dev` runs workers inside the
    /// server process (no separate worker process is spawned, since two
    /// processes can't share an in-memory queue). Point this at a real broker
    /// (redis / postgres / sqs) to have `dev` auto-spawn a dedicated worker
    /// process that shares the queue with the server.
    ///
    /// ```toml
    /// [jobs]
    /// backend = "redis"
    /// url = "redis://127.0.0.1:6379"
    /// ```
    #[serde(default)]
    pub jobs: Option<JobsConfig>,

    /// Local publish/subscribe backend for `idealyst dev`. Absent → the
    /// in-process `memory` backend (single instance). Point at redis/postgres
    /// for cross-instance fan-out (and to make multi-instance dev meaningful).
    ///
    /// ```toml
    /// [pubsub]
    /// backend = "redis"
    /// url = "redis://127.0.0.1:6379"
    /// ```
    #[serde(default)]
    pub pubsub: Option<PubsubConfig>,

    /// Local cache backend for `idealyst dev`. Absent → the in-process
    /// `memory` cache. Point at redis to share cached entries across
    /// instances (and exercise the shared-cache path in dev).
    ///
    /// ```toml
    /// [cache]
    /// backend = "redis"
    /// url = "redis://127.0.0.1:6379"
    /// ```
    #[serde(default)]
    pub cache: Option<CacheConfig>,

    /// Shell commands run on dev-session events: the simplest way to hook
    /// into a session from outside. Each key is an event kind — an event's
    /// `type` (`patch_built`, `build_finished`, …) or one of the derived
    /// kinds `build_failed` and `patched` (any tier: overlay, hot patch,
    /// sidecar) — and each value a command, run with `sh -c` in the
    /// project directory with the event's JSON object (the same one
    /// `--events-file` writes) on stdin and its kind in
    /// `IDEALYST_EVENT_KIND`. Hooks run in the background; one that fails
    /// is reported as a warning and the session carries on.
    ///
    /// ```toml
    /// [hooks]
    /// build_failed = "osascript -e 'display notification \"build failed\"'"
    /// patched = "jq -r .type >> .idealyst/patches.log"
    /// ```
    #[serde(default)]
    pub hooks: std::collections::BTreeMap<String, String>,
}

/// The `[cache]` block in `dev.toml`.
#[derive(Debug, Default, Deserialize, Clone)]
pub struct CacheConfig {
    /// `memory` (default) | `redis`.
    #[serde(default)]
    pub backend: Option<String>,
    /// Redis URL. Unused for `memory`.
    #[serde(default)]
    pub url: Option<String>,
}

/// The `[pubsub]` block in `dev.toml`.
#[derive(Debug, Default, Deserialize, Clone)]
pub struct PubsubConfig {
    /// `memory` (default) | `redis` | `postgres`.
    #[serde(default)]
    pub backend: Option<String>,
    /// Connection string for the broker (redis URL / Postgres URL). Unused for
    /// `memory`.
    #[serde(default)]
    pub url: Option<String>,
}

/// The `[jobs]` block in `dev.toml`.
#[derive(Debug, Default, Deserialize, Clone)]
pub struct JobsConfig {
    /// `memory` (default) | `redis` | `postgres` | `sqs`.
    #[serde(default)]
    pub backend: Option<String>,
    /// Connection string for the broker (redis URL, Postgres URL, SQS queue
    /// URL). Unused for `memory`.
    #[serde(default)]
    pub url: Option<String>,
}

impl JobsConfig {
    /// Whether this backend is shared across processes (so a dedicated worker
    /// process can drain the same queue the server enqueues to). `memory` is
    /// per-process and therefore not shared.
    pub fn is_shared(&self) -> bool {
        matches!(
            self.backend.as_deref(),
            Some("redis") | Some("postgres") | Some("sqs")
        )
    }
}

impl DevConfig {
    /// Load `<dir>/dev.toml`, then overlay the unified `idealyst.toml`
    /// (+ per-tool files) via the shared config loader.
    ///
    /// `dev.toml` keeps the dev-only knobs (`bridge_port`) and, for backward
    /// compatibility, still supplies `[jobs]`/`[pubsub]`. The newer
    /// `idealyst.toml` is the primary surface: when it declares `[jobs]` /
    /// `[pubsub]`, those win (with any `connection` reference resolved to a URL
    /// here so the worker-spawn decision + env bridge keep working). A migrated
    /// app reads `idealyst.toml` itself via `idealyst_config::configure_all()`;
    /// the CLI reads it only to decide whether to spawn a dedicated worker.
    ///
    /// Missing files → `Default`; parse errors are surfaced so typos don't
    /// silently no-op.
    pub fn load(dir: &Path) -> Result<Self> {
        // 1. Legacy dev.toml (dev-only knobs + optional jobs/pubsub).
        let mut cfg = {
            let path = dir.join("dev.toml");
            if path.is_file() {
                let raw = std::fs::read_to_string(&path)?;
                toml::from_str::<DevConfig>(&raw)?
            } else {
                DevConfig::default()
            }
        };

        // 2. Unified idealyst.toml (+ per-tool files) — overlays jobs/pubsub.
        let unified = idealyst_config::load_from(dir)?;
        if let Some(jobs) = &unified.jobs {
            // Resolve a `connection` ref (redis/postgres URL) or fall back to an
            // inline `url` / the SQS `queue_url`, so the env bridge has a URL.
            let url = unified
                .url_for(jobs.connection.as_deref(), jobs.url.as_deref())
                .ok()
                .or_else(|| jobs.queue_url.clone());
            cfg.jobs = Some(JobsConfig { backend: jobs.backend.clone(), url });
        }
        if let Some(pubsub) = &unified.pubsub {
            let url = unified
                .url_for(pubsub.connection.as_deref(), pubsub.url.as_deref())
                .ok();
            cfg.pubsub = Some(PubsubConfig { backend: pubsub.backend.clone(), url });
        }
        if let Some(cache) = &unified.cache {
            let url = unified
                .url_for(cache.connection.as_deref(), cache.url.as_deref())
                .ok();
            cfg.cache = Some(CacheConfig { backend: cache.backend.clone(), url });
        }

        Ok(cfg)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    #[test]
    fn the_build_table_is_optional_and_strict() {
        let no_table = "[package]\nname = \"a\"\n[package.metadata.idealyst.app]\nname = \"A\"\n";
        assert_eq!(ProjectBuildConfig::parse(no_table).unwrap(), None);
        let table = "[package.metadata.idealyst.dev]\nlocal = true\nfeatures = [\"beta\"]\ndebuginfo = \"line-tables-only\"\n";
        let config = ProjectBuildConfig::parse(table).unwrap().unwrap();
        assert_eq!(config.local, Some(true));
        assert_eq!(config.features.as_deref(), Some(&["beta".to_string()][..]));
        assert_eq!(config.debuginfo.as_deref(), Some("line-tables-only"));
        // A typo is an error rather than a different build. So is a behaviour
        // flag, which stays a flag.
        for bad in ["loca = true", "no_watch = true", "port = 3000"] {
            let text = format!("[package.metadata.idealyst.dev]\n{bad}\n");
            let e = ProjectBuildConfig::parse(&text).unwrap_err().to_string();
            assert!(e.contains("[package.metadata.idealyst.dev]"), "{e}");
        }
        assert_eq!(ProjectBuildConfig::parse("not toml [[").unwrap(), None, "cargo reports that");
    }

    /// Cargo has no inheritance for `package.metadata`, so a workspace with
    /// several apps repeated the table in each.
    #[test]
    fn regression_a_workspace_table_is_every_members_default() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        let write = |path: &str, text: &str| {
            let file = root.join(path).join("Cargo.toml");
            std::fs::create_dir_all(file.parent().unwrap()).unwrap();
            std::fs::write(file, text).unwrap();
        };
        write(
            "",
            "[workspace]\nmembers = [\"apps/*\"]\nexclude = [\"apps/out\"]\n\
             [workspace.metadata.idealyst.dev]\nlocal = true\nfeatures = [\"beta\"]\npremint = true\n",
        );
        write("apps/plain", "[package]\nname = \"plain\"\n");
        write(
            "apps/own",
            "[package]\nname = \"own\"\n[package.metadata.idealyst.dev]\nfeatures = [\"mine\"]\npremint = false\n",
        );
        write("apps/out", "[package]\nname = \"out\"\n");

        let load = |path: &str| ProjectBuildConfig::load(&root.join(path)).unwrap();
        let plain = load("apps/plain").expect("inherits the workspace's table");
        assert_eq!(plain.local, Some(true));
        assert_eq!(plain.features.as_deref(), Some(&["beta".to_string()][..]));

        let own = load("apps/own").unwrap();
        assert_eq!(own.local, Some(true), "a key it does not set is inherited");
        assert_eq!(own.premint, Some(false), "a key it sets wins");
        assert_eq!(own.features.as_deref(), Some(&["mine".to_string()][..]), "replaced, not added to");

        assert_eq!(load("apps/out"), None, "an excluded package is not a member");
        assert_eq!(load("").unwrap().local, Some(true), "a root package is its own workspace");
    }

    /// A package that names its workspace uses that one, wherever it sits.
    #[test]
    fn a_package_workspace_key_picks_the_root() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        std::fs::create_dir_all(root.join("ws")).unwrap();
        std::fs::create_dir_all(root.join("app")).unwrap();
        std::fs::write(root.join("ws/Cargo.toml"), "[workspace]\n[workspace.metadata.idealyst.dev]\nsplit = true\n").unwrap();
        std::fs::write(root.join("app/Cargo.toml"), "[package]\nname = \"a\"\nworkspace = \"../ws\"\n").unwrap();
        assert_eq!(ProjectBuildConfig::load(&root.join("app")).unwrap().unwrap().split, Some(true));
    }

    /// A typo in the shared table is an error naming that table.
    #[test]
    fn a_typo_in_the_workspace_table_names_it() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        std::fs::create_dir_all(root.join("app")).unwrap();
        std::fs::write(root.join("Cargo.toml"), "[workspace]\n[workspace.metadata.idealyst.dev]\nloca = true\n").unwrap();
        std::fs::write(root.join("app/Cargo.toml"), "[package]\nname = \"a\"\n").unwrap();
        let e = ProjectBuildConfig::load(&root.join("app")).unwrap_err().to_string();
        assert!(e.contains("[workspace.metadata.idealyst.dev]"), "{e}");
    }

    /// `idealyst.toml`'s `[cache]` overlays into DevConfig with its named
    /// connection resolved to a URL, exactly like `[pubsub]` — so the
    /// `IDEALYST_CACHE_*` env bridge to the spawned server/worker works.
    #[test]
    fn cache_section_resolves_connection_for_env_bridge() {
        let dir = std::env::temp_dir().join("idealyst-devconfig-test-cache");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let mut f = std::fs::File::create(dir.join("idealyst.toml")).unwrap();
        f.write_all(
            br#"
            [connections.main]
            kind = "redis"
            url = "redis://127.0.0.1:6379"

            [cache]
            backend = "redis"
            connection = "main"

            [pubsub]
            backend = "redis"
            connection = "main"
            "#,
        )
        .unwrap();

        let cfg = DevConfig::load(&dir).unwrap();
        let cache = cfg.cache.expect("[cache] carried into DevConfig");
        assert_eq!(cache.backend.as_deref(), Some("redis"));
        assert_eq!(cache.url.as_deref(), Some("redis://127.0.0.1:6379"));
        // Same profile, same URL as pubsub — the shared-connection contract.
        assert_eq!(cfg.pubsub.unwrap().url.as_deref(), cache.url.as_deref());
    }

    /// `robot_port` in `dev.toml` pins the dev relay's bridge (the flag
    /// overrides it); it is not `bridge_port`, a native app's own.
    #[test]
    fn robot_port_is_read_from_dev_toml() {
        let dir = std::env::temp_dir().join("idealyst-devconfig-test-robot-port");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("dev.toml"), "robot_port = 4778\n").unwrap();
        let cfg = DevConfig::load(&dir).unwrap();
        assert_eq!(cfg.robot_port, Some(4778));
        assert_eq!(cfg.bridge_port, None);
    }
}
