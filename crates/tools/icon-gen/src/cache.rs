//! Per-platform output-freshness cache.
//!
//! Each `sync_*_icons` function writes a tiny `.icon-gen.cache.json`
//! sidecar next to its output set with:
//!
//! 1. A `version` constant ([`CACHE_VERSION`]) bumped whenever the
//!    rendering math changes — guarantees a stale cache from an older
//!    crate build can never silently win.
//! 2. A SHA-256 fingerprint of (config-without-paths) + (input file
//!    bytes). Paths are excluded so moving the project doesn't bust
//!    the cache; file CONTENT is what defines a fresh render.
//! 3. The serialized output struct itself. On hit, we deserialize
//!    and return it as if we'd done the work — and we verify every
//!    file the cache claims actually exists before trusting it.
//!
//! Cache misses are silent: the sync function just regenerates and
//! overwrites the sidecar. The caller can't tell whether the work
//! was done or skipped — by design (output is identical either way).

use anyhow::Result;
use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::fs;
use std::path::Path;

use crate::manifest::IconBlock;

/// Bumped whenever the *output bytes* of a render could change for
/// the same input — gradient math, anti-alias defaults, padding
/// default, the standard size set, anything that affects pixel
/// output. Any change to the file naming convention also bumps this.
///
/// The bump turns every cached render into a miss, which costs one
/// re-rasterization — cheap insurance against shipping stale output
/// after a render-code change.
pub(crate) const CACHE_VERSION: u32 = 1;

/// On-disk shape. Generic over the per-platform output type so each
/// sync function can persist its own structured result without a
/// common base type.
#[derive(Serialize, Deserialize)]
struct CacheFile<T> {
    version: u32,
    fingerprint: String,
    outputs: T,
}

/// Try to use an existing cache. Returns `Ok(Some(outputs))` when:
/// - the cache file exists,
/// - its version matches [`CACHE_VERSION`],
/// - its fingerprint matches the fingerprint we just computed for
///   the current block,
/// - every file the cache references is still on disk (any missing
///   output triggers a full regen so partial cleanup can't poison
///   future runs).
///
/// Returns `Ok(None)` in every miss case — never errors on a stale
/// or malformed cache, because falling through to a fresh render is
/// always the correct response.
pub(crate) fn try_hit<T: DeserializeOwned + AllOutputs>(
    cache_path: &Path,
    block: &IconBlock,
) -> Result<Option<T>> {
    let Ok(raw) = fs::read(cache_path) else {
        return Ok(None);
    };
    let Ok(file): Result<CacheFile<T>, _> = serde_json::from_slice(&raw) else {
        return Ok(None);
    };
    if file.version != CACHE_VERSION {
        return Ok(None);
    }
    let current = fingerprint(block)?;
    if file.fingerprint != current {
        return Ok(None);
    }
    // Output-file existence check: a hit isn't valid if someone
    // deleted one of the PNGs the cache claims. Regen everything.
    if !file.outputs.all_files_exist() {
        return Ok(None);
    }
    Ok(Some(file.outputs))
}

pub(crate) fn write<T: Serialize>(
    cache_path: &Path,
    block: &IconBlock,
    outputs: &T,
) -> Result<()> {
    let file = CacheFile {
        version: CACHE_VERSION,
        fingerprint: fingerprint(block)?,
        outputs,
    };
    let bytes = serde_json::to_vec_pretty(&file)?;
    fs::write(cache_path, bytes)?;
    Ok(())
}

/// Standard sidecar name. Goes next to the generated assets so a
/// `rm -rf target/idealyst/icons/web/` blows it away cleanly.
pub(crate) const CACHE_FILE_NAME: &str = ".icon-gen.cache.json";

/// Hash the inputs that determine the output bytes. Excludes paths
/// (so moving the project tree doesn't bust the cache) and the
/// `source`/`foreground` filenames themselves; INCLUDES every file's
/// full content + the structural config (background, padding).
fn fingerprint(block: &IconBlock) -> Result<String> {
    let mut hasher = Sha256::new();

    // Config-without-paths in a stable JSON form. Using
    // `serde_json::to_value` and then `to_string` would also work,
    // but `to_vec_pretty` keeps key ordering deterministic across
    // serde-json versions in a way `to_string` historically hasn't.
    let cfg = ConfigHash {
        background: block.background.as_ref(),
        foreground_padding: block.foreground_padding,
    };
    let cfg_bytes = serde_json::to_vec(&cfg)?;
    hasher.update(b"cfg:");
    hasher.update(&cfg_bytes);

    // Tag each input so a file at `source` vs `foreground` with
    // the same content can't collide with a different IconBlock
    // shape that swapped the two — defense against an edge case
    // that's hard to hit but cheap to prevent.
    if let Some(p) = &block.source {
        let bytes = fs::read(p)?;
        hasher.update(b"source:");
        hasher.update(&bytes);
    }
    if let Some(p) = &block.foreground {
        let bytes = fs::read(p)?;
        hasher.update(b"foreground:");
        hasher.update(&bytes);
    }

    Ok(format!("{:x}", hasher.finalize()))
}

#[derive(Serialize)]
struct ConfigHash<'a> {
    background: Option<&'a crate::manifest::Background>,
    foreground_padding: Option<f32>,
}

/// Implemented by every per-platform output type. A cache hit is
/// only valid if every file the cached result claims still exists
/// on disk; this trait surfaces that check without each sync
/// function needing to know cache internals.
pub(crate) trait AllOutputs {
    fn all_files_exist(&self) -> bool;
}
