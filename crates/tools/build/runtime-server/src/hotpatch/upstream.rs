//! The running binary's own build products, for what a patch cannot get
//! from the binary itself: the rlibs cargo linked it from.
//!
//! The binary was linked with dead code stripped, so it holds less than its
//! crates compiled. A static missing from it may have been stripped or may
//! have been renamed by the edit, and only the crate's rlib, which holds every
//! static it compiled, can tell ([`super::statics::pair_vouched`]).
//!
//! An rlib speaks for the base only if the base's link came after it: a
//! later compile (a session check, a rebuild that failed further up) leaves
//! a newer rlib and an unchanged binary, and then neither answer is known.

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::SystemTime;

use anyhow::{Context, Result};
use object::read::archive::ArchiveFile;
use object::ReadCache;

use super::statics::ProgramStatics;

/// The rlibs a patch may read. Cheap to create; everything is read on demand,
/// because most patches never ask.
pub struct Rlibs {
    /// When the base was linked. `None`: unknown, and no rlib speaks for it.
    base_linked: Option<SystemTime>,
    /// Each replayed crate's statics by its rlib, read at most once.
    statics: Mutex<HashMap<PathBuf, Option<ProgramStatics>>>,
}

impl Rlibs {
    pub fn new(base_linked: Option<SystemTime>) -> Self {
        Self { base_linked, statics: Mutex::default() }
    }

    fn speaks_for_base(&self, rlib: &Path) -> bool {
        let built = std::fs::metadata(rlib).and_then(|m| m.modified()).ok();
        matches!((built, self.base_linked), (Some(built), Some(linked)) if built <= linked)
    }

    /// The statics `rlib` holds under [`super::statics::static_key`] `key`,
    /// if it speaks for the base.
    pub fn vouch(&self, rlib: &Path, key: &str) -> Option<HashSet<String>> {
        if !self.speaks_for_base(rlib) {
            return None;
        }
        let mut statics = self.statics.lock().unwrap();
        let read = statics.entry(rlib.to_path_buf()).or_insert_with(|| read_statics(rlib).ok());
        read.as_ref()?.group(key).cloned()
    }
}

/// Every static an rlib's objects define, grouped for pairing.
fn read_statics(rlib: &Path) -> Result<ProgramStatics> {
    let file = std::fs::File::open(rlib).with_context(|| format!("open {}", rlib.display()))?;
    let cache = ReadCache::new(file);
    let archive = ArchiveFile::parse(&cache).with_context(|| format!("parse {}", rlib.display()))?;
    let mut names = Vec::new();
    for member in archive.members() {
        let member = member?;
        if !member.name().ends_with(b".o") {
            continue;
        }
        let bytes = member.data(&cache)?;
        let Ok(object) = object::File::parse(bytes) else { continue };
        names.extend(super::cache::state_names(&object));
    }
    Ok(ProgramStatics::new(names.iter().map(String::as_str)))
}
