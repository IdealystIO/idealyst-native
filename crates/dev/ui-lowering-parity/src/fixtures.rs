//! The fixture corpus.
//!
//! Every entry is authored ONCE inside [`fixture!`] and expanded TWICE —
//! through `ui_lowered!(direct)` and `ui_lowered!(template)` — so the two
//! lowerings provably consume the same tokens. The macro also generates
//! the fixture's signal state, its `let`-prelude, and the drive list the
//! harness replays.
//!
//! Shape:
//!
//! ```ignore
//! fixture! {
//!     name = reactive_text;
//!     state { count: i32 = 0 }
//!     locals { }
//!     drive { "count=1" => |s| s.count.set(1) }
//!     body { text { move || format!("n={}", count.get()) } }
//! }
//! ```
//!
//! All four sections are mandatory (empty braces when unused): an
//! optional-section `macro_rules!` would have to guess, and a fixture
//! that silently drops its drive list would assert nothing.

use std::rc::Rc;

use runtime_macros::{component, stylesheet, ui, ui_lowered};
use runtime_vocabulary::glue::{
    memo, signal, Color, Easing, Element, Ref, Signal, Tokenized, ViewHandle,
};

use runtime_vocabulary::glue::primitives::flat_list::fixed_size;
use runtime_vocabulary::glue::primitives::overlay::{
    AnchorTarget, BackdropMode, ElementSide, ViewportPlacement,
};
use runtime_vocabulary::glue::primitives::presence::PresenceAnim;

use crate::{record, Fixture, Mode, Recording};

// ===========================================================================
// The fixture macro
// ===========================================================================

macro_rules! fixture {
    (
        name = $name:ident;
        state { $( $sname:ident : $sty:ty = $sinit:expr ),* $(,)? }
        locals { $($locals:tt)* }
        drive { $( $dlabel:literal => |$dbind:ident| $dbody:expr ),* $(,)? }
        body { $($body:tt)* }
    ) => {
        pub mod $name {
            #![allow(unused_variables, unused_imports, unused_mut, dead_code)]
            use super::*;

            /// The signals this fixture exposes to the driver.
            pub struct St {
                $( pub $sname: Signal<$sty>, )*
            }

            pub fn make() -> St {
                St { $( $sname: signal($sinit), )* }
            }

            pub const DRIVES: &[(&'static str, fn(&St))] = &[
                $( ($dlabel, |$dbind: &St| { let _ = $dbind; $dbody; }), )*
            ];

            pub fn direct(s: &St) -> Element {
                $( let $sname = s.$sname; )*
                $($locals)*
                ui_lowered!(direct { $($body)* })
            }

            /// The TEMPLATE expansion of the same tokens. Gated so
            /// phase 1 (no template emitter yet) still compiles; phase 2
            /// turns the feature on for good.
            #[cfg(feature = "template")]
            pub fn template(s: &St) -> Element {
                $( let $sname = s.$sname; )*
                $($locals)*
                ui_lowered!(template { $($body)* })
            }

            pub fn record_direct(mode: Mode) -> Recording {
                record(make, direct, DRIVES, mode)
            }

            #[cfg(feature = "template")]
            pub fn record_template(mode: Mode) -> Recording {
                record(make, template, DRIVES, mode)
            }

            pub fn fixture() -> Fixture {
                Fixture {
                    name: stringify!($name),
                    direct: record_direct,
                    #[cfg(feature = "template")]
                    template: Some(record_template),
                    #[cfg(not(feature = "template"))]
                    template: None,
                }
            }
        }
    };
}

// ===========================================================================
// Support declarations the fixtures build on
// ===========================================================================

stylesheet! {
    pub Panel<()> {
        base(_t) {
            padding: 8,
            background: Tokenized::token("color-surface", Color("#101010".into())),
        }
        variant size {
            #[default]
            medium(_t) {}
            large(_t) { padding: 16 }
        }
    }
}

/// Literal-prop component: every prop is a descriptor literal, so the
/// template lowering can drive it entirely from data.
#[component]
fn Badge(
    /// Reactive-by-default text prop.
    label: String,
    /// Integer prop with a per-arg default.
    #[prop(default = 3)]
    count: i32,
    /// Static boolean.
    #[prop(static, default = false)]
    loud: bool,
) -> Element {
    ui! {
        view {
            text { move || format!("{}:{}", label.get(), count.get()) }
            if loud {
                text { "!" }
            }
        }
    }
}

/// Container component: receives `children` and flattens them, the one
/// sanctioned `Vec<Element>` shape (repo rule 9.3).
#[component]
fn Frame(
    /// Heading text.
    title: String,
    children: Vec<Element>,
) -> Element {
    ui! {
        view {
            text { move || title.get() }
            children
        }
    }
}

/// Component with a non-literal prop (a signal handle) plus an optional
/// callback bound only when present (repo rule 9.6).
#[component]
fn Counter(
    /// The live count.
    value: Signal<i32>,
    on_bump: Option<Rc<dyn Fn()>>,
) -> Element {
    ui! {
        view {
            text { move || format!("v={}", value.get()) }
            if let Some(cb) = on_bump {
                button(label = "bump", on_click = move || (cb)())
            }
        }
    }
}

/// A plain enum for the `match` fixtures. (Named `Phase`, not `Mode`:
/// `crate::Mode` is the harness's anchored/spliced switch.)
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Phase {
    Idle,
    Busy,
    Done,
}

/// A keyed row payload.
#[derive(Clone, PartialEq)]
pub struct Row {
    pub id: u32,
    pub label: String,
}

/// File-local helper returning an `Element` — the bare-expression child
/// shape (repo rule 9.5: no props, one call site).
fn footer() -> Element {
    ui! { text { "footer" } }
}

// ===========================================================================
// Fixtures — static structure
// ===========================================================================

fixture! {
    name = static_nested_views;
    state { }
    locals { }
    drive { }
    body {
        view {
            view {
                text { "a" }
                text { "b" }
            }
            view {
                text { "c" }
            }
        }
    }
}

fixture! {
    name = multi_root_nodes;
    state { }
    locals { }
    drive { }
    body {
        text { "one" }
        text { "two" }
        text { "three" }
    }
}

fixture! {
    name = literal_props_and_identity;
    state { }
    locals { }
    drive { }
    body {
        view(test_id = "root") {
            text(test_id = "label") { "hello" }
            button(label = "press", on_click = || {})
        }
    }
}

fixture! {
    name = a11y_attrs;
    state { }
    locals { }
    drive { }
    body {
        view(a11y_label = "region", a11y_hint = "does a thing", a11y_hidden = false) {
            text(a11y_label = "the text") { "x" }
        }
    }
}

fixture! {
    name = static_style_sheet;
    state { }
    locals { }
    drive { }
    body {
        view(style = Panel()) {
            text { "styled" }
        }
    }
}

fixture! {
    name = style_sheet_variant;
    state { }
    locals { }
    drive { }
    body {
        view(style = Panel().size(PanelSize::Large)) {
            text { "big" }
        }
    }
}

// ===========================================================================
// Fixtures — text
// ===========================================================================

fixture! {
    name = reactive_text_closure;
    state { count: i32 = 0 }
    locals { }
    drive { "count=1" => |s| s.count.set(1), "count=7" => |s| s.count.set(7) }
    body {
        view {
            text { move || format!("n={}", count.get()) }
        }
    }
}

fixture! {
    name = fstring_text;
    state { count: i32 = 2 }
    locals { }
    drive { "count=9" => |s| s.count.set(9) }
    body {
        view {
            text { "{count} items" }
            text { "static prose" }
        }
    }
}

fixture! {
    name = fstring_with_spec;
    state { ratio: f32 = 0.5 }
    locals { }
    drive { "ratio=0.25" => |s| s.ratio.set(0.25) }
    body {
        text { "{ratio:.2} done" }
    }
}

fixture! {
    name = text_content_prop;
    state { }
    locals { let label = String::from("via-prop"); }
    drive { }
    body {
        text(content = label)
    }
}

fixture! {
    name = memo_text;
    state { count: i32 = 1 }
    locals { let doubled = memo(move || count.get() * 2); }
    drive { "count=4" => |s| s.count.set(4) }
    body {
        text { move || format!("d={}", doubled.get()) }
    }
}

// ===========================================================================
// Fixtures — components
// ===========================================================================

fixture! {
    name = component_literal_props;
    state { }
    locals { }
    drive { }
    body {
        Badge(label = "alpha", count = 5, loud = true)
    }
}

fixture! {
    name = component_default_props;
    state { }
    locals { }
    drive { }
    body {
        Badge(label = "beta")
    }
}

fixture! {
    name = component_dynamic_props;
    state { count: i32 = 0 }
    locals { let bump: Option<Rc<dyn Fn()>> = Some(Rc::new(move || {}) as Rc<dyn Fn()>); }
    drive { "count=3" => |s| s.count.set(3) }
    body {
        Counter(value = count, on_bump = bump)
    }
}

fixture! {
    name = component_children_splat;
    state { }
    locals { }
    drive { }
    body {
        Frame(title = "outer") {
            text { "inner-a" }
            text { "inner-b" }
        }
    }
}

fixture! {
    name = nested_components;
    state { count: i32 = 0 }
    locals { }
    drive { "count=2" => |s| s.count.set(2) }
    body {
        Frame(title = "shell") {
            Badge(label = "nested", count = 1)
            Counter(value = count)
            Frame(title = "deeper") {
                text { "leaf" }
            }
        }
    }
}

fixture! {
    name = bare_expression_children;
    state { }
    locals { let extra: Vec<Element> = vec![ui! { text { "spliced" } }]; }
    drive { }
    body {
        view {
            footer()
            extra
        }
    }
}

// ===========================================================================
// Fixtures — control flow
// ===========================================================================

fixture! {
    name = reactive_if;
    state { flag: bool = false }
    locals { }
    drive { "flag=true" => |s| s.flag.set(true), "flag=false" => |s| s.flag.set(false) }
    body {
        view {
            if flag.get() {
                text { "on" }
            } else {
                text { "off" }
            }
        }
    }
}

fixture! {
    name = reactive_if_no_else;
    state { flag: bool = false }
    locals { }
    drive { "flag=true" => |s| s.flag.set(true) }
    body {
        view {
            text { "always" }
            if flag.get() {
                text { "sometimes" }
            }
        }
    }
}

fixture! {
    name = reactive_if_else_chain;
    state { n: i32 = 0 }
    locals { }
    drive { "n=1" => |s| s.n.set(1), "n=2" => |s| s.n.set(2) }
    body {
        view {
            if n.get() == 0 {
                text { "zero" }
            } else if n.get() == 1 {
                text { "one" }
            } else {
                text { "many" }
            }
        }
    }
}

fixture! {
    name = static_if;
    state { }
    locals { let show = true; let hide = false; }
    drive { }
    body {
        view {
            if show {
                text { "shown" }
            }
            if hide {
                text { "never" }
            } else {
                text { "fallback" }
            }
        }
    }
}

fixture! {
    name = if_let_binding;
    state { }
    locals { let maybe: Option<String> = Some("bound".to_string()); let none: Option<u8> = None; }
    drive { }
    body {
        view {
            if let Some(v) = maybe {
                text { v }
            }
            if let Some(n) = none {
                text { move || format!("{n}") }
            } else {
                text { "empty" }
            }
        }
    }
}

fixture! {
    name = static_match;
    state { }
    locals { let mode = Phase::Busy; }
    drive { }
    body {
        view {
            match mode {
                Phase::Idle => { text { "idle" } }
                Phase::Busy => {
                    text { "busy" }
                    text { "spinner" }
                }
                Phase::Done => { text { "done" } }
            }
        }
    }
}

fixture! {
    name = reactive_match;
    state { mode: Phase = Phase::Idle }
    locals { }
    drive { "mode=Busy" => |s| s.mode.set(Phase::Busy), "mode=Done" => |s| s.mode.set(Phase::Done) }
    body {
        view {
            match mode.get() {
                Phase::Idle => { text { "idle" } }
                Phase::Busy => { text { "busy" } }
                Phase::Done => { text { "done" } }
            }
        }
    }
}

fixture! {
    name = match_with_guard;
    state { n: i32 = 0 }
    locals { }
    drive { "n=5" => |s| s.n.set(5) }
    body {
        view {
            match n.get() {
                x if *x > 3 => { text { "big" } }
                _ => { text { "small" } }
            }
        }
    }
}

fixture! {
    name = for_static_vec;
    state { }
    locals { let items: Vec<&'static str> = vec!["a", "b", "c"]; }
    drive { }
    body {
        view {
            for it in items {
                text { it }
            }
        }
    }
}

fixture! {
    name = for_static_range_repeat;
    state { }
    locals { }
    drive { }
    body {
        view {
            for i in 0..3 {
                text { move || format!("row {i}") }
            }
        }
    }
}

fixture! {
    name = for_reactive_range;
    state { n: usize = 2 }
    locals { }
    drive { "n=4" => |s| s.n.set(4), "n=1" => |s| s.n.set(1) }
    body {
        view {
            for i in 0..n.get() {
                text { move || format!("r{i}") }
            }
        }
    }
}

fixture! {
    name = for_keyed_reactive;
    state { rows: Vec<Row> = vec![
        Row { id: 1, label: "one".to_string() },
        Row { id: 2, label: "two".to_string() },
    ] }
    locals { }
    drive {
        "append" => |s| {
            let mut v = s.rows.get();
            v.push(Row { id: 3, label: "three".to_string() });
            s.rows.set(v);
        },
        "remove-middle" => |s| {
            let mut v = s.rows.get();
            v.retain(|r| r.id != 2);
            s.rows.set(v);
        }
    }
    body {
        view {
            for row in rows, key = row.id {
                text { row.label.clone() }
            }
        }
    }
}

fixture! {
    name = for_keyed_multi_node_rows;
    state { rows: Vec<Row> = vec![Row { id: 1, label: "a".to_string() }] }
    locals { }
    drive {
        "append" => |s| {
            let mut v = s.rows.get();
            v.push(Row { id: 2, label: "b".to_string() });
            s.rows.set(v);
        }
    }
    body {
        view {
            for row in rows, key = row.id {
                text { row.label.clone() }
                text { "-" }
            }
        }
    }
}

fixture! {
    name = nested_control_flow;
    state { flag: bool = true, rows: Vec<Row> = vec![Row { id: 1, label: "x".to_string() }] }
    locals { }
    drive { "flag=false" => |s| s.flag.set(false) }
    body {
        view {
            if flag.get() {
                for row in rows, key = row.id {
                    view {
                        text { row.label.clone() }
                    }
                }
            } else {
                text { "hidden" }
            }
        }
    }
}

fixture! {
    name = when_tag;
    state { flag: bool = false }
    locals { }
    drive { "flag=true" => |s| s.flag.set(true) }
    body {
        view {
            when(
                cond = move || flag.get(),
                then = move || ui! { text { "yes" } },
                otherwise = move || ui! { text { "no" } }
            )
        }
    }
}

// ===========================================================================
// Fixtures — the primitive set
// ===========================================================================

fixture! {
    name = input_primitives;
    state { draft: String = String::new(), on: bool = false, amount: f32 = 0.5 }
    locals { }
    drive {
        "draft=hi" => |s| s.draft.set("hi".to_string()),
        "on=true" => |s| s.on.set(true),
        "amount=0.9" => |s| s.amount.set(0.9)
    }
    body {
        view {
            text_input(value = draft, on_change = move |v: String| draft.set(v), placeholder = "type")
            toggle(value = on, on_change = move |v: bool| on.set(v))
            slider(value = amount, on_change = move |v: f32| amount.set(v), min = 0.0, max = 1.0, step = 0.1)
        }
    }
}

fixture! {
    name = media_primitives;
    state { }
    locals { }
    drive { }
    body {
        view {
            image(src = "https://example.test/x.png", alt = "an image")
            activity_indicator()
            link(external = "https://example.test") {
                text { "out" }
            }
        }
    }
}

fixture! {
    name = scroll_view_primitive;
    state { }
    locals { }
    drive { }
    body {
        scroll_view(horizontal = true, bounces = false) {
            text { "scrolled" }
        }
    }
}

fixture! {
    name = method_chain_bind;
    state { }
    locals { let handle: Ref<ViewHandle> = Ref::default(); }
    drive { }
    body {
        view {
            text { "chained" }
        }.bind(handle.clone())
    }
}

fixture! {
    name = overlay_and_presence;
    state { open: bool = false, toast: bool = false }
    locals { let anchor = AnchorTarget::from(Ref::<ViewHandle>::default()); }
    drive { "open=true" => |s| s.open.set(true), "toast=true" => |s| s.toast.set(true) }
    body {
        view {
            if open.get() {
                overlay(
                    placement = ViewportPlacement::Center,
                    backdrop = BackdropMode::Dismiss,
                    on_dismiss = move || open.set(false),
                    trap_focus = true
                ) {
                    text { "modal" }
                }
                anchored_overlay(target = anchor.clone(), side = ElementSide::Below) {
                    text { "tip" }
                }
            }
            presence(
                present = move || toast.get(),
                enter = PresenceAnim::fade(100, Easing::EaseOut),
                exit = PresenceAnim::fade(100, Easing::EaseIn)
            ) {
                text { "toast" }
            }
        }
    }
}

fixture! {
    name = flat_list_primitive;
    state { rows: Vec<Row> = vec![
        Row { id: 1, label: "l1".to_string() },
        Row { id: 2, label: "l2".to_string() },
    ] }
    locals { }
    drive {
        "append" => |s| {
            let mut v = s.rows.get();
            v.push(Row { id: 3, label: "l3".to_string() });
            s.rows.set(v);
        }
    }
    body {
        flat_list(
            data = rows,
            key = |_i, r: &Row| r.id as u64,
            size = fixed_size(24.0),
            render = |_i, r: &Row| ui! { text { r.label.clone() } }.into()
        )
    }
}

fixture! {
    name = button_arrow_action;
    state { count: i32 = 0, out: i32 = 0 }
    locals { }
    drive { }
    body {
        view {
            button(label = "go", on_click = double(count) => out)
        }
    }
}

/// The `on_click = method(sig) => out` shape's callee: a single-segment
/// fn over bare signal args (see `ui.rs`'s `reactive_call_with_gets`).
fn double(n: i32) -> i32 {
    n * 2
}

// ---------------------------------------------------------------------------
// The `scene-parity` structural corpus, re-authored in `ui!`
//
// `crates/dev/scene-parity` pins the frozen op sequences for 13 reactive
// scenarios — but it builds them with `runtime_scene`'s constructors
// directly and contains no `ui!` at all, so there is nothing there for a
// LOWERING to be applied to. These fixtures are the missing half: the
// same reactive shapes (`each_reverse`,
// `each_insert_middle_survivors`, `switch_rotation`,
// `nested_when_in_each_row`) authored through `ui!`, so both lowerings
// are measured against them.
// ---------------------------------------------------------------------------

fixture! {
    name = for_keyed_reorder_and_insert;
    state { rows: Vec<Row> = vec![
        Row { id: 1, label: "a".to_string() },
        Row { id: 2, label: "b".to_string() },
        Row { id: 3, label: "c".to_string() },
    ] }
    locals { }
    drive {
        "reverse" => |s| {
            let mut v = s.rows.get();
            v.reverse();
            s.rows.set(v);
        },
        "insert-middle" => |s| {
            let mut v = s.rows.get();
            v.insert(1, Row { id: 9, label: "mid".to_string() });
            s.rows.set(v);
        },
        "remove-first" => |s| {
            let mut v = s.rows.get();
            v.remove(0);
            s.rows.set(v);
        }
    }
    body {
        view {
            for row in rows, key = row.id {
                text { row.label.clone() }
            }
        }
    }
}

fixture! {
    name = reactive_if_in_keyed_row;
    state {
        rows: Vec<Row> = vec![
            Row { id: 1, label: "one".to_string() },
            Row { id: 2, label: "two".to_string() },
        ],
        expanded: bool = false,
    }
    locals { }
    drive {
        "expand" => |s| s.expanded.set(true),
        "append" => |s| {
            let mut v = s.rows.get();
            v.push(Row { id: 3, label: "three".to_string() });
            s.rows.set(v);
        },
        "collapse" => |s| s.expanded.set(false)
    }
    body {
        view {
            for row in rows, key = row.id {
                view {
                    text { row.label.clone() }
                    if expanded.get() {
                        text { "detail" }
                    }
                }
            }
        }
    }
}

fixture! {
    name = reactive_match_rotation;
    state { phase: Phase = Phase::Idle }
    locals { }
    drive {
        "to-Busy" => |s| s.phase.set(Phase::Busy),
        "to-Done" => |s| s.phase.set(Phase::Done),
        "back-to-Idle" => |s| s.phase.set(Phase::Idle),
        "to-Busy-again" => |s| s.phase.set(Phase::Busy)
    }
    body {
        view {
            match phase.get() {
                Phase::Idle => { text { "idle" } }
                Phase::Busy => {
                    text { "busy" }
                    text { "spinner" }
                }
                Phase::Done => { text { "done" } }
            }
        }
    }
}

// ===========================================================================
// Registry
// ===========================================================================

/// Every fixture, in declaration order.
pub fn all() -> Vec<Fixture> {
    vec![
        static_nested_views::fixture(),
        multi_root_nodes::fixture(),
        literal_props_and_identity::fixture(),
        a11y_attrs::fixture(),
        static_style_sheet::fixture(),
        style_sheet_variant::fixture(),
        reactive_text_closure::fixture(),
        fstring_text::fixture(),
        fstring_with_spec::fixture(),
        text_content_prop::fixture(),
        memo_text::fixture(),
        component_literal_props::fixture(),
        component_default_props::fixture(),
        component_dynamic_props::fixture(),
        component_children_splat::fixture(),
        nested_components::fixture(),
        bare_expression_children::fixture(),
        reactive_if::fixture(),
        reactive_if_no_else::fixture(),
        reactive_if_else_chain::fixture(),
        static_if::fixture(),
        if_let_binding::fixture(),
        static_match::fixture(),
        reactive_match::fixture(),
        match_with_guard::fixture(),
        for_static_vec::fixture(),
        for_static_range_repeat::fixture(),
        for_reactive_range::fixture(),
        for_keyed_reactive::fixture(),
        for_keyed_multi_node_rows::fixture(),
        nested_control_flow::fixture(),
        when_tag::fixture(),
        input_primitives::fixture(),
        media_primitives::fixture(),
        scroll_view_primitive::fixture(),
        method_chain_bind::fixture(),
        overlay_and_presence::fixture(),
        flat_list_primitive::fixture(),
        button_arrow_action::fixture(),
        for_keyed_reorder_and_insert::fixture(),
        reactive_if_in_keyed_row::fixture(),
        reactive_match_rotation::fixture(),
    ]
}
