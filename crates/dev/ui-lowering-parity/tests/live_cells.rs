//! A component's literal prop, live through its CELL.
//!
//! The bug: `Typography(content = "Hello")` / `Button(label = "Save")` —
//! most of the text in an idea-ui app — took the overlay tier, and the
//! screen did not change: the dev server said "waiting for the next
//! render". A component's props are consumed when its body runs, and the
//! only live path was re-running it from a `Clone` copy, which idea-ui's
//! props types (and every inline-props `#[component]`) do not have.
//!
//! The fix: under `ui-overlay`, a prop the call site wrote as a literal,
//! of a `Reactive` type, reaches the component as a signal-backed
//! `Reactive::Dynamic` registered under `(site, node, prop)`
//! (`runtime_vocabulary::overlay::cells`). A patch writes the signal and
//! the component's own bindings update. These tests pin what that must
//! and must not do. The components below are deliberately NOT `Clone`:
//! before the cells every one of these patches was refused.

#![cfg(feature = "ui-overlay")]

use std::borrow::Cow;
use std::cell::Cell;

use runtime_macros::{component, ui};
use runtime_template::{Edit, LiteralValue};
use runtime_vocabulary::glue::{signal, Element, Signal};
use runtime_vocabulary::overlay;
use ui_lowering_parity::{mount_scene, scene_shape, Mounted};

fn set(node: u32, name: &'static str, value: LiteralValue) -> Edit {
    Edit::SetProp { node, name: Cow::Borrowed(name), value }
}

fn str_lit(v: &'static str) -> LiteralValue {
    LiteralValue::Str(Cow::Borrowed(v))
}

/// The shape of idea-ui's `Button`: a `Reactive<String>` rendered by a
/// text binding INSIDE the component's own root node. Inline props, so
/// not `Clone`.
///
/// Not the shape of `Typography`, whose root IS the text node: there a
/// `content` edit happened to reach the root primitive's own setter even
/// before the cells, because the call site's tag lands on that root. A
/// prop whose name no primitive setter shares is the case that was
/// genuinely unreachable, and the one these tests need to fail without
/// the fix.
#[component]
fn Label(label: String) -> Element {
    ui! { view() { text { label } } }
}

fn label(v: &'static str) -> impl FnOnce() -> Element {
    move || match v {
        "before" => ui! { view() { Label(label = "before") } },
        _ => ui! { view() { Label(label = "after") } },
    }
}

/// Regression: the user's edit. A non-`Clone` component's literal prop
/// applies live and the screen matches what a recompile would show.
#[test]
fn regression_a_non_clone_components_literal_prop_applies_live() {
    overlay::reset();
    let mut mounted = Mounted::new(label("before"));
    let site = mounted.site();

    let outcome = mounted.apply_live(site, &[set(1, "label", str_lit("after"))]);

    assert_eq!(outcome.applied, 1, "{outcome:?}");
    assert_eq!(outcome.refused, 0, "it used to be refused: {outcome:?}");
    assert_eq!(
        scene_shape(&mounted.scene()),
        scene_shape(&mount_scene(label("after"))),
        "a cell-applied prop must be indistinguishable from a recompile"
    );
    overlay::reset();
}

thread_local! {
    static COUNT: Cell<Option<Signal<i32>>> = const { Cell::new(None) };
}

/// A component with local state beside its literal prop.
#[component]
fn Counter(label: String) -> Element {
    let n = signal(0i32);
    COUNT.with(|c| c.set(Some(n)));
    ui! {
        view() {
            text { label }
            text { "count {n}" }
        }
    }
}

/// The reason cells beat a rebuild: nothing is re-run, so the
/// component's own state is exactly where the author left it.
#[test]
fn the_component_keeps_its_local_state() {
    overlay::reset();
    let mut mounted = Mounted::new(|| ui! { view() { Counter(label = "Clicks") } });
    let n = COUNT.with(|c| c.get()).expect("the counter ran");
    mounted.drive(|| n.set(5));
    assert!(mounted.scene().contains("count 5"), "{}", mounted.scene());

    let site = mounted.site();
    let outcome = mounted.apply_live(site, &[set(1, "label", str_lit("Taps"))]);

    assert_eq!(outcome.applied, 1, "{outcome:?}");
    let scene = mounted.scene();
    assert!(scene.contains("Taps"), "{scene}");
    assert!(!scene.contains("Clicks"), "{scene}");
    assert!(scene.contains("count 5"), "state must survive the edit: {scene}");
    assert!(n.is_alive(), "the component was not re-run");
    let again = COUNT.with(|c| c.get()).expect("still set");
    assert_eq!(again.raw_id(), n.raw_id(), "no second body run minted a new signal");
    overlay::reset();
}

/// `#[prop(static)]` opts a prop out of `Reactive`, so it has no cell:
/// the edit keeps the old path and is REPORTED as waiting, never as
/// applied.
#[component]
fn Fixed(#[prop(static)] label: String) -> Element {
    ui! { text { label } }
}

#[test]
fn a_static_prop_has_no_cell_and_is_still_refused() {
    overlay::reset();
    let mut mounted = Mounted::new(|| ui! { view() { Fixed(label = "before") } });
    assert_eq!(overlay::cells::count(), 0, "a static prop must not get a cell");
    let before = mounted.scene();

    let site = mounted.site();
    let outcome = mounted.apply_live(site, &[set(1, "label", str_lit("after"))]);

    assert_eq!((outcome.applied, outcome.refused), (0, 1), "{outcome:?}");
    assert_eq!(mounted.scene(), before);
    overlay::reset();
}

/// A body that reads its prop ONCE while building bakes the value in.
/// The prop HAS a cell, but writing it would change nothing on screen —
/// and reporting that as applied is exactly the lie the overlay must not
/// tell. The kernel's subscriber count is what catches it.
#[component]
fn Baked(label: String) -> Element {
    let shown = format!("[{}]", label.get());
    ui! { text { shown } }
}

#[test]
fn a_baked_prop_is_refused_not_reported_applied() {
    overlay::reset();
    let mut mounted = Mounted::new(|| ui! { view() { Baked(label = "before") } });
    assert_eq!(overlay::cells::count(), 1, "the literal prop did get a cell");
    let before = mounted.scene();

    let site = mounted.site();
    let outcome = mounted.apply_live(site, &[set(1, "label", str_lit("after"))]);

    assert_eq!((outcome.applied, outcome.refused), (0, 1), "{outcome:?}");
    assert_eq!(mounted.scene(), before);
    overlay::reset();
}

/// A prop the call site passed as CODE is not a patch target and gets no
/// cell: the component sees exactly what it always saw.
#[test]
fn a_code_prop_gets_no_cell() {
    overlay::reset();
    let name = String::from("from code");
    let _mounted = Mounted::new(move || ui! { view() { Label(label = name) } });
    assert_eq!(overlay::cells::count(), 0);
    overlay::reset();
}

/// Every row of a `for` has its own cell, and a patch to the node
/// reaches all of them — the same accounting as a primitive in a `for`.
#[test]
fn every_row_of_a_for_takes_the_edit() {
    overlay::reset();
    let mut mounted = Mounted::new(|| {
        let rows = vec![1, 2];
        ui! {
            view() {
                for _row in rows {
                    Label(label = "row")
                }
            }
        }
    });
    let site = mounted.site();
    let outcome = mounted.apply_live(site, &[set(2, "label", str_lit("edited"))]);

    assert_eq!((outcome.applied, outcome.refused), (2, 0), "{outcome:?}");
    let scene = mounted.scene();
    assert_eq!(scene.matches("edited").count(), 2, "{scene}");
    assert!(!scene.contains("\"row\""), "{scene}");
    overlay::reset();
}

/// Wrapped literals (`Some("…".to_string())`, `String::from("…")`) take
/// the overlay tier at primitive sites; they take the cells here, with
/// the same conversion `__apply_literal` uses (the `Some` put back).
#[component]
fn Field(placeholder: Option<String>, title: String) -> Element {
    let p = placeholder.clone();
    ui! {
        view() {
            text { move || p.get().unwrap_or_default() }
            text { title }
        }
    }
}

#[test]
fn wrapped_literals_apply_live() {
    overlay::reset();
    let mut mounted = Mounted::new(|| {
        ui! {
            view() {
                Field(
                    placeholder = Some("Search...".to_string()),
                    title = String::from("Find"),
                )
            }
        }
    });
    let site = mounted.site();
    let outcome = mounted.apply_live(
        site,
        &[set(1, "placeholder", str_lit("Search all")), set(1, "title", str_lit("Look"))],
    );

    assert_eq!((outcome.applied, outcome.refused), (2, 0), "{outcome:?}");
    let scene = mounted.scene();
    assert!(scene.contains("Search all") && scene.contains("Look"), "{scene}");
    overlay::reset();
}

/// Number and bool literals too: each field converts through its own
/// width, exactly as `__apply_literal` casts.
#[component]
fn Gauge(size: u8, ratio: f32, on: bool) -> Element {
    let (s, r, o) = (size.clone(), ratio.clone(), on.clone());
    ui! { text { move || format!("{} {} {}", s.get(), r.get(), o.get()) } }
}

#[test]
fn number_and_bool_literals_apply_live() {
    overlay::reset();
    let mut mounted = Mounted::new(|| ui! { view() { Gauge(size = 3, ratio = 0.5, on = false) } });
    assert!(mounted.scene().contains("3 0.5 false"), "{}", mounted.scene());
    let site = mounted.site();
    let outcome = mounted.apply_live(
        site,
        &[
            set(1, "size", LiteralValue::Int(9)),
            set(1, "ratio", LiteralValue::Float(1.25)),
            set(1, "on", LiteralValue::Bool(true)),
        ],
    );
    assert_eq!((outcome.applied, outcome.refused), (3, 0), "{outcome:?}");
    assert!(mounted.scene().contains("9 1.25 true"), "{}", mounted.scene());
    overlay::reset();
}

/// The cell is the LIVE half; the staged patch is still what the next
/// build of the site uses. When the parent re-renders the component, the
/// new instance starts from the patched value (its fresh cell is seeded
/// AFTER the staged literal is applied), the old cells die with the old
/// scope, and a second patch reaches the new instance.
#[test]
fn a_rebuilt_parent_keeps_the_edit_and_the_next_patch_finds_the_new_cell() {
    overlay::reset();
    thread_local! {
        static SHOW: Cell<Option<Signal<bool>>> = const { Cell::new(None) };
    }
    let mut mounted = Mounted::new(|| {
        let show = signal(true);
        SHOW.with(|s| s.set(Some(show)));
        ui! {
            view() {
                if show.get() {
                    Label(label = "before")
                }
            }
        }
    });
    let show = SHOW.with(|s| s.get()).expect("mounted");
    let site = mounted.site();
    let node = 2;
    let edits = vec![set(node, "label", str_lit("after"))];
    overlay::stage_key(site, edits.clone());
    assert_eq!(mounted.apply_live(site, &edits).applied, 1);

    mounted.drive(|| show.set(false));
    mounted.drive(|| show.set(true));
    assert!(mounted.scene().contains("after"), "the rebuild kept the edit: {}", mounted.scene());
    assert_eq!(overlay::cells::count(), 1, "the old instance's cell died with its scope");

    let edits = vec![set(node, "label", str_lit("third"))];
    overlay::stage_key(site, edits.clone());
    let outcome = mounted.apply_live(site, &edits);
    assert_eq!((outcome.applied, outcome.refused), (1, 0), "{outcome:?}");
    assert!(mounted.scene().contains("third"), "{}", mounted.scene());
    overlay::reset();
}
