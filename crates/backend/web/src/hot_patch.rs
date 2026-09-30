//! Apply a wasm hot patch to the running page, and put the tree back on
//! screen against it.
//!
//! # The two halves
//!
//! [`load`] does the loading: it fetches the patch module, grows linear
//! memory and `__indirect_function_table`, instantiates the patch into
//! them — supplying every import from the plan the dev loop appended to
//! the module (`build_web::hotpatch_prepare`) — and commits the jump
//! table through `dev_hot`, so a `fn` pointer that used to name the
//! base's slot now names the patch's.
//!
//! It used to be `subsecond::apply_patch`, which supplies a patch only
//! the base's exports. Everything else the patch imports then had to be
//! rewritten away on the dev machine — a walrus pass over the whole
//! patch, 0.7–1.0 s of every save on CrewForge. Supplying the imports
//! here removes the rewrite: the dev loop only reads them.
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
    /// How many functions the last applied table redirected, for the ack
    /// the rebuild sends once the tree is back.
    static REDIRECTED: std::cell::Cell<Option<usize>> = const { std::cell::Cell::new(None) };
}

/// Report an outcome to the dev session through the page's reload script
/// (`window.__idealyst_dev_ack`, published by `dev_http`'s script; see
/// `dev_http::ACK_URL`). Absent — a page served some other way — the
/// outcome stays in the console, as it always did.
fn ack(fields: &[(&str, JsValue)]) {
    let Some(window) = web_sys::window() else { return };
    let Ok(f) = js_sys::Reflect::get(&window, &JsValue::from_str("__idealyst_dev_ack")) else {
        return;
    };
    let Some(f) = f.dyn_ref::<js_sys::Function>() else { return };
    let o = js_sys::Object::new();
    for (k, v) in fields {
        let _ = js_sys::Reflect::set(&o, &JsValue::from_str(k), v);
    }
    let _ = f.call1(&JsValue::NULL, &o);
}

/// Remember the app root and publish the page's entry point.
///
/// Called at the end of the mount, because the entry point patches a
/// mounted app.
pub(crate) fn install(root: Rc<dyn Fn() -> Element>) {
    ROOT.with(|slot| *slot.borrow_mut() = Some(root));

    // Re-render after every successful patch, including ones applied by
    // something other than our own entry point.
    dev_hot::register_handler(|| match remount() {
        Ok(carried) => {
            let mut fields = vec![
                ("kind", JsValue::from_str("hot_patch")),
                ("carried", JsValue::from_f64(carried as f64)),
            ];
            if let Some(n) = REDIRECTED.with(|r| r.take()) {
                fields.push(("redirected", JsValue::from_f64(n as f64)));
            }
            ack(&fields);
        }
        Err(e) => {
            web_sys::console::error_2(&"[idealyst] hot patch: rebuild failed".into(), &e);
            ack(&[
                ("kind", JsValue::from_str("failed")),
                ("what", JsValue::from_str("hot_patch")),
                ("error", e),
            ]);
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
/// `table_json` is a serialized `subsecond_types::JumpTable` — kept as
/// the wire type so the dev server and the page cannot disagree about the
/// shape. Its `map` and `ifunc_count` are what the loader uses; `url` is
/// where the patch is served, which only the page knows how to reach.
#[wasm_bindgen(js_name = __idealyst_hot_patch)]
pub fn apply_patch(url: &str, table_json: &str) {
    let table: subsecond_types::JumpTable = match serde_json::from_str(table_json) {
        Ok(t) => t,
        Err(e) => {
            let msg = format!("[idealyst] hot patch: unreadable jump table: {e}");
            web_sys::console::error_1(&msg.as_str().into());
            ack(&[
                ("kind", JsValue::from_str("failed")),
                ("what", JsValue::from_str("hot_patch")),
                ("error", JsValue::from_str(&msg)),
            ]);
            return;
        }
    };
    if table.map.is_empty() {
        web_sys::console::warn_1(
            &"[idealyst] hot patch: the jump table redirects nothing — ignoring".into(),
        );
        return;
    }
    let entries = table.map.len();
    REDIRECTED.with(|r| r.set(Some(entries)));
    let url = url.to_string();
    // The rebuild rides `dev_hot::register_handler`, which `dev_hot::commit`
    // fires once the patch is really in place. Loading is asynchronous —
    // it awaits the fetch, the compile and the instantiate — so
    // rebuilding here would rebuild against the OLD code and show nothing
    // changed.
    // The loader is wasm-only: it instantiates into the page's own memory
    // and table. A host build of this crate (workspace checks, clippy)
    // compiles the entry point and says so if it is ever called.
    #[cfg(not(target_arch = "wasm32"))]
    {
        let _ = (url, table);
        ack(&[
            ("kind", JsValue::from_str("failed")),
            ("what", JsValue::from_str("hot_patch")),
            ("error", JsValue::from_str("[idealyst] hot patch: loading a wasm patch needs wasm32")),
        ]);
    }
    #[cfg(target_arch = "wasm32")]
    wasm_bindgen_futures::spawn_local(async move {
        match load(&url, &table).await {
            Ok(map) => {
                web_sys::console::info_1(
                    &format!("[idealyst] hot patch: {entries} function(s) redirected, rebuilding")
                        .into(),
                );
                // SAFETY: the table pairs functions matched by identical
                // mangled name between the base and the patch, so their
                // signatures agree by construction (see
                // `build_web::hotpatch_wasm`), and `load` rebased every
                // value onto the slots the patch was instantiated into.
                unsafe { dev_hot::commit(map) };
            }
            Err(e) => {
                web_sys::console::error_2(&"[idealyst] hot patch: apply failed:".into(), &e);
                let msg = e.as_string().unwrap_or_else(|| format!("{e:?}"));
                ack(&[
                    ("kind", JsValue::from_str("failed")),
                    ("what", JsValue::from_str("hot_patch")),
                    ("error", JsValue::from_str(&format!("[idealyst] hot patch: apply failed: {msg}"))),
                ]);
            }
        }
    });
}

// The hybrid glue module's hot-patch entry points (`wasm_carve::glue_js`,
// `globalThis.__idealystGlue`). Dev-only, like this whole module.
#[cfg(target_arch = "wasm32")]
#[wasm_bindgen::prelude::wasm_bindgen]
extern "C" {
    #[wasm_bindgen(catch, js_namespace = __idealystGlue, js_name = compileImport)]
    fn glue_compile_import(name: &str) -> Result<JsValue, JsValue>;
    #[wasm_bindgen(catch, js_namespace = __idealystGlue, js_name = registerRecords)]
    fn glue_register_records(section: &JsValue) -> Result<(), JsValue>;
}

#[cfg(target_arch = "wasm32")]
/// The custom section the dev loop writes the import plan into — see
/// `build_web::hotpatch_prepare`, which owns the format.
const PLAN_SECTION: &str = "idealyst.hotpatch";
#[cfg(target_arch = "wasm32")]
const PLAN_VERSION: u32 = 2;

#[cfg(target_arch = "wasm32")]
/// Where one import comes from (`build_web::hotpatch_prepare::ImportSource`).
enum ImportSource {
    /// The base's export of that name, or `__memory_base`/`__table_base`.
    Runtime,
    /// The base's function in this table slot, passed as itself.
    Slot(u32),
    /// An `i32` global with this value.
    Global { value: u32, mutable: bool },
    /// A function that throws this when called.
    Trap(String),
    /// A web-glue import: compiled from the JS in its own name.
    Glue,
}

#[cfg(target_arch = "wasm32")]
struct Plan {
    memory_size: Option<u32>,
    memory_align: u32,
    imports: Vec<ImportSource>,
}

#[cfg(target_arch = "wasm32")]
/// Read the plan. The layout is `build_web::hotpatch_prepare::Plan::encode`'s;
/// a version this page does not know is refused rather than misread.
fn decode_plan(bytes: &[u8]) -> Result<Plan, String> {
    let mut at = 0usize;
    let u32le = |at: &mut usize| -> Result<u32, String> {
        let b = bytes.get(*at..*at + 4).ok_or("the import plan is truncated")?;
        *at += 4;
        Ok(u32::from_le_bytes([b[0], b[1], b[2], b[3]]))
    };
    let version = u32le(&mut at)?;
    if version != PLAN_VERSION {
        return Err(format!(
            "import plan version {version}, this page reads {PLAN_VERSION} — reload the page"
        ));
    }
    let has_size = u32le(&mut at)? != 0;
    let size = u32le(&mut at)?;
    let memory_align = u32le(&mut at)?;
    let count = u32le(&mut at)?;
    let mut imports = Vec::with_capacity(count as usize);
    for _ in 0..count {
        let tag = *bytes.get(at).ok_or("the import plan is truncated")?;
        at += 1;
        imports.push(match tag {
            0 => ImportSource::Runtime,
            1 => ImportSource::Slot(u32le(&mut at)?),
            2 | 3 => ImportSource::Global { value: u32le(&mut at)?, mutable: tag == 2 },
            5 => ImportSource::Glue,
            4 => {
                let len = u32le(&mut at)? as usize;
                let s = bytes.get(at..at + len).ok_or("the import plan is truncated")?;
                at += len;
                ImportSource::Trap(String::from_utf8_lossy(s).into_owned())
            }
            other => return Err(format!("unknown import plan tag {other}")),
        });
    }
    Ok(Plan { memory_size: has_size.then_some(size), memory_align, imports })
}

#[cfg(target_arch = "wasm32")]
/// Fetch, instantiate and relocate the patch at `url`, returning its jump
/// table rebased onto the table slots it now occupies — ready for
/// `dev_hot::commit`.
///
/// Nothing the page runs is touched until the instantiate: growing
/// memory and the table only adds zeroed pages and null slots. A failure
/// before the commit leaves the page on the old code, and the caller
/// acks it so the dev loop rebuilds.
async fn load(
    url: &str,
    table: &subsecond_types::JumpTable,
) -> Result<std::collections::HashMap<u64, u64>, JsValue> {
    use js_sys::{Array, Object, Reflect, Uint8Array, WebAssembly};
    use wasm_bindgen_futures::JsFuture;

    let window = web_sys::window().ok_or("no window")?;
    let response: web_sys::Response = JsFuture::from(window.fetch_with_str(url)).await?.dyn_into()?;
    if !response.ok() {
        return Err(format!("fetching {url}: HTTP {}", response.status()).into());
    }
    let bytes: js_sys::ArrayBuffer = JsFuture::from(response.array_buffer()?).await?.dyn_into()?;
    let module: WebAssembly::Module = JsFuture::from(WebAssembly::compile(&bytes)).await?.dyn_into()?;

    let sections = WebAssembly::Module::custom_sections(&module, PLAN_SECTION);
    if sections.length() != 1 {
        return Err(format!(
            "the patch carries {} `{PLAN_SECTION}` section(s), expected one — a dev loop older              than this page built it",
            sections.length()
        )
        .into());
    }
    let plan = decode_plan(&Uint8Array::new(&sections.get(0)).to_vec())?;
    // The glue records the patch links (the web-glue runtime, `js_module!`
    // sources): registered before any of its glue imports can run. The
    // hybrid glue module refuses one that CHANGES what this page has —
    // that edit needs a reload, and failing the apply is what gets one.
    let glue_records = WebAssembly::Module::custom_sections(&module, "__idealyst_glue");
    for section in glue_records.iter() {
        glue_register_records(&section)?;
    }
    let descriptors: Array = WebAssembly::Module::imports(&module);
    if descriptors.length() as usize != plan.imports.len() {
        return Err(format!(
            "the patch has {} imports but its plan describes {}",
            descriptors.length(),
            plan.imports.len()
        )
        .into());
    }

    // Reserve the patch's memory and table slots. `grow` returns the
    // PRIOR size, which is atomic with respect to any other grow — two
    // loads in flight cannot land on one region.
    const PAGE: u32 = 64 * 1024;
    if plan.memory_align > 16 {
        return Err(format!("the patch wants 2^{} alignment, above a page", plan.memory_align).into());
    }
    // Without `dylink.0` mem-info, the module's own length is an upper
    // bound on its data — what subsecond always reserved.
    let data = plan.memory_size.unwrap_or(bytes.byte_length());
    let memory: WebAssembly::Memory = wasm_bindgen::memory().unchecked_into();
    let prior_pages = memory.grow(data.div_ceil(PAGE) + 1);
    // Page-aligned, so any alignment up to a page holds.
    let memory_base = (prior_pages + 1) * PAGE;
    let funcs: WebAssembly::Table = wasm_bindgen::function_table().unchecked_into();
    let table_base = funcs.grow(table.ifunc_count as u32)?;

    // `env`: the base's exports, plus the two bases — what subsecond gave
    // every patch — and then whatever the plan adds.
    let env = Object::new();
    let exports: Object = wasm_bindgen::exports().unchecked_into();
    for key in Object::keys(&exports).iter() {
        Reflect::set(&env, &key, &Reflect::get(&exports, &key)?)?;
    }
    let i32_global = |value: u32, mutable: bool| -> Result<JsValue, JsValue> {
        let descriptor = Object::new();
        Reflect::set(&descriptor, &"value".into(), &"i32".into())?;
        Reflect::set(&descriptor, &"mutable".into(), &mutable.into())?;
        Ok(WebAssembly::Global::new(&descriptor, &JsValue::from(value as i32))?.into())
    };
    Reflect::set(&env, &"__memory_base".into(), &i32_global(memory_base, false)?)?;
    Reflect::set(&env, &"__table_base".into(), &i32_global(table_base, false)?)?;

    let imports = Object::new();
    Reflect::set(&imports, &"env".into(), &env)?;
    for (descriptor, source) in descriptors.iter().zip(&plan.imports) {
        let namespace = Reflect::get(&descriptor, &"module".into())?;
        let name = Reflect::get(&descriptor, &"name".into())?;
        let value: JsValue = match source {
            ImportSource::Runtime => {
                let ns = namespace.as_string().unwrap_or_default();
                if ns != "env" || !Reflect::has(&env, &name)? {
                    return Err(format!(
                        "{ns}.{}: the plan says the page supplies it, and the page has no such                          export",
                        name.as_string().unwrap_or_default()
                    )
                    .into());
                }
                continue;
            }
            ImportSource::Slot(slot) => {
                let f = funcs.get(*slot)?;
                if f.is_null() {
                    return Err(format!(
                        "{}: the base's table slot {slot} is empty",
                        name.as_string().unwrap_or_default()
                    )
                    .into());
                }
                f.into()
            }
            ImportSource::Global { value, mutable } => i32_global(*value, *mutable)?,
            ImportSource::Glue => {
                let name = name.as_string().unwrap_or_default();
                glue_compile_import(&name).map_err(|e| {
                    JsValue::from_str(&format!("compiling glue import {name:?}: {e:?}"))
                })?
            }
            ImportSource::Trap(why) => {
                let why = why.clone();
                Closure::<dyn Fn() -> Result<(), JsValue>>::new(move || {
                    Err(JsValue::from_str(&format!("[idealyst] hot patch: called {why}")))
                })
                .into_js_value()
            }
        };
        let ns = match Reflect::get(&imports, &namespace)? {
            existing if existing.is_object() => existing,
            _ => {
                let fresh: JsValue = Object::new().into();
                Reflect::set(&imports, &namespace, &fresh)?;
                fresh
            }
        };
        Reflect::set(&ns, &name, &value)?;
    }

    let instance: WebAssembly::Instance =
        JsFuture::from(WebAssembly::instantiate_module(&module, &imports)).await?.dyn_into()?;

    // The patch's relocation thunks and constructors, in wasm-ld's order:
    // data relocs (pointers in the patch's data, relative to the two
    // bases), then global relocs — exported only when the plan dropped a
    // start function that would otherwise have run them — then ctors.
    let instance_exports = instance.exports();
    for thunk in ["__wasm_apply_data_relocs", "__wasm_apply_global_relocs", "__wasm_call_ctors"] {
        if let Ok(f) = Reflect::get(&instance_exports, &thunk.into())?.dyn_into::<js_sys::Function>() {
            f.call0(&JsValue::UNDEFINED)?;
        }
    }

    // The table's values are indices within the patch's element segment,
    // which now starts at `table_base`.
    Ok(table.map.iter().map(|(k, v)| (*k, *v + table_base as u64)).collect())
}

/// Rebuild the tree against the patched code, carrying signal values.
/// Returns how many values were carried.
fn remount() -> Result<usize, JsValue> {
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
    Ok(count)
}
