//! The rustflags a wasm build starts from, before the CLI adds its own.
//!
//! The CLI hands cargo its flags as `CARGO_ENCODED_RUSTFLAGS` (see
//! `cargo_build_wasm`), and cargo then ignores every other source. So
//! whatever cargo would have used without the CLI has to be folded in
//! here, in cargo's own order — the first source present wins outright:
//!
//! 1. `CARGO_ENCODED_RUSTFLAGS`;
//! 2. `RUSTFLAGS`;
//! 3. the target's flags: `CARGO_TARGET_WASM32_UNKNOWN_UNKNOWN_RUSTFLAGS`,
//!    `[target.wasm32-unknown-unknown] rustflags` and every
//!    `[target.'cfg(…)'] rustflags` whose cfg holds for wasm32, joined;
//! 4. `CARGO_BUILD_RUSTFLAGS` / `[build] rustflags`.
//!
//! Before this only 1 and 2 were read, so config rustflags never reached
//! the wasm build (CrewForge could not set `-Zthreads` for it), and the
//! only route was env `RUSTFLAGS`, which lands on the host builds too —
//! a cranelift flag meant for the server failed the wasm build.
//!
//! Then `IDEALYST_WASM_RUSTFLAGS` (whitespace-separated) is APPENDED,
//! whatever the source: flags for the web build alone, without touching
//! cargo config or any other build.
//!
//! Config files are read the way cargo discovers them: `.cargo/config.toml`
//! (or `.cargo/config`) in the build's directory and each ancestor, then
//! `$CARGO_HOME`'s. Arrays from several files are joined with the closer
//! file's entries last, as cargo merges them. `--config` command-line
//! overrides are not seen (the CLI never passes any).

use std::path::{Path, PathBuf};

/// The target every flag here is for.
const TARGET: &str = "wasm32-unknown-unknown";

/// The env var appended to the wasm build's flags only.
pub const WASM_RUSTFLAGS_ENV: &str = "IDEALYST_WASM_RUSTFLAGS";

/// The base flags, and where they came from (for the build log).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BaseFlags {
    pub flags: Vec<String>,
    pub source: &'static str,
}

/// See the module docs. `env` reads an environment variable; `cfg` lists
/// `rustc --print cfg --target wasm32-unknown-unknown`, and is called only
/// when a config file has a `cfg(…)` target table.
pub fn base_wasm_rustflags(
    cwd: &Path,
    cargo_home: Option<&Path>,
    env: &dyn Fn(&str) -> Option<String>,
    cfg: &dyn Fn() -> Vec<String>,
) -> BaseFlags {
    let mut base = resolve(cwd, cargo_home, env, cfg);
    if let Some(extra) = env(WASM_RUSTFLAGS_ENV) {
        base.flags.extend(extra.split_whitespace().map(str::to_string));
    }
    base
}

fn resolve(
    cwd: &Path,
    cargo_home: Option<&Path>,
    env: &dyn Fn(&str) -> Option<String>,
    cfg: &dyn Fn() -> Vec<String>,
) -> BaseFlags {
    if let Some(encoded) = env("CARGO_ENCODED_RUSTFLAGS").filter(|e| !e.is_empty()) {
        return BaseFlags {
            flags: encoded.split('\x1f').map(str::to_string).collect(),
            source: "CARGO_ENCODED_RUSTFLAGS",
        };
    }
    if let Some(plain) = env("RUSTFLAGS") {
        return BaseFlags { flags: split(&plain), source: "RUSTFLAGS" };
    }

    let docs = config_docs(cwd, cargo_home);
    let mut cfgs: Option<Vec<String>> = None;
    let mut target: Option<Vec<String>> = None;
    for doc in &docs {
        let Some(targets) = doc.get("target").and_then(|t| t.as_table()) else { continue };
        for (key, table) in targets {
            let applies = if key == TARGET {
                true
            } else if let Some(expr) = key.strip_prefix("cfg(").and_then(|k| k.strip_suffix(')')) {
                let set = cfgs.get_or_insert_with(cfg);
                eval_cfg(expr, set).unwrap_or(false)
            } else {
                false
            };
            if let (true, Some(flags)) = (applies, table.get("rustflags").and_then(value_flags)) {
                target.get_or_insert_with(Vec::new).extend(flags);
            }
        }
    }
    let var = format!("CARGO_TARGET_{}_RUSTFLAGS", TARGET.to_uppercase().replace('-', "_"));
    if let Some(v) = env(&var) {
        target.get_or_insert_with(Vec::new).extend(split(&v));
    }
    if let Some(flags) = target {
        return BaseFlags { flags, source: "target rustflags" };
    }

    let mut build: Option<Vec<String>> = None;
    for doc in &docs {
        if let Some(flags) = doc
            .get("build")
            .and_then(|b| b.get("rustflags"))
            .and_then(value_flags)
        {
            build.get_or_insert_with(Vec::new).extend(flags);
        }
    }
    if let Some(v) = env("CARGO_BUILD_RUSTFLAGS") {
        build.get_or_insert_with(Vec::new).extend(split(&v));
    }
    match build {
        Some(flags) => BaseFlags { flags, source: "build.rustflags" },
        None => BaseFlags { flags: Vec::new(), source: "none" },
    }
}

fn split(s: &str) -> Vec<String> {
    s.split_whitespace().map(str::to_string).collect()
}

/// `rustflags` as cargo accepts it: an array of strings, or one
/// space-separated string.
fn value_flags(v: &toml::Value) -> Option<Vec<String>> {
    match v {
        toml::Value::String(s) => Some(split(s)),
        toml::Value::Array(a) => Some(a.iter().filter_map(|x| x.as_str().map(str::to_string)).collect()),
        _ => None,
    }
}

/// Every config file cargo would read from `cwd`, lowest precedence
/// first, so joining in order puts the closer file's flags last.
fn config_docs(cwd: &Path, cargo_home: Option<&Path>) -> Vec<toml::Table> {
    let mut files: Vec<PathBuf> = Vec::new();
    for dir in cwd.ancestors() {
        // `config.toml` wins over the legacy name when both exist, as
        // in cargo.
        let toml = dir.join(".cargo/config.toml");
        files.push(if toml.is_file() { toml } else { dir.join(".cargo/config") });
    }
    if let Some(home) = cargo_home {
        let toml = home.join("config.toml");
        let file = if toml.is_file() { toml } else { home.join("config") };
        // `$CARGO_HOME` is usually `~/.cargo`, which an ancestor walk from
        // a project under `~` already read.
        if !files.contains(&file) {
            files.push(file);
        }
    }
    files
        .iter()
        .rev()
        .filter_map(|f| std::fs::read_to_string(f).ok())
        .filter_map(|text| text.parse::<toml::Table>().ok())
        .collect()
}

/// Evaluate a `cfg(…)` predicate's contents against `set` — the lines of
/// `rustc --print cfg` (`target_arch="wasm32"`, `debug_assertions`).
/// `None` when it does not parse; cargo would refuse such a config, and
/// an unreadable table is left out.
pub fn eval_cfg(expr: &str, set: &[String]) -> Option<bool> {
    let tokens = tokenize(expr)?;
    let mut pos = 0;
    let value = eval(&tokens, &mut pos, set)?;
    (pos == tokens.len()).then_some(value)
}

#[derive(Debug, Clone, PartialEq)]
enum Tok {
    Ident(String),
    Str(String),
    Eq,
    Comma,
    Open,
    Close,
}

fn tokenize(s: &str) -> Option<Vec<Tok>> {
    let mut out = Vec::new();
    let mut chars = s.chars().peekable();
    while let Some(&c) = chars.peek() {
        match c {
            c if c.is_whitespace() => {
                chars.next();
            }
            '=' => {
                chars.next();
                out.push(Tok::Eq);
            }
            ',' => {
                chars.next();
                out.push(Tok::Comma);
            }
            '(' => {
                chars.next();
                out.push(Tok::Open);
            }
            ')' => {
                chars.next();
                out.push(Tok::Close);
            }
            '"' => {
                chars.next();
                let mut s = String::new();
                loop {
                    match chars.next()? {
                        '"' => break,
                        c => s.push(c),
                    }
                }
                out.push(Tok::Str(s));
            }
            c if c.is_alphanumeric() || c == '_' => {
                let mut s = String::new();
                while let Some(&c) = chars.peek() {
                    if c.is_alphanumeric() || c == '_' {
                        s.push(c);
                        chars.next();
                    } else {
                        break;
                    }
                }
                out.push(Tok::Ident(s));
            }
            _ => return None,
        }
    }
    Some(out)
}

fn eval(t: &[Tok], pos: &mut usize, set: &[String]) -> Option<bool> {
    let Tok::Ident(name) = t.get(*pos)?.clone() else { return None };
    *pos += 1;
    match (name.as_str(), t.get(*pos)) {
        ("all" | "any" | "not", Some(Tok::Open)) => {
            *pos += 1;
            let mut values = Vec::new();
            while t.get(*pos) != Some(&Tok::Close) {
                values.push(eval(t, pos, set)?);
                match t.get(*pos)? {
                    Tok::Comma => *pos += 1,
                    Tok::Close => {}
                    _ => return None,
                }
            }
            *pos += 1;
            match name.as_str() {
                "all" => Some(values.iter().all(|v| *v)),
                "any" => Some(values.iter().any(|v| *v)),
                _ if values.len() == 1 => Some(!values[0]),
                _ => None,
            }
        }
        (_, Some(Tok::Eq)) => {
            *pos += 1;
            let Tok::Str(value) = t.get(*pos)?.clone() else { return None };
            *pos += 1;
            Some(set.iter().any(|l| *l == format!("{name}=\"{value}\"")))
        }
        _ => Some(set.iter().any(|l| *l == name)),
    }
}

/// `rustc --print cfg --target wasm32-unknown-unknown`, one entry per line;
/// empty when rustc cannot be run (no `cfg(…)` table then applies).
pub fn wasm_cfg() -> Vec<String> {
    std::process::Command::new("rustc")
        .args(["--print", "cfg", "--target", TARGET])
        .output()
        .ok()
        .filter(|o| o.status.success())
        .map(|o| String::from_utf8_lossy(&o.stdout).lines().map(str::to_string).collect())
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;

    const WASM_CFG: &[&str] = &[
        "debug_assertions",
        "panic=\"abort\"",
        "target_arch=\"wasm32\"",
        "target_family=\"wasm\"",
        "target_os=\"unknown\"",
        "target_pointer_width=\"32\"",
        "target_vendor=\"unknown\"",
    ];

    fn cfg() -> Vec<String> {
        WASM_CFG.iter().map(|s| s.to_string()).collect()
    }

    fn resolve_in(
        dir: &Path,
        home: Option<&Path>,
        vars: &[(&str, &str)],
    ) -> BaseFlags {
        let vars: Vec<(String, String)> = vars.iter().map(|(k, v)| (k.to_string(), v.to_string())).collect();
        let env = move |k: &str| vars.iter().find(|(n, _)| n == k).map(|(_, v)| v.clone());
        base_wasm_rustflags(dir, home, &env, &cfg)
    }

    fn write(dir: &Path, text: &str) {
        std::fs::create_dir_all(dir.join(".cargo")).unwrap();
        std::fs::write(dir.join(".cargo/config.toml"), text).unwrap();
    }

    /// Regression (CrewForge): `[target.wasm32-unknown-unknown] rustflags`
    /// never reached the wasm build — the CLI's encoded flags replaced it —
    /// so `-Zthreads=8` could only be set through env `RUSTFLAGS`, which
    /// also reaches the host builds.
    #[test]
    fn regression_target_config_rustflags_reach_the_wasm_build() {
        let tmp = tempfile::tempdir().unwrap();
        let project = tmp.path().join("app");
        write(
            &project,
            r#"
[build]
rustflags = ["--cfg", "from_build"]
[target.wasm32-unknown-unknown]
rustflags = ["-Zthreads=8"]
[target.aarch64-unknown-linux-gnu]
rustflags = ["-Zcodegen-backend=cranelift"]
"#,
        );
        let got = resolve_in(&project, None, &[]);
        assert_eq!(got.flags, vec!["-Zthreads=8"], "target flags win over build.rustflags");
        assert_eq!(got.source, "target rustflags");
    }

    #[test]
    fn build_rustflags_apply_when_no_target_table_matches() {
        let tmp = tempfile::tempdir().unwrap();
        write(tmp.path(), "[build]\nrustflags = \"--cfg a --cfg b\"\n[target.x86_64-unknown-linux-gnu]\nrustflags = [\"-Cx\"]\n");
        assert_eq!(resolve_in(tmp.path(), None, &[]).flags, vec!["--cfg", "a", "--cfg", "b"]);
        assert_eq!(
            resolve_in(tmp.path(), None, &[("CARGO_BUILD_RUSTFLAGS", "--cfg c")]).flags,
            vec!["--cfg", "a", "--cfg", "b", "--cfg", "c"]
        );
    }

    #[test]
    fn env_rustflags_win_over_config_as_in_cargo() {
        let tmp = tempfile::tempdir().unwrap();
        write(tmp.path(), "[target.wasm32-unknown-unknown]\nrustflags = [\"-Zthreads=8\"]\n");
        let got = resolve_in(tmp.path(), None, &[("RUSTFLAGS", "--cfg rf_marker")]);
        assert_eq!((got.flags, got.source), (vec!["--cfg".to_string(), "rf_marker".into()], "RUSTFLAGS"));
        let got = resolve_in(tmp.path(), None, &[("RUSTFLAGS", "--cfg x"), ("CARGO_ENCODED_RUSTFLAGS", "-C\x1flink-arg=a b")]);
        assert_eq!(got.flags, vec!["-C", "link-arg=a b"], "encoded fields keep their spaces");
        // Set but empty: cargo uses the empty value, not the config.
        assert!(resolve_in(tmp.path(), None, &[("RUSTFLAGS", "")]).flags.is_empty());
    }

    #[test]
    fn matching_cfg_tables_join_with_the_target_table_closer_file_last() {
        let tmp = tempfile::tempdir().unwrap();
        let home = tmp.path().join("cargo-home");
        std::fs::create_dir_all(&home).unwrap();
        std::fs::write(home.join("config.toml"), "[target.wasm32-unknown-unknown]\nrustflags = [\"--cfg\", \"home\"]\n").unwrap();
        let project = tmp.path().join("ws").join("app");
        write(&tmp.path().join("ws"), "[target.'cfg(target_arch = \"wasm32\")']\nrustflags = [\"--cfg\", \"ws\"]\n[target.'cfg(unix)']\nrustflags = [\"--cfg\", \"unix\"]\n");
        write(&project, "[target.'cfg(all(target_family = \"wasm\", not(target_os = \"wasi\")))']\nrustflags = [\"--cfg\", \"app\"]\n");
        let got = resolve_in(&project, Some(&home), &[("CARGO_TARGET_WASM32_UNKNOWN_UNKNOWN_RUSTFLAGS", "--cfg env")]);
        assert_eq!(got.flags, vec!["--cfg", "home", "--cfg", "ws", "--cfg", "app", "--cfg", "env"]);
    }

    /// The wasm-only route: appended whatever the base came from, and
    /// it reaches no other build because only the wasm build reads it.
    #[test]
    fn idealyst_wasm_rustflags_append_to_every_source() {
        let tmp = tempfile::tempdir().unwrap();
        assert_eq!(resolve_in(tmp.path(), None, &[("IDEALYST_WASM_RUSTFLAGS", "-Zthreads=8")]).flags, vec!["-Zthreads=8"]);
        assert_eq!(
            resolve_in(tmp.path(), None, &[("RUSTFLAGS", "--cfg a"), ("IDEALYST_WASM_RUSTFLAGS", " -Zthreads=8 ")]).flags,
            vec!["--cfg", "a", "-Zthreads=8"]
        );
    }

    #[test]
    fn cfg_predicates() {
        let set = cfg();
        for (expr, want) in [
            ("target_arch = \"wasm32\"", Some(true)),
            ("target_arch=\"x86_64\"", Some(false)),
            ("unix", Some(false)),
            ("debug_assertions", Some(true)),
            ("any(unix, target_family = \"wasm\")", Some(true)),
            ("all(target_family = \"wasm\", target_os = \"wasi\")", Some(false)),
            ("not(windows)", Some(true)),
            ("not(a, b)", None),
            ("all(", None),
        ] {
            assert_eq!(eval_cfg(expr, &set), want, "{expr}");
        }
    }
}
