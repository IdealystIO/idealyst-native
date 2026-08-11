//! Integration tests for the devcontainer configuration engine.
//!
//! These exercise the pure plan → apply core (no TTY): fresh init, service
//! add/remove/reconfigure, re-run preselect state, the already-configured
//! no-op, and — critically — that a user's own compose file and services are
//! never touched.

use std::path::Path;

use configure::devcontainer::{self, Action, ConfigureRequest, ServiceRequest};

fn enable(id: &str) -> ServiceRequest {
    ServiceRequest { id: id.into(), variant: None, action: Action::Enable }
}
fn enable_variant(id: &str, variant: &str) -> ServiceRequest {
    ServiceRequest { id: id.into(), variant: Some(variant.into()), action: Action::Enable }
}
fn remove(id: &str) -> ServiceRequest {
    ServiceRequest { id: id.into(), variant: None, action: Action::Remove }
}
fn reconfigure(id: &str, variant: Option<&str>) -> ServiceRequest {
    ServiceRequest { id: id.into(), variant: variant.map(Into::into), action: Action::Reconfigure }
}
fn req(services: Vec<ServiceRequest>) -> ConfigureRequest {
    ConfigureRequest { services, config: None }
}

fn managed(dir: &Path) -> String {
    std::fs::read_to_string(dir.join(".devcontainer/docker-compose.idealyst.yml")).unwrap()
}
fn devcontainer_json(dir: &Path) -> serde_json::Value {
    let text =
        std::fs::read_to_string(dir.join(".devcontainer/devcontainer.json")).unwrap();
    serde_json::from_str(&text).unwrap()
}

/// Stand up a repo-shaped tree: a default devcontainer AND a named config
/// (like this repo's `.devcontainer/arena/`), both compose-based.
fn write_named_config(dir: &Path, name: &str) {
    let sub = dir.join(".devcontainer").join(name);
    std::fs::create_dir_all(&sub).unwrap();
    std::fs::write(
        sub.join("devcontainer.json"),
        serde_json::to_string_pretty(&serde_json::json!({
            "name": name,
            "dockerComposeFile": ["../docker-compose.yml", format!("docker-compose.{name}.yml")],
            "service": "dev"
        }))
        .unwrap(),
    )
    .unwrap();
}

#[test]
fn named_config_gets_a_relative_managed_reference() {
    let tmp = tempfile::tempdir().unwrap();
    let dir = tmp.path();
    // Default devcontainer exists (scaffolded by a plain enable+remove cycle
    // would churn; just init base via an empty request), plus a named config.
    devcontainer::apply(dir, &req(vec![])).unwrap();
    write_named_config(dir, "arena");

    let report = devcontainer::apply(
        dir,
        &ConfigureRequest {
            services: vec![enable_variant("database", "postgres")],
            config: Some("arena".into()),
        },
    )
    .unwrap();
    assert_eq!(report.added, vec!["database".to_string()]);

    // The managed file is SHARED — it lands at .devcontainer/, not inside the
    // named config's directory.
    assert!(dir.join(".devcontainer/docker-compose.idealyst.yml").exists());
    assert!(!dir.join(".devcontainer/arena/docker-compose.idealyst.yml").exists());

    // The named config references it RELATIVE to its own json (../).
    let text =
        std::fs::read_to_string(dir.join(".devcontainer/arena/devcontainer.json")).unwrap();
    let dc: serde_json::Value = serde_json::from_str(&text).unwrap();
    let files = dc.get("dockerComposeFile").unwrap().as_array().unwrap();
    assert!(files.iter().any(|v| v == "../docker-compose.idealyst.yml"));

    // The DEFAULT config's json is untouched — no managed reference added.
    let root = devcontainer_json(dir);
    match root.get("dockerComposeFile").unwrap() {
        serde_json::Value::String(s) => assert!(!s.contains("idealyst")),
        serde_json::Value::Array(a) => {
            assert!(!a.iter().any(|v| v.as_str().unwrap_or("").contains("idealyst")))
        }
        other => panic!("unexpected dockerComposeFile shape: {other:?}"),
    }
}

#[test]
fn named_config_removal_drops_the_relative_reference() {
    let tmp = tempfile::tempdir().unwrap();
    let dir = tmp.path();
    devcontainer::apply(dir, &req(vec![])).unwrap();
    write_named_config(dir, "arena");

    let cfg = |services| ConfigureRequest { services, config: Some("arena".into()) };
    devcontainer::apply(dir, &cfg(vec![enable("redis")])).unwrap();
    devcontainer::apply(dir, &cfg(vec![remove("redis")])).unwrap();

    assert!(!dir.join(".devcontainer/docker-compose.idealyst.yml").exists());
    let text =
        std::fs::read_to_string(dir.join(".devcontainer/arena/devcontainer.json")).unwrap();
    assert!(!text.contains("idealyst"), "reference should be gone: {text}");
}

#[test]
fn missing_named_config_errors_instead_of_scaffolding() {
    let tmp = tempfile::tempdir().unwrap();
    let dir = tmp.path();
    let err = devcontainer::apply(
        dir,
        &ConfigureRequest { services: vec![enable("redis")], config: Some("arena".into()) },
    )
    .unwrap_err();
    assert!(err.to_string().contains("arena"), "unexpected error: {err:#}");
    assert!(!dir.join(".devcontainer/arena").exists(), "must not scaffold named configs");
}

#[test]
fn service_fragments_have_expected_shape() {
    let db = devcontainer::service::find("database").unwrap();
    // Default (postgres) + explicit mysql.
    let pg = db.fragment(None, &devcontainer::service::Ctx { app_service: "dev".into() });
    let pg_service = pg.service.unwrap();
    assert_eq!(pg_service.get("image").unwrap().as_str().unwrap(), "postgres:16");
    assert!(pg.app_env.iter().any(|(k, v)| k == "DATABASE_URL" && v.starts_with("postgres://")));
    assert_eq!(pg.volumes, vec!["idealyst-database-data".to_string()]);

    let my = db.fragment(Some("mysql"), &devcontainer::service::Ctx { app_service: "dev".into() });
    assert_eq!(my.service.unwrap().get("image").unwrap().as_str().unwrap(), "mysql:8");
    assert!(my.app_env.iter().any(|(k, v)| k == "DATABASE_URL" && v.starts_with("mysql://")));

    assert!(devcontainer::service::find("redis").is_some());
    assert!(devcontainer::service::find("minio").is_some());
    assert!(devcontainer::service::find("playwright").is_some());
    assert!(devcontainer::service::find("claude").is_some());
    assert!(devcontainer::service::find("codex").is_some());
    assert!(devcontainer::service::find("nope").is_none());
}

#[test]
fn playwright_sidecar_serves_mcp_on_the_compose_network() {
    let tmp = tempfile::tempdir().unwrap();
    let dir = tmp.path();
    let report = devcontainer::apply(dir, &req(vec![enable("playwright")])).unwrap();
    assert_eq!(report.added, vec!["playwright".to_string()]);

    let m = managed(dir);
    assert!(m.contains("mcr.microsoft.com/playwright/mcp"), "official MCP image:\n{m}");
    // The dev container dials the sidecar by service name; the server must
    // bind beyond localhost AND allowlist that Host header, or every request
    // is rejected 403 by its DNS-rebinding guard ("Access is only allowed at
    // localhost:8931").
    assert!(m.contains("0.0.0.0"), "must bind all interfaces:\n{m}");
    assert!(m.contains("--allowed-hosts"), "host allowlist flag:\n{m}");
    assert!(m.contains("playwright:8931"), "compose-network host allowlisted:\n{m}");
    assert!(
        m.contains("PLAYWRIGHT_MCP_URL") && m.contains("http://playwright:8931/mcp"),
        "agents find the endpoint via env:\n{m}"
    );
    // Chromium in this image runs WITHOUT --disable-dev-shm-usage, so the
    // default 64 MB /dev/shm crashes tabs on heavy pages — the fragment must
    // size shm explicitly.
    assert!(m.contains("shm_size"), "shm sizing for chromium:\n{m}");
    // A sidecar (unlike the agent tools) gets a depends_on edge and touches
    // nothing in devcontainer.json beyond the compose-file reference.
    assert!(m.contains("depends_on"), "sidecar dependency edge:\n{m}");
    let dc = devcontainer_json(dir);
    assert!(dc.get("features").is_none(), "no features: {dc}");
    assert!(dc.get("postCreateCommand").is_none(), "no postCreate: {dc}");

    devcontainer::apply(dir, &req(vec![remove("playwright")])).unwrap();
    assert!(!dir.join(".devcontainer/docker-compose.idealyst.yml").exists());
}

// --- AI agent CLIs (claude / codex) ---

#[test]
fn claude_host_variant_installs_natively_and_mounts_host_config() {
    let tmp = tempfile::tempdir().unwrap();
    let dir = tmp.path();
    let report = devcontainer::apply(dir, &req(vec![enable("claude")])).unwrap();
    assert_eq!(report.added, vec!["claude".to_string()]);

    // Claude Code installs via the NATIVE installer (npm distribution is
    // deprecated; Anthropic's devcontainer feature still wraps it and its
    // Node auto-install breaks on the rust base image) — a keyed postCreate
    // entry, no devcontainer feature at all.
    let dc = devcontainer_json(dir);
    assert!(dc.get("features").is_none(), "claude must not add features: {dc}");
    let post = dc.get("postCreateCommand").unwrap().as_object().unwrap();
    let install = post.get("idealyst-claude-install").unwrap().as_str().unwrap();
    assert!(install.contains("claude.ai/install.sh"), "native installer expected: {install}");
    assert!(!install.contains("npm"), "must not use the deprecated npm install: {install}");
    // No chown for the host variant (bind mounts carry host ownership).
    assert!(!post.contains_key("idealyst-claude-chown"));

    // The managed compose file mounts the host config dir onto the app
    // service and points CLAUDE_CONFIG_DIR at it — this is what carries the
    // host's credentials into the container.
    let m = managed(dir);
    assert!(m.contains("~/.claude:/idealyst/agents/claude"), "host bind mount missing:\n{m}");
    assert!(m.contains("CLAUDE_CONFIG_DIR"), "env missing:\n{m}");
    assert!(!m.contains("depends_on"), "tools must not add a depends_on edge:\n{m}");

    // Ownership of the postCreate key is recorded for later removal.
    assert!(m.contains("idealyst-claude-install"));
}

#[test]
fn claude_volume_variant_uses_named_volume_and_chown() {
    let tmp = tempfile::tempdir().unwrap();
    let dir = tmp.path();
    devcontainer::apply(dir, &req(vec![enable_variant("claude", "volume")])).unwrap();

    let m = managed(dir);
    assert!(m.contains("idealyst-claude-config:/idealyst/agents/claude"), "volume mount:\n{m}");
    assert!(m.contains("idealyst-claude-config:"), "named volume declared:\n{m}");
    assert!(!m.contains("~/.claude"), "volume variant must not touch the host dir:\n{m}");

    // Fresh named volumes mount root-owned → a keyed postCreate chown hands
    // the dir to the container user so the CLI can write credentials.
    let dc = devcontainer_json(dir);
    let post = dc.get("postCreateCommand").unwrap().as_object().unwrap();
    assert!(post.get("idealyst-claude-chown").unwrap().as_str().unwrap().contains("chown"));
}

#[test]
fn codex_installs_node_feature_and_npm_post_create() {
    let tmp = tempfile::tempdir().unwrap();
    let dir = tmp.path();
    devcontainer::apply(dir, &req(vec![enable("codex")])).unwrap();

    let dc = devcontainer_json(dir);
    let features = dc.get("features").unwrap().as_object().unwrap();
    assert!(features.contains_key("ghcr.io/devcontainers/features/node:1"), "{features:?}");
    let post = dc.get("postCreateCommand").unwrap().as_object().unwrap();
    assert!(
        post.get("idealyst-codex-install").unwrap().as_str().unwrap().contains("@openai/codex")
    );

    let m = managed(dir);
    assert!(m.contains("~/.codex:/idealyst/agents/codex"), "host bind mount:\n{m}");
    assert!(m.contains("CODEX_HOME"), "env:\n{m}");
}

#[test]
fn removing_agent_drops_only_idealyst_owned_json_keys() {
    let tmp = tempfile::tempdir().unwrap();
    let dir = tmp.path();
    devcontainer::apply(dir, &req(vec![enable("claude"), enable("codex"), enable("redis")]))
        .unwrap();

    devcontainer::apply(dir, &req(vec![remove("claude"), remove("codex")])).unwrap();

    // Everything the agents added to devcontainer.json is gone again…
    let dc = devcontainer_json(dir);
    assert!(dc.get("features").is_none(), "features should be empty+removed: {dc}");
    assert!(dc.get("postCreateCommand").is_none(), "postCreate should be removed: {dc}");
    // …while redis (and its managed file) survives, with no agent leftovers.
    let m = managed(dir);
    assert!(m.contains("redis:7"));
    assert!(!m.contains("CLAUDE_CONFIG_DIR") && !m.contains("CODEX_HOME"), "{m}");
    assert!(!m.contains("idealyst-claude-config") && !m.contains("idealyst-codex-config"), "{m}");
}

#[test]
fn user_owned_feature_and_post_create_survive_agent_lifecycle() {
    let tmp = tempfile::tempdir().unwrap();
    let dir = tmp.path();

    // User already installs the node feature (their own pin, different tag)
    // and has a bare-string postCreateCommand.
    std::fs::create_dir_all(dir.join(".devcontainer")).unwrap();
    std::fs::write(
        dir.join(".devcontainer/devcontainer.json"),
        serde_json::to_string_pretty(&serde_json::json!({
            "name": "myapp",
            "dockerComposeFile": "docker-compose.yml",
            "service": "dev",
            "features": { "ghcr.io/devcontainers/features/node:1.6": {} },
            "postCreateCommand": "cargo fetch"
        }))
        .unwrap(),
    )
    .unwrap();
    std::fs::write(
        dir.join(".devcontainer/docker-compose.yml"),
        "services:\n  dev:\n    image: rust:1\n",
    )
    .unwrap();

    devcontainer::apply(dir, &req(vec![enable("claude"), enable("codex")])).unwrap();

    // The user's pinned node feature is respected — no duplicate `:1` entry
    // added for codex.
    let dc = devcontainer_json(dir);
    let features = dc.get("features").unwrap().as_object().unwrap();
    assert!(features.contains_key("ghcr.io/devcontainers/features/node:1.6"));
    assert!(
        !features.contains_key("ghcr.io/devcontainers/features/node:1"),
        "must not duplicate a feature the user already has: {features:?}"
    );
    // Their bare-string command was converted to a keyed entry beside ours.
    let post = dc.get("postCreateCommand").unwrap().as_object().unwrap();
    assert_eq!(post.get("main").unwrap().as_str(), Some("cargo fetch"));
    assert!(post.contains_key("idealyst-claude-install"));
    assert!(post.contains_key("idealyst-codex-install"));

    devcontainer::apply(dir, &req(vec![remove("claude"), remove("codex")])).unwrap();

    // Removal deletes only what idealyst added: the user's pinned feature and
    // converted command remain.
    let dc = devcontainer_json(dir);
    let features = dc.get("features").unwrap().as_object().unwrap();
    assert!(
        features.contains_key("ghcr.io/devcontainers/features/node:1.6"),
        "user's own feature must survive agent removal: {features:?}"
    );
    let post = dc.get("postCreateCommand").unwrap().as_object().unwrap();
    assert_eq!(post.get("main").unwrap().as_str(), Some("cargo fetch"));
    assert!(!post.contains_key("idealyst-claude-install"));
    assert!(!post.contains_key("idealyst-codex-install"));
}

#[test]
fn idealyst_cli_installs_from_git_with_cached_volume() {
    let tmp = tempfile::tempdir().unwrap();
    let dir = tmp.path();
    devcontainer::apply(dir, &req(vec![enable("idealyst-cli")])).unwrap();

    // Source build in a keyed postCreate, guarded so the cached volume skips
    // the rebuild, symlinked onto PATH.
    let dc = devcontainer_json(dir);
    let post = dc.get("postCreateCommand").unwrap().as_object().unwrap();
    let install = post.get("idealyst-cli-install").unwrap().as_str().unwrap();
    assert!(install.contains("cargo install --git"), "{install}");
    assert!(install.contains("test -x /idealyst/cli/bin/idealyst ||"), "cache guard: {install}");
    assert!(install.contains("/usr/local/bin/idealyst"), "PATH symlink: {install}");

    // Install root rides a named volume so the build survives rebuilds.
    let m = managed(dir);
    assert!(m.contains("idealyst-cli-cache:/idealyst/cli"), "volume mount:\n{m}");

    devcontainer::apply(dir, &req(vec![remove("idealyst-cli")])).unwrap();
    let dc = devcontainer_json(dir);
    assert!(dc.get("postCreateCommand").is_none(), "cleanup: {dc}");
}

#[test]
fn agent_enable_is_idempotent_byte_stable() {
    let tmp = tempfile::tempdir().unwrap();
    let dir = tmp.path();
    devcontainer::apply(dir, &req(vec![enable("claude")])).unwrap();
    let first_managed = managed(dir);
    let first_json = std::fs::read_to_string(dir.join(".devcontainer/devcontainer.json")).unwrap();

    let report = devcontainer::apply(dir, &req(vec![enable("claude")])).unwrap();
    assert!(report.warnings.iter().any(|w| w.contains("already configured")));
    assert_eq!(first_managed, managed(dir), "managed file must be byte-stable");
    assert_eq!(
        first_json,
        std::fs::read_to_string(dir.join(".devcontainer/devcontainer.json")).unwrap(),
        "devcontainer.json must be untouched on a no-op re-run"
    );
}

#[test]
fn fresh_init_scaffolds_base_and_managed_files() {
    let tmp = tempfile::tempdir().unwrap();
    let dir = tmp.path();

    let report = devcontainer::apply(dir, &req(vec![enable_variant("database", "postgres"), enable("minio")]))
        .unwrap();

    // Base devcontainer + user compose + managed file all exist.
    assert!(dir.join(".devcontainer/devcontainer.json").exists());
    assert!(dir.join(".devcontainer/docker-compose.yml").exists());
    assert!(dir.join(".devcontainer/docker-compose.idealyst.yml").exists());

    // devcontainer.json references both compose files.
    let dc = devcontainer_json(dir);
    let files = dc.get("dockerComposeFile").unwrap().as_array().unwrap();
    assert!(files.iter().any(|v| v == "docker-compose.yml"));
    assert!(files.iter().any(|v| v == "docker-compose.idealyst.yml"));

    // Managed file carries both services + the dev override env.
    let m = managed(dir);
    assert!(m.contains("postgres:16"));
    assert!(m.contains("minio/minio"));
    assert!(m.contains("DATABASE_URL"));
    assert!(m.contains("MINIO_ENDPOINT"));
    assert!(m.contains("MANAGED BY IDEALYST"));

    assert_eq!(report.added.len(), 2);
    assert!(report.removed.is_empty());
}

#[test]
fn read_state_round_trips_enabled_set() {
    let tmp = tempfile::tempdir().unwrap();
    let dir = tmp.path();
    devcontainer::apply(dir, &req(vec![enable_variant("database", "mysql"), enable("redis")]))
        .unwrap();

    let state = devcontainer::read_state(dir, None).unwrap();
    assert!(state.exists);
    let db = state.enabled.iter().find(|e| e.id == "database").unwrap();
    assert_eq!(db.variant.as_deref(), Some("mysql"));
    assert!(state.enabled.iter().any(|e| e.id == "redis"));
}

#[test]
fn enable_already_configured_is_warned_noop() {
    let tmp = tempfile::tempdir().unwrap();
    let dir = tmp.path();
    devcontainer::apply(dir, &req(vec![enable("minio")])).unwrap();
    let first = managed(dir);

    let report = devcontainer::apply(dir, &req(vec![enable("minio")])).unwrap();
    let second = managed(dir);

    assert_eq!(first, second, "managed file must be byte-identical on no-op re-enable");
    assert!(report.warnings.iter().any(|w| w.contains("already configured")));
    assert!(report.added.is_empty());
    assert!(report.unchanged.contains(&"minio".to_string()));
}

#[test]
fn reconfigure_resets_variant_and_remove_drops_service() {
    let tmp = tempfile::tempdir().unwrap();
    let dir = tmp.path();
    devcontainer::apply(dir, &req(vec![enable_variant("database", "postgres"), enable("redis")]))
        .unwrap();

    // Reconfigure database → mysql.
    let report = devcontainer::apply(dir, &req(vec![reconfigure("database", Some("mysql"))])).unwrap();
    assert!(report.reconfigured.contains(&"database".to_string()));
    assert!(managed(dir).contains("mysql:8"));

    // Remove redis.
    let report = devcontainer::apply(dir, &req(vec![remove("redis")])).unwrap();
    assert!(report.removed.contains(&"redis".to_string()));
    assert!(!managed(dir).contains("redis:7"));
    assert!(managed(dir).contains("mysql:8"), "database survives redis removal");
}

#[test]
fn removing_last_service_deletes_managed_file_and_dereferences() {
    let tmp = tempfile::tempdir().unwrap();
    let dir = tmp.path();
    devcontainer::apply(dir, &req(vec![enable("redis")])).unwrap();
    assert!(dir.join(".devcontainer/docker-compose.idealyst.yml").exists());

    devcontainer::apply(dir, &req(vec![remove("redis")])).unwrap();

    // Managed file gone; devcontainer.json no longer references it; base kept.
    assert!(!dir.join(".devcontainer/docker-compose.idealyst.yml").exists());
    assert!(dir.join(".devcontainer/devcontainer.json").exists());
    let dc = devcontainer_json(dir);
    let refs = dc.get("dockerComposeFile").unwrap();
    // Collapsed back to a bare string (only base remains).
    assert_eq!(refs.as_str(), Some("docker-compose.yml"));
}

#[test]
fn wiring_is_idempotent_and_leaves_user_compose_untouched() {
    let tmp = tempfile::tempdir().unwrap();
    let dir = tmp.path();

    // Pre-existing, user-authored, compose-based devcontainer with the user's
    // OWN service. Includes a comment (JSONC) to exercise the tolerant parse.
    std::fs::create_dir_all(dir.join(".devcontainer")).unwrap();
    let user_json = r#"{
  // user's own devcontainer
  "name": "myapp",
  "dockerComposeFile": "docker-compose.yml",
  "service": "app"
}"#;
    std::fs::write(dir.join(".devcontainer/devcontainer.json"), user_json).unwrap();
    let user_compose = "services:\n  app:\n    image: rust:1\n  worker:\n    image: rust:1\n";
    std::fs::write(dir.join(".devcontainer/docker-compose.yml"), user_compose).unwrap();

    devcontainer::apply(dir, &req(vec![enable("redis")])).unwrap();
    devcontainer::apply(dir, &req(vec![enable("redis")])).unwrap(); // idempotent

    // User's base compose is byte-for-byte untouched, including `worker`.
    assert_eq!(
        std::fs::read_to_string(dir.join(".devcontainer/docker-compose.yml")).unwrap(),
        user_compose
    );

    // devcontainer.json references the managed file exactly once, and the
    // override targets the user's app service name (`app`, not `dev`).
    let dc = devcontainer_json(dir);
    let files = dc.get("dockerComposeFile").unwrap().as_array().unwrap();
    assert_eq!(
        files.iter().filter(|v| *v == "docker-compose.idealyst.yml").count(),
        1
    );
    assert!(managed(dir).contains("\n  app:\n"), "override keyed on user's `app` service");
}

#[test]
fn unknown_service_and_bad_variant_error() {
    let tmp = tempfile::tempdir().unwrap();
    let dir = tmp.path();
    assert!(devcontainer::apply(dir, &req(vec![enable("postgres")])).is_err()); // it's "database"
    assert!(devcontainer::apply(dir, &req(vec![enable_variant("database", "sqlite")])).is_err());
}
