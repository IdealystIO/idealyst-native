//! `keyboard_avoiding_view` reaches the backend: the `ui!` tag lowers to the
//! glue wrapper, the props land in `ViewPrim::keyboard_avoid`, and the view
//! handler calls `SafeAreaOps::mark_keyboard_avoiding` once the node exists
//! (after its children and style, so a backend can measure it). A plain
//! `view` never does.

use host_mock::Harness;
use runtime_macros::ui;
use runtime_shared::KeyboardAvoidBehavior;
use runtime_vocabulary::glue::{text, Element};

fn marks(h: &Harness) -> Vec<String> {
    h.shared
        .log
        .borrow()
        .iter()
        .filter(|l| l.starts_with("mark_keyboard_avoiding"))
        .cloned()
        .collect()
}

#[test]
fn keyboard_avoiding_view_defaults_reach_the_backend() {
    let h = Harness::new();
    let tree: Element = h.world.enter(|| {
        ui! {
            keyboard_avoiding_view {
                text { "composer" }
            }
        }
    });
    let _r = h.mount(tree);
    h.flush();
    assert_eq!(marks(&h).len(), 1, "log: {:?}", h.shared.log.borrow());
    assert!(marks(&h)[0].ends_with("Padding animated=true"), "{:?}", marks(&h));
}

#[test]
fn keyboard_avoiding_view_inline_props_reach_the_backend() {
    let h = Harness::new();
    let tree: Element = h.world.enter(|| {
        ui! {
            keyboard_avoiding_view(behavior = KeyboardAvoidBehavior::Translate, animated = false) {
                text { "form" }
            }
        }
    });
    let _r = h.mount(tree);
    h.flush();
    assert_eq!(marks(&h).len(), 1);
    assert!(marks(&h)[0].ends_with("Translate animated=false"), "{:?}", marks(&h));
}

#[test]
fn plain_view_is_never_marked() {
    let h = Harness::new();
    let tree: Element = h.world.enter(|| ui! { view { text { "x" } } });
    let _r = h.mount(tree);
    h.flush();
    assert!(marks(&h).is_empty());
}
