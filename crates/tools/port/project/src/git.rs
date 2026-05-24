//! `git clone` shell-out for the `--from-git` flag.
//!
//! Kept simple: we shell to the system `git`, depth-1 clone into
//! a temp directory under `$TMPDIR/port-project-<hash>`. The
//! cloned tree is left in place after the port so the user can
//! inspect both inputs and outputs.

use std::path::{Path, PathBuf};
use std::process::Command;

pub fn clone_to_temp(url: &str) -> Result<PathBuf, String> {
    let dest = temp_dir_for(url);
    if dest.exists() {
        // Reuse an existing clone — rerunning with the same URL
        // shouldn't re-clone every time.
        return Ok(dest);
    }
    if let Some(parent) = dest.parent() {
        std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
    }
    let status = Command::new("git")
        .args(["clone", "--depth", "1", url])
        .arg(&dest)
        .status()
        .map_err(|e| format!("failed to run git: {}", e))?;
    if !status.success() {
        return Err(format!("git clone exited with {}", status));
    }
    Ok(dest)
}

fn temp_dir_for(url: &str) -> PathBuf {
    let mut name = String::from("port-project-");
    for ch in url.chars() {
        if ch.is_ascii_alphanumeric() {
            name.push(ch);
        } else {
            name.push('_');
        }
    }
    // Cap the directory name length so very long URLs don't blow
    // past filesystem limits.
    name.truncate(120);
    let mut p = std::env::temp_dir();
    p.push(name);
    p
}

pub fn is_url(s: &str) -> bool {
    s.starts_with("http://")
        || s.starts_with("https://")
        || s.starts_with("git@")
        || s.starts_with("ssh://")
}

#[allow(dead_code)]
pub fn _force_path(_root: &Path) {}
