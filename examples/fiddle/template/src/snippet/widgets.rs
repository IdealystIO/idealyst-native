//! Auto-generated per /compile request — overwritten on every build.

#![allow(unused_imports)]
#![allow(dead_code)]

use crate::__rt::*;

// Sibling module of `lib.rs`. The shared `use crate::__rt::*;`
// prelude is injected by the fiddle server, so framework types
// and `ui!` / `stylesheet!` are already in scope.

pub struct TitleProps {
    pub label: String,
}

#[component]
pub fn title(props: &TitleProps) -> Primitive {
    // idea-ui's `Heading` is a styled-text component; it takes its
    // string via the `content` prop, not as a `{ ... }` body.
    let label = props.label.clone();
    ui! { Typography(content = label, kind = idea_ui::typography_kind::H1.into()) }
}

