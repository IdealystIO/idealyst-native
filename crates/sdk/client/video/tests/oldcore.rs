//! Old-core suite: pins the `Element::External` lowering of the
//! authored surface (the byte-moved `oldcore` module) so the dual-core
//! restructure can't drift the default build.
#![cfg(not(feature = "new-core"))]

use std::any::TypeId;

use runtime_core::Element;
use video::prelude::*;

/// `Video(..)` lowers to `Element::External` keyed by `VideoProps`'s
/// TypeId (backend handlers dispatch on it) and starts childless.
#[test]
fn video_builds_external_keyed_by_video_props() {
    let el: Element = Video(VideoProps::default()).into();
    match el {
        Element::External {
            type_id,
            type_name,
            children,
            ..
        } => {
            assert_eq!(type_id, TypeId::of::<VideoProps>());
            assert!(type_name.contains("VideoProps"));
            assert!(children.is_empty(), "no children by default");
        }
        _ => panic!("Video must lower to Element::External"),
    }
}

/// The source rides the type-erased payload intact and resolves to the
/// URL content the author supplied.
#[test]
fn url_source_resolves_through_the_payload() {
    let el: Element = Video(VideoProps {
        source: video::url("https://example.com/clip.mp4"),
        ..Default::default()
    })
    .into();
    match el {
        Element::External { payload, .. } => {
            let props = payload
                .downcast_ref::<VideoProps>()
                .expect("payload is VideoProps");
            match props.source.resolve() {
                MediaContent::Url(u) => assert_eq!(u, "https://example.com/clip.mp4"),
                _ => panic!("url(...) source must resolve to MediaContent::Url"),
            }
        }
        _ => panic!("expected Element::External"),
    }
}
