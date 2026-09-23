//! Apply a wasm hot patch to the running page, and put the tree back on
//! screen against it.
//!
//! # The two halves
//!
//! `subsecond::apply_patch` does the loading: it fetches the patch
//! module, grows linear memory and `__indirect_function_table`,
//! instantiates the patch into them, and commits the jump table so a
//! `fn` pointer that used to name the base's slot now names the patch's.
//!
//! That alone changes nothing on screen. The patched bodies are live but
//! nothing has called them: the DOM in front of the user was built by
//! the old ones. So the second half re-runs the app root — and the
//! interesting part is doing that without throwing away what the user
//! typed, scrolled to, or counted up to.
//!
//! # Carrying state across the rebuild
//!
//! `runtime_world::hot_state` is the mechanism, and the ORDER is not a
//! preference. Values are MOVED out of the dying world's arena, so:
//!
//! 1. `harvest()` — take every `signal()` value, keyed by component path
//!    and ordinal, while the arena is still alive;
//! 2. drop the old app, which unmounts the tree and drops the world;
//! 3. `seed()` — hand them back, so each `signal()` call in the rebuild
//!    finds its predecessor's value instead of its initial one.
//!
//! Harvest after the drop finds nothing. Seed before it seeds the world
//! about to die. This mirrors the native sidecar's `SessionMsg::Rerender`
//! exactly, which is the point: one hot-patch semantics, two backends.
//!
//! # Why a failure here reloads the page
//!
//! A patch that applied and a tree that did not rebuild leaves the page
//! showing output from code that no longer exists, with no sign of it.
//! Every failure path below logs what went wrong and lets the dev loop
//! fall back to the reload it would otherwise have done.

use std::cell::RefCell;
use std::rc::Rc;

use wasm_bindgen::prelude::*;
use wasm_bindgen::JsCast;

use runtime_scene::Element;

thread_local! {
    /// The app root, kept so the tree can be built a second time.
    ///
    /// This is why the boot path takes `impl Fn() -> Element` rather
    /// than `FnOnce`: a hot patch is exactly the case where the root has
    /// to run again. Dev-only storage — a release bundle never compiles
    /// this module.
    static ROOT: RefCell<Option<Rc<dyn Fn() -> Element>>> = const { RefCell::new(None) };
}

/// Remember the app root and publish the page's entry point.
///
/// Called at the end of the mount, because the entry point patches a
/// mounted app.
pub(crate) fn install(root: Rc<dyn Fn() -> Element>) {
    ROOT.with(|slot| *slot.borrow_mut() = Some(root));

    // Re-render after every successful patch, including ones applied by
    // something other than our own entry point.
    dev_hot::register_handler(|| {
        if let Err(e) = remount() {
            web_sys::console::error_2(&"[idealyst] hot patch: rebuild failed".into(), &e);
        }
    });

    let Some(window) = web_sys::window() else { return };
    // `#[wasm_bindgen]` puts an export on the MODULE, and the livereload
    // script is inline JS in the page that never imports the module. It
    // has to find this on `window` or it cannot call it at all — the
    // same reason `__idealyst_overlay_patch` is published this way.
    //
    // Leaked deliberately: it must stay callable for the life of the
    // page, and a dev session's page is the only holder.
    let apply = Closure::<dyn Fn(String, String)>::new(|url: String, table: String| {
        apply_patch(&url, &table);
    });
    let _ = js_sys::Reflect::set(
        &window,
        &JsValue::from_str("__idealyst_hot_patch"),
        apply.as_ref().unchecked_ref(),
    );
    apply.forget();
}

/// The page's entry point: `window.__idealyst_hot_patch(url, tableJson)`.
///
/// `table_json` is a serialized `subsecond_types::JumpTable` — subsecond's
/// own type, so the dev server and the runtime cannot disagree about the
/// shape. `url` overrides its `lib` field, because the builder knows
/// where the file was written and only the page knows where it is served
/// from.
#[wasm_bindgen(js_name = __idealyst_hot_patch)]
pub fn apply_patch(url: &str, table_json: &str) {
    let mut table: subsecond_types::JumpTable = match serde_json::from_str(table_json) {
        Ok(t) => t,
        Err(e) => {
            web_sys::console::error_1(
                &format!("[idealyst] hot patch: unreadable jump table: {e}").into(),
            );
            return;
        }
    };
    if table.map.is_empty() {
        web_sys::console::warn_1(
            &"[idealyst] hot patch: the jump table redirects nothing — ignoring".into(),
        );
        return;
    }
    table.lib = std::path::PathBuf::from(url);

    let entries = table.map.len();
    // SAFETY: the table's entries pair functions matched by identical
    // mangled name between the base and the patch, so their signatures
    // agree by construction. See `build_web::hotpatch_wasm`.
    if let Err(e) = unsafe { dev_hot::apply_patch(table) } {
        web_sys::console::error_1(&format!("[idealyst] hot patch: apply failed: {e}").into());
        return;
    }
    web_sys::console::info_1(
        &format!("[idealyst] hot patch: {entries} function(s) redirected, rebuilding").into(),
    );
    // The rebuild rides `register_handler`, which subsecond fires once
    // the patch is really in place. `apply_patch` on wasm finishes
    // asynchronously — it has to await the fetch and the instantiate —
    // so rebuilding here would rebuild against the OLD code and show
    // nothing changed.
}

/// Rebuild the tree against the patched code, carrying signal values.
fn remount() -> Result<(), JsValue> {
    let root = ROOT
        .with(|slot| slot.borrow().clone())
        .ok_or_else(|| JsValue::from_str("no app root stored — was the app mounted?"))?;

    // Reuse the live backend and registry: the patch changed function
    // bodies, not the DOM host or the set of registered payload kinds,
    // and rebuilding the backend would detach the mount point.
    let Some((backend, registry)) = crate::newcore::live_host() else {
        return Err(JsValue::from_str("no mounted app to rebuild"));
    };

    // Harvest FIRST: the values move out of the tree's slots, and the
    // teardown below frees those slots. Only the tree's own signals are
    // taken — the world is kept (see `take_tree`), and what lives in it
    // outside the tree has to stay readable.
    let carried = runtime_world::hot_state::harvest_owned();
    let count = carried.len();
    let world = crate::newcore::take_tree();
    runtime_world::hot_state::seed(carried);
    // Overlay edits staged before this patch describe the OLD compiled
    // source; left in place, `tag` re-applies them to the rebuilt tree
    // and they override the patch. The dev loop rescans its archive
    // after a patch, so later literal saves diff against the new source.
    #[cfg(feature = "ui-overlay")]
    runtime_vocabulary::overlay::unstage_all();

    crate::newcore::mount_tree(&backend, &registry, &*root, world);
    web_sys::console::info_1(
        &format!("[idealyst] hot patch: rebuilt, carrying {count} signal value(s)").into(),
    );
    Ok(())
}
