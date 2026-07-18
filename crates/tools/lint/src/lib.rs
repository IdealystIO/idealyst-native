//! `lint` — the idealyst source linter engine.
//!
//! Flags three idiom-drift patterns over a project's **un-expanded** Rust
//! source (the only place the choice is still visible — `effect!` expands
//! to the raw constructor, so post-expansion the idiom has vanished):
//!
//! 1. Drift from the canonical reactive surface — redundant `Signal::new`
//!    and the removed `signal!` / `memo!` macros (→ the `signal(…)` /
//!    `memo(move || …)` functions; `prefer-signal-fn`, `prefer-memo-fn`),
//!    raw `Effect::new` (`prefer-effect-macro`).
//! 2. Hand-built elements instead of `ui!` / `jsx!` — `builder::…`,
//!    `BuildElement::build`, `Element::Variant { … }` (`prefer-ui-macro`).
//! 3. Non-PascalCase `#[component]` functions (`component-pascal-case`).
//!
//! The engine is consumed two ways from one implementation:
//! - the `idealyst lint` CLI subcommand (human report), and
//! - rust-analyzer's `check.overrideCommand` (cargo-JSON report) — RA runs
//!   the binary and renders the emitted diagnostics inline. There is no RA
//!   plugin; the integration is the shared diagnostic format.
//!
//! Every rule is individually configurable (`off` / `warn` / `error`) via
//! `idealyst-lint.toml` and individually suppressible with inline
//! `// idealyst-lint-disable …` directives — the ESLint model.

pub mod config;
pub mod diagnostic;
pub mod engine;
pub mod report;

mod rules;
mod source_map;

pub use config::{Config, Level, Loaded, Suppressions, CONFIG_FILE_NAME};
pub use diagnostic::{Diagnostic, Severity};
pub use engine::{discover_rs_files, lint_file, lint_path, lint_source, LintRun};
pub use rules::{all_rules, RuleInfo};
