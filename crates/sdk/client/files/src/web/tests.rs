//! Browser tests for the IndexedDB store: the CRUD + list surface, opening
//! databases this crate did not create (store-less, past version 1), the two
//! multi-connection cases (our connection yields to another upgrade; a
//! foreign connection that ignores `versionchange` blocks ours), error
//! mapping, and `loadable_url`.
//!
//! In-crate (not `tests/`) because the versionchange test holds a connection
//! the module opened, through the test-only `js_idb_open` hook. Run with
//! `cargo test -p files --lib --target wasm32-unknown-unknown` (the workspace
//! runner supplies web-glue's JS; `wasm-pack test` cannot).

use std::future::{poll_fn, Future};
use std::pin::Pin;
use std::task::Poll;

use wasm_bindgen_test::*;
use web_glue::{string, JsFuture, JsValue};

use super::{idb_module, js_idb_open, IndexedDbFileStore};
use crate::{FileError, FileStore};

wasm_bindgen_test_configure!(run_in_browser);

fn eval(body: &str) -> JsValue {
    let f = JsValue::global()
        .get("Function")
        .unwrap()
        .construct(&[&JsValue::from_str(body)])
        .unwrap();
    f.call(&JsValue::undefined(), &[]).unwrap()
}

/// Run an async JS body and await its result.
async fn eval_async(body: &str) -> Result<JsValue, String> {
    let p = eval(&format!("return (async () => {{ {body} }})();"));
    JsFuture::new(&p).await.map_err(|e| e.message())
}

async fn sleep(ms: u32) {
    let p = eval(&format!("return new Promise((r) => setTimeout(r, {ms}));"));
    JsFuture::new(&p).await.unwrap();
}

/// `fut`, or `None` after `ms` — so a hang fails as an assertion.
async fn within<F: Future>(ms: u32, fut: F) -> Option<F::Output> {
    let mut fut = Box::pin(fut);
    let mut timer = Box::pin(sleep(ms));
    poll_fn(|cx| {
        if let Poll::Ready(v) = fut.as_mut().poll(cx) {
            return Poll::Ready(Some(v));
        }
        if let Poll::Ready(()) = Pin::new(&mut timer).poll(cx) {
            return Poll::Ready(None);
        }
        Poll::Pending
    })
    .await
}

/// A fresh store name, its database deleted first (a re-run starts clean).
async fn fresh(tag: &str) -> (IndexedDbFileStore, String) {
    let store = IndexedDbFileStore::new(&format!("test-{tag}"));
    let db = store.db_name.clone();
    eval_async(&format!(
        "await new Promise((r) => {{ const q = indexedDB.deleteDatabase('{db}'); \
           q.onsuccess = q.onerror = q.onblocked = () => r(); }});"
    ))
    .await
    .unwrap();
    (store, db)
}

/// The JS for "open `db` at `version`, run `upgrade(db)` in onupgradeneeded".
fn raw_open(db: &str, version: u32, upgrade: &str) -> String {
    format!(
        "await new Promise((res, rej) => {{ const q = indexedDB.open('{db}', {version}); \
           q.onupgradeneeded = () => {{ const db = q.result; {upgrade} }}; \
           q.onsuccess = () => res(q.result); q.onerror = () => rej(q.error); }})"
    )
}

async fn db_version_and_stores(db: &str) -> String {
    eval_async(&format!(
        "const d = await new Promise((res, rej) => {{ const q = indexedDB.open('{db}'); \
           q.onsuccess = () => res(q.result); q.onerror = () => rej(q.error); }}); \
         const s = `${{d.version}}:${{Array.from(d.objectStoreNames).sort().join(',')}}`; d.close(); return s;"
    ))
    .await
    .unwrap()
    .as_string()
    .unwrap()
}

#[wasm_bindgen_test]
async fn write_read_exists_list_delete_round_trip() {
    let (store, db) = fresh("crud").await;
    let all: Vec<u8> = (0..=255).collect();

    assert_eq!(store.read("a/b.bin").await.unwrap(), None);
    assert!(!store.exists("a/b.bin").await.unwrap());
    assert!(store.list("").await.unwrap().is_empty(), "a new store lists nothing");

    store.write("a/b.bin", &all).await.unwrap();
    store.write("a/c/d.bin", b"nested").await.unwrap();
    store.write("top.bin", &[]).await.unwrap();

    assert_eq!(store.read("a/b.bin").await.unwrap(), Some(all.clone()), "byte-exact");
    assert_eq!(store.read("./a/b.bin").await.unwrap(), Some(all), "`.` components normalise");
    assert_eq!(store.read("top.bin").await.unwrap(), Some(Vec::new()), "an empty blob exists");
    assert!(store.exists("a/c/d.bin").await.unwrap());

    assert_eq!(store.list("").await.unwrap(), vec!["a".to_string(), "top.bin".to_string()]);
    assert_eq!(store.list("a").await.unwrap(), vec!["b.bin".to_string(), "c".to_string()]);
    assert_eq!(store.list("a/c").await.unwrap(), vec!["d.bin".to_string()]);
    assert!(store.list("missing").await.unwrap().is_empty());

    store.write("a/b.bin", b"replaced").await.unwrap();
    assert_eq!(store.read("a/b.bin").await.unwrap().as_deref(), Some(&b"replaced"[..]));

    store.delete("a/b.bin").await.unwrap();
    assert_eq!(store.read("a/b.bin").await.unwrap(), None);
    store.delete("a/b.bin").await.unwrap(); // deleting a missing blob is Ok
    assert_eq!(store.list("a").await.unwrap(), vec!["c".to_string()]);

    // A new database is created at version 1 with exactly the blobs store.
    assert_eq!(db_version_and_stores(&db).await, "1:blobs");
}

#[wasm_bindgen_test]
async fn a_write_is_visible_to_another_store_instance() {
    // Each op opens and closes its own connection, and a resolved write has
    // COMMITTED — a second store over the same name sees it.
    let (a, _) = fresh("shared").await;
    let b = IndexedDbFileStore::new("test-shared");
    a.write("k", b"v").await.unwrap();
    assert_eq!(b.read("k").await.unwrap().as_deref(), Some(&b"v"[..]));
}

#[wasm_bindgen_test]
async fn unsafe_paths_are_rejected_before_touching_the_database() {
    let (store, _) = fresh("unsafe").await;
    assert!(matches!(store.read("../x").await, Err(FileError::UnsafePath(_))));
    assert!(matches!(store.write("/etc/x", b"").await, Err(FileError::UnsafePath(_))));
    assert!(matches!(store.delete("a/../../b").await, Err(FileError::UnsafePath(_))));
    assert!(matches!(store.list("..").await, Err(FileError::UnsafePath(_))));
}

/// Regression: the idb arm opened at a fixed version 1 and created the store
/// only in `onupgradeneeded`, so a database that already existed under this
/// name WITHOUT the store (an older layout) never got one — every op failed
/// with a `NotFoundError`. It is now upgraded by one version, other stores
/// and their data untouched.
#[wasm_bindgen_test]
async fn regression_a_database_without_the_blobs_store_is_upgraded() {
    let (store, db) = fresh("legacy").await;
    eval_async(&format!(
        "const d = {}; \
         await new Promise((res) => {{ const t = d.transaction('legacy', 'readwrite'); \
           t.objectStore('legacy').put('kept', 'x'); t.oncomplete = res; }}); d.close();",
        raw_open(&db, 1, "db.createObjectStore('legacy');")
    ))
    .await
    .unwrap();

    store.write("new.bin", b"hi").await.expect("the store is created on demand");
    assert_eq!(store.read("new.bin").await.unwrap().as_deref(), Some(&b"hi"[..]));
    assert_eq!(db_version_and_stores(&db).await, "2:blobs,legacy");
    let kept = eval_async(&format!(
        "const d = {}; const v = await new Promise((res) => {{ \
           const q = d.transaction('legacy').objectStore('legacy').get('x'); q.onsuccess = () => res(q.result); }}); \
         d.close(); return v;",
        raw_open(&db, 2, "")
    ))
    .await
    .unwrap();
    assert_eq!(kept.as_string().as_deref(), Some("kept"), "the other store's data survived");
}

/// Regression: opening at a fixed version 1 fails with a `VersionError` on
/// any database past version 1 (e.g. one the upgrade above produced, opened
/// by an older build). The store now opens the current version.
#[wasm_bindgen_test]
async fn regression_a_database_past_version_1_opens() {
    let (store, db) = fresh("v3").await;
    eval_async(&format!(
        "const d = {}; \
         await new Promise((res) => {{ const t = d.transaction('blobs', 'readwrite'); \
           t.objectStore('blobs').put(new Uint8Array([7, 8, 9]), 'pre/existing.bin'); t.oncomplete = res; }}); \
         d.close();",
        raw_open(&db, 3, "db.createObjectStore('blobs');")
    ))
    .await
    .unwrap();
    assert_eq!(store.read("pre/existing.bin").await.unwrap(), Some(vec![7, 8, 9]));
    assert_eq!(store.list("pre").await.unwrap(), vec!["existing.bin".to_string()]);
    assert_eq!(db_version_and_stores(&db).await, "3:blobs", "no needless upgrade");
}

/// A connection this module opened closes itself on `versionchange`, so
/// another connection's upgrade (another tab, a newer build) proceeds
/// instead of being blocked.
#[wasm_bindgen_test]
async fn an_open_connection_yields_to_another_upgrade() {
    let (store, db) = fresh("yield").await;
    store.write("x", b"1").await.unwrap();

    idb_module();
    let (np, nl) = string::abi(&db);
    // SAFETY: a borrowed string; a fresh promise back.
    let p = unsafe { JsValue::from_raw(js_idb_open(np, nl)) };
    let held = JsFuture::new(&p).await.expect("module open");
    JsValue::global().set("__heldDb", &held).unwrap();

    let outcome = eval_async(&format!(
        "return await new Promise((res) => {{ const q = indexedDB.open('{db}', 2); \
           q.onblocked = () => res('blocked'); \
           q.onsuccess = () => {{ q.result.close(); res('upgraded'); }}; q.onerror = () => res('error'); }});"
    ))
    .await
    .unwrap();
    assert_eq!(outcome.as_string().as_deref(), Some("upgraded"));
    // The held connection is closed: starting a transaction on it throws.
    let closed = eval_async(
        "try { globalThis.__heldDb.transaction('blobs'); return false; } \
         catch (e) { return e.name === 'InvalidStateError'; }",
    )
    .await
    .unwrap();
    assert_eq!(closed.as_bool(), Some(true));
    assert_eq!(store.read("x").await.unwrap().as_deref(), Some(&b"1"[..]), "still readable at v2");
}

/// A foreign connection that ignores `versionchange` blocks the upgrade the
/// store needs: the op fails with a `Backend` error naming it instead of
/// hanging, and once that connection closes the store works.
#[wasm_bindgen_test]
async fn a_blocked_upgrade_fails_instead_of_hanging_then_recovers() {
    let (store, db) = fresh("blocked").await;
    // A store-less v1 database, held open with no versionchange handler.
    eval_async(&format!("globalThis.__foreign = {};", raw_open(&db, 1, "db.createObjectStore('other');")))
        .await
        .unwrap();

    let r = within(3_000, store.write("x", b"1")).await.expect("did not hang");
    match r {
        Err(FileError::Backend(m)) => {
            assert!(m.starts_with("indexeddb: "), "{m}");
            assert!(m.contains("BlockedError"), "{m}");
        }
        other => panic!("expected a blocked Backend error, got {other:?}"),
    }

    eval("globalThis.__foreign.close(); delete globalThis.__foreign;");
    store.write("x", b"1").await.expect("works once the blocker closed");
    assert_eq!(store.read("x").await.unwrap().as_deref(), Some(&b"1"[..]));
    assert_eq!(db_version_and_stores(&db).await, "2:blobs,other");
}

/// Regression: the idb arm read every value through `new Uint8Array(v)`, so
/// a value that is not bytes (another writer's string under the same key)
/// read back as `Ok(Some(empty))` — silently wrong data. It is now a
/// `Backend` error.
#[wasm_bindgen_test]
async fn regression_a_stored_value_that_is_not_bytes_is_an_error_not_empty_bytes() {
    let (store, db) = fresh("notbytes").await;
    store.write("ok", b"1").await.unwrap(); // creates the database
    eval_async(&format!(
        "const d = {}; await new Promise((res) => {{ const t = d.transaction('blobs', 'readwrite'); \
           t.objectStore('blobs').put('a string', 'bad'); t.oncomplete = res; }}); d.close();",
        raw_open(&db, 1, "")
    ))
    .await
    .unwrap();
    match store.read("bad").await {
        Err(FileError::Backend(m)) => assert!(m.contains("DataError"), "{m}"),
        other => panic!("expected a Backend error, got {other:?}"),
    }
}

#[wasm_bindgen_test]
async fn loadable_url_serves_the_bytes_with_a_sniffed_type() {
    let (store, _) = fresh("url").await;
    assert_eq!(store.loadable_url("missing.webm").await.unwrap(), None);

    let webm = [0x1A, 0x45, 0xDF, 0xA3, 1, 2, 3];
    store.write("clip.webm", &webm).await.unwrap();
    let url = store.loadable_url("clip.webm").await.unwrap().expect("a URL");
    assert!(url.starts_with("blob:"), "{url}");
    let got = eval_async(&format!(
        "const r = await fetch('{url}'); const b = new Uint8Array(await r.arrayBuffer()); \
         return r.headers.get('content-type') + '|' + Array.from(b).join(',');"
    ))
    .await
    .unwrap();
    assert_eq!(got.as_string().as_deref(), Some("video/webm|26,69,223,163,1,2,3"));
    assert_eq!(store.local_path("clip.webm"), None);
}
