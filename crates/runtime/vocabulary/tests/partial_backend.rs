//! A backend that implements only what `view` and `text` need can
//! register those two primitives and mount a tree — the progressive
//! bring-up a new backend goes through.
//!
//! Regression: every per-primitive `register_*` was bounded by `AllCaps`,
//! so a backend had to implement all thirty capability traits (as empty
//! impls) before it could register its first primitive.

use std::rc::Rc;

use runtime_vocabulary::backend::{
    caps, realize, register_text, register_view, Element, Host, Registry, World,
};
use runtime_vocabulary::backend::runtime_shared::accessibility::AccessibilityProps;
use runtime_vocabulary::backend::runtime_shared::StyleRules;
use runtime_vocabulary::builders::{text, view};

#[derive(Clone, Debug, PartialEq)]
struct Node(usize);

#[derive(Default)]
struct Partial {
    next: usize,
    log: Vec<String>,
}

impl Partial {
    fn mint(&mut self, kind: &str) -> Node {
        self.next += 1;
        self.log.push(format!("create {kind}"));
        Node(self.next)
    }
}

impl Host for Partial {
    type Node = Node;
    fn insert(&mut self, parent: &mut Node, child: Node) {
        self.log.push(format!("insert {} -> {}", child.0, parent.0));
    }
    fn insert_at(&mut self, parent: &mut Node, child: Node, _index: usize) {
        self.log.push(format!("insert {} -> {}", child.0, parent.0));
    }
    fn remove_child(&mut self, _parent: &Node, _child: &Node) {}
    fn clear_children(&mut self, _node: &Node) {}
    fn create_anchor(&mut self) -> Node {
        self.mint("anchor")
    }
    fn supports_splice(&self) -> bool {
        false
    }
}

// Exactly the capabilities `view` and `text` use.
impl caps::ViewOps for Partial {
    fn create_view(&mut self, _a11y: &AccessibilityProps) -> Node {
        self.mint("view")
    }
}
impl caps::TextOps for Partial {
    fn create_text(&mut self, content: &str, _a11y: &AccessibilityProps) -> Node {
        self.mint(&format!("text {content:?}"))
    }
    fn update_text(&mut self, _node: &Node, _content: &str) {}
}
impl caps::StyleOps for Partial {
    fn apply_style(&mut self, _node: &Node, _style: &Rc<StyleRules>) {}
}
impl caps::DocumentOps for Partial {}
impl caps::AssetOps for Partial {}
impl caps::AppEnvOps for Partial {}
impl caps::InputOps for Partial {}
impl caps::SafeAreaOps for Partial {}
impl caps::IntrospectionOps for Partial {}

#[test]
fn regression_a_partial_backend_registers_and_mounts_the_primitives_it_has() {
    let backend = Rc::new(std::cell::RefCell::new(Partial::default()));
    let mut registry: Registry<Partial> = Registry::new();
    register_view(&mut registry);
    register_text(&mut registry);
    let registry = Rc::new(registry);
    let world = World::new();
    let _realized = world.enter(|| {
        let tree: Element = view().child(text().content("hello")).build();
        realize(&backend, &registry, tree)
    });
    let log = backend.borrow().log.join("\n");
    assert!(log.contains("create view"), "{log}");
    assert!(log.contains("create text \"hello\""), "{log}");
}
