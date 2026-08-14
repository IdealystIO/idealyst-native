//! Slots: publishing content into a region an ancestor owns.
//!
//! These mount through the real `realize` path against `host-mock`. The
//! behavior under test is entirely in the reactive-ownership layer
//! (context lifetime, keyed reconciliation, scope-drop withdrawal), so a
//! mock host exercises the identical code a real backend would.
//!
//! Trees are built with `runtime_vocabulary::builders`, like the rest of
//! this suite — the crate has no `ui!` / `#[component]` available (those
//! live above it, and their emission targets `glue`). `component_scope`
//! stands in for a component body: it is exactly what `#[component]`
//! wraps a body in, so the scope lifetimes under test are the real ones.
//!
//! `Harness::tree` renders `n<id> <kind>` per node, and the mock's text
//! kind embeds the content it was CREATED with (`text "Save"`) — that
//! string is the assertion surface throughout.
//!
//! `host-mock` defaults `supports_splice` to `false`, but every backend
//! the framework actually ships against (web, iOS, Android, macOS)
//! splices. Tests whose subject is keyed-list behavior flip the flag to
//! match; `anchored_host_rebuilds_the_whole_slot` pins the fallback so
//! the divergence is recorded rather than discovered.

use std::cell::Cell;
use std::rc::Rc;

use host_mock::{Harness, Node};
use runtime_scene::{component_scope, dyn_keyed, Element, Realized};
use runtime_vocabulary::builders::{text, view};
use runtime_vocabulary::glue::{fill_slot, fill_slot_at, slot_is_filled, slot_outlet, Slot};
use runtime_world::{signal, Signal};

struct HeaderActions;
impl Slot for HeaderActions {}

/// A second slot type, to pin that feeds are keyed per type and don't
/// bleed into each other.
struct FooterActions;
impl Slot for FooterActions {}

/// A harness posture matching the backends the framework ships against.
fn splicing_harness() -> Harness {
    let harness = Harness::new();
    harness.shared.splice.set(true);
    harness
}

fn label(content: &str) -> Element {
    text().content(content).build()
}

/// The region owner: a header view holding the outlet.
fn shell() -> Element {
    component_scope(|| view().child(view().child(slot_outlet::<HeaderActions>())).build())
}

fn root_tree(harness: &Harness, realized: &Realized<Node>) -> String {
    let nodes = realized.collect_nodes();
    assert_eq!(nodes.len(), 1, "test trees have a single root");
    harness.tree(nodes[0])
}

/// Mount `build` under a `dyn_keyed` hole gated on `show` — the shape a
/// route swap or a revealed panel takes, and the one that gives the
/// filler a real scope to be torn down with.
fn gated(show: Signal<bool>, build: fn() -> Element) -> Element {
    dyn_keyed(
        move || show.get(),
        move |&on| {
            if on {
                build()
            } else {
                view().build()
            }
        },
    )
}

/// A `ui!` children block builds bottom-up, so a filler written inside a
/// shell's block runs its body BEFORE the shell's — and would find no
/// provider if the feed were scoped to the outlet's component. The
/// world-rooted feed is what makes the two directions equivalent; here
/// the filler is built first on purpose.
///
/// The fill is a staged write either way, so it lands on the mount flush.
/// That is the same flush an app's boot path runs before its first paint,
/// so there is no frame in which the slot renders empty.
#[test]
fn fill_published_before_the_outlet_builds_still_renders() {
    fn page() -> Element {
        component_scope(|| {
            fill_slot::<HeaderActions>(|| label("Save"));
            view().child(label("page body")).build()
        })
    }

    let harness = splicing_harness();
    let tree = harness
        .world
        .enter(|| view().child(page()).child(shell()).build());
    let realized = harness.mount(tree);
    harness.flush();

    let rendered = root_tree(&harness, &realized);
    assert!(
        rendered.contains(r#"text "Save""#),
        "a fill published before the outlet built must render once the \
         mount flush commits:\n{rendered}"
    );
    drop(realized);
}

/// The other direction: a filler that mounts later (a route swap, a
/// revealed panel) publishes into an already-live outlet.
#[test]
fn late_mounted_filler_appears_after_flush() {
    fn page() -> Element {
        component_scope(|| {
            fill_slot::<HeaderActions>(|| label("Save"));
            view().child(label("page body")).build()
        })
    }

    let harness = splicing_harness();
    let show: Signal<bool> = harness.world.enter(|| signal(false));
    let tree = harness
        .world
        .enter(|| view().child(shell()).child(gated(show, page)).build());
    let realized = harness.mount(tree);

    assert!(
        !root_tree(&harness, &realized).contains(r#"text "Save""#),
        "nothing is published yet"
    );

    show.set(true);
    harness.flush();

    let rendered = root_tree(&harness, &realized);
    assert!(
        rendered.contains(r#"text "Save""#),
        "mounting the filler must fill the live outlet:\n{rendered}"
    );
    drop(realized);
}

/// Withdrawal is the load-bearing half. When the filler unmounts its
/// signals are freed, so the entry must leave the feed in the same flush
/// — otherwise a later rebuild reads through a freed handle and aborts.
#[test]
fn filler_unmount_withdraws_its_content() {
    fn page() -> Element {
        component_scope(|| {
            // A page-owned signal captured by the builder: this is the
            // cross-scope capture that makes exact withdrawal necessary —
            // the handle is freed when the page unmounts. The static
            // sibling text is what the tree assertion below can see
            // (reactive text mints empty and updates later).
            let detail = signal(String::from("unsaved"));
            fill_slot::<HeaderActions>(move || {
                view()
                    .child(label("Save"))
                    .child(text().content(move || detail.get()).build())
                    .build()
            });
            view().child(label("page body")).build()
        })
    }

    let harness = splicing_harness();
    let show: Signal<bool> = harness.world.enter(|| signal(true));
    let tree = harness
        .world
        .enter(|| view().child(shell()).child(gated(show, page)).build());
    let realized = harness.mount(tree);
    harness.flush();
    assert!(
        root_tree(&harness, &realized).contains(r#"text "Save""#),
        "precondition: the fill rendered"
    );

    show.set(false);
    harness.flush();

    let rendered = root_tree(&harness, &realized);
    assert!(
        !rendered.contains(r#"text "Save""#),
        "unmounting the filler must withdraw its slot content:\n{rendered}"
    );
    // A second flush would surface a deferred stale read if the entry had
    // outlived the page's signals.
    harness.flush();
    drop(realized);
}

/// Several fills coexist, sorted by `order`, with publish order breaking
/// ties. Publish deliberately out of order to prove the sort runs.
#[test]
fn multiple_fills_render_in_order() {
    fn page() -> Element {
        component_scope(|| {
            fill_slot_at::<HeaderActions>(10, || label("last"));
            fill_slot_at::<HeaderActions>(-10, || label("first"));
            fill_slot::<HeaderActions>(|| label("middle-a"));
            fill_slot::<HeaderActions>(|| label("middle-b"));
            view().child(label("page body")).build()
        })
    }

    let harness = splicing_harness();
    let tree = harness
        .world
        .enter(|| view().child(shell()).child(page()).build());
    let realized = harness.mount(tree);
    harness.flush();

    let rendered = root_tree(&harness, &realized);
    let position = |needle: &str| {
        rendered
            .find(needle)
            .unwrap_or_else(|| panic!("{needle} missing from:\n{rendered}"))
    };
    assert!(
        position(r#"text "first""#) < position(r#"text "middle-a""#),
        "negative order sorts ahead of the default:\n{rendered}"
    );
    assert!(
        position(r#"text "middle-a""#) < position(r#"text "middle-b""#),
        "equal order keeps publish order:\n{rendered}"
    );
    assert!(
        position(r#"text "middle-b""#) < position(r#"text "last""#),
        "higher order sorts behind the default:\n{rendered}"
    );
    drop(realized);
}

/// The outlet is a KEYED list, not a `dyn` hole: a contribution that
/// stays published must keep its live subtree when a *different* one
/// arrives or leaves. Counting builder invocations proves it — the keyed
/// reconciler never re-runs a kept row's build.
#[test]
fn kept_fill_does_not_rebuild_when_a_sibling_fill_arrives() {
    fn transient() -> Element {
        component_scope(|| {
            fill_slot_at::<HeaderActions>(1, || label("transient"));
            view().child(label("transient body")).build()
        })
    }

    let harness = splicing_harness();
    let builds = Rc::new(Cell::new(0usize));
    let counter = Rc::clone(&builds);
    let show: Signal<bool> = harness.world.enter(|| signal(false));
    let tree = harness.world.enter(|| {
        // The persistent fill is published from the root scope rather than
        // a component so only the transient one's lifetime varies.
        fill_slot_at::<HeaderActions>(0, move || {
            counter.set(counter.get() + 1);
            label("persistent")
        });
        view().child(shell()).child(gated(show, transient)).build()
    });
    let realized = harness.mount(tree);
    harness.flush();
    assert_eq!(builds.get(), 1, "the persistent fill built once on mount");

    show.set(true);
    harness.flush();
    assert!(
        root_tree(&harness, &realized).contains(r#"text "transient""#),
        "precondition: the transient fill arrived"
    );
    assert_eq!(
        builds.get(),
        1,
        "a sibling fill arriving must not rebuild the kept contribution"
    );

    show.set(false);
    harness.flush();
    assert_eq!(
        builds.get(),
        1,
        "a sibling fill leaving must not rebuild the kept contribution"
    );
    drop(realized);
}

/// The builder captures the filler's signals and is realized in the
/// OUTLET's scope. Reactivity has to survive that crossing, or slot
/// content would be a one-shot snapshot.
#[test]
fn content_stays_reactive_to_the_fillers_signals() {
    let harness = splicing_harness();
    let name: Signal<String> = harness.world.enter(|| signal(String::from("Save")));
    let tree = harness.world.enter(|| {
        fill_slot::<HeaderActions>(move || text().content(move || name.get()).build());
        view().child(shell()).build()
    });
    let realized = harness.mount(tree);
    harness.flush();
    harness.clear_ops();

    name.set(String::from("Saved"));
    harness.flush();

    let ops = harness.ops().join("\n");
    assert!(
        ops.contains("update_text") && ops.contains(r#""Saved""#),
        "a write to the filler's signal must update the node rendered in \
         the outlet's scope:\n{ops}"
    );
    drop(realized);
}

/// `slot_is_filled` is a tracked read, so chrome can collapse when
/// nothing is published and reappear when something is.
#[test]
fn slot_is_filled_tracks_the_feed() {
    fn page() -> Element {
        component_scope(|| {
            fill_slot::<HeaderActions>(|| label("Save"));
            view().child(label("page body")).build()
        })
    }

    // The conditional-chrome shape from the module docs: the bar itself
    // only exists while the slot has content.
    fn collapsing_shell() -> Element {
        component_scope(|| {
            view()
                .child(dyn_keyed(
                    || slot_is_filled::<HeaderActions>(),
                    |&filled| {
                        if filled {
                            view().child(slot_outlet::<HeaderActions>()).build()
                        } else {
                            view().build()
                        }
                    },
                ))
                .build()
        })
    }

    let harness = splicing_harness();
    let show: Signal<bool> = harness.world.enter(|| signal(false));
    let tree = harness
        .world
        .enter(|| view().child(collapsing_shell()).child(gated(show, page)).build());
    let realized = harness.mount(tree);
    harness.flush();
    assert!(
        !root_tree(&harness, &realized).contains(r#"text "Save""#),
        "empty slot renders no chrome"
    );

    show.set(true);
    harness.flush();
    let rendered = root_tree(&harness, &realized);
    assert!(
        rendered.contains(r#"text "Save""#),
        "the tracked read must re-evaluate the branch when a fill lands:\n{rendered}"
    );
    drop(realized);
}

/// Feeds are keyed by slot TYPE. Two slots in one tree must not see each
/// other's contributions — this is what makes the marker-type key worth
/// having over a string.
#[test]
fn slot_types_do_not_share_a_feed() {
    let harness = splicing_harness();
    let tree = harness.world.enter(|| {
        fill_slot::<HeaderActions>(|| label("header-only"));
        fill_slot::<FooterActions>(|| label("footer-only"));
        view()
            .child(view().child(slot_outlet::<HeaderActions>()))
            .child(view().child(slot_outlet::<FooterActions>()))
            .build()
    });
    let realized = harness.mount(tree);
    harness.flush();

    let nodes = realized.collect_nodes();
    let regions = harness.children_of(nodes[0]);
    let header_tree = harness.tree(regions[0]);
    let footer_tree = harness.tree(regions[1]);

    assert!(
        header_tree.contains(r#"text "header-only""#)
            && !header_tree.contains(r#"text "footer-only""#),
        "the header outlet renders only its own slot:\n{header_tree}"
    );
    assert!(
        footer_tree.contains(r#"text "footer-only""#)
            && !footer_tree.contains(r#"text "header-only""#),
        "the footer outlet renders only its own slot:\n{footer_tree}"
    );
    drop(realized);
}

/// On a splicing host an outlet with nothing published contributes no
/// node at all, so dropping one into a layout that may never be filled
/// costs nothing — no stray flex child, no empty box with the parent's
/// `gap` around it.
#[test]
fn empty_outlet_contributes_no_nodes_on_a_splicing_host() {
    struct UnusedSlot;
    impl Slot for UnusedSlot {}

    let harness = splicing_harness();
    let tree = harness.world.enter(|| {
        view()
            .child(label("sibling"))
            .child(slot_outlet::<UnusedSlot>())
            .build()
    });
    let realized = harness.mount(tree);
    harness.flush();

    let nodes = realized.collect_nodes();
    assert_eq!(
        harness.children_of(nodes[0]).len(),
        1,
        "an empty outlet adds no wrapper node:\n{}",
        harness.tree(nodes[0])
    );
    drop(realized);
}

/// The no-splice fallback, pinned so the divergence is documented rather
/// than found in the field. A host that can't splice (`supports_splice()
/// == false` — the wgpu scene today) puts the whole keyed list under one
/// anchor node and rebuilds it wholesale on every change.
///
/// Slot content must therefore not hold state that has to survive a
/// sibling fill — that state belongs in the filler's scope, which the
/// builder captures. This is inherited from keyed lists in general, not
/// specific to slots.
#[test]
fn anchored_host_rebuilds_the_whole_slot() {
    struct AnchoredSlot;
    impl Slot for AnchoredSlot {}

    fn anchored_shell() -> Element {
        component_scope(|| view().child(slot_outlet::<AnchoredSlot>()).build())
    }

    fn transient() -> Element {
        component_scope(|| {
            fill_slot_at::<AnchoredSlot>(1, || text().content("transient").build());
            view().build()
        })
    }

    // Deliberately NOT `splicing_harness` — the anchored path is the subject.
    let harness = Harness::new();
    assert!(
        !harness.shared.splice.get(),
        "this test's subject is the no-splice fallback"
    );

    let builds = Rc::new(Cell::new(0usize));
    let counter = Rc::clone(&builds);
    let show: Signal<bool> = harness.world.enter(|| signal(false));
    let tree = harness.world.enter(|| {
        fill_slot_at::<AnchoredSlot>(0, move || {
            counter.set(counter.get() + 1);
            text().content("persistent").build()
        });
        view()
            .child(anchored_shell())
            .child(gated(show, transient))
            .build()
    });
    let realized = harness.mount(tree);
    harness.flush();
    assert_eq!(builds.get(), 1, "built once on mount");

    show.set(true);
    harness.flush();
    assert_eq!(
        builds.get(),
        2,
        "an anchored host rebuilds every row when the list changes — if this \
         ever reads 1, the fallback gained reconciliation and the slot docs \
         should drop the caveat"
    );
    drop(realized);
}

/// The documented consequence of a world-lifetime feed: two outlets for
/// one slot type both render the same content. Pinned deliberately —
/// `slot_outlet` warns about it, but the behavior is defined, not
/// undefined, and authors who want two distinct regions need two slot
/// types rather than a second outlet.
#[test]
fn two_outlets_for_one_slot_both_render() {
    struct SharedSlot;
    impl Slot for SharedSlot {}

    let harness = splicing_harness();
    let tree = harness.world.enter(|| {
        fill_slot::<SharedSlot>(|| label("shared"));
        view()
            .child(view().child(slot_outlet::<SharedSlot>()))
            .child(view().child(slot_outlet::<SharedSlot>()))
            .build()
    });
    let realized = harness.mount(tree);
    harness.flush();

    let rendered = root_tree(&harness, &realized);
    assert_eq!(
        rendered.matches(r#"text "shared""#).count(),
        2,
        "both outlets render the feed:\n{rendered}"
    );
    drop(realized);
}

/// An outlet that unmounts and remounts — a route swap back to the same
/// shell — must still see fills published while it was gone. This is the
/// half of world-lifetime that a scope-owned feed would break: the feed
/// has to outlive the outlet, not just the filler.
#[test]
fn outlet_remount_still_renders_existing_fills() {
    struct RemountSlot;
    impl Slot for RemountSlot {}

    fn remount_shell() -> Element {
        component_scope(|| view().child(slot_outlet::<RemountSlot>()).build())
    }

    let harness = splicing_harness();
    let mounted: Signal<bool> = harness.world.enter(|| signal(true));
    let tree = harness.world.enter(|| {
        fill_slot::<RemountSlot>(|| text().content("persistent").build());
        view().child(gated(mounted, remount_shell)).build()
    });
    let realized = harness.mount(tree);
    harness.flush();
    assert!(
        root_tree(&harness, &realized).contains(r#"text "persistent""#),
        "precondition: the first outlet rendered"
    );

    mounted.set(false);
    harness.flush();
    assert!(
        !root_tree(&harness, &realized).contains(r#"text "persistent""#),
        "the outlet is gone, so nothing renders"
    );

    mounted.set(true);
    harness.flush();
    let rendered = root_tree(&harness, &realized);
    assert!(
        rendered.contains(r#"text "persistent""#),
        "a remounted outlet must pick the still-published fill back up:\n{rendered}"
    );
    drop(realized);
}

// ===========================================================================
// Author-facing spelling
// ===========================================================================

/// Everything above builds trees with `builders`, which is not how the
/// docs tell authors to write this. This module pins the documented
/// spelling instead: `#[component]`, `ui!`, and — the part that could
/// silently break — `slot_outlet::<S>()` as a `ui!` *expression child*.
///
/// That last one is load-bearing and non-obvious: `ui!` routes a bare
/// identifier followed by `(` to tag dispatch, and only falls through to
/// the expression parser for everything else. A change to that routing
/// would turn `slot_outlet::<HeaderActions>()` into an attempted tag
/// lookup and break every documented call site.
///
/// Its own module because the suite above imports `builders::{text, view}`,
/// which shadow the same-named `glue` constructors `ui!` expands to.
mod author_surface {
    use runtime_macros::{component, ui};
    // `ui!` emits the `text` constructor UNQUALIFIED — an author gets it
    // from `runtime_core::*`; here it comes straight from `glue`, which is
    // what `runtime_core` re-exports. (`view` needs no import: the macro
    // emits that one through an absolute path.)
    use runtime_vocabulary::glue::{fill_slot, slot_outlet, text, Element, Slot};

    struct DocSlot;
    impl Slot for DocSlot {}

    #[component]
    fn DocShell(children: Vec<Element>) -> Element {
        ui! {
            view() {
                view() { slot_outlet::<DocSlot>() }
                view() { children }
            }
        }
    }

    #[component]
    fn DocPage() -> Element {
        fill_slot::<DocSlot>(|| ui! { text("Save") });
        ui! { view() { text("page body") } }
    }

    #[test]
    fn documented_ui_macro_spelling_renders() {
        let harness = super::splicing_harness();
        let tree = harness.world.enter(|| {
            ui! {
                DocShell {
                    DocPage()
                }
            }
        });
        let realized = harness.mount(tree);
        harness.flush();

        let rendered = super::root_tree(&harness, &realized);
        assert!(
            rendered.contains(r#"text "Save""#) && rendered.contains(r#"text "page body""#),
            "the documented shell shape must render both the slot content \
             and the children block:\n{rendered}"
        );
        drop(realized);
    }
}
