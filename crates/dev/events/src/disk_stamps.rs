//! A stat of every file under a watcher's roots, for a `--no-watch` rebuild
//! request.
//!
//! A watcher reports a save only after its debounce and the OS's own latency,
//! so a request sent right after a save reaches the loop first. A request must
//! not be answered from what the watcher has reported so far: that build or
//! decision leaves the save out, the next request is answered from the earlier
//! one ("no change"), and the new edit stays queued behind it. So a request also
//! asks the disk what moved ([`DiskStamps::moved`]).
//!
//! A stat of each file, so a request does not read every source. A rewrite that
//! keeps a file's size and modification time (a filesystem with coarse
//! timestamps) is not seen here, and reaches the queue through the watcher.

use std::collections::HashMap;
use std::fs::Metadata;
use std::path::{Path, PathBuf};
use std::time::SystemTime;

type Stamp = (Option<SystemTime>, u64);

fn stamp(meta: &Metadata) -> Stamp {
    (meta.modified().ok(), meta.len())
}

/// Every file under `roots` a save of could matter: hidden entries, `target/`
/// and an editor's scratch files are skipped, as the watchers' filters would.
pub fn for_each_watched_file(roots: &[PathBuf], mut each: impl FnMut(PathBuf, Metadata)) {
    let mut stack: Vec<PathBuf> = roots.to_vec();
    while let Some(path) = stack.pop() {
        let Ok(meta) = std::fs::metadata(&path) else { continue };
        if meta.is_dir() {
            let Ok(entries) = std::fs::read_dir(&path) else { continue };
            for entry in entries.flatten() {
                let name = entry.file_name();
                let name = name.to_string_lossy();
                if name.starts_with('.') || name == "target" {
                    continue;
                }
                stack.push(entry.path());
            }
            continue;
        }
        let name = path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
        if crate::is_scratch_file(&name) {
            continue;
        }
        each(path, meta);
    }
}

/// The size and modification time of every file under a watcher's roots, as of
/// the last [`Self::moved`].
#[derive(Debug, Default)]
pub struct DiskStamps {
    roots: Vec<PathBuf>,
    files: HashMap<PathBuf, Stamp>,
}

impl DiskStamps {
    fn stamps(roots: &[PathBuf]) -> HashMap<PathBuf, Stamp> {
        let mut files = HashMap::new();
        for_each_watched_file(roots, |path, meta| {
            files.insert(path, stamp(&meta));
        });
        files
    }

    /// Follow `roots` from now on. A root new to this set is recorded as it
    /// stands, since its files are not saves; a root kept keeps its record, so
    /// what moved under it is still reported.
    ///
    /// Canonical, as a watcher reports paths, so a save found here and its late
    /// event name the same file (`/var` is `/private/var` on macOS).
    pub fn follow(&mut self, roots: &[PathBuf]) {
        let roots: Vec<PathBuf> = roots.iter().map(|r| r.canonicalize().unwrap_or_else(|_| r.clone())).collect();
        let added: Vec<PathBuf> = roots.iter().filter(|r| !self.roots.contains(r)).cloned().collect();
        self.files.retain(|file, _| roots.iter().any(|r| file.starts_with(r)));
        self.files.extend(Self::stamps(&added));
        self.roots = roots;
    }

    /// The files changed, added or removed since the last call, sorted.
    pub fn moved(&mut self) -> Vec<PathBuf> {
        let now = Self::stamps(&self.roots);
        let mut moved: Vec<PathBuf> = now
            .iter()
            .filter(|(file, stamp)| self.files.get(*file) != Some(*stamp))
            .map(|(file, _)| file.clone())
            .chain(self.files.keys().filter(|file| !now.contains_key(*file)).cloned())
            .collect();
        moved.sort();
        self.files = now;
        moved
    }

    /// Whether `path` stands as recorded: an event for it reports nothing the
    /// last [`Self::moved`] did not.
    pub fn covers(&self, path: &Path) -> bool {
        match std::fs::metadata(path) {
            Ok(meta) => self.files.get(path) == Some(&stamp(&meta)),
            Err(_) => !self.files.contains_key(path),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn names(paths: &[PathBuf]) -> Vec<String> {
        paths.iter().map(|p| p.file_name().unwrap().to_string_lossy().into_owned()).collect()
    }

    #[test]
    fn it_reports_what_moved_since_the_last_look() {
        let dir = std::env::temp_dir().join(format!("disk-stamps-{}", std::process::id()));
        let other = dir.join("other");
        let root = dir.join("root");
        std::fs::create_dir_all(&root).unwrap();
        std::fs::create_dir_all(&other).unwrap();
        let (root, other) = (root.canonicalize().unwrap(), other.canonicalize().unwrap());
        let (a, b) = (root.join("a.rs"), root.join("b.rs"));
        std::fs::write(&a, "a").unwrap();
        std::fs::write(&b, "b").unwrap();
        let mut disk = DiskStamps::default();
        disk.follow(std::slice::from_ref(&root));
        assert!(disk.moved().is_empty(), "files present when it started following are no saves");
        assert!(disk.covers(&a));

        std::fs::write(&a, "a2").unwrap();
        assert!(!disk.covers(&a), "not looked at yet");
        assert_eq!(names(&disk.moved()), ["a.rs"]);
        assert!(disk.covers(&a), "its late event reports nothing new");
        assert!(disk.moved().is_empty(), "reported once");

        // Editors' scratch files and hidden files are not saves.
        std::fs::write(root.join(".a.rs.swp"), "x").unwrap();
        std::fs::write(root.join("a.rs~"), "x").unwrap();
        assert!(disk.moved().is_empty());

        std::fs::remove_file(&b).unwrap();
        assert!(!disk.covers(&b), "removed since the last look");
        assert_eq!(names(&disk.moved()), ["b.rs"]);
        assert!(disk.covers(&b));

        // A root new to the set is not a save; a kept root still reports.
        std::fs::write(other.join("c.rs"), "c").unwrap();
        std::fs::write(&a, "a3").unwrap();
        disk.follow(&[root.clone(), other.clone()]);
        assert_eq!(names(&disk.moved()), ["a.rs"]);
        std::fs::write(other.join("c.rs"), "c2").unwrap();
        assert_eq!(names(&disk.moved()), ["c.rs"]);

        // A root dropped is forgotten.
        disk.follow(std::slice::from_ref(&root));
        std::fs::write(other.join("c.rs"), "c3").unwrap();
        assert!(disk.moved().is_empty());

        let _ = std::fs::remove_dir_all(&dir);
    }
}
