//! A patch must reach inside a navigator screen.
//!
//! This is the case the live path could not serve before. A navigator
//! handler realizes each screen into its OWN `Realized` — held by the
//! navigator, not hung under the tree it was called from — so a walk
//! down from the app root never saw it. In a navigator-based app that is
//! essentially the whole UI: patches arrived, the applier honestly
//! reported `0 applied`, and nothing changed until the screen next
//! rebuilt.
//!
//! The fix is not a seam per navigator. `mount_item` registers every
//! node it mounts, so any handler that realizes through `realize()` is
//! covered — including ones that do not exist yet, which is the point.
//!
//! The other half matters as much: a POPPED screen must stop receiving
//! patches. That falls out of ownership rather than a cleanup hook — the
//! registry holds `Weak`, the `Rc` lives in the screen's live tree, and
//! dropping a `Realized` is unmount.

#![cfg(feature = "ui-overlay")]

use std::borrow::Cow;
use std::cell::RefCell;
use std::rc::Rc;

use runtime_macros::ui;
use runtime_shared::primitives::navigator::Route;
use runtime_template::{Edit, LiteralValue};
use runtime_vocabulary::builders::{navigator_outlet, stack_navigator};
use runtime_vocabulary::glue::Element;
use runtime_vocabulary::overlay;
use runtime_vocabulary::prims::{NavHandle, StackRetention};
use ui_lowering_parity::{scene_shape, Mounted};

const HOME: Route<()> = Route::new("home", "/");
const DETAIL: Route<()> = Route::new("detail", "/detail");

/// The screen whose literal we patch. Its `ui!` is an ordinary site; the
/// only thing special about it is WHERE the navigator puts the result.
fn detail_screen() -> Element {
    ui! {
        view() {
            text { "detail before" }
        }
    }
}

fn set_str(node: u32, name: &'static str, value: &'static str) -> Edit {
    Edit::SetProp {
        node,
        name: Cow::Borrowed(name),
        value: LiteralValue::Str(Cow::Borrowed(value)),
    }
}

/// The site key of the detail screen's `ui!`, read off a standalone
/// build of it — the same number the navigator-mounted instance carries,
/// because a site key is a property of the source position, not of where
/// the result ends up.
fn detail_site() -> u64 {
    overlay::reset();
    let probe = Mounted::new(detail_screen);
    let site = probe.site();
    drop(probe);
    overlay::reset();
    site
}

struct Nav {
    mounted: Mounted,
    handle: NavHandle,
}

fn mount_nav() -> Nav {
    let slot: Rc<RefCell<Option<NavHandle>>> = Rc::new(RefCell::new(None));
    let for_build = slot.clone();
    let mounted = Mounted::new(move || {
        stack_navigator(&HOME)
            .screen(HOME, |_| ui! { view() { text { "home" } } })
            .screen(DETAIL, |_| detail_screen())
            .retention(StackRetention::Retain)
            .layout(|| {
                runtime_vocabulary::builders::view()
                    .child(navigator_outlet())
                    .build()
            })
            .on_handle(move |handle| *for_build.borrow_mut() = Some(handle))
            .build()
    });
    let handle = slot.borrow().clone().expect("handle bound at mount");
    Nav { mounted, handle }
}

#[test]
fn a_patch_reaches_a_node_inside_a_pushed_navigator_screen() {
    let site = detail_site();
    let mut nav = mount_nav();

    let pushed = nav.handle.clone();
    nav.mounted.drive(move || pushed.push(&DETAIL, ()));
    assert!(
        nav.mounted.scene().contains("detail before"),
        "the screen must be on screen first:\n{}",
        nav.mounted.scene()
    );

    let outcome = nav.mounted.apply_live(site, &[set_str(1, "content", "detail after")]);
    assert_eq!(
        outcome.applied, 1,
        "a navigator screen's nodes must be reachable; got {outcome:?}"
    );
    assert_eq!(outcome.refused, 0);
    assert!(
        nav.mounted.scene().contains("detail after"),
        "and the change must be on screen:\n{}",
        nav.mounted.scene()
    );

    overlay::reset();
}

/// A popped screen is unmounted, so its registrations are dead and a
/// later patch applies nothing. It must also not panic: a dev loop that
/// took the app down because an edit arrived a moment after a
/// navigation would be worse than one that did nothing.
#[test]
fn a_popped_screen_stops_receiving_patches() {
    let site = detail_site();
    let mut nav = mount_nav();

    let pushed = nav.handle.clone();
    nav.mounted.drive(move || pushed.push(&DETAIL, ()));
    assert_eq!(
        nav.mounted.apply_live(site, &[set_str(1, "content", "while pushed")]).applied,
        1
    );

    let popped = nav.handle.clone();
    nav.mounted.drive(move || popped.pop());
    let after_pop = nav.mounted.scene();

    let outcome = nav.mounted.apply_live(site, &[set_str(1, "content", "after pop")]);
    assert_eq!(outcome.applied, 0, "the screen is gone; nothing to apply to");
    assert_eq!(outcome.refused, 0, "and nothing to refuse either");
    assert_eq!(
        scene_shape(&nav.mounted.scene()),
        scene_shape(&after_pop),
        "a patch for an unmounted screen must change nothing"
    );

    overlay::reset();
}

/// Pushing the screen again after a pop re-registers it, so patches
/// reach the NEW instance. The registry is a set of live nodes, not a
/// map keyed once at boot.
#[test]
fn pushing_again_reaches_the_new_instance() {
    let site = detail_site();
    let mut nav = mount_nav();

    let push1 = nav.handle.clone();
    nav.mounted.drive(move || push1.push(&DETAIL, ()));
    let pop = nav.handle.clone();
    nav.mounted.drive(move || pop.pop());

    let push2 = nav.handle.clone();
    nav.mounted.drive(move || push2.push(&DETAIL, ()));

    let outcome = nav.mounted.apply_live(site, &[set_str(1, "content", "second push")]);
    assert_eq!(outcome.applied, 1, "{outcome:?}");
    assert!(nav.mounted.scene().contains("second push"), "{}", nav.mounted.scene());

    overlay::reset();
}
