//! Integration tests for the VS Code configuration engine: fresh apply,
//! preselect round-trip, the already-configured no-op, removal, and — like the
//! devcontainer tests — that a user's own `.vscode/settings.json` keys are
//! preserved through a merge.

use std::path::Path;

use configure::vscode::{self, Action, AspectRequest, ConfigureRequest};

fn enable(id: &str) -> AspectRequest {
    AspectRequest { id: id.into(), action: Action::Enable }
}
fn remove(id: &str) -> AspectRequest {
    AspectRequest { id: id.into(), action: Action::Remove }
}
fn req(aspects: Vec<AspectRequest>) -> ConfigureRequest {
    ConfigureRequest { aspects }
}

fn settings(dir: &Path) -> serde_json::Value {
    serde_json::from_str(&std::fs::read_to_string(dir.join(".vscode/settings.json")).unwrap())
        .unwrap()
}
fn extensions(dir: &Path) -> serde_json::Value {
    serde_json::from_str(&std::fs::read_to_string(dir.join(".vscode/extensions.json")).unwrap())
        .unwrap()
}

#[test]
fn enable_all_writes_settings_extensions_and_script() {
    let tmp = tempfile::tempdir().unwrap();
    let dir = tmp.path();

    let report = vscode::apply(dir, &vscode::enable_all_request()).unwrap();
    assert!(report.added.contains(&"lint".to_string()));
    assert!(report.added.contains(&"extensions".to_string()));

    // settings.json: overrideCommand + disabled diagnostics.
    let s = settings(dir);
    assert_eq!(
        s["rust-analyzer.check.overrideCommand"],
        serde_json::json!(["sh", ".vscode/ra-check.sh"])
    );
    let disabled = s["rust-analyzer.diagnostics.disabled"].as_array().unwrap();
    assert!(disabled.iter().any(|v| v == "non_snake_case"));
    assert!(disabled.iter().any(|v| v == "incorrect-case"));

    // extensions.json recommends rust-analyzer + the idealyst extension.
    let recs = extensions(dir)["recommendations"].as_array().unwrap().clone();
    assert!(recs.iter().any(|v| v == "rust-lang.rust-analyzer"));
    assert!(recs.iter().any(|v| v == "idealyst.vscode-idealyst"));

    // ra-check.sh exists, is executable, and runs both checkers.
    let script = dir.join(".vscode/ra-check.sh");
    assert!(script.exists());
    let body = std::fs::read_to_string(&script).unwrap();
    assert!(body.contains("cargo check --message-format=json"));
    assert!(body.contains("idealyst lint --format json"));
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mode = std::fs::metadata(&script).unwrap().permissions().mode();
        assert!(mode & 0o111 != 0, "ra-check.sh must be executable");
    }
}

#[test]
fn read_state_round_trips_enabled_aspects() {
    let tmp = tempfile::tempdir().unwrap();
    let dir = tmp.path();
    vscode::apply(dir, &req(vec![enable("lint")])).unwrap();

    let state = vscode::read_state(dir).unwrap();
    assert!(state.exists);
    assert!(state.enabled.contains(&"lint".to_string()));
    assert!(!state.enabled.contains(&"extensions".to_string()));
}

#[test]
fn enable_already_configured_is_warned_noop() {
    let tmp = tempfile::tempdir().unwrap();
    let dir = tmp.path();
    vscode::apply(dir, &vscode::enable_all_request()).unwrap();
    let before = std::fs::read_to_string(dir.join(".vscode/settings.json")).unwrap();

    let report = vscode::apply(dir, &vscode::enable_all_request()).unwrap();
    let after = std::fs::read_to_string(dir.join(".vscode/settings.json")).unwrap();

    assert_eq!(before, after, "settings unchanged on no-op re-enable");
    assert!(report.added.is_empty());
    assert!(report.warnings.iter().any(|w| w.contains("already configured")));
    assert!(report.unchanged.contains(&"lint".to_string()));
}

#[test]
fn remove_pulls_our_keys_and_deletes_script() {
    let tmp = tempfile::tempdir().unwrap();
    let dir = tmp.path();
    vscode::apply(dir, &vscode::enable_all_request()).unwrap();

    let report = vscode::apply(dir, &req(vec![remove("lint"), remove("extensions")])).unwrap();
    assert!(report.removed.contains(&"lint".to_string()));
    assert!(report.removed.contains(&"extensions".to_string()));

    // Script gone; our settings keys gone.
    assert!(!dir.join(".vscode/ra-check.sh").exists());
    let s = settings(dir);
    assert!(s.get("rust-analyzer.check.overrideCommand").is_none());
    assert!(s.get("rust-analyzer.diagnostics.disabled").is_none());
    // recommendations emptied → key dropped.
    let e = extensions(dir);
    assert!(e.get("recommendations").is_none(), "empty recommendations dropped: {e}");
}

#[test]
fn merge_preserves_user_settings_and_unions_arrays() {
    let tmp = tempfile::tempdir().unwrap();
    let dir = tmp.path();
    std::fs::create_dir_all(dir.join(".vscode")).unwrap();

    // User's own settings (JSONC, with their own key + a pre-existing entry in
    // the diagnostics array we union into).
    let user = r#"{
  // my settings
  "editor.formatOnSave": true,
  "rust-analyzer.diagnostics.disabled": ["unused_variables"]
}"#;
    std::fs::write(dir.join(".vscode/settings.json"), user).unwrap();

    vscode::apply(dir, &req(vec![enable("lint")])).unwrap();

    let s = settings(dir);
    // User's own key survives.
    assert_eq!(s["editor.formatOnSave"], serde_json::json!(true));
    // Their diagnostics entry is preserved; ours are unioned in.
    let disabled = s["rust-analyzer.diagnostics.disabled"].as_array().unwrap();
    assert!(disabled.iter().any(|v| v == "unused_variables"));
    assert!(disabled.iter().any(|v| v == "non_snake_case"));
    assert!(disabled.iter().any(|v| v == "incorrect-case"));

    // Removing pulls only OUR entries, leaving the user's.
    vscode::apply(dir, &req(vec![remove("lint")])).unwrap();
    let s = settings(dir);
    assert_eq!(s["editor.formatOnSave"], serde_json::json!(true));
    let disabled = s["rust-analyzer.diagnostics.disabled"].as_array().unwrap();
    assert_eq!(disabled, &vec![serde_json::json!("unused_variables")]);
}

#[test]
fn unknown_aspect_errors() {
    let tmp = tempfile::tempdir().unwrap();
    assert!(vscode::apply(tmp.path(), &req(vec![enable("nope")])).is_err());
}
