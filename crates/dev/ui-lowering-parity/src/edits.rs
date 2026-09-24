//! Edit PAIRS: one site, written twice.
//!
//! The corpus in [`crate::fixtures`] proves that a `ui!` site builds
//! what it should. This one proves the other half of the overlay:
//!
//! ```text
//! Element(original) + apply(diff(desc(original), desc(edited))) == Element(edited)
//! ```
//!
//! Both bodies are COMPILED, so the suite has the two `Element`s a real
//! rebuild would produce, and both are kept as SOURCE, so the suite can
//! run the build-time producer on them exactly as the CLI would. That
//! closes the loop — parser, descriptor, diff, applier — with no dev
//! server anywhere in it.
//!
//! A pair may also be a REFUSAL: some edits cannot be patched (the code
//! changed), and the suite pins which ones, because "we refuse this"
//! silently becoming "we accept this and get it wrong" is the failure
//! that matters.

use runtime_macros::ui;
use runtime_vocabulary::glue::{signal, Element, Signal};

use crate::fixtures::Hinted;
use crate::{record, Mode, Recording};

/// One site, before and after an edit.
pub struct Pair {
    pub name: &'static str,
    /// The original body as source — what `desc()` parses.
    pub original_body: &'static str,
    /// The edited body as source.
    pub edited_body: &'static str,
    pub original: fn(Mode) -> Recording,
    pub edited: fn(Mode) -> Recording,
    /// Whether the diff is expected to produce a patch. A pair marked
    /// `false` is an edit the overlay must REFUSE — see the module docs.
    pub patchable: bool,
}

macro_rules! pair {
    (
        name = $name:ident;
        patchable = $patchable:literal;
        state { $( $sname:ident : $sty:ty = $sinit:expr ),* $(,)? }
        original { $($original:tt)* }
        edited { $($edited:tt)* }
    ) => {
        pub mod $name {
            #![allow(unused_variables, unused_imports, unused_mut, dead_code)]
            use super::*;

            pub struct St {
                $( pub $sname: Signal<$sty>, )*
            }

            pub fn make() -> St {
                St { $( $sname: signal($sinit), )* }
            }

            pub const DRIVES: &[(&'static str, fn(&St))] = &[];

            pub fn original(s: &St) -> Element {
                $( let $sname = s.$sname; )*
                ui! { $($original)* }
            }

            pub fn edited(s: &St) -> Element {
                $( let $sname = s.$sname; )*
                ui! { $($edited)* }
            }

            pub fn record_original(mode: Mode) -> Recording {
                record(make, original, DRIVES, mode)
            }

            pub fn record_edited(mode: Mode) -> Recording {
                record(make, edited, DRIVES, mode)
            }

            pub fn pair() -> Pair {
                Pair {
                    name: stringify!($name),
                    original_body: stringify!($($original)*),
                    edited_body: stringify!($($edited)*),
                    original: record_original,
                    edited: record_edited,
                    patchable: $patchable,
                }
            }
        }
    };
}

// ===========================================================================
// Patchable edits
// ===========================================================================

pair! {
    name = text_literal;
    patchable = true;
    state { }
    original { view() { text { "before" } } }
    edited { view() { text { "after" } } }
}

pair! {
    name = nested_text_literal;
    patchable = true;
    state { }
    original {
        view() {
            view() {
                text { "deep before" }
                text { "sibling" }
            }
        }
    }
    edited {
        view() {
            view() {
                text { "deep after" }
                text { "sibling" }
            }
        }
    }
}

pair! {
    name = a11y_literal;
    patchable = true;
    state { }
    original { view(a11y_label = "old") { text { "x" } } }
    edited { view(a11y_label = "new") { text { "x" } } }
}

pair! {
    name = child_appended;
    patchable = true;
    state { }
    original {
        view() {
            text { "one" }
        }
    }
    edited {
        view() {
            text { "one" }
            text { "two" }
        }
    }
}

pair! {
    name = child_removed;
    patchable = true;
    state { }
    original {
        view() {
            text { "one" }
            text { "two" }
        }
    }
    edited {
        view() {
            text { "one" }
        }
    }
}

pair! {
    name = children_reordered;
    patchable = true;
    state { }
    original {
        view() {
            text { "a" }
            text { "b" }
        }
    }
    edited {
        view() {
            text { "b" }
            text { "a" }
        }
    }
}

pair! {
    name = literal_beside_a_reactive_sibling;
    patchable = true;
    state { count: i32 = 0 }
    original {
        view() {
            text { "static before" }
            text { move || format!("n={}", count.get()) }
        }
    }
    edited {
        view() {
            text { "static after" }
            text { move || format!("n={}", count.get()) }
        }
    }
}

// A string literal in a conversion wrapper, edited under the SAME
// wrapper: the slot's literal is data, and the patch writes it the way
// the wrapper would have (`Some(…)` into an `Option<String>` field).

pair! {
    name = wrapped_some_to_string;
    patchable = true;
    state { }
    original { view() { Hinted(hint = Some("Search projects, headings, people...".to_string())) } }
    edited { view() { Hinted(hint = Some("Search everything...".to_string())) } }
}

pair! {
    name = wrapped_some_into;
    patchable = true;
    state { }
    original { view() { Hinted(hint = Some("before".into())) } }
    edited { view() { Hinted(hint = Some("after".into())) } }
}

pair! {
    name = wrapped_some_to_owned;
    patchable = true;
    state { }
    original { view() { Hinted(hint = Some("before".to_owned())) } }
    edited { view() { Hinted(hint = Some("after".to_owned())) } }
}

pair! {
    name = wrapped_some_string_from;
    patchable = true;
    state { }
    original { view() { Hinted(hint = Some(String::from("before"))) } }
    edited { view() { Hinted(hint = Some(String::from("after"))) } }
}

pair! {
    name = wrapped_string_from_on_a_primitive;
    patchable = true;
    state { }
    original { view() { button(label = String::from("Go"), on_click = || {}) } }
    edited { view() { button(label = String::from("Stop"), on_click = || {}) } }
}

// ===========================================================================
// Edits the overlay must REFUSE
// ===========================================================================

pair! {
    name = refused_wrapper_changed;
    patchable = false;
    state { }
    original { view() { Hinted(hint = Some("same".to_string())) } }
    edited { view() { Hinted(hint = Some("same".into())) } }
}

// Regression: a slot's code could change with nothing in the descriptor
// moving, and the diff produced NO edits — the save was dropped as "no
// UI or code change" and the page kept the old value.
pair! {
    name = refused_slot_code_changed;
    patchable = false;
    state { }
    original { view() { Hinted(hint = Some(["a", "b"].concat())) } }
    edited { view() { Hinted(hint = Some(["a", "c"].concat())) } }
}

pair! {
    name = refused_condition_changed;
    patchable = false;
    state { count: i32 = 1 }
    original {
        view() {
            if count.get() > 0 {
                text { "yes" }
            }
        }
    }
    edited {
        view() {
            if count.get() > 5 {
                text { "yes" }
            }
        }
    }
}

pair! {
    name = refused_reactive_content_changed;
    patchable = false;
    state { count: i32 = 0 }
    original {
        view() {
            text { move || format!("n={}", count.get()) }
        }
    }
    edited {
        view() {
            text { move || format!("count={}", count.get()) }
        }
    }
}

pair! {
    name = refused_prop_became_code;
    patchable = false;
    state { count: i32 = 7 }
    original { view() { text(content = "fixed") { } } }
    edited { view() { text(content = move || format!("{}", count.get())) { } } }
}

pair! {
    name = refused_static_child_added_beside_control_flow;
    patchable = false;
    state { count: i32 = 1 }
    original {
        view() {
            if count.get() > 0 {
                text { "a" }
            }
        }
    }
    edited {
        view() {
            if count.get() > 0 {
                text { "a" }
            }
            text { "b" }
        }
    }
}

/// Every pair in the corpus.
pub fn all() -> Vec<Pair> {
    vec![
        text_literal::pair(),
        nested_text_literal::pair(),
        a11y_literal::pair(),
        child_appended::pair(),
        child_removed::pair(),
        children_reordered::pair(),
        literal_beside_a_reactive_sibling::pair(),
        wrapped_some_to_string::pair(),
        wrapped_some_into::pair(),
        wrapped_some_to_owned::pair(),
        wrapped_some_string_from::pair(),
        wrapped_string_from_on_a_primitive::pair(),
        refused_wrapper_changed::pair(),
        refused_slot_code_changed::pair(),
        refused_condition_changed::pair(),
        refused_reactive_content_changed::pair(),
        refused_prop_became_code::pair(),
        refused_static_child_added_beside_control_flow::pair(),
    ]
}
