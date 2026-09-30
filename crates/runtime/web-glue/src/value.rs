//! [`JsValue`]: an owned handle to a JS value.

use std::cell::Cell;
use std::fmt;
use std::mem::ManuallyDrop;

use crate::{error::JsError, ffi, string};

thread_local! {
    static LIVE: Cell<usize> = const { Cell::new(0) };
}

/// An owned reference to a JS value: an index into the runtime's handle
/// slab (`G.heap` in `js/runtime.js`).
///
/// RAII: dropping it releases the slot, cloning it takes a second slot for
/// the same JS value. Index 0 is `undefined` and index 1 is `null`; they own
/// no slot, so [`JsValue::UNDEFINED`] / [`JsValue::NULL`] are constants and
/// dropping or cloning them never crosses the boundary.
///
/// Not `Send`: the slab lives on the one JS thread the module runs on.
pub struct JsValue {
    idx: u32,
    _not_send: std::marker::PhantomData<*const ()>,
}

/// The kind of a JS value, as `typeof` sees it (plus `null`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum JsType {
    Undefined,
    Null,
    Boolean,
    Number,
    String,
    Object,
    Function,
    Symbol,
    BigInt,
}

impl JsValue {
    /// Take ownership of slot `idx`, as returned by a snippet that did
    /// `G.add(v)`.
    ///
    /// # Safety
    /// `idx` must be a slot nothing else owns (a fresh `G.add` result, or
    /// one released from a `JsValue` with [`JsValue::into_raw`]).
    #[inline]
    pub unsafe fn from_raw(idx: u32) -> JsValue {
        if idx > 1 {
            LIVE.with(|l| l.set(l.get() + 1));
        }
        JsValue { idx, _not_send: std::marker::PhantomData }
    }

    /// Give up ownership of the slot without releasing it.
    #[inline]
    pub fn into_raw(self) -> u32 {
        let me = ManuallyDrop::new(self);
        if me.idx > 1 {
            LIVE.with(|l| l.set(l.get() - 1));
        }
        me.idx
    }

    /// The slot index, borrowed — for passing to a snippet that only reads
    /// it (`G.get(h)`).
    #[inline]
    pub fn raw(&self) -> u32 {
        self.idx
    }

    /// `undefined`. Owns no slot.
    #[inline]
    pub fn undefined() -> JsValue {
        JsValue::UNDEFINED
    }

    /// `null`. Owns no slot.
    pub fn null() -> JsValue {
        JsValue::NULL
    }

    /// `undefined` (slot 0), usable in constant position.
    pub const UNDEFINED: JsValue = JsValue { idx: 0, _not_send: std::marker::PhantomData };
    /// `null` (slot 1), usable in constant position.
    pub const NULL: JsValue = JsValue { idx: 1, _not_send: std::marker::PhantomData };

    /// `globalThis`.
    pub fn global() -> JsValue {
        unsafe { JsValue::from_raw(ffi::global()) }
    }

    // Named like `wasm_bindgen::JsValue::from_str` so ports read the same;
    // it is infallible, so `FromStr` would be the wrong shape.
    #[allow(clippy::should_implement_trait)]
    pub fn from_str(s: &str) -> JsValue {
        let (p, l) = string::abi(s);
        unsafe { JsValue::from_raw(ffi::str_new(p, l)) }
    }

    pub fn from_f64(n: f64) -> JsValue {
        unsafe { JsValue::from_raw(ffi::num_new(n)) }
    }

    pub fn from_bool(b: bool) -> JsValue {
        unsafe { JsValue::from_raw(ffi::bool_new(b as u32)) }
    }

    pub fn js_type(&self) -> JsType {
        if self.idx == 0 {
            return JsType::Undefined;
        }
        match unsafe { ffi::type_of(self.idx) } {
            0 => JsType::Undefined,
            1 => JsType::Null,
            2 => JsType::Boolean,
            3 => JsType::Number,
            4 => JsType::String,
            6 => JsType::Function,
            7 => JsType::Symbol,
            8 => JsType::BigInt,
            _ => JsType::Object,
        }
    }

    pub fn is_undefined(&self) -> bool {
        self.idx == 0 || self.js_type() == JsType::Undefined
    }

    pub fn is_function(&self) -> bool {
        self.js_type() == JsType::Function
    }

    pub fn is_object(&self) -> bool {
        matches!(self.js_type(), JsType::Object | JsType::Function)
    }

    pub fn is_string(&self) -> bool {
        self.js_type() == JsType::String
    }

    pub fn is_null(&self) -> bool {
        self.js_type() == JsType::Null
    }

    pub fn as_f64(&self) -> Option<f64> {
        (self.js_type() == JsType::Number).then(|| unsafe { ffi::num_get(self.idx) })
    }

    pub fn as_bool(&self) -> Option<bool> {
        (self.js_type() == JsType::Boolean).then(|| self.truthy())
    }

    /// JS truthiness.
    pub fn truthy(&self) -> bool {
        self.idx != 0 && unsafe { ffi::truthy(self.idx) } != 0
    }

    /// The string, if this is a JS string. One boundary crossing; the
    /// bytes land in a buffer Rust allocated and now owns.
    pub fn as_string(&self) -> Option<String> {
        if self.idx == 0 {
            return None;
        }
        let mut hit = 0;
        let s = string::receive(|out| hit = unsafe { ffi::str_get(self.idx, out) });
        (hit != 0).then_some(s)
    }

    /// `String(value)`.
    pub fn to_js_string(&self) -> Result<String, JsError> {
        let mut res = Ok(());
        let s = string::receive(|out| res = unsafe { ffi::to_string(self.idx, out) });
        res.map(|()| s)
    }

    /// `a === b`.
    pub fn strict_eq(&self, other: &JsValue) -> bool {
        unsafe { ffi::strict_eq(self.idx, other.idx) != 0 }
    }

    /// `value[key]`. Errors if `value` is null/undefined or a getter throws.
    pub fn get(&self, key: &str) -> Result<JsValue, JsError> {
        let (p, l) = string::abi(key);
        unsafe { ffi::get(self.idx, p, l).map(|h| JsValue::from_raw(h)) }
    }

    /// `value[key] = v`.
    pub fn set(&self, key: &str, v: &JsValue) -> Result<(), JsError> {
        let (p, l) = string::abi(key);
        unsafe { ffi::set(self.idx, p, l, v.idx) }
    }

    /// `value[name](...args)`.
    pub fn call_method(&self, name: &str, args: &[&JsValue]) -> Result<JsValue, JsError> {
        let (p, l) = string::abi(name);
        let raw: Vec<u32> = args.iter().map(|a| a.idx).collect();
        unsafe {
            ffi::call_method(self.idx, p, l, raw.as_ptr() as usize, raw.len())
                .map(|h| JsValue::from_raw(h))
        }
    }

    /// `value.apply(this, args)`.
    pub fn call(&self, this: &JsValue, args: &[&JsValue]) -> Result<JsValue, JsError> {
        let raw: Vec<u32> = args.iter().map(|a| a.idx).collect();
        unsafe {
            ffi::call(self.idx, this.idx, raw.as_ptr() as usize, raw.len())
                .map(|h| JsValue::from_raw(h))
        }
    }

    /// `new value(...args)`.
    pub fn construct(&self, args: &[&JsValue]) -> Result<JsValue, JsError> {
        let raw: Vec<u32> = args.iter().map(|a| a.idx).collect();
        unsafe {
            ffi::construct(self.idx, raw.as_ptr() as usize, raw.len())
                .map(|h| JsValue::from_raw(h))
        }
    }

    /// Handles this thread's Rust code currently owns (slots in use,
    /// excluding `undefined`). Test/debug accounting.
    pub fn live_count() -> usize {
        LIVE.with(Cell::get)
    }

    /// Slots in use in the JS slab. Equals [`JsValue::live_count`] plus
    /// whatever JS-side code (the callback trampoline's in-flight args,
    /// minted-but-not-yet-owned values) holds at the moment.
    pub fn js_live_count() -> u32 {
        unsafe { ffi::live_js() }
    }
}

impl Clone for JsValue {
    fn clone(&self) -> JsValue {
        if self.idx <= 1 {
            return JsValue { idx: self.idx, _not_send: std::marker::PhantomData };
        }
        unsafe { JsValue::from_raw(ffi::clone_ref(self.idx)) }
    }
}

impl Drop for JsValue {
    #[inline]
    fn drop(&mut self) {
        if self.idx > 1 {
            LIVE.with(|l| l.set(l.get() - 1));
            unsafe { ffi::drop_ref(self.idx) }
        }
    }
}

impl fmt::Debug for JsValue {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "JsValue(#{}: {:?})", self.idx, self.js_type())
    }
}

impl From<&str> for JsValue {
    fn from(s: &str) -> JsValue {
        JsValue::from_str(s)
    }
}

impl From<f64> for JsValue {
    fn from(n: f64) -> JsValue {
        JsValue::from_f64(n)
    }
}

impl From<bool> for JsValue {
    fn from(b: bool) -> JsValue {
        JsValue::from_bool(b)
    }
}

impl From<String> for JsValue {
    fn from(s: String) -> JsValue {
        JsValue::from_str(&s)
    }
}

impl From<&String> for JsValue {
    fn from(s: &String) -> JsValue {
        JsValue::from_str(s)
    }
}

macro_rules! from_number {
    ($($t:ty),*) => {$(
        impl From<$t> for JsValue {
            fn from(n: $t) -> JsValue {
                JsValue::from_f64(n as f64)
            }
        }
    )*};
}
from_number!(f32, i8, i16, i32, u8, u16, u32, usize, isize, i64, u64);

impl<T: Into<JsValue>> From<Option<T>> for JsValue {
    /// `None` is `undefined`, as with wasm-bindgen.
    fn from(v: Option<T>) -> JsValue {
        v.map_or(JsValue::UNDEFINED, Into::into)
    }
}

impl PartialEq for JsValue {
    /// `===`.
    fn eq(&self, other: &JsValue) -> bool {
        self.strict_eq(other)
    }
}
