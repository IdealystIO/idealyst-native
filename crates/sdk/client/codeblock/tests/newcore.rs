//! New-core suite: the SAME authored surface —
//! `code_block(spans).with_style(…)` — mounted through the scene
//! registry on a minimal recording backend (the vocab-test harness
//! pattern: old-core `Backend` mock bridged through `LegacyBridge`).
//!
//! The op-log assertions pin the DOM shape to the old-core web/SSR
//! handler call-for-call (`create_element("pre")` → per-run
//! `create_text` + literal-color `apply_style` + `insert`, author style
//! on the `<pre>`): this is the regression fence for the
//! pixel-identical old/new website parity — if the handler's emission
//! order or per-span styling drifts, these fail before the SSG diff
//! does.
#![cfg(feature = "new-core")]

use std::cell::RefCell;
use std::rc::Rc;

use codeblock::code_block;
use runtime_core::accessibility::AccessibilityProps;
use runtime_core::{Backend, Color, StyleRules, Tokenized};
use runtime_scene::{realize, Realized, Registry};
use runtime_vocabulary::glue::IntoElement;
use runtime_vocabulary::register_builtins;
use runtime_vocabulary::LegacyBridge;
use runtime_world::World;

// ===========================================================================
// Minimal recording backend (vocab-test harness pattern)
// ===========================================================================

type Log = Rc<RefCell<Vec<String>>>;

struct Mini {
    log: Log,
    next: u32,
}

impl Mini {
    fn mint(&mut self, desc: String) -> u32 {
        let n = self.next;
        self.next += 1;
        self.log.borrow_mut().push(format!("create n{n} {desc}"));
        n
    }
}

impl Backend for Mini {
    type Node = u32;

    fn create_view(&mut self, _a11y: &AccessibilityProps) -> u32 {
        self.mint("view".into())
    }

    // The handler's outer node: log the tag so the test can assert the
    // `<pre>` shape (the default impl would silently fall back to
    // `create_view` and hide a lost tag).
    fn create_element(&mut self, tag: &str) -> u32 {
        self.mint(format!("element {tag:?}"))
    }

    fn create_text(&mut self, content: &str, _a11y: &AccessibilityProps) -> u32 {
        self.mint(format!("text {content:?}"))
    }

    fn create_button(
        &mut self,
        label: &str,
        _on_click: &runtime_core::Action,
        _leading: Option<&runtime_core::IconData>,
        _trailing: Option<&runtime_core::IconData>,
        _a11y: &AccessibilityProps,
    ) -> u32 {
        self.mint(format!("button {label:?}"))
    }

    fn update_text(&mut self, _node: &u32, _content: &str) {}
    fn on_node_unstyled(&mut self, _node: &u32) {}
    fn mark_container(&mut self, _node: &u32) {}

    fn apply_style(&mut self, node: &u32, style: &Rc<StyleRules>) {
        let color = match &style.color {
            Some(Tokenized::Literal(c)) => c.0.clone(),
            Some(_) => "<token>".into(),
            None => "<none>".into(),
        };
        self.log
            .borrow_mut()
            .push(format!("style n{node} color={color}"));
    }

    fn insert(&mut self, parent: &mut u32, child: u32) {
        self.log
            .borrow_mut()
            .push(format!("insert n{parent} <- n{child}"));
    }

    fn clear_children(&mut self, node: &u32) {
        self.log.borrow_mut().push(format!("clear_children n{node}"));
    }

    fn finish(&mut self, _root: u32) {}
}

struct Harness {
    world: World,
    backend: Rc<RefCell<LegacyBridge<Mini>>>,
    registry: Rc<Registry<LegacyBridge<Mini>>>,
    log: Log,
}

fn harness() -> Harness {
    let log: Log = Rc::new(RefCell::new(Vec::new()));
    let backend = Rc::new(RefCell::new(LegacyBridge(Mini {
        log: log.clone(),
        next: 0,
    })));
    let mut registry = Registry::new();
    register_builtins(&mut registry);
    // The SDK's boot registration seam — the same fn an app passes to
    // `backend_web::newcore::start_in` / `backend_ssr::newcore::
    // render_path_with`.
    codeblock::register(&mut registry);
    Harness {
        world: World::new(),
        backend,
        registry: Rc::new(registry),
        log,
    }
}

fn spans() -> Vec<(String, Color)> {
    vec![
        ("fn ".into(), Color("#888".into())),
        ("hello".into(), Color("#0a0".into())),
    ]
}

#[test]
fn mounts_one_pre_with_one_colored_text_per_run() {
    let h = harness();
    let element = code_block(spans()).into_element();
    let _realized: Realized<u32> = h.world.enter(|| realize(&h.backend, &h.registry, element));

    // Exact op-log: the old-core web handler call-for-call. One outer
    // `<pre>`, then per run create_text → literal-color apply_style →
    // insert, in span order.
    let log = h.log.borrow().join("\n");
    assert_eq!(
        log,
        "create n0 element \"pre\"\n\
         create n1 text \"fn \"\n\
         style n1 color=#888\n\
         insert n0 <- n1\n\
         create n2 text \"hello\"\n\
         style n2 color=#0a0\n\
         insert n0 <- n2",
        "codeblock new-core DOM shape drifted from the old-core handler"
    );
}

#[test]
fn author_style_lands_on_the_pre() {
    let h = harness();
    let mut author = StyleRules::default();
    author.color = Some(Tokenized::Literal(Color("#123456".into())));
    let element = code_block(spans()).with_style(author).into_element();
    let _realized: Realized<u32> = h.world.enter(|| realize(&h.backend, &h.registry, element));

    // The author style is the LAST style op and targets the outer node
    // (n0, the `<pre>`) — the old walker's external style attach.
    let log = h.log.borrow().clone();
    assert_eq!(
        log.last().map(String::as_str),
        Some("style n0 color=#123456"),
        "author style must attach to the outer <pre>: {log:?}"
    );
}
