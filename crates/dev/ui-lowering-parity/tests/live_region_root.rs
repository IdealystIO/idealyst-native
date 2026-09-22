//! A component whose ROOT is a reactive region.
//!
//! `with_tag` used to tag an `Item` and recurse through `Owned`, and
//! return everything else unchanged. That is fine for a component whose
//! body is a `view` — and wrong for one whose body is a `switch`, a
//! `when` or a keyed list, because the thing the call site gets back is
//! the REGION, not a node. Nothing carried the caller's tag, so nothing
//! registered, so a patch to that call site reported `0 applied` and the
//! author saw no change and no reason.
//!
//! idea-ui's `Button` is exactly that shape the moment a structural prop
//! is live — which is most buttons in a real app.
//!
//! The fix tags the region's CONTENTS, from inside its build closure. A
//! region builds its contents again on every swap, so the tag rides
//! along: the branch that is showing right now is the one that carries
//! the tag, and the one after the flip carries it too. Registration then
//! happens where it happens for every other node — `mount_item`, on the
//! way in — and the previous branch's registration goes dead with the
//! subtree it belonged to.
//!
//! These fixtures build their roots with `glue::switch` /
//! `scene::keyed` directly rather than through `ui!` (rule 9.2's
//! documented-deviation clause): the point of each one is the ELEMENT
//! VARIANT its component returns, and spelling the variant is the only
//! way to pin it.

#![cfg(feature = "ui-overlay")]

use std::borrow::Cow;
use std::cell::RefCell;

use runtime_macros::{component, ui};
use runtime_template::{Edit, LiteralValue};
use runtime_vocabulary::glue::{self, signal, Element, Signal};
use runtime_vocabulary::overlay;
use ui_lowering_parity::Mounted;

thread_local! {
    /// The flip the fixtures share, so a test can drive it. Same shape
    /// `live.rs` uses: `Mounted::new` takes a plain builder.
    static FLIP: RefCell<Option<Signal<bool>>> = const { RefCell::new(None) };
}

fn flip() -> Signal<bool> {
    FLIP.with(|f| f.borrow().expect("the fixture made the signal"))
}

fn set_str(node: u32, name: &'static str, value: &'static str) -> Edit {
    Edit::SetProp {
        node,
        name: Cow::Borrowed(name),
        value: LiteralValue::Str(Cow::Borrowed(value)),
    }
}

// ===========================================================================
// A `switch` root
// ===========================================================================

#[runtime_macros::props]
#[derive(Clone, Default)]
pub struct BannerProps {
    pub label: String,
}

/// The `Button` shape: one prop, and a root that is a guarded reactive
/// hole rather than a node.
#[component]
fn Banner(props: &BannerProps) -> Element {
    let label = props.label.get();
    let flip = flip();
    glue::switch(
        move || flip.get(),
        move |on: &bool| {
            let text = format!("{} {}", if *on { "B" } else { "A" }, label.clone());
            ui! { text { text } }
        },
    )
}

fn banner() -> Element {
    FLIP.with(|f| *f.borrow_mut() = Some(signal(false)));
    ui! {
        view() {
            Banner(label = "before")
        }
    }
}

/// The case the whole item exists for: the component's only node lives
/// inside its region, and a patch to the call site has to reach it.
#[test]
fn a_switch_rooted_component_is_reachable_live() {
    overlay::reset();
    let mut mounted = Mounted::new(banner);
    assert!(mounted.scene().contains("A before"), "{}", mounted.scene());

    let site = mounted.site();
    let outcome = mounted.apply_live(site, &[set_str(1, "content", "patched")]);

    assert_eq!(outcome.applied, 1, "the region's contents carry the tag: {outcome:?}");
    assert_eq!(outcome.refused, 0);
    assert!(mounted.scene().contains("patched"), "{}", mounted.scene());
    overlay::reset();
}

/// And after the region swaps. The branch that took the first patch is
/// gone; the one that replaced it has to be registered in its place, or
/// the second patch lands on a node nothing is showing.
#[test]
fn a_switch_rooted_component_is_reachable_after_the_switch_flips() {
    overlay::reset();
    let mut mounted = Mounted::new(banner);
    let site = mounted.site();

    assert_eq!(mounted.apply_live(site, &[set_str(1, "content", "first")]).applied, 1);
    assert!(mounted.scene().contains("first"), "{}", mounted.scene());

    // Flip: the guarded hole tears the A branch down and builds B.
    mounted.drive(|| flip().set(true));
    let after = mounted.scene();
    assert!(after.contains("B before"), "the branch must have flipped:\n{after}");
    assert!(!after.contains("first"), "the old branch is gone:\n{after}");

    // Exactly ONE instance takes it: the retired branch must not match.
    let outcome = mounted.apply_live(site, &[set_str(1, "content", "second")]);
    assert_eq!(outcome.applied, 1, "the NEW branch takes it: {outcome:?}");
    assert!(mounted.scene().contains("second"), "{}", mounted.scene());
    overlay::reset();
}

// ===========================================================================
// A `dynamic` root — the unguarded hole
// ===========================================================================

#[runtime_macros::props]
#[derive(Clone, Default)]
pub struct TickerProps {
    pub label: String,
}

#[component]
fn Ticker(props: &TickerProps) -> Element {
    let label = props.label.get();
    let flip = flip();
    glue::dynamic(move || {
        let text = format!("{} {}", if flip.get() { "B" } else { "A" }, label.clone());
        ui! { text { text } }
    })
}

fn ticker() -> Element {
    FLIP.with(|f| *f.borrow_mut() = Some(signal(false)));
    ui! {
        view() {
            Ticker(label = "before")
        }
    }
}

/// The plain hole rebuilds on every fire rather than on a guard, so it
/// takes a different driver — and the tag has to ride the same way.
#[test]
fn a_dynamic_rooted_component_is_reachable_live() {
    overlay::reset();
    let mut mounted = Mounted::new(ticker);
    let site = mounted.site();

    assert_eq!(mounted.apply_live(site, &[set_str(1, "content", "first")]).applied, 1);
    mounted.drive(|| flip().set(true));
    let outcome = mounted.apply_live(site, &[set_str(1, "content", "second")]);

    assert_eq!(outcome.applied, 1, "{outcome:?}");
    assert!(mounted.scene().contains("second"), "{}", mounted.scene());
    overlay::reset();
}

// ===========================================================================
// A keyed-list root
// ===========================================================================

#[runtime_macros::props]
#[derive(Clone, Default)]
pub struct ListProps {
    pub label: String,
}

#[component]
fn List(props: &ListProps) -> Element {
    let label = props.label.get();
    runtime_scene::keyed(
        || vec![1u32, 2u32],
        |n: &u32| *n as u64,
        move |n: u32| {
            let text = format!("{} {n}", label.clone());
            ui! { text { text } }
        },
    )
}

fn list() -> Element {
    ui! {
        view() {
            List(label = "row")
        }
    }
}

/// A keyed root means the call site's ONE node renders N times, so the
/// tag names all N and a patch applies to all N. That is what "this
/// node, as the source wrote it" means when the source wrote one node
/// that a list repeats — the same accounting a `for` over components
/// already gets.
#[test]
fn a_keyed_rooted_component_is_reachable_in_every_row() {
    overlay::reset();
    let mut mounted = Mounted::new(list);
    let site = mounted.site();

    let outcome = mounted.apply_live(site, &[set_str(1, "content", "patched")]);

    assert_eq!(outcome.applied, 2, "one tag, two rows: {outcome:?}");
    let after = mounted.scene();
    assert_eq!(
        after.matches("patched").count(),
        2,
        "both rows must show it:\n{after}"
    );
    overlay::reset();
}
