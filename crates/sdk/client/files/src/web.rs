//! Web blob storage via IndexedDB (a browser has no filesystem).
//!
//! Blobs are stored in one object store (`blobs`) keyed by their full path
//! string, values `Uint8Array`s. "Directories" are emulated by the
//! `/`-separated key prefix: `list("docs")` returns the immediate child names
//! of keys under `docs/`.
//!
//! The store holds only its database name (a `String`), so it stays
//! `Send + Sync`. Each operation is ONE call into the `files/idb` JS module
//! below, which opens the database, runs one transaction, waits for it to
//! COMPLETE (so a resolved write is durable, not merely queued), and closes
//! the connection — no connection outlives the operation. `local_path`
//! returns `None` — there's no filesystem path to hand out on web.
//!
//! Every browser call is a web-glue binding (docs/proposals/own-web-bindings.md;
//! this arm ran on the `idb` crate before phase 4).
//!
//! # Opening and upgrading
//!
//! The database is opened at its CURRENT version (`indexedDB.open(name)`,
//! no version): a new database is created at version 1 with the store, and an
//! existing one opens whatever version it is at. Only when the store is
//! missing — a database some older layout or other code created under this
//! name — is it reopened at `version + 1` to create the store; other stores
//! and their data are left alone. (Opening at a fixed version 1, as the idb
//! arm did, could never repair a store-less database and fails with a
//! `VersionError` on any database past version 1.)
//!
//! # Other connections
//!
//! Every connection this module opens closes itself on `versionchange`, so
//! it never blocks another tab's (or this tab's) upgrade. If an upgrade IS
//! blocked — another connection that ignores `versionchange` holds the
//! database open — the operation fails with a `Backend` error naming it
//! rather than hanging; the queued upgrade still completes once that
//! connection closes, and its connection is closed on arrival.
//!
//! Errors (a failed open, a transaction abort — e.g. quota — a request
//! error) all surface as [`FileError::Backend`] with `indexeddb: ` and the
//! DOMException's name and message.

use std::path::PathBuf;

use web_glue::js::{Array, Uint8Array};
use web_glue::{string, JsCast, JsError, JsFuture, JsValue};

use crate::{safe_relative, FileError, FileFuture, FileStore};

web_glue::js_module!(fn idb_module = "files/idb", r#"
const STORE = 'blobs';

// One `indexedDB.open`. `version` undefined opens the current version.
const openOnce = (name, version) => new Promise((resolve, reject) => {
  let req;
  try {
    req = version === undefined ? indexedDB.open(name) : indexedDB.open(name, version);
  } catch (e) { reject(e); return; }
  let gaveUp = false;
  req.onupgradeneeded = () => {
    const db = req.result;
    if (!db.objectStoreNames.contains(STORE)) db.createObjectStore(STORE);
  };
  req.onsuccess = () => {
    const db = req.result;
    // Never block someone else's upgrade: close as soon as one is asked for.
    db.onversionchange = () => db.close();
    if (gaveUp) { db.close(); return; }
    resolve(db);
  };
  req.onerror = () => reject(req.error);
  req.onblocked = () => {
    gaveUp = true;
    reject(new DOMException(
      `upgrading '${name}' is blocked by another open connection to it (another tab?)`,
      'BlockedError'));
  };
});

// Open with the store present, upgrading by one version if it is missing.
// A VersionError means another connection upgraded past the version we
// computed in between; the current version then has the store, so retry.
const open = async (name) => {
  for (let attempt = 0; ; attempt++) {
    const db = await openOnce(name);
    if (db.objectStoreNames.contains(STORE)) return db;
    const next = db.version + 1;
    db.close();
    try {
      const up = await openOnce(name, next);
      if (up.objectStoreNames.contains(STORE)) return up;
      up.close();
    } catch (e) {
      if (!(e && e.name === 'VersionError') || attempt >= 2) throw e;
    }
  }
};

// Run `fn(store)` in one transaction and resolve with its request's result
// once the transaction COMPLETES; reject with the transaction's error on
// abort. The connection is closed either way.
const run = async (name, mode, fn) => {
  const db = await open(name);
  try {
    return await new Promise((resolve, reject) => {
      let tx;
      try { tx = db.transaction(STORE, mode); } catch (e) { reject(e); return; }
      let out;
      tx.oncomplete = () => resolve(out);
      tx.onabort = () => reject(tx.error || new DOMException('transaction aborted', 'AbortError'));
      try {
        const r = fn(tx.objectStore(STORE));
        r.onsuccess = () => { out = r.result; };
      } catch (e) {
        try { tx.abort(); } catch (_) {}
        reject(e);
      }
    });
  } finally {
    db.close();
  }
};

// A stored value as bytes: what this crate stores is a Uint8Array; accept
// any ArrayBuffer / view too.
const bytes = (v) => {
  if (v === undefined) return undefined;
  if (v instanceof ArrayBuffer) return new Uint8Array(v);
  if (ArrayBuffer.isView(v)) return new Uint8Array(v.buffer, v.byteOffset, v.byteLength);
  throw new DOMException('stored value is not bytes', 'DataError');
};

return {
  open,
  get: (name, key) => run(name, 'readonly', (s) => s.get(key)).then(bytes),
  put: (name, key, value) => run(name, 'readwrite', (s) => s.put(value, key)).then(() => undefined),
  del: (name, key) => run(name, 'readwrite', (s) => s.delete(key)).then(() => undefined),
  keys: (name) => run(name, 'readonly', (s) => s.getAllKeys())
    .then((ks) => ks.filter((k) => typeof k === 'string')),
};
"#);

web_glue::import! {
    fn js_idb_get(np: usize, nl: usize, kp: usize, kl: usize) -> u32 =
        "(np, nl, kp, kl) => G.add(G.m('files/idb').get(G.str(np, nl), G.str(kp, kl)))";
    // The bytes are COPIED (`slice`) now: the write runs after this call
    // returns, and a view of wasm memory would not survive a memory growth.
    fn js_idb_put(np: usize, nl: usize, kp: usize, kl: usize, bp: usize, bl: usize) -> u32 =
        "(np, nl, kp, kl, bp, bl) => G.add(G.m('files/idb').put(G.str(np, nl), G.str(kp, kl), \
           G.u8().slice(bp >>> 0, (bp >>> 0) + (bl >>> 0))))";
    fn js_idb_del(np: usize, nl: usize, kp: usize, kl: usize) -> u32 =
        "(np, nl, kp, kl) => G.add(G.m('files/idb').del(G.str(np, nl), G.str(kp, kl)))";
    fn js_idb_keys(np: usize, nl: usize) -> u32 =
        "(np, nl) => G.add(G.m('files/idb').keys(G.str(np, nl)))";
    // `URL.createObjectURL(new Blob([copy of bytes], { type }))` into `out`.
    #[catch]
    fn js_object_url(bp: usize, bl: usize, tp: usize, tl: usize, out: usize) =
        "(bp, bl, tp, tl, o) => { const b = new Blob([G.u8().slice(bp >>> 0, (bp >>> 0) + (bl >>> 0))], \
           { type: G.str(tp, tl) }); G.retStr(URL.createObjectURL(b), o); }";
}

#[cfg(test)]
web_glue::import! {
    fn js_idb_open(np: usize, nl: usize) -> u32 =
        "(np, nl) => G.add(G.m('files/idb').open(G.str(np, nl)))";
}

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

fn err(e: &JsError) -> FileError {
    FileError::Backend(format!("indexeddb: {}", e.message()))
}

/// Await one `files/idb` operation's promise (a fresh handle from an import).
async fn settle(promise: u32) -> Result<JsValue, FileError> {
    idb_module(); // the anchor: keeps the `files/idb` record linked
    // SAFETY: a fresh `G.add` slot the import minted for us.
    let promise = unsafe { JsValue::from_raw(promise) };
    JsFuture::new(&promise).await.map_err(|e| err(&e))
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

async fn get(db: &str, key: &str) -> Result<Option<Vec<u8>>, FileError> {
    idb_module();
    let (np, nl) = string::abi(db);
    let (kp, kl) = string::abi(key);
    // SAFETY: borrowed strings for the call; the result is a fresh promise.
    let v = settle(unsafe { js_idb_get(np, nl, kp, kl) }).await?;
    Ok((!v.is_undefined()).then(|| v.unchecked_into::<Uint8Array>().to_vec()))
}

impl FileStore for IndexedDbFileStore {
    fn read(&self, path: &str) -> FileFuture<'_, Option<Vec<u8>>> {
        let key = key_for(path);
        Box::pin(async move { get(&self.db_name, &key?).await })
    }

    fn write(&self, path: &str, bytes: &[u8]) -> FileFuture<'_, ()> {
        let key = key_for(path);
        // The future may outlive the caller's slice (it borrows only
        // `self`), so own a copy until the call hands it to JS.
        let bytes = bytes.to_vec();
        Box::pin(async move {
            let key = key?;
            idb_module();
            let (np, nl) = string::abi(&self.db_name);
            let (kp, kl) = string::abi(&key);
            // SAFETY: borrowed strings and bytes for the call (the module
            // copies the bytes before returning); a fresh promise back.
            let p = unsafe { js_idb_put(np, nl, kp, kl, bytes.as_ptr() as usize, bytes.len()) };
            drop(bytes);
            settle(p).await?;
            Ok(())
        })
    }

    fn delete(&self, path: &str) -> FileFuture<'_, ()> {
        let key = key_for(path);
        Box::pin(async move {
            let key = key?;
            idb_module();
            let (np, nl) = string::abi(&self.db_name);
            let (kp, kl) = string::abi(&key);
            // SAFETY: borrowed strings for the call; a fresh promise back.
            settle(unsafe { js_idb_del(np, nl, kp, kl) }).await?;
            Ok(())
        })
    }

    fn exists(&self, path: &str) -> FileFuture<'_, bool> {
        let fut = self.read(path);
        Box::pin(async move { Ok(fut.await?.is_some()) })
    }

    fn list(&self, dir: &str) -> FileFuture<'_, Vec<String>> {
        // Normalize the dir into a key prefix ("" → root, else "dir/").
        let prefix = if dir.is_empty() {
            Ok(String::new())
        } else {
            key_for(dir).map(|k| format!("{k}/"))
        };
        Box::pin(async move {
            let prefix = prefix?;
            idb_module();
            let (np, nl) = string::abi(&self.db_name);
            // SAFETY: a borrowed string for the call; a fresh promise back.
            let keys = settle(unsafe { js_idb_keys(np, nl) }).await?.unchecked_into::<Array>();
            Ok(children(&prefix, keys.iter().filter_map(|k| k.as_string())))
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

/// Immediate children of `prefix` among `keys`: the segment after the prefix
/// up to the next `/`, deduplicated and sorted (so nested dirs show once).
fn children(prefix: &str, keys: impl Iterator<Item = String>) -> Vec<String> {
    let mut names = std::collections::BTreeSet::new();
    for k in keys {
        if let Some(rest) = k.strip_prefix(prefix) {
            if rest.is_empty() {
                continue;
            }
            let child = rest.split('/').next().unwrap_or(rest);
            names.insert(child.to_string());
        }
    }
    names.into_iter().collect()
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
    let mime = sniff_mime(bytes);
    let (tp, tl) = string::abi(mime);
    let mut result = Ok(());
    // SAFETY: borrowed bytes and type for the call (the snippet copies the
    // bytes before returning); `out` is the string out-slot.
    let url = string::receive(|out| {
        result = unsafe { js_object_url(bytes.as_ptr() as usize, bytes.len(), tp, tl, out) };
    });
    result.ok().map(|()| url)
}

fn sniff_mime(bytes: &[u8]) -> &'static str {
    if bytes.len() >= 8 && &bytes[4..8] == b"ftyp" {
        "video/mp4"
    } else if bytes.len() >= 4 && bytes[..4] == [0x1A, 0x45, 0xDF, 0xA3] {
        "video/webm"
    } else {
        "application/octet-stream"
    }
}

#[cfg(test)]
mod tests;
