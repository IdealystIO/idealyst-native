//! `cargo run -p capability-coverage` — print the coverage matrix as
//! Markdown, then the allowlist verdict. Exits non-zero when the verdict
//! fails (the same check `cargo test -p capability-coverage` runs).

use capability_coverage::{check, config_path, markdown, workspace_root, Config, Coverage};

fn main() {
    let config = Config::load(&config_path()).unwrap_or_else(|e| panic!("{e}"));
    let coverage = Coverage::scan(&workspace_root(), &config).unwrap_or_else(|e| panic!("{e}"));
    print!("{}", markdown(&coverage, &config));
    let verdict = check(&coverage, &config);
    for f in &verdict.unlisted {
        eprintln!("UNLISTED  {:<10} {}", f.backend, f.method);
    }
    for f in &verdict.stale {
        eprintln!("STALE     {:<10} {}", f.backend, f.method);
    }
    for b in &verdict.unknown_backends {
        eprintln!("UNKNOWN   {b}");
    }
    if !verdict.ok() {
        std::process::exit(1);
    }
}
