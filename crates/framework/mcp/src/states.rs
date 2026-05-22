//! Hand-curated registration table for [`StateEntry`].
//!
//! The four interaction states the `stylesheet!` macro accepts in
//! `state <name>(theme) { ... }` arms — locked because adding a new
//! state name would silently never activate on backends that didn't
//! learn about it. See the parser in
//! `framework_macros::stylesheet` for the same whitelist.

use crate::StateEntry;

const ALL_BACKENDS: &[&str] = &["ios", "android", "web", "macos"];
const POINTER_BACKENDS: &[&str] = &["web", "macos"];
const KEYBOARD_BACKENDS: &[&str] = &["web", "macos", "android", "ios"];

inventory::submit! {
    StateEntry {
        name: "hovered",
        docs: "Pointer is over the element. Fires on backends with a real pointer (mouse / trackpad). Silent on touch-only mobile — use `pressed` for the universal tap-feedback path.",
        backends: POINTER_BACKENDS,
        _seal: (),
    }
}

inventory::submit! {
    StateEntry {
        name: "pressed",
        docs: "Element is being actively pressed / clicked. Universal — fires on every backend (touch down on mobile, mouse down on desktop/web). Pair with `Button`/`Pressable` for the canonical tap-feedback overlay.",
        backends: ALL_BACKENDS,
        _seal: (),
    }
}

inventory::submit! {
    StateEntry {
        name: "focused",
        docs: "Element has keyboard focus. Fires when the user tabs to a `TextInput`, `Button`, etc. Critical for accessibility — the `state focused { ... }` overlay is where focus rings live.",
        backends: KEYBOARD_BACKENDS,
        _seal: (),
    }
}

inventory::submit! {
    StateEntry {
        name: "disabled",
        docs: "Element is marked inert via the `disabled` reactive flag (currently only on `Button`). Universal — every backend honors the `DISABLED` state bit and the corresponding native inert flag (`disabled` attr on web, `setEnabled(false)` on native).",
        backends: ALL_BACKENDS,
        _seal: (),
    }
}
