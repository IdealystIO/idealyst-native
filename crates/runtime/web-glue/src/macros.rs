//! `import!` (declare JS functions) and `js_module!` (ship a JS module).

/// Declare JS functions callable from Rust, each with its JS inline.
///
/// ```ignore
/// web_glue::import! {
///     /// `document.createElement(tag)` → a new handle.
///     pub fn create_element(tag_ptr: usize, tag_len: usize) -> u32 =
///         "(p, l) => G.add(document.createElement(G.str(p, l)))";
///
///     // `#[catch]` must come FIRST: a JS throw becomes `Err(JsError)`.
///     #[catch]
///     pub fn parse_json(p: usize, l: usize) -> u32 =
///         "(p, l) => G.add(JSON.parse(G.str(p, l)))";
/// }
/// ```
///
/// Each item becomes an `unsafe fn` (the snippet can read and write linear
/// memory through the pointers it is handed) wrapping one wasm import
/// from the `./__idealyst_glue.js` module. Parameters and returns are raw
/// wasm scalars (`u32`/`i32`/`usize`/`f64`/`f32`); handles are `u32`
/// (see [`JsValue::into_raw`](crate::JsValue::into_raw) /
/// [`JsValue::from_raw`](crate::JsValue::from_raw)), strings are
/// `(ptr, len)` in and an out-slot out (see [`crate::string`]). The snippet
/// is a JS expression evaluating to a function; `G` (the runtime,
/// `js/runtime.js`) is in scope.
///
/// # Why the JS rides in the import NAME
///
/// The import's name is `<key>\n<flags>\n<js>`. The build pass
/// (`wasm_carve::glue`) reads it back, renames the import to a short id,
/// and writes the snippet into `pkg/<lib>.js`. Carrying the JS in the
/// import itself means it is present in the linked module exactly when
/// the import is: LLD drops an unused import, and with it its JS. A
/// `#[link_section]` custom section can NOT promise that — it only
/// survives if some symbol in the same object file is referenced (see
/// [`js_module!`]).
///
/// The key is `module_path::name(arg types) -> ret`. It makes the name
/// unique per declaration: LLD merges same-named imports, and two
/// declarations with the same JS but different Rust signatures would
/// otherwise collide into one import with a mismatched type.
#[macro_export]
macro_rules! import {
    () => {};
    (
        #[catch]
        $(#[$meta:meta])*
        $vis:vis fn $name:ident ( $($arg:ident : $ty:ty),* $(,)? ) $(-> $ret:ty)? = $js:expr ;
        $($rest:tt)*
    ) => {
        $(#[$meta])*
        #[inline(always)]
        #[allow(clippy::missing_safety_doc)]
        $vis unsafe fn $name( $($arg : $ty),* ) -> ::core::result::Result<$crate::__ret!($($ret)?), $crate::JsError> {
            #[cfg(target_arch = "wasm32")]
            {
                #[link(wasm_import_module = "./__idealyst_glue.js")]
                unsafe extern "C" {
                    #[link_name = concat!(
                        module_path!(), "::", stringify!($name),
                        stringify!(( $($ty),* ) $(-> $ret)?),
                        "\ncatch\n", $js
                    )]
                    fn __glue( $($arg : $ty),* ) $(-> $ret)?;
                }
                let r = unsafe { __glue( $($arg),* ) };
                match $crate::error::take_pending() {
                    ::core::option::Option::Some(e) => ::core::result::Result::Err(e),
                    ::core::option::Option::None => ::core::result::Result::Ok(r),
                }
            }
            #[cfg(not(target_arch = "wasm32"))]
            {
                $( let _ = $arg; )*
                ::core::panic!(concat!("web-glue import `", stringify!($name), "` only exists on wasm32"))
            }
        }
        $crate::import! { $($rest)* }
    };
    (
        $(#[$meta:meta])*
        $vis:vis fn $name:ident ( $($arg:ident : $ty:ty),* $(,)? ) $(-> $ret:ty)? = $js:expr ;
        $($rest:tt)*
    ) => {
        $(#[$meta])*
        #[inline(always)]
        #[allow(clippy::missing_safety_doc)]
        $vis unsafe fn $name( $($arg : $ty),* ) $(-> $ret)? {
            #[cfg(target_arch = "wasm32")]
            {
                #[link(wasm_import_module = "./__idealyst_glue.js")]
                unsafe extern "C" {
                    #[link_name = concat!(
                        module_path!(), "::", stringify!($name),
                        stringify!(( $($ty),* ) $(-> $ret)?),
                        "\n\n", $js
                    )]
                    fn __glue( $($arg : $ty),* ) $(-> $ret)?;
                }
                unsafe { __glue( $($arg),* ) }
            }
            #[cfg(not(target_arch = "wasm32"))]
            {
                $( let _ = $arg; )*
                ::core::panic!(concat!("web-glue import `", stringify!($name), "` only exists on wasm32"))
            }
        }
        $crate::import! { $($rest)* }
    };
}

#[doc(hidden)]
#[macro_export]
macro_rules! __ret {
    () => { () };
    ($t:ty) => { $t };
}

/// Ship a JS module with a crate, reachable from snippets as `G.m(name)`.
///
/// ```ignore
/// web_glue::js_module!(pub(crate) fn dom_module = "my-crate/dom", include_str!("dom.js"));
///
/// web_glue::import! {
///     fn fast_append(parent: u32, child: u32) =
///         "(p, c) => G.m('my-crate/dom').append(G.get(p), G.get(c))";
/// }
/// fn append(p: &JsValue, c: &JsValue) {
///     dom_module(); // the anchor — see below
///     unsafe { fast_append(p.raw(), c.raw()) }
/// }
/// ```
///
/// The module source is the BODY of a function that receives `G` and
/// returns the module's value; it runs once, on first `G.m(name)`.
///
/// # The anchor rule
///
/// The source travels in a `#[link_section = "__idealyst_glue"]` static.
/// LLD concatenates every such section — but only from object files it
/// actually loads, and it loads an rlib member only when a symbol in it
/// is referenced. A bare static in a module nothing calls is silently
/// dropped (measured: a dependency's static in its own `mod glue` was
/// missing from both the debug and the release link, while one nested in
/// a called function was present in both). So the static is nested inside
/// the generated `#[inline(never)]` anchor fn, which puts it in the same
/// codegen unit — the same object — as the anchor's code (verified with
/// the static un-`#[used]`, in debug and release). **Call the
/// anchor on every path that uses the module**; the call is what makes
/// the object load. The body is one volatile read, so neither local
/// ThinLTO nor attribute inference can prove the call dead and delete
/// the reference.
#[macro_export]
macro_rules! js_module {
    ($vis:vis fn $anchor:ident = $name:literal, $src:expr $(,)?) => {
        #[inline(never)]
        $vis fn $anchor() {
            const __NAME: &str = $name;
            const __SRC: &str = $src;
            const __LEN: usize = $crate::record::len(__NAME, __SRC);
            // No `#[used]`: on wasm32 it makes rustc emit the bytes TWICE,
            // once in the custom section and once more in the data
            // section (measured, debug and release). The anchor is what
            // keeps the record linked.
            #[cfg(target_arch = "wasm32")]
            #[unsafe(link_section = "__idealyst_glue")]
            #[allow(dead_code)]
            static __RECORD: [u8; __LEN] =
                $crate::record::encode::<__LEN>($crate::record::KIND_MODULE, __NAME, __SRC);
            static __ANCHOR: u8 = 0;
            // SAFETY: a read of a valid, initialized static.
            let _ = unsafe { ::core::ptr::read_volatile(&__ANCHOR) };
        }
    };
}
