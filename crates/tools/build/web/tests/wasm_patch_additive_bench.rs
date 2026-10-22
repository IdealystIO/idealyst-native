//! What an ADDITIVE hot patch saves, measured: one save — a helper
//! added to the bottom crate of a dependency chain and called from an
//! existing body there — taken as the rebuild it was before additive
//! patches (cargo, every crate above the edit recompiled) and as the
//! patch it is now, on the same generated workspace.
//!
//! The workspace mirrors the shape the CrewForge study measured
//! (2026-10-21): a chain of library crates under an app, each crate
//! heavy with derive-macro output that every dependent re-expands and
//! re-checks whenever cargo rebuilds it. Here that output is serde's
//! `Serialize` / `Deserialize` on many structs; in an idealyst app it is
//! `#[component]`, `ui!` and `stylesheet!`. A body-only patch of the same
//! crate is timed beside them, as the floor an additive patch should sit
//! on.
//!
//! Each figure is the median of three saves, each a different edit, as a
//! person's next save is. The rebuild figure stops at cargo's link: the
//! dev loop's post-cargo passes (wasm-bindgen, base prep, the reload)
//! come on top of it, so the comparison is conservative.
//!
//! Sizes: `IDEALYST_BENCH_CRATES` (chain length, default 8) and
//! `IDEALYST_BENCH_ITEMS` (derive-heavy structs per crate, default 150).
//! The first run compiles serde for wasm32 into the scratch target; later
//! runs reuse it.
//!
//! ```text
//! cargo test -p build-web --release --test wasm_patch_additive_bench -- --ignored --nocapture
//! ```

use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::Arc;
use std::time::{Duration, Instant};

use build_runtime_server::hotpatch::replay::CapturedInvocation;
use build_web::hotpatch_aliases;
use build_web::hotpatch_base::{prepare_base_module, Flavor};
use build_web::hotpatch_build::{PatchCrate, SourceProbe, WasmPatchBuilder};

/// A `RUSTC_WRAPPER` recording each invocation (argv NUL-separated, cwd,
/// env), as `wasm_patch_roundtrip` does: the `idealyst` binary does this
/// for real.
const WRAPPER_SH: &str = r#"#!/bin/sh
d="$CAPTURE_RAW/$$"
mkdir -p "$d"
printf '%s\0' "$@" > "$d/args"
pwd > "$d/cwd"
env -0 > "$d/env"
exec "$@"
"#;

fn env_usize(name: &str, default: usize) -> usize {
    std::env::var(name).ok().and_then(|v| v.parse().ok()).unwrap_or(default)
}

/// One library crate of the chain: `items` derive-heavy structs and a
/// `value` the crate above calls directly.
fn lib_source(level: usize, items: usize) -> String {
    let mut s = String::from("use serde::{Deserialize, Serialize};\n\n");
    for k in 0..items {
        s.push_str(&format!(
            "#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Default)]\n\
             pub struct Rec{level}x{k} {{\n    pub id: u64,\n    pub name: String,\n    \
             pub tags: Vec<String>,\n    pub score: Option<f64>,\n    pub flags: [u8; 4],\n}}\n\n\
             pub fn describe{k}(r: &Rec{level}x{k}) -> String {{\n    format!(\"{{:?}}\", r)\n}}\n\n"
        ));
    }
    let below = if level == 0 { "n".to_string() } else { format!("chain{}::value(n)", level - 1) };
    s.push_str(&format!(
        "#[inline(never)]\npub fn value(n: u64) -> u64 {{\n    // BODY\n    {below} + {}\n}}\n",
        level + 1
    ));
    s
}

const APP_RS: &str = r#"
#![allow(non_snake_case)]

#[doc(hidden)]
#[inline(never)]
fn __Root_hot_impl() -> u64 {
    TOP::value(1)
}

#[no_mangle]
pub extern "C" fn root() -> u64 {
    std::hint::black_box(__Root_hot_impl as fn() -> u64)()
}
"#;

fn median(mut v: Vec<Duration>) -> Duration {
    v.sort();
    v[v.len() / 2]
}

fn secs(d: Duration) -> String {
    format!("{:.2}", d.as_secs_f64())
}

#[test]
#[ignore = "builds a generated multi-crate workspace for wasm32 several times; run with --ignored"]
fn bench_an_additive_save_against_the_rebuild_it_replaces() {
    use std::os::unix::fs::PermissionsExt;

    let crates = env_usize("IDEALYST_BENCH_CRATES", 8);
    let items = env_usize("IDEALYST_BENCH_ITEMS", 150);
    let dir = Path::new(env!("CARGO_TARGET_TMPDIR")).join(format!("additive-bench-{crates}x{items}"));
    std::fs::create_dir_all(&dir).unwrap();

    // ── The workspace ────────────────────────────────────────────────
    let mut members = vec!["\"app\"".to_string()];
    for level in 0..crates {
        let name = format!("chain{level}");
        members.push(format!("\"{name}\""));
        std::fs::create_dir_all(dir.join(&name).join("src")).unwrap();
        let dep = if level == 0 {
            String::new()
        } else {
            format!("chain{} = {{ path = \"../chain{}\" }}\n", level - 1, level - 1)
        };
        std::fs::write(
            dir.join(&name).join("Cargo.toml"),
            format!(
                "[package]\nname = \"{name}\"\nversion = \"0.0.0\"\nedition = \"2021\"\n\n[dependencies]\n\
                 serde = {{ version = \"1\", features = [\"derive\"] }}\n{dep}"
            ),
        )
        .unwrap();
        std::fs::write(dir.join(&name).join("src/lib.rs"), lib_source(level, items)).unwrap();
    }
    let top = format!("chain{}", crates - 1);
    std::fs::create_dir_all(dir.join("app/src")).unwrap();
    std::fs::write(
        dir.join("app/Cargo.toml"),
        format!(
            "[package]\nname = \"bench_app\"\nversion = \"0.0.0\"\nedition = \"2021\"\n\n[lib]\n\
             crate-type = [\"cdylib\"]\n\n[dependencies]\n{top} = {{ path = \"../{top}\" }}\n"
        ),
    )
    .unwrap();
    std::fs::write(dir.join("app/src/lib.rs"), APP_RS.replace("TOP", &top)).unwrap();
    std::fs::write(
        dir.join("Cargo.toml"),
        format!("[workspace]\nresolver = \"2\"\nmembers = [{}]\n", members.join(", ")),
    )
    .unwrap();
    let wrapper = dir.join("capture.sh");
    std::fs::write(&wrapper, WRAPPER_SH).unwrap();
    std::fs::set_permissions(&wrapper, std::fs::Permissions::from_mode(0o755)).unwrap();

    let bottom = dir.join("chain0/src/lib.rs");
    let pristine = lib_source(0, items);
    let link_args = build_web::hot_patch_link_args()
        .iter()
        .map(|a| format!("-Clink-arg={a}"))
        .collect::<Vec<_>>()
        .join(" ");
    let raw = dir.join("raw-captures");
    let cargo = |capture: bool| -> Duration {
        let mut cmd = Command::new("cargo");
        cmd.current_dir(&dir)
            .args(["build", "--offline", "-p", "bench_app", "--target", "wasm32-unknown-unknown"])
            .env("RUSTFLAGS", &link_args)
            .env("CARGO_INCREMENTAL", "1");
        if capture {
            let _ = std::fs::remove_dir_all(&raw);
            std::fs::create_dir_all(&raw).unwrap();
            cmd.env("RUSTC_WRAPPER", &wrapper).env("CAPTURE_RAW", &raw);
        }
        let started = Instant::now();
        let out = cmd.output().expect("run cargo");
        assert!(out.status.success(), "cargo build failed:\n{}", String::from_utf8_lossy(&out.stderr));
        started.elapsed()
    };

    // ── The base: built warm, every invocation captured ──────────────
    std::fs::write(&bottom, &pristine).unwrap();
    cargo(false);
    // Touch every source so the captured build recompiles (and records)
    // each workspace crate — still warm, incremental.
    for level in 0..crates {
        let p = dir.join(format!("chain{level}/src/lib.rs"));
        let text = std::fs::read(&p).unwrap();
        std::fs::write(&p, text).unwrap();
    }
    let app_src = dir.join("app/src/lib.rs");
    std::fs::write(&app_src, std::fs::read(&app_src).unwrap()).unwrap();
    cargo(true);
    let captures = dir.join("idealyst-hotpatch/captures");
    let _ = std::fs::remove_dir_all(&captures);
    std::fs::create_dir_all(&captures).unwrap();
    let workspace_crates: Vec<String> =
        (0..crates).map(|l| format!("chain{l}")).chain(std::iter::once("bench_app".to_string())).collect();
    for entry in std::fs::read_dir(&raw).unwrap().flatten() {
        let split0 = |p: PathBuf| -> Vec<String> {
            std::fs::read(p)
                .unwrap()
                .split(|b| *b == 0)
                .filter(|s| !s.is_empty())
                .map(|s| String::from_utf8_lossy(s).into_owned())
                .collect()
        };
        let argv = split0(entry.path().join("args"));
        let Some(at) = argv.iter().position(|a| a == "--crate-name") else { continue };
        let name = argv[at + 1].clone();
        if !workspace_crates.contains(&name) || !argv.iter().any(|a| a.starts_with("--emit=") && a.contains("link")) {
            continue;
        }
        let kind = argv
            .iter()
            .position(|a| a == "--crate-type")
            .map(|i| argv[i + 1].clone())
            .unwrap_or_else(|| "lib".into());
        let capture = CapturedInvocation {
            rustc: argv[0].clone(),
            args: argv[1..].to_vec(),
            cwd: std::fs::read_to_string(entry.path().join("cwd")).unwrap().trim().to_string(),
            env: split0(entry.path().join("env"))
                .into_iter()
                .filter_map(|kv| kv.split_once('=').map(|(k, v)| (k.to_string(), v.to_string())))
                .collect(),
        };
        std::fs::write(captures.join(format!("{name}.{kind}.json")), serde_json::to_string(&capture).unwrap())
            .unwrap();
    }
    for name in &workspace_crates {
        assert!(
            std::fs::read_dir(&captures)
                .unwrap()
                .flatten()
                .any(|e| e.file_name().to_string_lossy().starts_with(&format!("{name}."))),
            "no capture of {name}"
        );
    }

    let linked = std::fs::read(dir.join("target/wasm32-unknown-unknown/debug/bench_app.wasm")).unwrap();
    let alias_path = dir.join("bench_app.aliases.tsv");
    hotpatch_aliases::write(&alias_path, &hotpatch_aliases::read_from_linked(&linked).unwrap()).unwrap();
    let data_path = dir.join("bench_app.datasyms.tsv");
    hotpatch_aliases::write_data_symbols(&data_path, &hotpatch_aliases::read_data_symbols_from_linked(&linked).unwrap())
        .unwrap();
    let (prepared, _) = prepare_base_module(&linked, Flavor::Own).unwrap();
    let served = dir.join("bench_app.served.wasm");
    std::fs::write(&served, &prepared).unwrap();

    // ── The patch builder, seeded as a session is after a rebuild ────
    let probe_file = bottom.clone();
    let probe_pristine = pristine.clone();
    let probe: SourceProbe = Arc::new(move |name: &str| {
        let base = name != "chain0" || std::fs::read_to_string(&probe_file).unwrap() == probe_pristine;
        Some(if base { "base".into() } else { format!("edit-{}", std::fs::read_to_string(&probe_file).unwrap().len()) })
    });
    let builder = WasmPatchBuilder::new(&served, Some(&alias_path), Some(&data_path), &captures, "bench_app", dir.join("patches"))
        .unwrap()
        .with_source_probe(probe);
    builder.seed_all(workspace_crates.iter().map(|c| PatchCrate::carried(c.clone(), Some("base".into()))).collect());
    let seed_started = Instant::now();
    builder.wait_for_seeds();
    let seeded = seed_started.elapsed();

    // The plan the dev loop builds for a save in `chain0`: it, edited,
    // and every crate above it, carried with their base sources.
    let plan = |key: &str, additive: bool| -> Vec<PatchCrate> {
        workspace_crates
            .iter()
            .map(|c| {
                if c == "chain0" {
                    PatchCrate::new(c.clone(), Some(key.to_string())).with_additions(additive)
                } else {
                    PatchCrate::carried(c.clone(), Some("base".into()))
                }
            })
            .collect()
    };
    let additive_edit = |i: usize| -> String {
        pristine.replace("    // BODY\n    n + 1", &format!("    // BODY\n    bench_helper_{i}(n) + 1"))
            + &format!("\n#[inline(never)]\nfn bench_helper_{i}(n: u64) -> u64 {{\n    n * {} + 1\n}}\n", i + 3)
    };
    let body_edit = |i: usize| -> String { pristine.replace("    // BODY\n    n + 1", &format!("    // BODY\n    n + {}", i + 2)) };

    let mut rebuild = Vec::new();
    let mut additive = Vec::new();
    let mut body = Vec::new();
    let mut redirected = Vec::new();
    for i in 0..3 {
        // The patch, against the base.
        let edit = additive_edit(i);
        std::fs::write(&bottom, &edit).unwrap();
        let started = Instant::now();
        let patch = builder
            .build_crates(&plan(&format!("additive-{i}"), true))
            .unwrap_or_else(|e| panic!("the additive patch did not build:\n{e:#}"));
        additive.push(started.elapsed());
        redirected.push(patch.jump_table.map.len());
        assert!(patch.timings.iter().any(|(s, _)| *s == "verify-additive"));

        // A body-only patch of the same crate: the floor.
        std::fs::write(&bottom, body_edit(i)).unwrap();
        let started = Instant::now();
        builder.build_crates(&plan(&format!("body-{i}"), false)).unwrap();
        body.push(started.elapsed());

        // The same additive save as the rebuild it used to be. Then back
        // to the base sources, rebuilt unmeasured, so every timed rebuild
        // starts from the state the base was built from.
        std::fs::write(&bottom, &edit).unwrap();
        rebuild.push(cargo(false));
        std::fs::write(&bottom, &pristine).unwrap();
        cargo(false);
    }

    let (rebuild, additive, body) = (median(rebuild), median(additive), median(body));
    eprintln!(
        "\n[additive bench] {crates} crates x {items} derive-heavy structs (seeded in {} s)\n\
         | save | tier | median (s) |\n|---|---|---|\n\
         | helper added + called, bottom crate | rebuild (cargo, to the linked .wasm) | {} |\n\
         | same save | additive hot patch | {} |\n\
         | body-only edit, same crate | hot patch (floor) | {} |\n\
         speedup: {:.1}x; functions redirected: {:?}\n",
        secs(seeded),
        secs(rebuild),
        secs(additive),
        secs(body),
        rebuild.as_secs_f64() / additive.as_secs_f64(),
        redirected,
    );
    assert!(
        additive * 2 < rebuild,
        "an additive patch ({}) should cost well under half the rebuild it replaces ({})",
        secs(additive),
        secs(rebuild)
    );
}
