//! Web backend: drives DOM nodes via web-sys/wasm-bindgen.

use framework_core::Backend;
use std::rc::Rc;
use wasm_bindgen::closure::Closure;
use wasm_bindgen::JsCast;
use web_sys::{Document, Node};

pub struct WebBackend {
    doc: Document,
    mount: web_sys::Element,
    /// Click closures must outlive their listeners. We hold them here, keyed
    /// off nothing for now: when the backend drops, all closures drop.
    _click_closures: Vec<Closure<dyn FnMut()>>,
}

impl WebBackend {
    /// Constructs a backend that will mount its root under `mount_selector`
    /// (e.g. `"#app"`). Panics if the element is not found.
    pub fn new(mount_selector: &str) -> Self {
        let window = web_sys::window().expect("no window");
        let doc = window.document().expect("no document");
        let mount = doc
            .query_selector(mount_selector)
            .expect("query failed")
            .expect("mount element not found");
        Self {
            doc,
            mount,
            _click_closures: Vec::new(),
        }
    }
}

impl Backend for WebBackend {
    type Node = Node;

    fn create_view(&mut self) -> Self::Node {
        self.doc
            .create_element("div")
            .expect("create_element failed")
            .unchecked_into::<Node>()
    }

    fn create_text(&mut self, content: &str) -> Self::Node {
        self.doc.create_text_node(content).unchecked_into::<Node>()
    }

    fn create_button(&mut self, label: &str, on_click: Rc<dyn Fn()>) -> Self::Node {
        let button = self
            .doc
            .create_element("button")
            .expect("create button")
            .unchecked_into::<web_sys::HtmlElement>();
        button.set_text_content(Some(label));
        let closure = Closure::<dyn FnMut()>::new(move || on_click());
        button.set_onclick(Some(closure.as_ref().unchecked_ref()));
        self._click_closures.push(closure);
        button.unchecked_into::<Node>()
    }

    fn insert(&mut self, parent: &mut Self::Node, child: Self::Node) {
        parent.append_child(&child).expect("append_child failed");
    }

    fn update_text(&mut self, node: &Self::Node, content: &str) {
        node.set_node_value(Some(content));
    }

    fn clear_children(&mut self, node: &Self::Node) {
        // `Node.first_child()` + `remove_child` in a loop. Avoids
        // `Element::set_inner_html("")` which only works on Element nodes.
        while let Some(child) = node.first_child() {
            node.remove_child(&child).expect("remove_child failed");
        }
    }

    fn finish(&mut self, root: Self::Node) {
        self.mount
            .append_child(&root)
            .expect("mount append failed");
    }
}
