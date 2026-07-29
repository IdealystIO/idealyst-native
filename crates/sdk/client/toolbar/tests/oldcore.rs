//! Old-core suite: pins the `Element::External` lowering of the
//! authored surface (the byte-moved `oldcore` module) so the dual-core
//! restructure can't drift the default build, plus the item-builder
//! shapes the shared `items` module owns.
#![cfg(not(feature = "new-core"))]

use std::any::TypeId;
use std::cell::Cell;
use std::rc::Rc;

use runtime_core::Element;
use toolbar::prelude::*;

/// `Toolbar(..)` lowers to `Element::External` keyed by
/// `ToolbarProps`'s TypeId (backend handlers dispatch on it) and
/// starts childless.
#[test]
fn toolbar_builds_external_keyed_by_toolbar_props() {
    let el: Element = Toolbar(ToolbarProps::default()).into();
    match el {
        Element::External {
            type_id,
            type_name,
            children,
            ..
        } => {
            assert_eq!(type_id, TypeId::of::<ToolbarProps>());
            assert!(type_name.contains("ToolbarProps"));
            assert!(children.is_empty(), "no children by default");
        }
        _ => panic!("Toolbar must lower to Element::External"),
    }
}

/// The items closure + `visible` flag ride the type-erased payload
/// intact (the backend handler reads them back out).
#[test]
fn items_closure_and_visible_reach_the_payload() {
    let calls = Rc::new(Cell::new(0u32));
    let calls_in = calls.clone();
    let el: Element = Toolbar(ToolbarProps {
        items: Box::new(move || {
            calls_in.set(calls_in.get() + 1);
            vec![ToolbarItem::button("Save").into(), ToolbarItem::space()]
        }),
        visible: false,
    })
    .into();
    match el {
        Element::External { payload, .. } => {
            let props = payload
                .downcast_ref::<ToolbarProps>()
                .expect("payload is ToolbarProps");
            assert!(!props.visible);
            assert_eq!(calls.get(), 0, "build must not evaluate the items closure");
            let items = (props.items)();
            assert_eq!(items.len(), 2);
            assert_eq!(calls.get(), 1);
        }
        _ => panic!("expected Element::External"),
    }
}

/// The four item constructors produce the expected variants, and the
/// button builder chain fills every optional field.
#[test]
fn item_builders_produce_the_expected_shapes() {
    let clicked = Rc::new(Cell::new(false));
    let clicked_in = clicked.clone();
    let button: ToolbarItem = ToolbarItem::button("Save")
        .icon("square.and.arrow.down")
        .tooltip("Save the document")
        .on_click(move || clicked_in.set(true))
        .into();
    match button {
        ToolbarItem::Button(b) => {
            assert_eq!(b.label, "Save");
            assert_eq!(b.icon.as_deref(), Some("square.and.arrow.down"));
            assert_eq!(b.tooltip.as_deref(), Some("Save the document"));
            let cb = b.on_click.expect("handler stored");
            cb();
            assert!(clicked.get(), "stored handler is the author's");
        }
        _ => panic!("button() must produce ToolbarItem::Button"),
    }

    assert!(matches!(ToolbarItem::separator(), ToolbarItem::Separator));
    assert!(matches!(ToolbarItem::space(), ToolbarItem::Space));
    assert!(matches!(
        ToolbarItem::flexible_space(),
        ToolbarItem::FlexibleSpace
    ));

    // A bare builder keeps optionals empty (inert, label-only button).
    match ToolbarItem::button("Reload").into() {
        ToolbarItem::Button(b) => {
            assert_eq!(b.label, "Reload");
            assert!(b.icon.is_none());
            assert!(b.tooltip.is_none());
            assert!(b.on_click.is_none());
        }
        _ => panic!("expected ToolbarItem::Button"),
    }
}

/// Default props: empty items, visible.
#[test]
fn default_props_are_empty_and_visible() {
    let props = ToolbarProps::default();
    assert!((props.items)().is_empty());
    assert!(props.visible);
}
