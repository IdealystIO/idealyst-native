//! Native blob storage on the real filesystem (iOS / macOS / Windows /
//! Linux / Android). All five share one `std::fs` implementation; the only
//! per-platform difference is *where* the app's private data directory
//! lives, resolved in [`app_data_dir`].

use std::path::PathBuf;

use crate::{safe_relative, FileError, FileFuture, FileStore};

/// A [`FileStore`] rooted at a base directory on the real filesystem.
pub struct FsFileStore {
    base: PathBuf,
}

impl FsFileStore {
    /// Resolve the app data dir + `name`, create it, and return the store.
    pub(crate) fn open(name: &str) -> Result<Self, FileError> {
        let base = app_data_dir(name)?;
        std::fs::create_dir_all(&base)
            .map_err(|e| FileError::Backend(format!("create {}: {e}", base.display())))?;
        Ok(Self { base })
    }

    fn resolve(&self, path: &str) -> Result<PathBuf, FileError> {
        Ok(self.base.join(safe_relative(path)?))
    }
}

impl FileStore for FsFileStore {
    fn read(&self, path: &str) -> FileFuture<'_, Option<Vec<u8>>> {
        let resolved = self.resolve(path);
        Box::pin(async move {
            let p = resolved?;
            match std::fs::read(&p) {
                Ok(bytes) => Ok(Some(bytes)),
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
                Err(e) => Err(FileError::Backend(format!("read {}: {e}", p.display()))),
            }
        })
    }

    fn write(&self, path: &str, bytes: &[u8]) -> FileFuture<'_, ()> {
        let resolved = self.resolve(path);
        let bytes = bytes.to_vec();
        Box::pin(async move {
            let p = resolved?;
            if let Some(parent) = p.parent() {
                std::fs::create_dir_all(parent)
                    .map_err(|e| FileError::Backend(format!("mkdir {}: {e}", parent.display())))?;
            }
            std::fs::write(&p, &bytes)
                .map_err(|e| FileError::Backend(format!("write {}: {e}", p.display())))
        })
    }

    fn delete(&self, path: &str) -> FileFuture<'_, ()> {
        let resolved = self.resolve(path);
        Box::pin(async move {
            let p = resolved?;
            match std::fs::remove_file(&p) {
                Ok(()) => Ok(()),
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
                Err(e) => Err(FileError::Backend(format!("delete {}: {e}", p.display()))),
            }
        })
    }

    fn exists(&self, path: &str) -> FileFuture<'_, bool> {
        let resolved = self.resolve(path);
        Box::pin(async move { Ok(resolved?.exists()) })
    }

    fn list(&self, dir: &str) -> FileFuture<'_, Vec<String>> {
        // `dir` may be empty (the store root); join handles that.
        let resolved = if dir.is_empty() {
            Ok(self.base.clone())
        } else {
            self.resolve(dir)
        };
        Box::pin(async move {
            let d = resolved?;
            match std::fs::read_dir(&d) {
                Ok(entries) => {
                    let mut names = Vec::new();
                    for entry in entries.flatten() {
                        if let Some(name) = entry.file_name().to_str() {
                            names.push(name.to_string());
                        }
                    }
                    names.sort();
                    Ok(names)
                }
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(Vec::new()),
                Err(e) => Err(FileError::Backend(format!("list {}: {e}", d.display()))),
            }
        })
    }

    fn local_path(&self, path: &str) -> Option<PathBuf> {
        self.resolve(path).ok()
    }
}

// ---------------------------------------------------------------------------
// App data directory resolution — the one per-platform piece.
// ---------------------------------------------------------------------------

/// Desktop: a per-user app-data directory from the platform's standard env
/// vars, plus the `name` subdir. macOS uses `~/Library/Application Support`,
/// Windows `%APPDATA%`, Linux `$XDG_DATA_HOME` (or `~/.local/share`).
#[cfg(all(not(target_os = "ios"), not(target_os = "android")))]
fn app_data_dir(name: &str) -> Result<PathBuf, FileError> {
    let base = if cfg!(target_os = "macos") {
        std::env::var_os("HOME").map(|h| PathBuf::from(h).join("Library/Application Support"))
    } else if cfg!(target_os = "windows") {
        std::env::var_os("APPDATA").map(PathBuf::from)
    } else {
        std::env::var_os("XDG_DATA_HOME")
            .map(PathBuf::from)
            .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".local/share")))
    };
    let base = base.ok_or_else(|| {
        FileError::NoAppDir("no HOME / APPDATA / XDG_DATA_HOME in the environment".into())
    })?;
    Ok(base.join(name))
}

/// iOS: the app sandbox's Application Support directory, via NSFileManager,
/// plus the `name` subdir. Resolved through the Obj-C runtime — no typed
/// framework crate needed.
#[cfg(target_os = "ios")]
fn app_data_dir(name: &str) -> Result<PathBuf, FileError> {
    use objc2::runtime::{AnyObject, Bool};
    use objc2::{class, msg_send};
    use std::ffi::CStr;
    use std::os::raw::c_char;

    // NSApplicationSupportDirectory = 14, NSUserDomainMask = 1.
    const NS_APPLICATION_SUPPORT_DIRECTORY: usize = 14;
    const NS_USER_DOMAIN_MASK: usize = 1;

    unsafe {
        let fm: *mut AnyObject = msg_send![class!(NSFileManager), defaultManager];
        // Single-URL API (NOT `URLsForDirectory:inDomains:`): returns ONE NSURL
        // directly, so we never touch an array's `count`. `URLsForDirectory`
        // returns a Swift-bridged `__SwiftDeferredNSArray` whose `count`
        // selector is encoded `'Q'` (NSUInteger), while objc2 encodes Rust
        // `usize` as `'q'` — a signedness mismatch that objc2's debug-build
        // runtime encoding check turns into a non-unwinding panic → abort, which
        // crashed the app the instant a record tap resolved the recordings store
        // (regular `CALayer.sublayers` arrays don't trip it; the Swift bridge
        // does). `error:` is ignored — a nil return is handled below.
        let mut err: *mut AnyObject = std::ptr::null_mut();
        let url: *mut AnyObject = msg_send![
            fm,
            URLForDirectory: NS_APPLICATION_SUPPORT_DIRECTORY,
            inDomain: NS_USER_DOMAIN_MASK,
            appropriateForURL: std::ptr::null::<AnyObject>(),
            create: Bool::NO,
            error: &mut err,
        ];
        if url.is_null() {
            return Err(FileError::NoAppDir(
                "NSFileManager could not resolve the Application Support URL".into(),
            ));
        }
        // NSURL.path → NSString → UTF-8.
        let path_ns: *mut AnyObject = msg_send![url, path];
        if path_ns.is_null() {
            return Err(FileError::NoAppDir("Application Support URL has no path".into()));
        }
        let utf8: *const c_char = msg_send![path_ns, UTF8String];
        if utf8.is_null() {
            return Err(FileError::NoAppDir("path was not convertible to UTF-8".into()));
        }
        let dir = CStr::from_ptr(utf8).to_string_lossy().into_owned();
        Ok(PathBuf::from(dir).join(name))
    }
}

/// Android: `Context.getFilesDir()` (internal, app-private storage), plus the
/// `name` subdir. Resolved through JNI against the host Activity context.
#[cfg(target_os = "android")]
fn app_data_dir(name: &str) -> Result<PathBuf, FileError> {
    use jni::objects::JObject;
    use jni::JavaVM;

    let ctx = ndk_context::android_context();
    let vm = unsafe { JavaVM::from_raw(ctx.vm() as *mut jni::sys::JavaVM) }
        .map_err(|e| FileError::NoAppDir(format!("invalid JavaVM pointer: {e}")))?;
    let mut env = vm
        .attach_current_thread()
        .map_err(|e| FileError::NoAppDir(format!("JNI attach: {e}")))?;
    let activity = unsafe { JObject::from_raw(ctx.context().cast()) };

    let map = |e: jni::errors::Error| FileError::NoAppDir(format!("JNI: {e}"));
    let files_dir = env
        .call_method(&activity, "getFilesDir", "()Ljava/io/File;", &[])
        .map_err(map)?
        .l()
        .map_err(map)?;
    let path_obj = env
        .call_method(&files_dir, "getAbsolutePath", "()Ljava/lang/String;", &[])
        .map_err(map)?
        .l()
        .map_err(map)?;
    let path: String = env.get_string(&path_obj.into()).map_err(map)?.into();
    Ok(PathBuf::from(path).join(name))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::FileStore;

    /// A store rooted at a unique temp dir, so tests touch no real app data.
    fn temp_store() -> FsFileStore {
        let base = std::env::temp_dir().join(format!("idealyst_files_test_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&base);
        FsFileStore { base }
    }

    #[tokio::test]
    async fn round_trip_read_write_delete() {
        let s = temp_store();
        assert_eq!(s.read("a/b.bin").await.unwrap(), None);
        assert!(!s.exists("a/b.bin").await.unwrap());
        s.write("a/b.bin", b"hello").await.unwrap(); // creates a/ too
        assert!(s.exists("a/b.bin").await.unwrap());
        assert_eq!(s.read("a/b.bin").await.unwrap(), Some(b"hello".to_vec()));
        s.write("a/b.bin", b"world!!").await.unwrap(); // overwrite
        assert_eq!(s.read("a/b.bin").await.unwrap(), Some(b"world!!".to_vec()));
        s.delete("a/b.bin").await.unwrap();
        assert_eq!(s.read("a/b.bin").await.unwrap(), None);
        s.delete("a/b.bin").await.unwrap(); // idempotent
    }

    #[tokio::test]
    async fn list_and_local_path() {
        let s = temp_store();
        s.write("docs/one.txt", b"1").await.unwrap();
        s.write("docs/two.txt", b"2").await.unwrap();
        assert_eq!(
            s.list("docs").await.unwrap(),
            vec!["one.txt".to_string(), "two.txt".to_string()]
        );
        assert!(s.list("missing").await.unwrap().is_empty());
        assert!(s.local_path("docs/one.txt").unwrap().ends_with("docs/one.txt"));
    }

    #[tokio::test]
    async fn rejects_unsafe_paths() {
        let s = temp_store();
        assert!(s.write("../escape", b"x").await.is_err());
        assert!(s.read("../escape").await.is_err());
        assert!(s.local_path("../escape").is_none());
    }
}
