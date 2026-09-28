// #3: a sheet whose blocks never read their theme binding (`base(_t)`)
// did not mention the `<ThemeType>` at all in its expansion, so the
// `use` that brought the type into scope was reported unused — and a
// crate with `deny(unused_imports)` failed to build.
#![deny(unused_imports)]

mod theme {
    /// A vocabulary-free marker, as most sheets in the tree name.
    pub struct Marker;
}

use runtime_macros::stylesheet;
use theme::Marker;

stylesheet! {
    pub Panel<Marker> {
        base(_t) {
            padding: 8,
        }
    }
}

fn main() {
    let _ = Panel();
}
