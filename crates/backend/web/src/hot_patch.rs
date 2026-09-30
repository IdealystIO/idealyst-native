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

use web_glue::{JsCast, JsValue};

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
    let Some(window) = web_glue::dom::window() else { return };
    let Ok(f) = web_glue::js::Reflect::get(&window, &JsValue::from_str("__idealyst_dev_ack")) else {
        return;
    };
    let Some(f) = f.dyn_ref::<web_glue::js::Function>() else { return };
    let o = web_glue::js::Object::new();
    for (k, v) in fields {
        let _ = web_glue::js::Reflect::set(&o, &JsValue::from_str(k), v);
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
            web_glue::dom::console::error_2(&"[idealyst] hot patch: rebuild failed".into(), &e);
            ack(&[
                ("kind", JsValue::from_str("failed")),
                ("what", JsValue::from_str("hot_patch")),
                ("error", e),
            ]);
        }
    });

    let Some(window) = web_glue::dom::window() else { return };
    // The livereload script is inline JS in the page that never imports
    // the module, so the entry point lives on `window` — the same reason
    // `__idealyst_overlay_patch` is published this way. `window` owns the
    // function (and so the closure) for the life of the page.
    let apply = web_glue::Closure::new_with_args(|args| {
        let url = args.first().and_then(JsValue::as_string).unwrap_or_default();
        let table = args.get(1).and_then(JsValue::as_string).unwrap_or_default();
        apply_patch(&url, &table);
        JsValue::UNDEFINED
    });
    let _ = web_glue::js::Reflect::set(
        &window,
        &JsValue::from_str("__idealyst_hot_patch"),
        &apply.into_js_value(),
    );
}

/// The page's entry point: `window.__idealyst_hot_patch(url, tableJson)`.
///
/// `table_json` is a serialized `subsecond_types::JumpTable` — kept as
/// the wire type so the dev server and the page cannot disagree about the
/// shape. Its `map` and `ifunc_count` are what the loader uses; `url` is
/// where the patch is served, which only the page knows how to reach.
pub fn apply_patch(url: &str, table_json: &str) {
    let table: subsecond_types::JumpTable = match serde_json::from_str(table_json) {
        Ok(t) => t,
        Err(e) => {
            let msg = format!("[idealyst] hot patch: unreadable jump table: {e}");
            web_glue::dom::console::error_1(&msg.as_str().into());
            ack(&[
                ("kind", JsValue::from_str("failed")),
                ("what", JsValue::from_str("hot_patch")),
                ("error", JsValue::from_str(&msg)),
            ]);
            return;
        }
    };
    if table.map.is_empty() {
        web_glue::dom::console::warn_1(
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
    web_glue::spawn_local(async move {
        match load(&url, &table).await {
            Ok(map) => {
                web_glue::dom::console::info_1(
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
                web_glue::dom::console::error_2(&"[idealyst] hot patch: apply failed:".into(), &e);
                let msg = e.as_string().unwrap_or_else(|| web_glue::JsError::from(e.clone()).message());
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
// The WebAssembly JS API the loader drives, plus the hybrid glue module's
// hot-patch entry points (`wasm_carve::glue_js`, `globalThis.__idealystGlue`).
// Dev-only, like this whole module.
#[cfg(target_arch = "wasm32")]
web_glue::import! {
    #[catch]
    fn js_compile_import(p: usize, l: usize) -> u32 =
        "(p, l) => G.add(globalThis.__idealystGlue.compileImport(G.str(p, l)))";
    #[catch]
    fn js_register_records(section: u32) = "(s) => { globalThis.__idealystGlue.registerRecords(G.get(s)); }";
    // → a promise of [module, byteLength].
    fn js_fetch_compile(p: usize, l: usize) -> u32 =
        "(p, l) => { const url = G.str(p, l); return G.add(fetch(url).then(async (r) => { \
           if (!r.ok) throw new Error(`fetching ${url}: HTTP ${r.status}`); \
           const b = await r.arrayBuffer(); return [await WebAssembly.compile(b), b.byteLength]; })); }";
    fn js_custom_sections(m: u32, p: usize, l: usize) -> u32 =
        "(m, p, l) => G.add(WebAssembly.Module.customSections(G.get(m), G.str(p, l)))";
    fn js_module_imports(m: u32) -> u32 = "(m) => G.add(WebAssembly.Module.imports(G.get(m)))";
    fn js_memory_grow(pages: u32) -> u32 = "(n) => G.exports().memory.grow(n) >>> 0";
    #[catch]
    fn js_table_grow(n: u32) -> u32 = "(n) => G.exports().__indirect_function_table.grow(n) >>> 0";
    fn js_table_get(slot: u32) -> u32 = "(s) => G.add(G.exports().__indirect_function_table.get(s))";
    #[catch]
    fn js_i32_global(v: i32, mutable: u32) -> u32 =
        "(v, m) => G.add(new WebAssembly.Global({ value: 'i32', mutable: m !== 0 }, v))";
    fn js_exports() -> u32 = "() => G.add(G.exports())";
    fn js_instantiate(m: u32, imports: u32) -> u32 =
        "(m, i) => G.add(WebAssembly.instantiate(G.get(m), G.get(i)))";
    fn js_thrower(p: usize, l: usize) -> u32 =
        "(p, l) => { const why = G.str(p, l); return G.add(function () { throw new Error(why); }); }";
}

#[cfg(target_arch = "wasm32")]
fn glue_compile_import(name: &str) -> Result<JsValue, web_glue::JsError> {
    let (p, l) = web_glue::string::abi(name);
    unsafe { js_compile_import(p, l) }.map(|h| unsafe { JsValue::from_raw(h) })
}

#[cfg(target_arch = "wasm32")]
fn glue_register_records(section: &JsValue) -> Result<(), web_glue::JsError> {
    unsafe { js_register_records(section.raw()) }
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
    use web_glue::js::{Array, Object, Reflect, Uint8Array};
    use web_glue::{string, JsFuture};

    let (up, ul) = string::abi(url);
    let compiled: Array =
        JsFuture::new(&unsafe { JsValue::from_raw(js_fetch_compile(up, ul)) }).await?.unchecked_into();
    let module = compiled.get(0);
    let byte_length = compiled.get(1).as_f64().unwrap_or(0.0) as u32;

    let (sp, sl) = string::abi(PLAN_SECTION);
    let sections: Array = unsafe { JsValue::from_raw(js_custom_sections(module.raw(), sp, sl)) }.unchecked_into();
    if sections.length() != 1 {
        return Err(format!(
            "the patch carries {} `{PLAN_SECTION}` section(s), expected one — a dev loop older \
             than this page built it",
            sections.length()
        )
        .into());
    }
    let plan = decode_plan(&Uint8Array::new(&sections.get(0)).to_vec())?;
    // The glue records the patch links (the web-glue runtime, `js_module!`
    // sources): registered before any of its glue imports can run. The
    // hybrid glue module refuses one that CHANGES what this page has —
    // that edit needs a reload, and failing the apply is what gets one.
    let (gp, gl) = string::abi("__idealyst_glue");
    let glue_records: Array =
        unsafe { JsValue::from_raw(js_custom_sections(module.raw(), gp, gl)) }.unchecked_into();
    for section in glue_records.iter() {
        glue_register_records(&section)?;
    }
    let descriptors: Array = unsafe { JsValue::from_raw(js_module_imports(module.raw())) }.unchecked_into();
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
    let data = plan.memory_size.unwrap_or(byte_length);
    let prior_pages = unsafe { js_memory_grow(data.div_ceil(PAGE) + 1) };
    // Page-aligned, so any alignment up to a page holds.
    let memory_base = (prior_pages + 1) * PAGE;
    let table_base = unsafe { js_table_grow(table.ifunc_count as u32) }?;

    // `env`: the base's exports, plus the two bases — what subsecond gave
    // every patch — and then whatever the plan adds.
    let env = Object::new();
    let exports: Object = unsafe { JsValue::from_raw(js_exports()) }.unchecked_into();
    for key in Object::keys(&exports).iter() {
        Reflect::set(&env, &key, &Reflect::get(&exports, &key)?)?;
    }
    let i32_global = |value: u32, mutable: bool| -> Result<JsValue, JsValue> {
        Ok(unsafe { js_i32_global(value as i32, mutable as u32) }.map(|h| unsafe { JsValue::from_raw(h) })?)
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
                        "{ns}.{}: the plan says the page supplies it, and the page has no such \
                         export",
                        name.as_string().unwrap_or_default()
                    )
                    .into());
                }
                continue;
            }
            ImportSource::Slot(slot) => {
                let f = unsafe { JsValue::from_raw(js_table_get(*slot)) };
                if f.is_null() {
                    return Err(format!(
                        "{}: the base's table slot {slot} is empty",
                        name.as_string().unwrap_or_default()
                    )
                    .into());
                }
                f
            }
            ImportSource::Global { value, mutable } => i32_global(*value, *mutable)?,
            ImportSource::Glue => {
                let name = name.as_string().unwrap_or_default();
                glue_compile_import(&name).map_err(|e| {
                    JsValue::from_str(&format!("compiling glue import {name:?}: {}", e.message()))
                })?
            }
            ImportSource::Trap(why) => {
                let why = format!("[idealyst] hot patch: called {why}");
                let (p, l) = string::abi(&why);
                unsafe { JsValue::from_raw(js_thrower(p, l)) }
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

    // `WebAssembly.instantiate(module, imports)` with a Module resolves
    // to the Instance itself.
    let instance =
        JsFuture::new(&unsafe { JsValue::from_raw(js_instantiate(module.raw(), imports.as_js().raw())) }).await?;

    // The patch's relocation thunks and constructors, in wasm-ld's order:
    // data relocs (pointers in the patch's data, relative to the two
    // bases), then global relocs — exported only when the plan dropped a
    // start function that would otherwise have run them — then ctors.
    let instance_exports = Reflect::get(&instance, &"exports".into())?;
    for thunk in ["__wasm_apply_data_relocs", "__wasm_apply_global_relocs", "__wasm_call_ctors"] {
        if let Ok(f) = Reflect::get(&instance_exports, &thunk.into())?.dyn_into::<web_glue::js::Function>() {
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
    web_glue::dom::console::info_1(
        &format!("[idealyst] hot patch: rebuilt, carrying {count} signal value(s)").into(),
    );
    Ok(count)
}
