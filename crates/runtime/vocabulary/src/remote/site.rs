//! Records a bundle leaves of what it uses from the app, read by the
//! release build to list what the bundle requires (`build-remote`).
//!
//! Each place a bundle reaches into the app (a `ui!` call of an app
//! component, a host function's stub, a remote component's mount export,
//! a context type) holds a [`Site`]: a constant in the bundle's data, with
//! a magic prefix, naming what it uses and pointing at a function that
//! writes the shapes involved ([`super::shape`]). The code that uses the
//! thing also uses its record (`mark`), so the linker keeps exactly the
//! records of reachable code and drops the rest with the dead code: a
//! component referenced only from a function nothing calls is not
//! required. The build tool finds the records by their magic in the data
//! segments, and runs each shape function once, in an interpreter.
//!
//! Why not a custom section, like the codec version: a custom section keeps
//! whatever was compiled, reachable or not, and a release build compiles
//! every helper of every library a bundle depends on. And why not compute
//! the list when the bundle runs: the list has to be known before an app
//! downloads the bundle.
//!
//! The layout is `#[repr(C)]` on wasm32, 40 bytes, every field 4-byte
//! aligned; the build tool reads it by offset (`build_remote::requires`).
//! Changing it means changing [`MAGIC`]'s version byte and the reader.

/// The first 16 bytes of every record. The last-but-one byte is the
/// layout's version.
pub const MAGIC: [u8; 16] = *b"\xffidealyst-site\x01\x00";

/// A use of an app component: `key` is its name, `set` the props the
/// call site sets, each followed by `\n` (or `*\n`: every prop), and
/// `shapes` writes `prop\tshape\n` for every prop of the bundle's copy.
pub const COMPONENT: u32 = 1;
/// A host function's stub: `key` is its import name, `shapes` writes the
/// signature's shape.
pub const HOST_FN: u32 = 2;
/// A remote component the bundle provides: `key` is its name, `shapes`
/// writes `param\tshape\n` for its parameters, in order.
pub const REMOTE: u32 = 3;
/// A context type: `key` is its name, `shapes` its shape.
pub const CONTEXT: u32 = 4;

/// A `&'static str` as two words, laid out the same in every compiler
/// version (a `&str`'s own layout isn't specified).
#[repr(C)]
pub struct Str {
    ptr: *const u8,
    len: usize,
}

// SAFETY: points at immutable `'static` bytes.
unsafe impl Sync for Str {}

impl Str {
    pub const fn new(s: &'static str) -> Self {
        Str { ptr: s.as_ptr(), len: s.len() }
    }
}

#[repr(C)]
pub struct Site {
    pub magic: [u8; 16],
    pub kind: u32,
    pub key: Str,
    pub set: Str,
    /// Writes the shapes; returns `ptr << 32 | len` of the text, which it
    /// leaks (the build tool calls each once, in a throwaway instance).
    pub shapes: extern "C" fn() -> u64,
}

// SAFETY: plain constants.
unsafe impl Sync for Site {}

impl Site {
    pub const fn new(kind: u32, key: &'static str, set: &'static str, shapes: extern "C" fn() -> u64) -> Self {
        Site { magic: MAGIC, kind, key: Str::new(key), set: Str::new(set), shapes }
    }
}

#[cfg(target_arch = "wasm32")]
const _: () = assert!(core::mem::size_of::<Site>() == 40);

/// Keep `site` in the binary for as long as the code calling this is.
/// Without the opaque use, the optimizer would read the fields it needs
/// straight out of the constant and drop the record.
#[inline(always)]
pub fn mark(site: &'static Site) {
    core::hint::black_box(site);
}

/// Hand `text` to the build tool: `ptr << 32 | len`, leaked.
#[doc(hidden)]
pub fn leak(text: String) -> u64 {
    let text: &'static str = Box::leak(text.into_boxed_str());
    ((text.as_ptr() as usize as u64) << 32) | text.len() as u64
}

/// `prop\tshape\n` lines, from `(name, shape)` pairs.
#[doc(hidden)]
pub fn lines(pairs: &[(&'static str, String)]) -> String {
    let mut out = String::new();
    for (name, shape) in pairs {
        out.push_str(name);
        out.push('\t');
        out.push_str(shape);
        out.push('\n');
    }
    out
}
