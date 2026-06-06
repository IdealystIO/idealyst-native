//! Web blob storage via IndexedDB (a browser has no filesystem).
//!
//! Blobs are stored in one object store keyed by their full path string.
//! "Directories" are emulated by the `/`-separated key prefix: `list("docs")`
//! returns the immediate child names of keys under `docs/`.
//!
//! The store holds only its database name (a `String`), so it stays
//! `Send + Sync`; each operation opens the database. `local_path` returns
//! `None` — there's no filesystem path to hand out on web.

use idb::{Database, DatabaseEvent, Factory, ObjectStoreParams, TransactionMode};
use js_sys::Uint8Array;
use std::path::PathBuf;
use wasm_bindgen::JsValue;

use crate::{safe_relative, FileError, FileFuture, FileStore};

const STORE: &str = "blobs";

/// A [`FileStore`] over an IndexedDB database, blobs keyed by path.
pub struct IndexedDbFileStore {
    db_name: String,
}

impl IndexedDbFileStore {
    pub(crate) fn new(name: &str) -> Self {
        Self {
            db_name: format!("idealyst.files.{name}"),
        }
    }
}

fn err(e: impl std::fmt::Display) -> FileError {
    FileError::Backend(format!("indexeddb: {e}"))
}

/// Open (and on first use, create) the database + its single object store.
async fn open_db(name: &str) -> Result<Database, FileError> {
    let factory = Factory::new().map_err(err)?;
    let mut request = factory.open(name, Some(1)).map_err(err)?;
    request.on_upgrade_needed(|event| {
        if let Ok(db) = event.database() {
            // Ignore an already-exists error on a racing upgrade.
            let _ = db.create_object_store(STORE, ObjectStoreParams::new());
        }
    });
    request.await.map_err(err)
}

/// Validate a relative path and return its `/`-joined key.
fn key_for(path: &str) -> Result<String, FileError> {
    let rel = safe_relative(path)?;
    Ok(rel
        .components()
        .filter_map(|c| c.as_os_str().to_str())
        .collect::<Vec<_>>()
        .join("/"))
}

impl FileStore for IndexedDbFileStore {
    fn read(&self, path: &str) -> FileFuture<'_, Option<Vec<u8>>> {
        let db_name = self.db_name.clone();
        let key = key_for(path);
        Box::pin(async move {
            let key = key?;
            let db = open_db(&db_name).await?;
            let tx = db
                .transaction(&[STORE], TransactionMode::ReadOnly)
                .map_err(err)?;
            let store = tx.object_store(STORE).map_err(err)?;
            let value = store
                .get(JsValue::from_str(&key))
                .map_err(err)?
                .await
                .map_err(err)?;
            Ok(value.map(|v| Uint8Array::new(&v).to_vec()))
        })
    }

    fn write(&self, path: &str, bytes: &[u8]) -> FileFuture<'_, ()> {
        let db_name = self.db_name.clone();
        let key = key_for(path);
        let value = Uint8Array::from(bytes);
        Box::pin(async move {
            let key = key?;
            let db = open_db(&db_name).await?;
            let tx = db
                .transaction(&[STORE], TransactionMode::ReadWrite)
                .map_err(err)?;
            let store = tx.object_store(STORE).map_err(err)?;
            store
                .put(&value, Some(&JsValue::from_str(&key)))
                .map_err(err)?
                .await
                .map_err(err)?;
            tx.commit().map_err(err)?.await.map_err(err)?;
            Ok(())
        })
    }

    fn delete(&self, path: &str) -> FileFuture<'_, ()> {
        let db_name = self.db_name.clone();
        let key = key_for(path);
        Box::pin(async move {
            let key = key?;
            let db = open_db(&db_name).await?;
            let tx = db
                .transaction(&[STORE], TransactionMode::ReadWrite)
                .map_err(err)?;
            let store = tx.object_store(STORE).map_err(err)?;
            store
                .delete(JsValue::from_str(&key))
                .map_err(err)?
                .await
                .map_err(err)?;
            tx.commit().map_err(err)?.await.map_err(err)?;
            Ok(())
        })
    }

    fn exists(&self, path: &str) -> FileFuture<'_, bool> {
        let fut = self.read(path);
        Box::pin(async move { Ok(fut.await?.is_some()) })
    }

    fn list(&self, dir: &str) -> FileFuture<'_, Vec<String>> {
        let db_name = self.db_name.clone();
        // Normalize the dir into a key prefix ("" → root, else "dir/").
        let prefix = if dir.is_empty() {
            Ok(String::new())
        } else {
            key_for(dir).map(|k| format!("{k}/"))
        };
        Box::pin(async move {
            let prefix = prefix?;
            let db = open_db(&db_name).await?;
            let tx = db
                .transaction(&[STORE], TransactionMode::ReadOnly)
                .map_err(err)?;
            let store = tx.object_store(STORE).map_err(err)?;
            let keys = store.get_all_keys(None, None).map_err(err)?.await.map_err(err)?;

            // Immediate children of `prefix`: the segment after the prefix up
            // to the next `/`, deduplicated (so nested dirs show once).
            let mut names = std::collections::BTreeSet::new();
            for k in keys {
                if let Some(s) = k.as_string() {
                    if let Some(rest) = s.strip_prefix(&prefix) {
                        if rest.is_empty() {
                            continue;
                        }
                        let child = rest.split('/').next().unwrap_or(rest);
                        names.insert(child.to_string());
                    }
                }
            }
            Ok(names.into_iter().collect())
        })
    }

    fn local_path(&self, _path: &str) -> Option<PathBuf> {
        None // no filesystem on web
    }

    fn loadable_url(&self, path: &str) -> FileFuture<'_, Option<String>> {
        let fut = self.read(path);
        Box::pin(async move {
            let Some(bytes) = fut.await? else { return Ok(None) };
            Ok(blob_url_from_bytes(&bytes))
        })
    }
}

/// Wrap stored bytes in an object URL (`URL.createObjectURL`) the browser can
/// load. The container is sniffed from magic bytes so a media element picks the
/// right decoder: MP4 (`ftyp` box, Safari's MediaRecorder) and WebM/Matroska
/// (EBML header, Chromium's) — the two recording containers — are labeled
/// explicitly; anything else falls back to `application/octet-stream`.
///
/// The URL is intentionally not revoked here — the caller owns its lifetime
/// (a recorded blob is released on page reload; a long-lived app should
/// `URL.revokeObjectURL` when done).
fn blob_url_from_bytes(bytes: &[u8]) -> Option<String> {
    let arr = js_sys::Uint8Array::from(bytes);
    let parts = js_sys::Array::new();
    parts.push(&arr);
    let opts = web_sys::BlobPropertyBag::new();
    let mime = if bytes.len() >= 8 && &bytes[4..8] == b"ftyp" {
        "video/mp4"
    } else if bytes.len() >= 4 && bytes[..4] == [0x1A, 0x45, 0xDF, 0xA3] {
        "video/webm"
    } else {
        "application/octet-stream"
    };
    opts.set_type(mime);
    let blob = web_sys::Blob::new_with_u8_array_sequence_and_options(&parts, &opts).ok()?;
    web_sys::Url::create_object_url_with_blob(&blob).ok()
}
