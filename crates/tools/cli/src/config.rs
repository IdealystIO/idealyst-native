//! Project manifest parsing.
//!
//! Idealyst configuration lives under `[package.metadata.idealyst]`
//! in the project's `Cargo.toml`. Keeping it inside Cargo.toml avoids
//! two sources of truth and lets tools that already understand Cargo
//! workspaces parse it for free.
//!
//! Most of this module is still stub-shaped — the wider shape
//! exists so subcommands can plug in as they need it. The icon
//! block has graduated out of here: its parser lives in `icon-gen`
//! (`icon_gen::load_spec_from_manifest`) so CLI and build crates
//! share one schema source of truth.

#![allow(dead_code)]

use serde::Deserialize;
use std::collections::BTreeMap;
use std::path::Path;

use crate::Platform;

/// Top-level project config, as it appears under
/// `[package.metadata.idealyst]`.
#[derive(Debug, Deserialize)]
pub struct ProjectConfig {
    pub app: AppConfig,
    #[serde(default)]
    pub platforms: BTreeMap<Platform, PlatformOverrides>,
}

#[derive(Debug, Deserialize)]
pub struct AppConfig {
    pub name: String,
    pub bundle_id: String,
    #[serde(default)]
    pub version: Option<String>,
    #[serde(default)]
    pub icon: Option<IconConfig>,
    #[serde(default)]
    pub splash: Option<SplashConfig>,
}

#[derive(Debug, Deserialize)]
pub struct IconConfig {
    /// Master icon, sliced into per-platform sizes by `idealyst sync`.
    /// PNG or SVG. Android adaptive icons take `foreground` +
    /// `background` instead.
    #[serde(default)]
    pub source: Option<String>,
    #[serde(default)]
    pub foreground: Option<String>,
    #[serde(default)]
    pub background: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct SplashConfig {
    #[serde(default)]
    pub background: Option<String>,
    #[serde(default)]
    pub background_image: Option<String>,
    #[serde(default)]
    pub logo: Option<String>,
    #[serde(default)]
    pub logo_scale: Option<String>,
    #[serde(default)]
    pub dark: Option<Box<SplashConfig>>,
}

#[derive(Debug, Default, Deserialize)]
pub struct PlatformOverrides {
    #[serde(default)]
    pub team_id: Option<String>,
    #[serde(default)]
    pub deployment_target: Option<String>,
    #[serde(default)]
    pub min_sdk: Option<u32>,
    #[serde(default)]
    pub target_sdk: Option<u32>,
}

/// Load `ProjectConfig` from a project's `Cargo.toml`. Not wired into
/// any command yet — kept here so the shape is reviewable and the
/// first command that needs it can plug straight in.
#[allow(dead_code)]
pub fn load(_manifest_dir: &Path) -> anyhow::Result<ProjectConfig> {
    anyhow::bail!("config::load is not yet implemented")
}
