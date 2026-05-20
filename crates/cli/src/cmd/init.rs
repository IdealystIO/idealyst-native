//! `idealyst init` — scaffold a project or library into the current
//! (existing, possibly non-empty) directory.
//!
//! Mirrors `cargo init`: useful when you've already `mkdir`'d a
//! directory and want to fill it. Refuses to overwrite files unless
//! `--force`.

use std::path::PathBuf;

use anyhow::{Context, Result};

use super::scaffold_template::{self, Kind};

#[derive(clap::Args, Debug)]
pub struct Args {
    /// Scaffold a library (External-primitive extension) instead of
    /// a project. Mirrors `cargo init --lib`.
    #[arg(long)]
    pub lib: bool,

    /// Cargo package name. Defaults to the current directory's name.
    /// Required if the dir's name isn't a valid Cargo package name.
    #[arg(long)]
    pub name: Option<String>,

    /// Reverse-DNS bundle identifier. Project-only. Defaults to
    /// `com.example.<name-with-underscores>`.
    #[arg(long)]
    pub bundle_id: Option<String>,

    /// Overwrite existing files (Cargo.toml, src/lib.rs, ...).
    /// Without this, `init` refuses to touch a directory that already
    /// contains any of the files it would write.
    #[arg(long)]
    pub force: bool,
}

pub fn run(args: Args) -> Result<()> {
    let dir = std::env::current_dir().context("read current dir")?;
    let name = match args.name {
        Some(n) => n,
        None => derive_name_from_dir(&dir)?,
    };
    crate::cmd::new::validate_name_pub(&name)?;

    let kind = if args.lib { Kind::Library } else { Kind::Project };

    if !args.force {
        let would_overwrite = files_for_kind(kind)
            .iter()
            .map(|f| dir.join(f))
            .filter(|p| p.exists())
            .collect::<Vec<_>>();
        if !would_overwrite.is_empty() {
            anyhow::bail!(
                "refusing to overwrite existing files:\n{}\n\nPass --force to overwrite, or run \
                 `idealyst init` in an empty directory.",
                would_overwrite
                    .iter()
                    .map(|p| format!("  {}", p.display()))
                    .collect::<Vec<_>>()
                    .join("\n")
            );
        }
    }

    let source = crate::framework_source::resolve(&dir)?;
    scaffold_template::write(&dir, &name, kind, &source, args.bundle_id.as_deref())?;

    eprintln!(
        "[idealyst init] scaffolded {} ({}) into {}",
        name,
        crate::cmd::new::kind_label(kind),
        dir.display(),
    );
    crate::cmd::new::print_next_steps(&name, kind);
    Ok(())
}

fn derive_name_from_dir(dir: &PathBuf) -> Result<String> {
    let basename = dir
        .file_name()
        .and_then(|n| n.to_str())
        .ok_or_else(|| anyhow::anyhow!(
            "couldn't derive a project name from {} — pass --name explicitly",
            dir.display(),
        ))?;
    Ok(basename.to_string())
}

fn files_for_kind(kind: Kind) -> &'static [&'static str] {
    match kind {
        Kind::Project => &[
            "Cargo.toml",
            "src/lib.rs",
            "src/web.rs",
            "index.html",
            ".gitignore",
        ],
        Kind::Library => &[
            "Cargo.toml",
            "src/lib.rs",
            "src/web.rs",
            "src/ios.rs",
            "src/android.rs",
            ".gitignore",
        ],
    }
}
