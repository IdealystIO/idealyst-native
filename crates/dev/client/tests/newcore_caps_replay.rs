//! The caps-only replay gate: a backend that implements ONLY the scene
//! seam (`runtime_scene::Host`) plus the `runtime_vocabulary::caps`
//! traits can be driven end-to-end by the wire replayer.
//!
//! This is the client half of the dev-session wire-chain port: the
//! compile succeeding is itself the proof that the replayer's dispatch
//! reaches native UI through the capability surface, and the assertions
//! pin that create / insert / update / style / finish commands land on
//! the caps impls with the right payloads.
//!
//! Historically the replayer was bounded on the `Backend` mega-trait and
//! this file's whole point was that a `CapsReplay<B>` adapter bridged the
//! two; it therefore lived behind a `new-core` cargo feature and opened
//! with `#![cfg(feature = "new-core")]`. When the bound flipped onto
//! `caps::AllCaps` the feature was deleted — and the `#![cfg]` silently
//! turned the whole file into ZERO tests rather than a compile error.
//! **That is the failure mode this wave exists to prevent**, so the gate
//! is unconditional now: it is the only in-repo test that drives the wire
//! replayer against a hand-written capability surface.
use std::cell::RefCell;
use std::rc::Rc;

use dev_client::{OutboundSender, WireBackend};
use runtime_shared::accessibility::AccessibilityProps;
use runtime_shared::{Action, StyleRules};
use runtime_scene::Host;
use runtime_vocabulary::caps;
use wire::{Command, NodeId, StyleId, WireAccessibilityProps, WireStyleRules};

// ---------------------------------------------------------------------------
// A minimal caps-only backend. NOTE: no `impl Backend for CapsOnly`
// anywhere in this file — that absence is the point.
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq)]
enum Kind {
    View,
    Text(String),
    Button(String),
    Anchor,
}

#[derive(Debug, Default)]
struct Store {
    kinds: Vec<Kind>,
    children: Vec<Vec<usize>>,
    styled: Vec<usize>,
    finished_root: Option<usize>,
}

#[derive(Default)]
struct CapsOnly {
    store: Rc<RefCell<Store>>,
}

impl CapsOnly {
    fn alloc(&mut self, kind: Kind) -> usize {
        let mut s = self.store.borrow_mut();
        s.kinds.push(kind);
        s.children.push(Vec::new());
        s.kinds.len() - 1
    }
}

impl Host for CapsOnly {
    type Node = usize;

    fn insert(&mut self, parent: &mut usize, child: usize) {
        self.store.borrow_mut().children[*parent].push(child);
    }

    fn insert_at(&mut self, parent: &mut usize, child: usize, index: usize) {
        let mut s = self.store.borrow_mut();
        let kids = &mut s.children[*parent];
        let at = index.min(kids.len());
        kids.insert(at, child);
    }

    fn remove_child(&mut self, parent: &usize, child: &usize) {
        self.store.borrow_mut().children[*parent].retain(|c| c != child);
    }

    fn clear_children(&mut self, node: &usize) {
        self.store.borrow_mut().children[*node].clear();
    }

    fn create_anchor(&mut self) -> usize {
        self.alloc(Kind::Anchor)
    }

    fn supports_splice(&self) -> bool {
        false
    }
}

impl caps::ViewOps for CapsOnly {
    fn create_view(&mut self, _a11y: &AccessibilityProps) -> usize {
        self.alloc(Kind::View)
    }
}

impl caps::TextOps for CapsOnly {
    fn create_text(&mut self, content: &str, _a11y: &AccessibilityProps) -> usize {
        self.alloc(Kind::Text(content.to_string()))
    }

    fn update_text(&mut self, node: &usize, content: &str) {
        self.store.borrow_mut().kinds[*node] = Kind::Text(content.to_string());
    }
}

impl caps::ButtonOps for CapsOnly {
    fn create_button(
        &mut self,
        label: &str,
        _on_click: &Action,
        _leading_icon: Option<&runtime_shared::primitives::icon::IconData>,
        _trailing_icon: Option<&runtime_shared::primitives::icon::IconData>,
        _a11y: &AccessibilityProps,
    ) -> usize {
        self.alloc(Kind::Button(label.to_string()))
    }
}

impl caps::StyleOps for CapsOnly {
    fn apply_style(&mut self, node: &usize, _style: &Rc<StyleRules>) {
        self.store.borrow_mut().styled.push(*node);
    }
}

impl caps::LifecycleOps for CapsOnly {
    fn finish(&mut self, root: usize) {
        self.store.borrow_mut().finished_root = Some(root);
    }
}

// Every remaining capability rides its trait defaults — the empty impl
// blocks are the whole opt-in.
impl caps::AppEnvOps for CapsOnly {}
impl caps::InputOps for CapsOnly {}
impl caps::PressableOps for CapsOnly {}
impl caps::ImageOps for CapsOnly {}
impl caps::IconOps for CapsOnly {}
impl caps::LinkOps for CapsOnly {}
impl caps::TextInputOps for CapsOnly {}
impl caps::ToggleOps for CapsOnly {}
impl caps::SliderOps for CapsOnly {}
impl caps::ActivityIndicatorOps for CapsOnly {}
impl caps::ScrollOps for CapsOnly {}
impl caps::SafeAreaOps for CapsOnly {}
impl caps::VirtualizerOps for CapsOnly {}
impl caps::GridOps for CapsOnly {}
impl caps::GraphicsOps for CapsOnly {}
impl caps::PortalOps for CapsOnly {}
impl caps::PresenceOps for CapsOnly {}
impl caps::NavigatorOps for CapsOnly {}
impl caps::ExternalOps for CapsOnly {}
impl caps::DocumentOps for CapsOnly {}
impl caps::AssetOps for CapsOnly {}
impl caps::A11yOps for CapsOnly {}
impl caps::AnimationOps for CapsOnly {}
impl caps::IntrospectionOps for CapsOnly {}
impl caps::BatchOps for CapsOnly {}
impl caps::WireBindingOps for CapsOnly {}

// ---------------------------------------------------------------------------
// The replay itself
// ---------------------------------------------------------------------------

#[test]
fn caps_only_backend_replays_wire_commands() {
    let backend = CapsOnly::default();
    let store = backend.store.clone();

    let mut client = WireBackend::new(backend, OutboundSender::new());

    let a11y = WireAccessibilityProps::default;
    client
        .apply_batch(vec![
            Command::RegisterStyle { id: StyleId(1), rules: WireStyleRules::default() },
            Command::CreateView { id: NodeId(1), a11y: a11y() },
            Command::CreateText { id: NodeId(2), content: "hello".into(), a11y: a11y() },
            Command::CreateButton {
                id: NodeId(3),
                label: "Go".into(),
                on_click: wire::HandlerId(1),
                leading_icon: None,
                trailing_icon: None,
                a11y: a11y(),
            },
            Command::Insert { parent: NodeId(1), child: NodeId(2) },
            Command::Insert { parent: NodeId(1), child: NodeId(3) },
            Command::ApplyStyle { node: NodeId(1), style: StyleId(1) },
            Command::UpdateText { node: NodeId(2), content: "hello, caps".into() },
            Command::Finish { root: NodeId(1) },
        ])
        .expect("replay into a caps-only backend must succeed");

    let s = store.borrow();
    assert_eq!(s.kinds[0], Kind::View, "CreateView reached caps::ViewOps");
    assert_eq!(
        s.kinds[1],
        Kind::Text("hello, caps".into()),
        "CreateText + UpdateText reached caps::TextOps"
    );
    assert_eq!(s.kinds[2], Kind::Button("Go".into()), "CreateButton reached caps::ButtonOps");
    assert_eq!(s.children[0], vec![1, 2], "Insert order preserved through Host::insert");
    assert_eq!(s.styled, vec![0], "ApplyStyle reached caps::StyleOps::apply_style");
    assert_eq!(s.finished_root, Some(0), "Finish reached caps::LifecycleOps::finish");
}
