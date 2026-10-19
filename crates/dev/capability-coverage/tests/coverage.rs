//! The coverage check against the real workspace. Fails when a backend
//! leaves a capability default unimplemented that `coverage.toml` does not
//! account for, or when the file lists something no longer flagged.

use capability_coverage::{check, config_path, workspace_root, Config, Coverage, Flag};

fn scan() -> (Config, Coverage) {
    let config = Config::load(&config_path()).unwrap_or_else(|e| panic!("{e}"));
    let coverage = Coverage::scan(&workspace_root(), &config).unwrap_or_else(|e| panic!("{e}"));
    (config, coverage)
}

#[test]
fn every_capability_gap_is_accounted_for() {
    let (config, coverage) = scan();
    let v = check(&coverage, &config);
    let mut msg = String::new();
    for f in &v.unlisted {
        msg.push_str(&format!(
            "\n  UNLISTED {} {}: implement it, or list it in coverage.toml (na with a reason, or gap)",
            f.backend, f.method
        ));
    }
    for f in &v.stale {
        msg.push_str(&format!(
            "\n  STALE    {} {}: no longer flagged (implemented?) — remove it from coverage.toml",
            f.backend, f.method
        ));
    }
    for b in &v.unknown_backends {
        msg.push_str(&format!("\n  UNKNOWN  backend {b}"));
    }
    assert!(v.ok(), "capability coverage:{msg}\n(`cargo run -p capability-coverage` prints the matrix)");
}

/// Windows shipped with `StyleOps::attach_states` at its no-op default, so
/// no `state hovered` / `state pressed` variant ever lit and nothing
/// noticed. The scan must see the method as contested (other backends
/// implement it) and see Windows' override now, so removing that override
/// would surface as an UNLISTED flag.
#[test]
fn regression_windows_attach_states_is_checked() {
    let (_, coverage) = scan();
    assert!(coverage.contested().iter().any(|(t, m)| t == "StyleOps" && m == "attach_states"));
    assert!(coverage.implements("windows", "StyleOps", "attach_states"));
    let flag = Flag { backend: "windows".into(), method: "StyleOps::attach_states".into() };
    assert!(!coverage.flags().contains(&flag));
}
