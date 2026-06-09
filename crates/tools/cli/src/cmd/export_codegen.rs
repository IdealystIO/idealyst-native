//! Pure codegen for `idealyst export` — turns the external-component
//! manifest (the JSON `dump_external_components_json` prints) into the
//! Rust wasm bridge, the custom-element JS shells, the `.d.ts`
//! declarations, and the per-framework (React/Vue) typed wrappers.
//!
//! Everything here is a pure `&[ExternalComponent] -> String` function so
//! it's unit-testable without invoking cargo or a browser. The
//! orchestration (running the manifest extractor, building wasm, writing
//! files) lives in `export.rs`.

use serde::Deserialize;

/// One `#[component(external)]` as parsed from the manifest JSON.
#[derive(Debug, Clone, Deserialize)]
pub struct ExternalComponent {
    pub name: String,
    pub module_path: String,
    pub tag: String,
    #[serde(default)]
    pub props: Vec<RawProp>,
}

/// A prop as it appears in the manifest — name + raw `quote!`-stringified
/// type + docs. The classifier turns this into a [`Prop`].
#[derive(Debug, Clone, Deserialize)]
pub struct RawProp {
    pub name: String,
    pub type_str: String,
    #[serde(default)]
    pub doc: String,
}

/// A classified prop ready for codegen.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Prop {
    pub name: String,
    pub doc: String,
    pub kind: PropKind,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PropKind {
    /// A value prop (string / number / boolean). `reactive` is true when
    /// the field is `Reactive<T>` (a JS write re-renders); false for a
    /// plain `T` (set once at mount).
    Value { rust: String, ts: String, reactive: bool },
    /// A callback prop. `optional` is true for `Option<Rc<dyn Fn..>>`.
    /// `arg` is the single argument (v1 supports 0 or 1 arg).
    Callback { optional: bool, arg: Option<CbArg> },
    /// Unsupported prop type — skipped, with a reason for the warning.
    Skip { reason: String },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CbArg {
    pub rust: String,
    pub ts: String,
}

impl Prop {
    /// The JS-facing property name (camelCase): `on_greet` → `onGreet`.
    pub fn js_name(&self) -> String {
        camel_case(&self.name)
    }
    /// For callbacks, the DOM event name: `on_greet` → `greet`,
    /// `changed` → `changed`.
    pub fn event_name(&self) -> String {
        self.name.strip_prefix("on_").unwrap_or(&self.name).replace('_', "-")
    }
}

/// Classify each raw prop. Props that can't cross the JS boundary become
/// `Skip` (the caller warns + omits them).
pub fn classify_props(raw: &[RawProp]) -> Vec<Prop> {
    raw.iter()
        .map(|p| Prop { name: p.name.clone(), doc: p.doc.clone(), kind: classify_type(&p.type_str) })
        .collect()
}

/// Classify one `quote!`-stringified type string. Whitespace-insensitive.
pub fn classify_type(type_str: &str) -> PropKind {
    let t: String = type_str.chars().filter(|c| !c.is_whitespace()).collect();

    // Callback shapes first (they contain `Fn`).
    if let Some(inner) = strip_wrap(&t, "Option<", ">") {
        if let Some(cb) = parse_callback(inner) {
            // The whole prop is `Option<Rc<dyn Fn..>>` → optional callback.
            // `parse_callback` itself always reports `optional: false`
            // (it sees only the inner `Rc<..>`), so flip it here.
            return match cb {
                PropKind::Callback { arg, .. } => PropKind::Callback { optional: true, arg },
                other => other, // Skip (e.g. multi-arg)
            };
        }
        // Option<value> — treat as the inner value (optional at the JS
        // layer, which already allows undefined).
        if let PropKind::Value { rust, ts, reactive } = classify_type(inner) {
            return PropKind::Value { rust, ts, reactive };
        }
        return PropKind::Skip { reason: format!("unsupported optional type `{type_str}`") };
    }
    if let Some(cb) = parse_callback(&t) {
        return cb;
    }

    // Reactive<T> value prop.
    if let Some(inner) = strip_wrap(&t, "Reactive<", ">") {
        return match value_type(inner) {
            Some((rust, ts)) => PropKind::Value { rust, ts, reactive: true },
            None => PropKind::Skip { reason: format!("unsupported Reactive inner type `{inner}`") },
        };
    }

    // Bare value prop.
    match value_type(&t) {
        Some((rust, ts)) => PropKind::Value { rust, ts, reactive: false },
        None => PropKind::Skip { reason: format!("unsupported prop type `{type_str}`") },
    }
}

/// Parse an `Rc<dyn Fn(..)>` callback shape. Returns `None` if `s` isn't a
/// callback. Supports 0 or 1 argument; more than one → `Skip`.
fn parse_callback(s: &str) -> Option<PropKind> {
    let inner = strip_wrap(s, "Rc<dynFn(", ")>").or_else(|| strip_wrap(s, "Box<dynFn(", ")>"))?;
    let inner = inner.trim();
    if inner.is_empty() {
        return Some(PropKind::Callback { optional: false, arg: None });
    }
    if inner.contains(',') {
        return Some(PropKind::Skip {
            reason: format!("callbacks with multiple args are not supported yet (`{s}`)"),
        });
    }
    match value_type(inner) {
        Some((rust, ts)) => {
            Some(PropKind::Callback { optional: false, arg: Some(CbArg { rust, ts }) })
        }
        None => Some(PropKind::Skip {
            reason: format!("callback arg type `{inner}` can't cross to JS yet"),
        }),
    }
}

/// Map a primitive Rust type to `(rust, ts)`. `None` for non-primitives.
fn value_type(t: &str) -> Option<(String, String)> {
    let ts = match t {
        "String" | "&str" => "string",
        "bool" => "boolean",
        "i8" | "i16" | "i32" | "i64" | "isize" | "u8" | "u16" | "u32" | "u64" | "usize"
        | "f32" | "f64" => "number",
        _ => return None,
    };
    // Normalize `&str` to an owned `String` on the Rust setter side.
    let rust = if t == "&str" { "String" } else { t };
    Some((rust.to_string(), ts.to_string()))
}

/// If `s` starts with `pre` and ends with `post`, return the slice
/// between them. Whitespace must already be stripped.
fn strip_wrap<'a>(s: &'a str, pre: &str, post: &str) -> Option<&'a str> {
    s.strip_prefix(pre)?.strip_suffix(post)
}

/// `on_greet` → `onGreet`, `name` → `name`.
pub fn camel_case(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut upper = false;
    for c in s.chars() {
        if c == '_' {
            upper = true;
        } else if upper {
            out.extend(c.to_uppercase());
            upper = false;
        } else {
            out.push(c);
        }
    }
    out
}

/// `Greeter` → `GreeterElement` (the wasm bridge class name).
pub fn bridge_class(name: &str) -> String {
    format!("{name}Element")
}

// ===========================================================================
// Rust bridge codegen
// ===========================================================================

/// Generate the bridge crate's `lib.rs`: one `#[wasm_bindgen]` class per
/// component. Each component is reached through its `module_path` (the
/// full Rust path the `#[component]` registered), so it must be `pub`
/// down that path.
pub fn gen_bridge_lib(components: &[ExternalComponent]) -> String {
    let mut out = String::new();
    out.push_str(
        "//! GENERATED by `idealyst export` — do not edit.\n\
         //!\n\
         //! One `#[wasm_bindgen]` bridge class per `#[component(external)]`.\n\
         //! Props become signals (JS writes re-render); callbacks store a\n\
         //! `js_sys::Function`. See the framework's external-export docs.\n\
         #![allow(non_snake_case, clippy::new_without_default)]\n\n\
         use std::cell::{Cell, RefCell};\n\
         use std::rc::Rc;\n\n\
         use backend_web::WebBackend;\n\
         use runtime_core::{build_detached, current_identity, with_current_identity, \
         DetachedScope, Identity, Signal};\n\
         use wasm_bindgen::prelude::*;\n\n\
         thread_local! {\n\
         \x20   static BOOTED: Cell<bool> = const { Cell::new(false) };\n\
         \x20   static SHARED_BACKEND: RefCell<Option<Rc<RefCell<WebBackend>>>> =\n\
         \x20       const { RefCell::new(None) };\n\
         \x20   static NEXT_ELEMENT_ID: Cell<u64> = const { Cell::new(1) };\n\
         }\n\n\
         /// A fresh per-element identity seed. Every mounted element builds\n\
         /// its subtree under a UNIQUE identity so the runtime's id-keyed\n\
         /// registrations (reactive bindings, event closures) of two\n\
         /// elements never collide — a collision would release the earlier\n\
         /// element's closures (a `null function` trap on its handlers).\n\
         fn next_element_id() -> u64 {\n\
         \x20   NEXT_ELEMENT_ID.with(|c| {\n\
         \x20       let v = c.get();\n\
         \x20       c.set(v + 1);\n\
         \x20       v\n\
         \x20   })\n\
         }\n\n\
         /// One-time runtime boot — a component bundle has no app `main()`.\n\
         fn ensure_runtime() {\n\
         \x20   BOOTED.with(|b| {\n\
         \x20       if !b.get() {\n\
         \x20           backend_web::install_scheduler();\n\
         \x20           backend_web::install_time_source();\n\
         \x20           backend_web::install_logger();\n\
         \x20           b.set(true);\n\
         \x20       }\n\
         \x20   });\n\
         }\n\n\
         /// One backend shared by every exported element on the page. Each\n\
         /// element builds its OWN detached subtree into its host, so the\n\
         /// runtime's per-backend JS shims (text/class batchers, …) install\n\
         /// exactly once. Multiple independent `mount()` roots would each\n\
         /// re-install those global shims, freeing the previous root's\n\
         /// closures — a `null function` trap on the first tree's handlers.\n\
         /// The backend's own mount node is a throwaway detached <div>; we\n\
         /// append each subtree's root into the real host instead.\n\
         fn shared_backend() -> Rc<RefCell<WebBackend>> {\n\
         \x20   SHARED_BACKEND.with(|s| {\n\
         \x20       if let Some(b) = s.borrow().as_ref() {\n\
         \x20           return b.clone();\n\
         \x20       }\n\
         \x20       ensure_runtime();\n\
         \x20       let doc = web_sys::window().expect(\"window\").document().expect(\"document\");\n\
         \x20       let dummy = doc.create_element(\"div\").expect(\"create div\");\n\
         \x20       let b = Rc::new(RefCell::new(WebBackend::new_in(dummy)));\n\
         \x20       backend_web::install_global_self(&b);\n\
         \x20       *s.borrow_mut() = Some(b.clone());\n\
         \x20       b\n\
         \x20   })\n\
         }\n\n",
    );
    for c in components {
        out.push_str(&gen_bridge_class(c));
        out.push('\n');
    }
    out
}

fn gen_bridge_class(c: &ExternalComponent) -> String {
    let class = bridge_class(&c.name);
    let props = classify_props(&c.props);
    let comp_path = format!("{}::{}", c.module_path, c.name);
    let props_path = format!("{}::{}Props", c.module_path, c.name);

    let mut fields = String::new();
    let mut inits = String::new();
    let mut captures = String::new();
    let mut prop_inits = String::new();
    let mut setters = String::new();

    for p in &props {
        match &p.kind {
            PropKind::Value { rust, reactive, .. } => {
                fields.push_str(&format!("    {}: Signal<{}>,\n", p.name, rust));
                inits.push_str(&format!("            {}: Signal::new(Default::default()),\n", p.name));
                captures.push_str(&format!("        let {n} = self.{n};\n", n = p.name));
                if *reactive {
                    prop_inits.push_str(&format!("                {n}: {n}.into(),\n", n = p.name));
                } else {
                    prop_inits.push_str(&format!("                {n}: {n}.get(),\n", n = p.name));
                }
                setters.push_str(&format!(
                    "    #[wasm_bindgen(setter, js_name = {js})]\n    \
                     pub fn set_{n}(&self, value: {rust}) {{ self.{n}.set(value); }}\n",
                    js = p.js_name(),
                    n = p.name,
                    rust = rust,
                ));
            }
            PropKind::Callback { optional, arg } => {
                fields.push_str(&format!(
                    "    {}: Rc<RefCell<Option<js_sys::Function>>>,\n",
                    p.name
                ));
                inits.push_str(&format!("            {}: Rc::new(RefCell::new(None)),\n", p.name));
                captures.push_str(&format!("        let {n} = self.{n}.clone();\n", n = p.name));

                let closure = match arg {
                    None => format!(
                        "Rc::new(move || {{ if let Some(f) = {n}.borrow().as_ref() {{ \
                         let _ = f.call0(&JsValue::NULL); }} }})",
                        n = p.name
                    ),
                    Some(a) => format!(
                        "Rc::new(move |v: {rust}| {{ if let Some(f) = {n}.borrow().as_ref() {{ \
                         let _ = f.call1(&JsValue::NULL, &JsValue::from(v)); }} }})",
                        rust = a.rust,
                        n = p.name
                    ),
                };
                if *optional {
                    prop_inits.push_str(&format!("                {n}: Some({closure}),\n", n = p.name));
                } else {
                    prop_inits.push_str(&format!("                {n}: {closure},\n", n = p.name));
                }
                setters.push_str(&format!(
                    "    #[wasm_bindgen(setter, js_name = {js})]\n    \
                     pub fn set_{n}(&self, f: js_sys::Function) {{ \
                     *self.{n}.borrow_mut() = Some(f); }}\n",
                    js = p.js_name(),
                    n = p.name,
                ));
            }
            PropKind::Skip { .. } => {} // omitted; export.rs logs the reason
        }
    }

    format!(
        "#[wasm_bindgen]\n\
         pub struct {class} {{\n\
         {fields}    scope: Option<DetachedScope>,\n\
         }}\n\n\
         #[wasm_bindgen]\n\
         impl {class} {{\n\
         \x20   #[wasm_bindgen(constructor)]\n\
         \x20   pub fn new() -> {class} {{\n\
         \x20       ensure_runtime();\n\
         \x20       {class} {{\n\
         {inits}            scope: None,\n\
         \x20       }}\n\
         \x20   }}\n\n\
         \x20   pub fn mount(&mut self, host: web_sys::Element) {{\n\
         \x20       let backend = shared_backend();\n\
         {captures}        let props = {props_path} {{\n\
         {prop_inits}            ..Default::default()\n\
         \x20       }};\n\
         \x20       let element = {comp_path}(&props);\n\
         \x20       let seed = Identity::node(current_identity(), 0, None, Some(next_element_id()));\n\
         \x20       let (node, scope) =\n\
         \x20           with_current_identity(seed, || build_detached(&backend, element, None));\n\
         \x20       let _ = host.append_child(&node);\n\
         \x20       self.scope = Some(scope);\n\
         \x20   }}\n\n\
         \x20   pub fn unmount(&mut self) {{ self.scope = None; }}\n\n\
         {setters}}}\n",
    )
}

// ===========================================================================
// Custom-element JS codegen
// ===========================================================================

/// Generate the custom-element shell for one component (`idl-greeter.js`).
/// `wasm_module` is the wasm-bindgen JS filename (without extension).
pub fn gen_element_js(c: &ExternalComponent, wasm_module: &str) -> String {
    let class = bridge_class(&c.name);
    let props = classify_props(&c.props);
    let values: Vec<&Prop> = props.iter().filter(|p| matches!(p.kind, PropKind::Value { .. })).collect();
    let callbacks: Vec<&Prop> = props.iter().filter(|p| matches!(p.kind, PropKind::Callback { .. })).collect();

    let observed: String = values
        .iter()
        .map(|p| format!("\"{}\"", p.js_name()))
        .collect::<Vec<_>>()
        .join(", ");

    // Per value-prop: a property getter/setter + attribute coercion.
    let mut value_members = String::new();
    let mut attr_cases = String::new();
    for p in &values {
        let PropKind::Value { ts, .. } = &p.kind else { continue };
        let coerce = match ts.as_str() {
            "number" => "Number(v)",
            "boolean" => "v === \"\" || v === \"true\" || v === true",
            _ => "v",
        };
        let js = p.js_name();
        value_members.push_str(&format!(
            "  get {js}() {{ return this._bridge ? undefined : this._pending[\"{js}\"]; }}\n\
             \x20 set {js}(v) {{ this._set(\"{js}\", v); }}\n",
        ));
        attr_cases.push_str(&format!(
            "      case \"{js}\": this._set(\"{js}\", {coerce}); break;\n",
        ));
    }

    // Per callback-prop: assignable JS property + DOM CustomEvent.
    let mut cb_wiring = String::new();
    let mut cb_members = String::new();
    for p in &callbacks {
        let js = p.js_name();
        let ev = p.event_name();
        cb_wiring.push_str(&format!(
            "    this._bridge.{js} = (...args) => {{\n\
             \x20     this.dispatchEvent(new CustomEvent(\"{ev}\", {{ bubbles: true, detail: args[0] }}));\n\
             \x20     if (typeof this._{js} === \"function\") this._{js}(...args);\n\
             \x20   }};\n",
        ));
        cb_members.push_str(&format!("  set {js}(fn) {{ this._{js} = fn; }}\n"));
    }

    // Apply pending value props on connect.
    let apply_pending: String = values
        .iter()
        .map(|p| {
            let js = p.js_name();
            format!(
                "    if (this._pending[\"{js}\"] !== undefined) this._bridge.{js} = this._pending[\"{js}\"];\n\
                 \x20   else if (this.hasAttribute(\"{js}\")) this.attributeChangedCallback(\"{js}\", null, this.getAttribute(\"{js}\"));\n"
            )
        })
        .collect();

    format!(
        "// GENERATED by `idealyst export` — do not edit.\n\
         import init, {{ {class} }} from \"./pkg/{wasm_module}.js\";\n\n\
         // ONE wasm init shared across every element module on the page.\n\
         // Each element file imports the same wasm glue, but a module-local\n\
         // promise wouldn't dedupe across files: two concurrent init() calls\n\
         // both pass wasm-bindgen's post-init guard and re-instantiate the\n\
         // module, orphaning the first elements' closures (a `null function`\n\
         // trap). A globalThis-keyed promise guarantees a single init.\n\
         const ready = () => (globalThis.__idealystReady_{wasm_module} ??= init());\n\n\
         class {elem_class} extends HTMLElement {{\n\
         \x20 static get observedAttributes() {{ return [{observed}]; }}\n\n\
         \x20 constructor() {{\n\
         \x20   super();\n\
         \x20   this._bridge = null;\n\
         \x20   this._pending = {{}};\n\
         \x20 }}\n\n\
         \x20 async connectedCallback() {{\n\
         \x20   await ready();\n\
         \x20   if (this._bridge) return;\n\
         \x20   this._bridge = new {class}();\n\
         {cb_wiring}{apply_pending}    this._bridge.mount(this);\n\
         \x20 }}\n\n\
         \x20 disconnectedCallback() {{\n\
         \x20   if (this._bridge) {{ this._bridge.unmount(); this._bridge = null; }}\n\
         \x20 }}\n\n\
         \x20 attributeChangedCallback(name, _old, v) {{\n\
         \x20   switch (name) {{\n\
         {attr_cases}    }}\n\
         \x20 }}\n\n\
         \x20 _set(key, v) {{\n\
         \x20   if (this._bridge) this._bridge[key] = v;\n\
         \x20   else this._pending[key] = v;\n\
         \x20 }}\n\n\
         {value_members}{cb_members}}}\n\n\
         customElements.define(\"{tag}\", {elem_class});\n\
         export {{ {elem_class} }};\n",
        elem_class = format!("Idl{}", c.name),
        tag = c.tag,
    )
}

// ===========================================================================
// TypeScript declarations + React wrapper
// ===========================================================================

/// `(name?: T; onGreet?: (v: A) => void; …)` — the shared prop interface
/// body used by both the `.d.ts` and the React wrapper.
fn ts_prop_lines(props: &[Prop]) -> String {
    let mut out = String::new();
    for p in props {
        let doc = if p.doc.is_empty() {
            String::new()
        } else {
            format!("  /** {} */\n", p.doc.replace('\n', " "))
        };
        match &p.kind {
            PropKind::Value { ts, .. } => {
                out.push_str(&format!("{doc}  {}?: {ts};\n", p.js_name()));
            }
            PropKind::Callback { arg, .. } => {
                let sig = match arg {
                    None => "() => void".to_string(),
                    Some(a) => format!("(value: {}) => void", a.ts),
                };
                out.push_str(&format!("{doc}  {}?: {sig};\n", p.js_name()));
            }
            PropKind::Skip { .. } => {}
        }
    }
    out
}

/// Generate `<component>.d.ts` for one component.
pub fn gen_dts(c: &ExternalComponent) -> String {
    let props = classify_props(&c.props);
    let lines = ts_prop_lines(&props);
    format!(
        "// GENERATED by `idealyst export` — do not edit.\n\
         export interface {name}Props {{\n{lines}}}\n\n\
         export declare class Idl{name} extends HTMLElement {{}}\n\n\
         declare global {{\n\
         \x20 interface HTMLElementTagNameMap {{\n\
         \x20   \"{tag}\": Idl{name};\n\
         \x20 }}\n\
         }}\n",
        name = c.name,
        tag = c.tag,
    )
}

/// Generate a typed React wrapper (`<Component>.tsx`) for one component.
pub fn gen_react_wrapper(c: &ExternalComponent) -> String {
    let props = classify_props(&c.props);
    let lines = ts_prop_lines(&props);
    let values: Vec<&Prop> = props.iter().filter(|p| matches!(p.kind, PropKind::Value { .. })).collect();
    let callbacks: Vec<&Prop> = props.iter().filter(|p| matches!(p.kind, PropKind::Callback { .. })).collect();

    let value_effects: String = values
        .iter()
        .map(|p| {
            let js = p.js_name();
            format!(
                "  useEffect(() => {{ if (ref.current && {js} !== undefined) ref.current.{js} = {js}; }}, [{js}]);\n"
            )
        })
        .collect();
    let cb_effects: String = callbacks
        .iter()
        .map(|p| {
            let js = p.js_name();
            format!("  useEffect(() => {{ if (ref.current) ref.current.{js} = {js}; }}, [{js}]);\n")
        })
        .collect();
    let destructure: String = props
        .iter()
        .filter(|p| !matches!(p.kind, PropKind::Skip { .. }))
        .map(|p| p.js_name())
        .collect::<Vec<_>>()
        .join(", ");

    format!(
        "// GENERATED by `idealyst export` — do not edit.\n\
         import {{ useEffect, useRef, createElement }} from \"react\";\n\
         import \"../idl-{kebab}.js\";\n\n\
         export interface {name}Props {{\n{lines}}}\n\n\
         export function {name}({{ {destructure} }}: {name}Props) {{\n\
         \x20 const ref = useRef<any>(null);\n\
         {value_effects}{cb_effects}  return createElement(\"{tag}\", {{ ref }});\n\
         }}\n",
        name = c.name,
        tag = c.tag,
        kebab = kebab_of_tag(&c.tag),
    )
}

/// Generate a minimal Vue wrapper (`<Component>.vue.js`) for one component.
pub fn gen_vue_wrapper(c: &ExternalComponent) -> String {
    let props = classify_props(&c.props);
    let value_names: Vec<String> = props
        .iter()
        .filter(|p| matches!(p.kind, PropKind::Value { .. }))
        .map(|p| p.js_name())
        .collect();
    let cb_names: Vec<String> = props
        .iter()
        .filter(|p| matches!(p.kind, PropKind::Callback { .. }))
        .map(|p| p.js_name())
        .collect();
    let prop_list = value_names
        .iter()
        .chain(cb_names.iter())
        .map(|n| format!("\"{n}\""))
        .collect::<Vec<_>>()
        .join(", ");
    let bindings: String = value_names
        .iter()
        .chain(cb_names.iter())
        .map(|n| format!("        if (props.{n} !== undefined) el.{n} = props.{n};\n"))
        .collect();

    format!(
        "// GENERATED by `idealyst export` — do not edit.\n\
         import {{ defineComponent, h, ref, watchEffect, onMounted }} from \"vue\";\n\
         import \"../idl-{kebab}.js\";\n\n\
         export const {name} = defineComponent({{\n\
         \x20 name: \"{name}\",\n\
         \x20 props: [{prop_list}],\n\
         \x20 setup(props) {{\n\
         \x20   const elRef = ref(null);\n\
         \x20   const apply = () => {{\n\
         \x20     const el = elRef.value;\n\
         \x20     if (!el) return;\n\
         {bindings}      }};\n\
         \x20   onMounted(() => watchEffect(apply));\n\
         \x20   return () => h(\"{tag}\", {{ ref: elRef }});\n\
         \x20 }},\n\
         }});\n",
        name = c.name,
        tag = c.tag,
        kebab = kebab_of_tag(&c.tag),
    )
}

/// Generate a Svelte wrapper (`<Component>.svelte`). Svelte consumes
/// custom elements natively; this is the typed/ergonomic shell.
pub fn gen_svelte_wrapper(c: &ExternalComponent) -> String {
    let props = classify_props(&c.props);
    let exports: String = props
        .iter()
        .filter(|p| !matches!(p.kind, PropKind::Skip { .. }))
        .map(|p| format!("  export let {} = undefined;\n", p.js_name()))
        .collect();
    let reactive: String = props
        .iter()
        .filter_map(|p| match p.kind {
            PropKind::Value { .. } => {
                let js = p.js_name();
                Some(format!("  $: if (el && {js} !== undefined) el.{js} = {js};\n"))
            }
            PropKind::Callback { .. } => {
                let js = p.js_name();
                Some(format!("  $: if (el) el.{js} = {js};\n"))
            }
            PropKind::Skip { .. } => None,
        })
        .collect();
    format!(
        "<!-- GENERATED by `idealyst export` — do not edit. -->\n\
         <script>\n\
         \x20 import \"../idl-{kebab}.js\";\n\
         {exports}  let el;\n\
         {reactive}</script>\n\n\
         <{tag} bind:this={{el}}></{tag}>\n",
        kebab = kebab_of_tag(&c.tag),
        tag = c.tag,
    )
}

/// Generate a standalone Angular component wrapper
/// (`<component>.component.ts`). Value props → `@Input`, callbacks →
/// `@Output` `EventEmitter`s, so consumers write
/// `[name]="x" (greet)="…"`.
pub fn gen_angular_wrapper(c: &ExternalComponent) -> String {
    let props = classify_props(&c.props);
    let inputs: String = props
        .iter()
        .filter_map(|p| match &p.kind {
            PropKind::Value { ts, .. } => Some(format!("  @Input() {}?: {ts};\n", p.js_name())),
            _ => None,
        })
        .collect();
    let outputs: String = props
        .iter()
        .filter_map(|p| match &p.kind {
            PropKind::Callback { arg, .. } => {
                let ty = arg.as_ref().map(|a| a.ts.as_str()).unwrap_or("void");
                Some(format!(
                    "  @Output() {ev} = new EventEmitter<{ty}>();\n",
                    ev = p.event_name().replace('-', "_"),
                ))
            }
            _ => None,
        })
        .collect();
    let listeners: String = props
        .iter()
        .filter_map(|p| match p.kind {
            PropKind::Callback { .. } => Some(format!(
                "    el.addEventListener(\"{ev}\", (e: any) => this.{out}.emit(e.detail));\n",
                ev = p.event_name(),
                out = p.event_name().replace('-', "_"),
            )),
            _ => None,
        })
        .collect();
    let applies: String = props
        .iter()
        .filter_map(|p| match p.kind {
            PropKind::Value { .. } => {
                let js = p.js_name();
                Some(format!("    if (this.{js} !== undefined) el.{js} = this.{js};\n"))
            }
            _ => None,
        })
        .collect();
    format!(
        "// GENERATED by `idealyst export` — do not edit.\n\
         import {{ Component, ElementRef, Input, Output, EventEmitter, ViewChild, \
         AfterViewInit, OnChanges, CUSTOM_ELEMENTS_SCHEMA }} from \"@angular/core\";\n\
         import \"../idl-{kebab}.js\";\n\n\
         @Component({{\n\
         \x20 selector: \"{tag}-ng\",\n\
         \x20 standalone: true,\n\
         \x20 template: \"<{tag} #el></{tag}>\",\n\
         \x20 schemas: [CUSTOM_ELEMENTS_SCHEMA],\n\
         }})\n\
         export class {name}Component implements AfterViewInit, OnChanges {{\n\
         \x20 @ViewChild(\"el\", {{ static: true }}) elRef!: ElementRef<any>;\n\
         {inputs}{outputs}\n\
         \x20 ngAfterViewInit(): void {{\n\
         \x20   const el = this.elRef.nativeElement;\n\
         {listeners}    this.apply();\n\
         \x20 }}\n\
         \x20 ngOnChanges(): void {{ this.apply(); }}\n\
         \x20 private apply(): void {{\n\
         \x20   const el = this.elRef?.nativeElement;\n\
         \x20   if (!el) return;\n\
         {applies}  }}\n\
         }}\n",
        kebab = kebab_of_tag(&c.tag),
        tag = c.tag,
        name = c.name,
    )
}

/// Generate a raw-JS helper (`<Component>.js`): imperative `create*` +
/// `bind*` functions for use with no framework at all.
pub fn gen_vanilla_helper(c: &ExternalComponent) -> String {
    let props = classify_props(&c.props);
    let bindings: String = props
        .iter()
        .filter(|p| !matches!(p.kind, PropKind::Skip { .. }))
        .map(|p| {
            let js = p.js_name();
            format!("  if (props.{js} !== undefined) el.{js} = props.{js};\n")
        })
        .collect();
    format!(
        "// GENERATED by `idealyst export` — do not edit.\n\
         import \"../idl-{kebab}.js\";\n\n\
         /** Create a configured <{tag}> element. */\n\
         export function create{name}(props = {{}}) {{\n\
         \x20 return bind{name}(document.createElement(\"{tag}\"), props);\n\
         }}\n\n\
         /** Apply props to an existing <{tag}> element. */\n\
         export function bind{name}(el, props = {{}}) {{\n\
         {bindings}  return el;\n\
         }}\n",
        kebab = kebab_of_tag(&c.tag),
        tag = c.tag,
        name = c.name,
    )
}

/// Generate the barrel `index.d.ts` re-exporting every component's
/// element declarations (so a consumer gets typed `HTMLElementTagNameMap`
/// entries + the `*Props` interfaces from one import).
pub fn gen_index_dts(components: &[ExternalComponent]) -> String {
    let mut s = String::from("// GENERATED by `idealyst export` — do not edit.\n");
    for c in components {
        let stem = kebab_of_tag(&c.tag);
        s.push_str(&format!("export * from \"./idl-{stem}\";\n"));
    }
    s
}

/// Generate a `package.json` so `dist/external` is an installable package
/// (publish it, or `file:`-depend on it from a consumer). No `exports`
/// map — deep subpath imports (`pkg/react/Greeter.tsx`) stay open, which
/// is what the framework wrappers rely on.
pub fn gen_package_json(pkg_name: &str, frameworks: &[Framework]) -> String {
    let mut files: Vec<String> = vec![
        "\"pkg\"".into(),
        "\"index.js\"".into(),
        "\"index.d.ts\"".into(),
        "\"idl-*.js\"".into(),
        "\"idl-*.d.ts\"".into(),
    ];
    for fw in frameworks {
        files.push(format!("\"{}\"", fw.slug()));
    }
    format!(
        "{{\n\
         \x20 \"name\": \"{pkg_name}-components\",\n\
         \x20 \"version\": \"0.0.1\",\n\
         \x20 \"type\": \"module\",\n\
         \x20 \"main\": \"index.js\",\n\
         \x20 \"module\": \"index.js\",\n\
         \x20 \"types\": \"index.d.ts\",\n\
         \x20 \"sideEffects\": [\"./index.js\", \"./idl-*.js\"],\n\
         \x20 \"files\": [{files}]\n\
         }}\n",
        files = files.join(", "),
    )
}

/// A foreign front-end framework an exported component can be consumed
/// from. The custom element itself works in *any* framework that
/// renders DOM; these are the ergonomic, typed adapters on top.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Framework {
    /// No framework — imperative `create*`/`bind*` helpers.
    Vanilla,
    React,
    Vue,
    Svelte,
    Angular,
}

impl Framework {
    /// Every framework an export can emit, in a stable order.
    pub const ALL: [Framework; 5] =
        [Self::Vanilla, Self::React, Self::Vue, Self::Svelte, Self::Angular];

    /// Parse a `--frameworks` token (case-insensitive; `js` aliases
    /// `vanilla`). `None` for an unknown name.
    pub fn parse(s: &str) -> Option<Framework> {
        match s.trim().to_ascii_lowercase().as_str() {
            "vanilla" | "js" | "rawjs" => Some(Self::Vanilla),
            "react" => Some(Self::React),
            "vue" => Some(Self::Vue),
            "svelte" => Some(Self::Svelte),
            "angular" => Some(Self::Angular),
            _ => None,
        }
    }

    /// Output subdirectory / slug (`react`, `vue`, …).
    pub fn slug(self) -> &'static str {
        match self {
            Self::Vanilla => "vanilla",
            Self::React => "react",
            Self::Vue => "vue",
            Self::Svelte => "svelte",
            Self::Angular => "angular",
        }
    }

    /// Per-component filename in this framework's subdir.
    pub fn filename(self, c: &ExternalComponent) -> String {
        match self {
            Self::Vanilla => format!("{}.js", c.name),
            Self::React => format!("{}.tsx", c.name),
            Self::Vue => format!("{}.js", c.name),
            Self::Svelte => format!("{}.svelte", c.name),
            Self::Angular => format!("{}.component.ts", kebab_of_tag(&c.tag)),
        }
    }

    /// Generate this framework's wrapper source for `c`.
    pub fn generate(self, c: &ExternalComponent) -> String {
        match self {
            Self::Vanilla => gen_vanilla_helper(c),
            Self::React => gen_react_wrapper(c),
            Self::Vue => gen_vue_wrapper(c),
            Self::Svelte => gen_svelte_wrapper(c),
            Self::Angular => gen_angular_wrapper(c),
        }
    }
}

/// The element-file stem for a tag (`idl-greeter` → `greeter`). We name
/// element files `idl-<x>.js`, so strip the leading `idl-` if present.
fn kebab_of_tag(tag: &str) -> String {
    tag.strip_prefix("idl-").unwrap_or(tag).to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn raw(name: &str, ty: &str) -> RawProp {
        RawProp { name: name.into(), type_str: ty.into(), doc: String::new() }
    }

    #[test]
    fn classifies_reactive_string() {
        assert_eq!(
            classify_type("Reactive < String >"),
            PropKind::Value { rust: "String".into(), ts: "string".into(), reactive: true }
        );
    }

    #[test]
    fn classifies_plain_number_and_bool() {
        assert_eq!(
            classify_type("i32"),
            PropKind::Value { rust: "i32".into(), ts: "number".into(), reactive: false }
        );
        assert_eq!(
            classify_type("Reactive < bool >"),
            PropKind::Value { rust: "bool".into(), ts: "boolean".into(), reactive: true }
        );
    }

    #[test]
    fn classifies_zero_arg_optional_callback() {
        assert_eq!(
            classify_type("Option < Rc < dyn Fn () > >"),
            PropKind::Callback { optional: true, arg: None }
        );
    }

    #[test]
    fn classifies_one_arg_callback() {
        assert_eq!(
            classify_type("Rc < dyn Fn (bool) >"),
            PropKind::Callback {
                optional: false,
                arg: Some(CbArg { rust: "bool".into(), ts: "boolean".into() })
            }
        );
    }

    #[test]
    fn skips_unsupported_and_multiarg() {
        assert!(matches!(classify_type("Ref < PressableHandle >"), PropKind::Skip { .. }));
        assert!(matches!(
            classify_type("Rc < dyn Fn (i32 , bool) >"),
            PropKind::Skip { .. }
        ));
    }

    #[test]
    fn camel_and_event_names() {
        let p = Prop { name: "on_greet".into(), doc: String::new(), kind: PropKind::Callback { optional: true, arg: None } };
        assert_eq!(p.js_name(), "onGreet");
        assert_eq!(p.event_name(), "greet");
    }

    fn greeter() -> ExternalComponent {
        ExternalComponent {
            name: "Greeter".into(),
            module_path: "external_export_demo".into(),
            tag: "idl-greeter".into(),
            props: vec![
                raw("name", "Reactive < String >"),
                raw("on_greet", "Option < Rc < dyn Fn () > >"),
            ],
        }
    }

    #[test]
    fn bridge_lib_has_class_and_setters() {
        let src = gen_bridge_lib(&[greeter()]);
        assert!(src.contains("pub struct GreeterElement"));
        assert!(src.contains("name: Signal<String>"));
        assert!(src.contains("on_greet: Rc<RefCell<Option<js_sys::Function>>>"));
        // reactive value passed live; callback wrapped in Some.
        assert!(src.contains("name: name.into()"));
        assert!(src.contains("on_greet: Some(Rc::new(move ||"));
        assert!(src.contains("external_export_demo::Greeter(&props)"));
        assert!(src.contains("..Default::default()"));
        // setters mapped to JS names.
        assert!(src.contains("js_name = onGreet"));
        // Shared backend + detached subtree (so multiple elements on one
        // page don't clobber each other's per-backend JS shims).
        assert!(src.contains("fn shared_backend()"));
        assert!(src.contains("let backend = shared_backend();"));
        assert!(src.contains("build_detached(&backend, element, None)"));
        assert!(!src.contains("mount(backend"));
        // Unique identity seed per element (no id collisions between
        // multiple elements on one page).
        assert!(src.contains("with_current_identity(seed"));
        assert!(src.contains("Some(next_element_id())"));
    }

    #[test]
    fn element_js_defines_tag_and_callback_event() {
        let js = gen_element_js(&greeter(), "external_export_demo");
        assert!(js.contains("customElements.define(\"idl-greeter\", IdlGreeter)"));
        assert!(js.contains("new GreeterElement()"));
        assert!(js.contains("new CustomEvent(\"greet\""));
        assert!(js.contains("this._bridge.onGreet ="));
        // Single shared wasm init across element modules (no concurrent
        // double-init that would orphan other elements' closures).
        assert!(js.contains("globalThis.__idealystReady_"));
    }

    #[test]
    fn dts_and_react_have_typed_props() {
        let dts = gen_dts(&greeter());
        assert!(dts.contains("name?: string;"));
        assert!(dts.contains("onGreet?: () => void;"));
        assert!(dts.contains("\"idl-greeter\": IdlGreeter;"));

        let tsx = gen_react_wrapper(&greeter());
        assert!(tsx.contains("export function Greeter("));
        // Wrapper lives in `react/`; the element file is one level up.
        assert!(tsx.contains("import \"../idl-greeter.js\""));
        assert!(tsx.contains("createElement(\"idl-greeter\""));
    }

    #[test]
    fn vue_wrapper_lists_props() {
        let vue = gen_vue_wrapper(&greeter());
        assert!(vue.contains("export const Greeter ="));
        assert!(vue.contains("\"name\", \"onGreet\""));
        assert!(vue.contains("h(\"idl-greeter\""));
        // Wrapper is in `vue/`; element is one level up.
        assert!(vue.contains("import \"../idl-greeter.js\""));
    }

    #[test]
    fn svelte_wrapper_binds_props() {
        let s = gen_svelte_wrapper(&greeter());
        assert!(s.contains("export let name = undefined;"));
        assert!(s.contains("export let onGreet = undefined;"));
        assert!(s.contains("$: if (el && name !== undefined) el.name = name;"));
        assert!(s.contains("<idl-greeter bind:this={el}>"));
        assert!(s.contains("import \"../idl-greeter.js\""));
    }

    #[test]
    fn angular_wrapper_inputs_outputs() {
        let a = gen_angular_wrapper(&greeter());
        assert!(a.contains("export class GreeterComponent"));
        assert!(a.contains("@Input() name?: string;"));
        assert!(a.contains("@Output() greet = new EventEmitter<void>();"));
        assert!(a.contains("el.addEventListener(\"greet\""));
        assert!(a.contains("CUSTOM_ELEMENTS_SCHEMA"));
        assert!(a.contains("selector: \"idl-greeter-ng\""));
    }

    #[test]
    fn angular_wrapper_callback_arg_typed() {
        let c = ExternalComponent {
            name: "Slider".into(),
            module_path: "demo".into(),
            tag: "idl-slider".into(),
            props: vec![raw("on_change", "Rc < dyn Fn (i32) >")],
        };
        let a = gen_angular_wrapper(&c);
        assert!(a.contains("@Output() change = new EventEmitter<number>();"), "{a}");
    }

    #[test]
    fn vanilla_helper_create_and_bind() {
        let v = gen_vanilla_helper(&greeter());
        assert!(v.contains("export function createGreeter(props = {})"));
        assert!(v.contains("export function bindGreeter(el, props = {})"));
        assert!(v.contains("document.createElement(\"idl-greeter\")"));
        assert!(v.contains("if (props.name !== undefined) el.name = props.name;"));
    }

    #[test]
    fn package_json_and_index_dts() {
        let comps = [greeter()];
        let pkg = gen_package_json("my-app", &Framework::ALL);
        assert!(pkg.contains("\"name\": \"my-app-components\""));
        assert!(pkg.contains("\"type\": \"module\""));
        assert!(pkg.contains("\"react\""));
        assert!(pkg.contains("\"angular\""));
        // No exports map — deep subpath imports stay open.
        assert!(!pkg.contains("\"exports\""));

        let dts = gen_index_dts(&comps);
        assert!(dts.contains("export * from \"./idl-greeter\";"));
    }

    #[test]
    fn framework_parse_and_dispatch() {
        assert_eq!(Framework::parse("React"), Some(Framework::React));
        assert_eq!(Framework::parse("js"), Some(Framework::Vanilla));
        assert_eq!(Framework::parse("nope"), None);
        assert_eq!(Framework::Angular.filename(&greeter()), "greeter.component.ts");
        assert_eq!(Framework::Vanilla.slug(), "vanilla");
        // generate() routes to the right generator.
        assert!(Framework::Svelte.generate(&greeter()).contains("bind:this"));
        assert!(Framework::ALL.contains(&Framework::Vue));
    }
}
