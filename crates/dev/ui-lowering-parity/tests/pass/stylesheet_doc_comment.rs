// Wave-42 / #2: `stylesheet!` rejected a `///` doc comment (and any
// other outer attribute) on the sheet it declares — the parser read a
// visibility and then demanded an identifier. The doc must be accepted
// and land on the generated builder; lint/cfg attributes forward too.
// `missing_docs` is denied so the rest of the expansion must carry docs
// of its own: a documented sheet is only useful if the crate can then
// require docs everywhere.
#![deny(missing_docs, dead_code)]
//! Crate docs, so `missing_docs` has nothing else to say.

use runtime_macros::stylesheet;

stylesheet! {
    /// The card surface every panel sits on.
    #[allow(dead_code)]
    pub Card<()> {
        base(_t) {
            padding: 8,
        }
        variant size {
            #[default]
            medium(_t) {}
            large(_t) { padding: 16 }
        }
        override padding: f32
    }
}

stylesheet! {
    /// Compiled out entirely: `cfg` reaches every generated item, or
    /// the builder's `impl` would reference a struct that isn't there.
    #[cfg(any())]
    pub Gone<()> {
        base(_t) { padding: 1 }
    }
}

fn main() {}
