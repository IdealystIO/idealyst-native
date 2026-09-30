//! The JavaScript built-ins the framework uses, named and shaped like
//! `js_sys`'s (`Reflect::get`, `Function::call1`, `Uint32Array::from`, …) so
//! a port from js-sys changes paths, not call sites. Errors are
//! [`JsError`] rather than a bare `JsValue`; `From<JsError> for JsValue`
//! keeps `?` working in code that returns `Result<_, JsValue>`.
//!
//! Only what the framework calls is bound. Typed-array constructors COPY
//! out of wasm memory: a view into it would be detached by the next
//! memory growth, and the framework's shims may keep what they are given.

use crate::cast::JsCast;
use crate::{string, Closure, JsError, JsValue};

crate::js_class! {
    /// `Object`.
    pub struct Object = "Object";
    /// `Array`.
    pub struct Array: Object = "Array";
    /// `Function`.
    pub struct Function: Object = "Function";
    /// `Promise`.
    pub struct Promise: Object = "Promise";
    /// `ArrayBuffer`.
    pub struct ArrayBuffer: Object = "ArrayBuffer";
    /// `Uint8Array`.
    pub struct Uint8Array: Object = "Uint8Array";
    /// `Uint32Array`.
    pub struct Uint32Array: Object = "Uint32Array";
    /// `Set`.
    pub struct Set: Object = "Set";
    /// `Map`.
    pub struct Map: Object = "Map";
}

fn owned<T: JsCast>(idx: u32) -> T {
    T::unchecked_from_js(unsafe { JsValue::from_raw(idx) })
}

crate::import! {
    fn js_object_new() -> u32 = "() => G.add({})";
    fn js_object_keys(o: u32) -> u32 = "(o) => G.add(Object.keys(G.get(o)))";
    #[catch]
    fn js_define_property(o: u32, k: u32, d: u32) =
        "(o, k, d) => { Object.defineProperty(G.get(o), G.get(k), G.get(d)); }";

    fn js_array_new(n: u32) -> u32 = "(n) => G.add(new Array(n >>> 0))";
    fn js_array_of(args: usize, n: usize) -> u32 = "(a, n) => G.add(G.args(a, n))";
    fn js_array_len(a: u32) -> u32 = "(a) => G.get(a).length >>> 0";
    fn js_array_get(a: u32, i: u32) -> u32 = "(a, i) => G.add(G.get(a)[i >>> 0])";
    fn js_array_set(a: u32, i: u32, v: u32) = "(a, i, v) => { G.get(a)[i >>> 0] = G.get(v); }";
    fn js_array_push(a: u32, v: u32) -> u32 = "(a, v) => G.get(a).push(G.get(v)) >>> 0";
    #[catch]
    fn js_array_from(v: u32) -> u32 = "(v) => G.add(Array.from(G.get(v)))";

    #[catch]
    fn js_function_new(args: usize, al: usize, body: usize, bl: usize) -> u32 =
        "(a, al, b, bl) => { const ps = G.str(a, al); \
           return G.add(ps.length ? new Function(...ps.split(','), G.str(b, bl)) : new Function(G.str(b, bl))); }";

    #[catch]
    fn js_reflect_get(t: u32, k: u32) -> u32 = "(t, k) => G.add(Reflect.get(G.get(t), G.get(k)))";
    #[catch]
    fn js_reflect_get_u32(t: u32, k: u32) -> u32 = "(t, k) => G.add(Reflect.get(G.get(t), k >>> 0))";
    #[catch]
    fn js_reflect_set(t: u32, k: u32, v: u32) -> u32 =
        "(t, k, v) => Reflect.set(G.get(t), G.get(k), G.get(v)) ? 1 : 0";
    #[catch]
    fn js_reflect_has(t: u32, k: u32) -> u32 = "(t, k) => Reflect.has(G.get(t), G.get(k)) ? 1 : 0";
    #[catch]
    fn js_reflect_delete(t: u32, k: u32) -> u32 =
        "(t, k) => Reflect.deleteProperty(G.get(t), G.get(k)) ? 1 : 0";
    #[catch]
    fn js_reflect_construct(f: u32, a: u32) -> u32 =
        "(f, a) => G.add(Reflect.construct(G.get(f), G.get(a)))";

    fn js_promise_resolve(v: u32) -> u32 = "(v) => G.add(Promise.resolve(G.get(v)))";
    // Writes [resolve, reject] handles into the two u32s at `out`.
    fn js_promise_with_resolvers(out: usize) -> u32 =
        "(o) => { let a, b; const p = new Promise((x, y) => { a = x; b = y; }); \
           const w0 = G.add(a), w1 = G.add(b); const w = G.u32(); \
           w[(o >>> 0) >>> 2] = w0; w[((o >>> 0) >>> 2) + 1] = w1; return G.add(p); }";
    fn js_then(p: u32, f: u32) -> u32 = "(p, f) => G.add(G.get(p).then(G.get(f)))";

    fn js_u8_new_len(n: u32) -> u32 = "(n) => G.add(new Uint8Array(n >>> 0))";
    fn js_u8_new(b: u32) -> u32 = "(b) => G.add(new Uint8Array(G.get(b)))";
    fn js_u8_from(p: usize, l: usize) -> u32 =
        "(p, l) => G.add(G.u8().slice(p >>> 0, (p >>> 0) + (l >>> 0)))";
    fn js_u8_len(a: u32) -> u32 = "(a) => G.get(a).length >>> 0";
    fn js_u8_copy_to(a: u32, p: usize) = "(a, p) => { G.u8().set(G.get(a), p >>> 0); }";
    fn js_u8_copy_from(a: u32, p: usize, l: usize) =
        "(a, p, l) => { G.get(a).set(G.u8().subarray(p >>> 0, (p >>> 0) + (l >>> 0))); }";
    fn js_u32_from(p: usize, n: usize) -> u32 =
        "(p, n) => { const b = (p >>> 0) >>> 2; return G.add(G.u32().slice(b, b + (n >>> 0))); }";
    fn js_u32_len(a: u32) -> u32 = "(a) => G.get(a).length >>> 0";
    fn js_u32_copy_to(a: u32, p: usize) = "(a, p) => { G.u32().set(G.get(a), (p >>> 0) >>> 2); }";

    fn js_set_new(init: u32) -> u32 = "(i) => G.add(new Set(G.get(i) ?? undefined))";
    fn js_set_add(s: u32, v: u32) = "(s, v) => { G.get(s).add(G.get(v)); }";
    fn js_set_has(s: u32, v: u32) -> u32 = "(s, v) => G.get(s).has(G.get(v)) ? 1 : 0";
    fn js_set_delete(s: u32, v: u32) -> u32 = "(s, v) => G.get(s).delete(G.get(v)) ? 1 : 0";
    fn js_size(s: u32) -> u32 = "(s) => G.get(s).size >>> 0";
    fn js_map_new() -> u32 = "() => G.add(new Map())";
    fn js_map_get(m: u32, k: u32) -> u32 = "(m, k) => G.add(G.get(m).get(G.get(k)))";
    fn js_map_set(m: u32, k: u32, v: u32) = "(m, k, v) => { G.get(m).set(G.get(k), G.get(v)); }";
    fn js_map_has(m: u32, k: u32) -> u32 = "(m, k) => G.get(m).has(G.get(k)) ? 1 : 0";

    fn js_date_now() -> f64 = "() => Date.now()";
    fn js_encode_uri_component(p: usize, l: usize, out: usize) =
        "(p, l, o) => G.retStr(encodeURIComponent(G.str(p, l)), o)";
    fn js_is_iterable(v: u32) -> u32 =
        "(v) => { const x = G.get(v); return x != null && typeof x[Symbol.iterator] === 'function' ? 1 : 0; }";
}

impl From<JsError> for JsValue {
    fn from(e: JsError) -> JsValue {
        e.into_value()
    }
}

impl Object {
    pub fn new() -> Object {
        owned(unsafe { js_object_new() })
    }

    /// `Object.keys(o)`.
    pub fn keys(o: &Object) -> Array {
        owned(unsafe { js_object_keys(o.as_js().raw()) })
    }

    /// `Object.defineProperty(o, key, descriptor)`.
    pub fn define_property(o: &Object, key: &JsValue, descriptor: &Object) -> Result<(), JsError> {
        unsafe { js_define_property(o.as_js().raw(), key.raw(), descriptor.as_js().raw()) }
    }
}

impl Default for Object {
    fn default() -> Object {
        Object::new()
    }
}

impl Array {
    pub fn new() -> Array {
        Array::new_with_length(0)
    }

    pub fn new_with_length(n: u32) -> Array {
        owned(unsafe { js_array_new(n) })
    }

    fn of(items: &[&JsValue]) -> Array {
        let raw: Vec<u32> = items.iter().map(|v| v.raw()).collect();
        owned(unsafe { js_array_of(raw.as_ptr() as usize, raw.len()) })
    }

    pub fn of1(a: &JsValue) -> Array {
        Array::of(&[a])
    }
    pub fn of2(a: &JsValue, b: &JsValue) -> Array {
        Array::of(&[a, b])
    }
    pub fn of3(a: &JsValue, b: &JsValue, c: &JsValue) -> Array {
        Array::of(&[a, b, c])
    }
    pub fn of4(a: &JsValue, b: &JsValue, c: &JsValue, d: &JsValue) -> Array {
        Array::of(&[a, b, c, d])
    }
    pub fn of5(a: &JsValue, b: &JsValue, c: &JsValue, d: &JsValue, e: &JsValue) -> Array {
        Array::of(&[a, b, c, d, e])
    }

    /// `Array.from(iterable)`.
    pub fn from(v: &JsValue) -> Array {
        unsafe { js_array_from(v.raw()) }.map(owned).unwrap_or_else(|_| Array::new())
    }

    pub fn length(&self) -> u32 {
        unsafe { js_array_len(self.as_js().raw()) }
    }

    /// `a[i]` (`undefined` past the end).
    pub fn get(&self, i: u32) -> JsValue {
        unsafe { JsValue::from_raw(js_array_get(self.as_js().raw(), i)) }
    }

    pub fn set(&self, i: u32, v: JsValue) {
        unsafe { js_array_set(self.as_js().raw(), i, v.raw()) }
    }

    /// `push(v)` → the new length.
    pub fn push(&self, v: &JsValue) -> u32 {
        unsafe { js_array_push(self.as_js().raw(), v.raw()) }
    }

    pub fn iter(&self) -> impl Iterator<Item = JsValue> + '_ {
        (0..self.length()).map(move |i| self.get(i))
    }

    pub fn to_vec(&self) -> Vec<JsValue> {
        self.iter().collect()
    }
}

impl Default for Array {
    fn default() -> Array {
        Array::new()
    }
}

impl Function {
    /// `new Function(body)`. Evaluates source text: the framework uses it
    /// only in tests.
    pub fn new_no_args(body: &str) -> Function {
        Function::new_with_args("", body)
    }

    /// `new Function(...args.split(','), body)`.
    pub fn new_with_args(args: &str, body: &str) -> Function {
        let (a, al) = string::abi(args);
        let (b, bl) = string::abi(body);
        owned(unsafe { js_function_new(a, al, b, bl) }.expect("new Function: bad source"))
    }

    pub fn call0(&self, this: &JsValue) -> Result<JsValue, JsError> {
        self.as_js().call(this, &[])
    }
    pub fn call1(&self, this: &JsValue, a: &JsValue) -> Result<JsValue, JsError> {
        self.as_js().call(this, &[a])
    }
    pub fn call2(&self, this: &JsValue, a: &JsValue, b: &JsValue) -> Result<JsValue, JsError> {
        self.as_js().call(this, &[a, b])
    }
    pub fn call3(&self, this: &JsValue, a: &JsValue, b: &JsValue, c: &JsValue) -> Result<JsValue, JsError> {
        self.as_js().call(this, &[a, b, c])
    }
    pub fn call4(
        &self,
        this: &JsValue,
        a: &JsValue,
        b: &JsValue,
        c: &JsValue,
        d: &JsValue,
    ) -> Result<JsValue, JsError> {
        self.as_js().call(this, &[a, b, c, d])
    }

    /// `f.apply(this, args)`.
    pub fn apply(&self, this: &JsValue, args: &Array) -> Result<JsValue, JsError> {
        let items = args.to_vec();
        let refs: Vec<&JsValue> = items.iter().collect();
        self.as_js().call(this, &refs)
    }
}

/// `Reflect.*`.
pub struct Reflect;

impl Reflect {
    pub fn get(target: &JsValue, key: &JsValue) -> Result<JsValue, JsError> {
        unsafe { js_reflect_get(target.raw(), key.raw()) }.map(|i| unsafe { JsValue::from_raw(i) })
    }

    pub fn get_u32(target: &JsValue, index: u32) -> Result<JsValue, JsError> {
        unsafe { js_reflect_get_u32(target.raw(), index) }.map(|i| unsafe { JsValue::from_raw(i) })
    }

    pub fn set(target: &JsValue, key: &JsValue, value: &JsValue) -> Result<bool, JsError> {
        unsafe { js_reflect_set(target.raw(), key.raw(), value.raw()) }.map(|r| r != 0)
    }

    pub fn has(target: &JsValue, key: &JsValue) -> Result<bool, JsError> {
        unsafe { js_reflect_has(target.raw(), key.raw()) }.map(|r| r != 0)
    }

    pub fn delete_property(target: &JsValue, key: &JsValue) -> Result<bool, JsError> {
        unsafe { js_reflect_delete(target.raw(), key.raw()) }.map(|r| r != 0)
    }

    pub fn construct(ctor: &Function, args: &Array) -> Result<JsValue, JsError> {
        unsafe { js_reflect_construct(ctor.as_js().raw(), args.as_js().raw()) }
            .map(|i| unsafe { JsValue::from_raw(i) })
    }
}

impl Promise {
    pub fn resolve(v: &JsValue) -> Promise {
        owned(unsafe { js_promise_resolve(v.raw()) })
    }

    /// `new Promise(executor)`: `f` is handed `resolve` and `reject`, and
    /// may keep them to settle the promise later.
    pub fn new(f: &mut dyn FnMut(Function, Function)) -> Promise {
        let mut slot = [0u32; 2];
        let p: Promise = owned(unsafe { js_promise_with_resolvers(slot.as_mut_ptr() as usize) });
        f(owned(slot[0]), owned(slot[1]));
        p
    }

    /// `p.then(f)`; `f` is called with the fulfilment value.
    pub fn then(&self, f: &Closure) -> Promise {
        owned(unsafe { js_then(self.as_js().raw(), f.as_js().raw()) })
    }
}

impl Uint8Array {
    /// A view of `buffer` (an `ArrayBuffer`).
    pub fn new(buffer: &JsValue) -> Uint8Array {
        owned(unsafe { js_u8_new(buffer.raw()) })
    }

    pub fn new_with_length(n: u32) -> Uint8Array {
        owned(unsafe { js_u8_new_len(n) })
    }

    pub fn length(&self) -> u32 {
        unsafe { js_u8_len(self.as_js().raw()) }
    }

    /// A copy of the bytes, in Rust.
    pub fn to_vec(&self) -> Vec<u8> {
        let mut out = vec![0u8; self.length() as usize];
        if !out.is_empty() {
            unsafe { js_u8_copy_to(self.as_js().raw(), out.as_mut_ptr() as usize) }
        }
        out
    }

    /// Overwrite the array's contents with `src` (lengths must match).
    pub fn copy_from(&self, src: &[u8]) {
        unsafe { js_u8_copy_from(self.as_js().raw(), src.as_ptr() as usize, src.len()) }
    }
}

impl From<&[u8]> for Uint8Array {
    /// A COPY of `src` (see the module docs).
    fn from(src: &[u8]) -> Uint8Array {
        owned(unsafe { js_u8_from(src.as_ptr() as usize, src.len()) })
    }
}

impl Uint32Array {
    pub fn length(&self) -> u32 {
        unsafe { js_u32_len(self.as_js().raw()) }
    }

    pub fn to_vec(&self) -> Vec<u32> {
        let mut out = vec![0u32; self.length() as usize];
        if !out.is_empty() {
            unsafe { js_u32_copy_to(self.as_js().raw(), out.as_mut_ptr() as usize) }
        }
        out
    }
}

impl From<&[u32]> for Uint32Array {
    /// A COPY of `src` (see the module docs).
    fn from(src: &[u32]) -> Uint32Array {
        owned(unsafe { js_u32_from(src.as_ptr() as usize, src.len()) })
    }
}

impl From<&Vec<u32>> for Uint32Array {
    fn from(src: &Vec<u32>) -> Uint32Array {
        Uint32Array::from(src.as_slice())
    }
}

impl Set {
    /// `new Set(init)` — `init` an iterable or `undefined`.
    pub fn new(init: &JsValue) -> Set {
        owned(unsafe { js_set_new(init.raw()) })
    }
    pub fn add(&self, v: &JsValue) -> &Set {
        unsafe { js_set_add(self.as_js().raw(), v.raw()) };
        self
    }
    pub fn has(&self, v: &JsValue) -> bool {
        unsafe { js_set_has(self.as_js().raw(), v.raw()) != 0 }
    }
    pub fn delete(&self, v: &JsValue) -> bool {
        unsafe { js_set_delete(self.as_js().raw(), v.raw()) != 0 }
    }
    pub fn size(&self) -> u32 {
        unsafe { js_size(self.as_js().raw()) }
    }
}

impl Map {
    pub fn new() -> Map {
        owned(unsafe { js_map_new() })
    }
    pub fn get(&self, k: &JsValue) -> JsValue {
        unsafe { JsValue::from_raw(js_map_get(self.as_js().raw(), k.raw())) }
    }
    pub fn set(&self, k: &JsValue, v: &JsValue) -> &Map {
        unsafe { js_map_set(self.as_js().raw(), k.raw(), v.raw()) };
        self
    }
    pub fn has(&self, k: &JsValue) -> bool {
        unsafe { js_map_has(self.as_js().raw(), k.raw()) != 0 }
    }
    pub fn size(&self) -> u32 {
        unsafe { js_size(self.as_js().raw()) }
    }
}

impl Default for Map {
    fn default() -> Map {
        Map::new()
    }
}

/// `Date`.
pub struct Date;

impl Date {
    pub fn now() -> f64 {
        unsafe { js_date_now() }
    }
}

/// `encodeURIComponent(s)`.
pub fn encode_uri_component(s: &str) -> String {
    let (p, l) = string::abi(s);
    string::receive(|o| unsafe { js_encode_uri_component(p, l, o) })
}

/// Iterate a JS iterable (`None` if `v` is not iterable). Materialised
/// through `Array.from`, so the iteration itself makes no further calls.
/// Items are `Result`s, as with `js_sys::try_iter` (materialising cannot
/// fail part-way here, so every item is `Ok`).
pub fn try_iter(v: &JsValue) -> Result<Option<impl Iterator<Item = Result<JsValue, JsError>>>, JsError> {
    if unsafe { js_is_iterable(v.raw()) } == 0 {
        return Ok(None);
    }
    let arr: Array = unsafe { js_array_from(v.raw()) }.map(owned)?;
    Ok(Some(arr.to_vec().into_iter().map(Ok)))
}
