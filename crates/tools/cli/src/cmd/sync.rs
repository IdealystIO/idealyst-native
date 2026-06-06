//! `idealyst sync` — regenerate derived assets (icons today, splash
//! later) from the project's `[package.metadata.idealyst]` config.
//!
//! Icons today cover web (favicon set), iOS (non-Xcode .app bundle
//! sizes), and Android (mipmap-*dpi launchers). Each platform reads
//! the same [`IconConfig`] but its own resolved [`IconBlock`] —
//! base + per-platform override.

use std::path::PathBuf;

use anyhow::Result;
use icon_gen::Target;

#[derive(clap::Args, Debug)]
pub struct Args {
    /// What to regenerate. Empty means "all kinds".
    #[arg(value_enum, num_args = 0..)]
    pub kinds: Vec<SyncKind>,

    /// Project directory. Defaults to the current directory. Kept
    /// as a flag (not a trailing positional) so `idealyst sync
    /// icons <path>` doesn't have to disambiguate between a kind
    /// and a path at parse time.
    #[arg(long, default_value = ".")]
    pub dir: PathBuf,
}

#[derive(Clone, Copy, Debug, clap::ValueEnum, PartialEq, Eq)]
pub enum SyncKind {
    Icons,
    Splash,
}

pub fn run(args: Args) -> Result<()> {
    let dir = args.dir.canonicalize().unwrap_or(args.dir.clone());
    let kinds = if args.kinds.is_empty() {
        vec![SyncKind::Icons, SyncKind::Splash]
    } else {
        args.kinds.clone()
    };

    for kind in kinds {
        match kind {
            SyncKind::Icons => sync_icons(&dir)?,
            SyncKind::Splash => {
                eprintln!("[sync] skipping splash: not yet implemented");
            }
        }
    }
    Ok(())
}

/// Generate every icon set the project's `[icon]` block produces.
/// Web / iOS / Android each receive their own resolved block (base
/// + per-platform overrides). No-op when the manifest has no
/// `[icon]` block.
fn sync_icons(project_dir: &std::path::Path) -> Result<()> {
    let Some(config) = icon_gen::load_config_from_manifest(project_dir)? else {
        println!(
            "[sync icons] no `[package.metadata.idealyst.app.icon]` block — \
             nothing to do (declare `source = \"path/to/icon.svg\"` to opt in)"
        );
        return Ok(());
    };

    let icons_root = project_dir
        .join("target")
        .join("idealyst")
        .join("icons");

    let web_block = config.resolved_for(Target::Web);
    let web_out = icons_root.join("web");
    if icon_gen::sync_web_icons(Some(&web_block), &web_out)?.is_some() {
        println!(
            "[sync icons] web    → {} (favicon.ico + 192/512/180 PNGs)",
            web_out.display()
        );
    }

    let ios_block = config.resolved_for(Target::Ios);
    let ios_out = icons_root.join("ios");
    if let Some(outs) = icon_gen::sync_ios_icons(Some(&ios_block), &ios_out)? {
        println!(
            "[sync icons] ios    → {} ({} home/spotlight/settings entries + 1024 marketing)",
            ios_out.display(),
            outs.entries.len(),
        );
    }

    let android_block = config.resolved_for(Target::Android);
    let android_out = icons_root.join("android");
    if let Some(outs) = icon_gen::sync_android_icons(Some(&android_block), &android_out)?
    {
        println!(
            "[sync icons] android → {} ({} dpi buckets + round variants)",
            android_out.display(),
            outs.launcher_pngs.len(),
        );
    }

    let macos_block = config.resolved_for(Target::Macos);
    let macos_out = icons_root.join("macos");
    icon_gen::sync_macos_icns(Some(&macos_block), &macos_out)?;
    println!(
        "[sync icons] macos   → {} (AppIcon.icns)",
        macos_out.display(),
    );
    Ok(())
}
