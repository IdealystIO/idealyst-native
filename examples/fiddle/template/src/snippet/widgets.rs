//! Auto-generated per /compile request — overwritten on every build.

#![allow(unused_imports)]
#![allow(dead_code)]

use crate::__rt::*;

// Sibling module of `lib.rs`. The shared `use crate::__rt::*;`
// prelude is injected by the fiddle server, so framework types
// and `ui!` / `stylesheet!` are already in scope.

pub fn title(label: &str) -> Primitive {
    // idea-ui's `Heading` is a styled-text component; it takes its
    // string via the `content` prop, not as a `{ ... }` body.
    let text = label.to_string();
    ui! { Heading(content = text, kind = HeadingKind::H1) }
}

