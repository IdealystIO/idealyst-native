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
    assert_eq!(pg.service.get("image").unwrap().as_str().unwrap(), "postgres:16");
    assert!(pg.app_env.iter().any(|(k, v)| k == "DATABASE_URL" && v.starts_with("postgres://")));
    assert_eq!(pg.volumes, vec!["idealyst-database-data".to_string()]);

    let my = db.fragment(Some("mysql"), &devcontainer::service::Ctx { app_service: "dev".into() });
    assert_eq!(my.service.get("image").unwrap().as_str().unwrap(), "mysql:8");
    assert!(my.app_env.iter().any(|(k, v)| k == "DATABASE_URL" && v.starts_with("mysql://")));

    assert!(devcontainer::service::find("redis").is_some());
    assert!(devcontainer::service::find("minio").is_some());
    assert!(devcontainer::service::find("nope").is_none());
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
