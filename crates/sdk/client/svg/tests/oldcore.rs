//! Old-core suite: pins the `Element::External` lowering of the
//! authored surface (the byte-moved `oldcore` module) so the dual-core
//! restructure can't drift the default build.
#![cfg(not(feature = "new-core"))]

use std::any::TypeId;

use runtime_core::Element;
use svg::prelude::*;

/// `Svg(..)` lowers to `Element::External` keyed by `SvgProps`'s TypeId
/// (backend handlers dispatch on it) and starts childless.
#[test]
fn svg_builds_external_keyed_by_svg_props() {
    let el: Element = Svg(SvgProps::default()).into();
    match el {
        Element::External {
            type_id,
            type_name,
            children,
            ..
        } => {
            assert_eq!(type_id, TypeId::of::<SvgProps>());
            assert!(type_name.contains("SvgProps"));
            assert!(children.is_empty(), "no children by default");
        }
        _ => panic!("Svg must lower to Element::External"),
    }
}

/// The markup closure rides the type-erased payload intact.
#[test]
fn markup_closure_reaches_the_payload() {
    let el: Element = Svg(SvgProps {
        markup: markup("<svg viewBox=\"0 0 4 2\"></svg>"),
        ..Default::default()
    })
    .into();
    match el {
        Element::External { payload, .. } => {
            let props = payload
                .downcast_ref::<SvgProps>()
                .expect("payload is SvgProps");
            assert_eq!((props.markup)(), "<svg viewBox=\"0 0 4 2\"></svg>");
        }
        _ => panic!("expected Element::External"),
    }
}
