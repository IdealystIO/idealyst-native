//! A `ui!` body with several nodes adds no node of its own, wherever it lands:
//! the nodes join the parent, like a fragment, on every host. Before this a
//! multi-node reactive branch and a multi-node root became a `view`, which
//! stacked vertically whatever the parent's layout said.

use host_mock::Harness;
use runtime_macros::ui;
use runtime_vocabulary::glue::*;

fn views(log: &[String]) -> usize {
    log.iter().filter(|l| l.ends_with(" view")).count()
}

fn inserts(log: &[String]) -> Vec<String> {
    log.iter().filter(|l| l.starts_with("insert")).cloned().collect()
}

#[test]
fn regression_a_reactive_branch_with_two_children_adds_no_view() {
    let h = Harness::new();
    let open = h.world.enter(|| runtime_world::signal(true));
    let _r = h.mount(h.world.enter(|| {
        ui! {
            view() {
                if open.get() {
                    text { "a" }
                    text { "b" }
                }
            }
        }
    }));
    assert_eq!(views(&h.take_log()), 1, "only the parent is a view");
}

#[test]
fn regression_a_false_branch_adds_no_placeholder_node() {
    let h = Harness::new();
    let open = h.world.enter(|| runtime_world::signal(false));
    let _r = h.mount(h.world.enter(|| {
        ui! {
            view() {
                if open.get() {
                    text { "a" }
                }
            }
        }
    }));
    assert_eq!(views(&h.take_log()), 1, "an absent branch mounts nothing");
    h.world.enter(|| open.set(true));
    h.flush();
    let log = h.take_log();
    assert_eq!(views(&log), 0, "{log:?}");
    assert_eq!(log.iter().filter(|l| l.contains(" text ")).count(), 1, "{log:?}");
}

#[test]
fn regression_toggling_a_two_child_branch_keeps_the_order_among_its_siblings() {
    let h = Harness::new();
    let open = h.world.enter(|| runtime_world::signal(false));
    let _r = h.mount(h.world.enter(|| {
        ui! {
            view() {
                text { "before" }
                if open.get() {
                    text { "a" }
                    text { "b" }
                }
                text { "after" }
            }
        }
    }));
    h.take_log();
    h.world.enter(|| open.set(true));
    h.flush();
    let log = h.take_log();
    assert_eq!(views(&log), 0, "{log:?}");
    assert_eq!(log.iter().filter(|l| l.contains(" text ")).count(), 2, "{log:?}");
    h.world.enter(|| open.set(false));
    h.flush();
    let log = h.take_log();
    assert_eq!(views(&log), 0, "{log:?}");
}

#[test]
fn regression_a_match_arm_with_two_children_adds_no_view() {
    let h = Harness::new();
    let which = h.world.enter(|| runtime_world::signal(0));
    let _r = h.mount(h.world.enter(|| {
        ui! {
            view() {
                match which.get() {
                    0 => {
                        text { "a" }
                        text { "b" }
                    }
                    _ => {
                        text { "c" }
                    }
                }
            }
        }
    }));
    assert_eq!(views(&h.take_log()), 1);
}

#[test]
fn regression_a_signal_condition_dispatched_by_type_adds_no_view() {
    let h = Harness::new();
    let open = h.world.enter(|| runtime_world::signal(true));
    let _r = h.mount(h.world.enter(|| {
        ui! {
            view() {
                if open {
                    text { "a" }
                    text { "b" }
                }
            }
        }
    }));
    assert_eq!(views(&h.take_log()), 1);
}

/// The reported shape: `ui! { A() B() }` from a helper, placed in a parent.
#[test]
fn regression_a_multi_node_ui_body_joins_the_parent_it_lands_in() {
    fn pair() -> Element {
        ui! {
            text { "a" }
            text { "b" }
        }
    }
    let h = Harness::new();
    let _r = h.mount(h.world.enter(|| {
        ui! {
            view() {
                { pair() }
            }
        }
    }));
    let log = h.take_log();
    assert_eq!(views(&log), 1, "{log:?}");
    assert_eq!(inserts(&log).len(), 2, "both texts go straight into the parent: {log:?}");
    assert!(inserts(&log).iter().all(|l| l.starts_with("insert n0 ")), "{log:?}");
}

/// With no parent to join, a root gets the one node every mount attaches, and
/// that node is the layout-transparent anchor, not a view.
#[test]
fn regression_a_multi_node_root_mounts_under_an_anchor_not_a_view() {
    let h = Harness::new();
    let r = h.mount(h.world.enter(|| {
        ui! {
            text { "a" }
            text { "b" }
        }
    }));
    let log = h.take_log();
    assert_eq!(views(&log), 0, "{log:?}");
    assert!(log.iter().any(|l| l.ends_with(" anchor")), "{log:?}");
    assert_eq!(r.collect_nodes().len(), 1, "a mount attaches exactly one node");
}

#[test]
fn an_empty_ui_body_mounts_one_anchor_and_no_view() {
    let h = Harness::new();
    let r = h.mount(h.world.enter(|| ui! {}));
    let log = h.take_log();
    assert_eq!(views(&log), 0, "{log:?}");
    assert_eq!(r.collect_nodes().len(), 1);
}

/// On a splice-capable host (web, iOS, macOS, Android) the branch's nodes go
/// into the real parent, at the right index, with no anchor between.
#[test]
fn regression_a_spliced_two_child_branch_lands_between_its_siblings_with_no_anchor() {
    let h = Harness::new();
    h.shared.splice.set(true);
    let open = h.world.enter(|| runtime_world::signal(false));
    let _r = h.mount(h.world.enter(|| {
        ui! {
            view() {
                text { "before" }
                if open.get() {
                    text { "a" }
                    text { "b" }
                }
                text { "after" }
            }
        }
    }));
    let log = h.take_log();
    assert_eq!(views(&log), 1, "{log:?}");
    assert!(!log.iter().any(|l| l.ends_with(" anchor")), "{log:?}");
    h.world.enter(|| open.set(true));
    h.flush();
    let log = h.take_log();
    assert_eq!(views(&log), 0, "{log:?}");
    assert!(!log.iter().any(|l| l.ends_with(" anchor")), "{log:?}");
    let placed: Vec<&String> = log.iter().filter(|l| l.starts_with("insert")).collect();
    assert_eq!(placed.len(), 2, "{log:?}");
    assert!(placed[0].starts_with("insert_at n0 ") && placed[0].ends_with("@ 1"), "{log:?}");
    assert!(placed[1].starts_with("insert_at n0 ") && placed[1].ends_with("@ 2"), "{log:?}");
    h.world.enter(|| open.set(false));
    h.flush();
    let log = h.take_log();
    assert_eq!(log.iter().filter(|l| l.starts_with("remove")).count(), 2, "{log:?}");
}

#[test]
fn regression_a_spliced_multi_node_ui_body_joins_the_parent_it_lands_in() {
    fn pair() -> Element {
        ui! {
            text { "a" }
            text { "b" }
        }
    }
    let h = Harness::new();
    h.shared.splice.set(true);
    let _r = h.mount(h.world.enter(|| ui! { view() { { pair() } } }));
    let log = h.take_log();
    assert_eq!(views(&log), 1, "{log:?}");
    assert!(!log.iter().any(|l| l.ends_with(" anchor")), "{log:?}");
    assert!(inserts(&log).iter().all(|l| l.starts_with("insert n0 ")), "{log:?}");
}
