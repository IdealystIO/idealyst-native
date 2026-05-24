//! `idealyst new <name>` — create a new project or library in a new
//! directory.

use std::fs;
use std::path::PathBuf;

use anyhow::{Context, Result};

use super::scaffold_template::{self, Kind};

#[derive(clap::Args, Debug)]
pub struct Args {
    /// Crate name. Becomes the directory name and the `[package] name`
    /// of the generated Cargo.toml. Must be a valid Cargo package
    /// name (lowercase, hyphens or underscores).
    pub name: String,

    /// Scaffold a library (External-primitive extension) instead of
    /// a project. Mirrors `cargo new --lib`.
    #[arg(long)]
    pub lib: bool,

    /// Reverse-DNS bundle identifier (e.g. `com.example.hello`).
    /// Project-only. Must use underscores, not hyphens. Defaults to
    /// `com.example.<name-with-underscores>`.
    #[arg(long)]
    pub bundle_id: Option<String>,
}

pub fn run(args: Args) -> Result<()> {
    validate_name(&args.name)?;

    let dir = PathBuf::from(&args.name);
    if dir.exists() {
        anyhow::bail!(
            "{} already exists. Use `idealyst init` to scaffold into an existing directory, \
             or pick a different name.",
            dir.display(),
        );
    }
    fs::create_dir_all(&dir)
        .with_context(|| format!("create {}", dir.display()))?;

    // The new directory doesn't exist on disk yet from the resolver's
    // POV — `FrameworkSource::detect` walks up from `project_dir`
    // looking for a framework workspace, and the canonical answer is
    // the same regardless of whether the project dir already has its
    // own files. We pass the new dir directly.
    let source = crate::framework_source::resolve(&dir)?;

    let kind = if args.lib { Kind::Library } else { Kind::Project };
    scaffold_template::write(&dir, &args.name, kind, &source, args.bundle_id.as_deref())?;

    eprintln!("[idealyst new] scaffolded {} ({}) at {}", args.name, kind_label(kind), dir.display());
    print_next_steps(&args.name, kind);
    Ok(())
}

/// Public alias used by `init` so the two commands enforce the same
/// name validity rules.
pub(super) fn validate_name_pub(name: &str) -> Result<()> {
    validate_name(name)
}

fn validate_name(name: &str) -> Result<()> {
    if name.is_empty() {
        anyhow::bail!("project name must not be empty");
    }
    let first = name.chars().next().unwrap();
    if !first.is_ascii_alphabetic() && first != '_' {
        anyhow::bail!(
            "project name {:?} must start with an ASCII letter or underscore",
            name
        );
    }
    for c in name.chars() {
        if !(c.is_ascii_alphanumeric() || c == '-' || c == '_') {
            anyhow::bail!(
                "project name {:?} contains invalid character {:?} — only ASCII letters, \
                 digits, '-', and '_' are allowed in Cargo package names",
                name,
                c,
            );
        }
    }
    Ok(())
}

pub(super) fn kind_label(kind: Kind) -> &'static str {
    match kind {
        Kind::Project => "project",
        Kind::Library => "library",
    }
}

pub(super) fn print_next_steps(name: &str, kind: Kind) {
    eprintln!();
    match kind {
        Kind::Project => {
            eprintln!("Next steps:");
            eprintln!("  cd {name}");
            eprintln!("  idealyst dev          # hot-reload web preview");
            eprintln!("  idealyst build ios    # produce a staticlib (requires Xcode)");
            eprintln!("  idealyst build android  # produce a cdylib (requires ANDROID_NDK_HOME)");
        }
        Kind::Library => {
            eprintln!("Next steps:");
            eprintln!("  cd {name}");
            eprintln!("  cargo check           # confirm the scaffold compiles");
            eprintln!("  edit src/lib.rs to define your props type");
            eprintln!("  implement the platform leaves in src/web.rs, src/ios.rs, src/android.rs");
        }
    }
}
