//! External primitive suite — third-party primitive infrastructure.
//!
//! Tests cover:
//! - `ExternalRegistry<B>` registration + dispatch + `has`
//! - The `external::<T>(props)` constructor
//! - `Element::External` mounting through MockBackend
//! - Type erasure roundtrip (props go in typed, come back via downcast)
//! - Unregistered kind falls through to `create_external_unsupported`

#[path = "common/mod.rs"]
mod common;

mod tests {
    #[allow(unused_imports)]
    use std::any::TypeId;
    use std::rc::Rc;

    use runtime_core::{external, Backend, ExternalRegistry};

    use crate::common::{Event, MockBackend, TestRuntime};

    #[derive(Clone, Debug, PartialEq)]
    struct MapViewProps {
        lat: f64,
        lon: f64,
    }

    #[derive(Clone, Debug, PartialEq)]
    struct CameraProps {
        front: bool,
    }

    // =========================================================================
    // ExternalRegistry — direct unit-ish tests
    // =========================================================================

    /// Empty registry has nothing.
    #[test]
    fn empty_registry_has_nothing() {
        let r: ExternalRegistry<MockBackend> = ExternalRegistry::new();
        assert!(!r.has::<MapViewProps>());
        assert!(!r.has::<CameraProps>());
        assert!(r.get(TypeId::of::<MapViewProps>()).is_none());
    }

    /// Registering for T makes `has::<T>()` return true.
    #[test]
    fn register_then_has_returns_true() {
        let mut r: ExternalRegistry<MockBackend> = ExternalRegistry::new();
        r.register::<MapViewProps, _>(|_props, _b| {
            unreachable!("not invoked by `has` check")
        });

        assert!(r.has::<MapViewProps>());
        assert!(!r.has::<CameraProps>(), "different type not registered");
    }

    /// `get(TypeId)` returns the handler; invoking it runs the user's
    /// closure with the typed payload.
    #[test]
    fn handler_dispatches_with_typed_payload() {
        use std::cell::Cell;

        let mut r: ExternalRegistry<MockBackend> = ExternalRegistry::new();
        let saw_lat: Rc<Cell<f64>> = Rc::new(Cell::new(0.0));
        let saw_lat_for_handler = saw_lat.clone();

        r.register::<MapViewProps, _>(move |props, backend| {
            saw_lat_for_handler.set(props.lat);
            backend.create_view(&Default::default())
        });

        let payload: Rc<dyn std::any::Any> =
            Rc::new(MapViewProps { lat: 37.77, lon: -122.42 });

        let handler = r.get(TypeId::of::<MapViewProps>()).expect("handler exists");
        let mut bk = MockBackend::new();
        let _node = handler(&payload, &mut bk);

        assert_eq!(saw_lat.get(), 37.77);
    }

    /// Registering twice for the same T returns the previous handler.
    #[test]
    fn re_registering_returns_prior_handler() {
        let mut r: ExternalRegistry<MockBackend> = ExternalRegistry::new();
        let first = r.register::<MapViewProps, _>(|_props, b| b.create_view(&Default::default()));
        assert!(first.is_none(), "first registration: no prior handler");

        let second = r.register::<MapViewProps, _>(|_props, b| b.create_view(&Default::default()));
        assert!(second.is_some(), "second registration: prior handler returned");
    }

    // =========================================================================
    // End-to-end through the walker
    // =========================================================================

    /// Mount a `Element::External` whose TypeId matches a registered
    /// handler — the handler runs.
    #[test]
    fn registered_external_mounts_via_handler() {
        // We can't `register_external` on our MockBackend because it
        // doesn't expose that inherent method (we'd need to add one).
        // Instead we verify via the trait method directly: walker
        // calls `create_external(type_id, type_name, &payload)`; the
        // MockBackend records `Event::CreateExternal { type_name }`.
        let rt = TestRuntime::new();
        let _owner = rt.render(external(MapViewProps { lat: 1.0, lon: 2.0 }).into());

        rt.backend().assert_any(|e| {
            matches!(e, Event::CreateExternal { type_name }
                if type_name.contains("MapViewProps"))
        });
    }

    /// Different external kinds in one tree both mount.
    #[test]
    fn multiple_external_kinds_in_one_tree() {
        let rt = TestRuntime::new();
        let _owner = rt.render(
            runtime_core::view(vec![
                external(MapViewProps { lat: 0.0, lon: 0.0 }).into(),
                external(CameraProps { front: true }).into(),
            ])
            .into(),
        );

        let count = rt.backend().count_matching(|e| matches!(e, Event::CreateExternal { .. }));
        assert_eq!(count, 2);
    }

    /// `external::<T>(props)` captures the right TypeId — verified
    /// indirectly by checking the type_name in the event log matches
    /// the props type's full path.
    #[test]
    fn external_captures_type_name() {
        let rt = TestRuntime::new();
        let _owner = rt.render(external(MapViewProps { lat: 0.0, lon: 0.0 }).into());

        let events = rt.events();
        let names: Vec<&'static str> = events
            .iter()
            .filter_map(|e| match e {
                Event::CreateExternal { type_name } => Some(*type_name),
                _ => None,
            })
            .collect();
        assert_eq!(names.len(), 1);
        assert!(
            names[0].contains("MapViewProps"),
            "expected type_name to contain MapViewProps, got '{}'",
            names[0]
        );
    }

    /// Children passed to an `Element::External` are parented INTO the
    /// node the external handler returns — same lifecycle as `Portal`.
    /// Before this was wired, External was a leaf: it produced zero
    /// Insert events regardless of children. The web `<form>` SDK
    /// depends on its inputs becoming real DOM descendants of the
    /// returned `<form>` node (autofill / submit-on-enter need it).
    #[test]
    fn external_parents_children_into_handler_node() {
        use crate::common::NodeId;
        use runtime_core::text;

        let rt = TestRuntime::new();
        // External is the root → built first → NodeId(0); the two text
        // children mint afterwards and get inserted into it.
        let _owner = rt.render(
            external(MapViewProps { lat: 0.0, lon: 0.0 })
                .children(vec![text("a").into(), text("b").into()])
                .into(),
        );

        let events = rt.events();
        let inserts: Vec<(NodeId, NodeId)> = events
            .iter()
            .filter_map(|e| match e {
                Event::Insert { parent, child } => Some((*parent, *child)),
                _ => None,
            })
            .collect();

        assert_eq!(inserts.len(), 2, "both children inserted into the external");
        let parent = inserts[0].0;
        assert!(
            inserts.iter().all(|(p, _)| *p == parent),
            "all children share one parent (the external's node)"
        );
        assert!(
            inserts.iter().all(|(p, c)| p != c),
            "the shared parent is the external, not one of the children"
        );

        let external_creates = events
            .iter()
            .filter(|e| matches!(e, Event::CreateExternal { .. }))
            .count();
        let text_creates = events
            .iter()
            .filter(|e| matches!(e, Event::CreateText { .. }))
            .count();
        assert_eq!(external_creates, 1, "exactly one external node created");
        assert_eq!(text_creates, 2, "exactly two text children created");
    }

    // =========================================================================
    // build_detached + External adopt sentinel (runtime-server wire client)
    // =========================================================================

    /// Marker type standing in for `wire::WireSidebarAdopt`.
    #[derive(Debug)]
    struct AdoptMarker;

    /// `build_detached` with an `adopt` whose `TypeId` matches an
    /// `Element::External` returns the pre-built adopt node for that leaf
    /// instead of calling `create_external`. This is the wire client's
    /// sidebar-adopt path: dev-client passes its holder node, the SDK's
    /// `leading_slot` stamps a marker-typed External, and the walker
    /// adopts the holder. Regression: the prior design routed this
    /// through an `ExternalRegistry` handler keyed by `type_id` that then
    /// downcast the `payload` (panicked on a marker-typed `Rc<()>`), and
    /// a cross-crate global that was incoherent across wasm-split chunks.
    /// Threading the adopt inside `build_detached` fixes both.
    #[test]
    fn build_detached_adopts_matching_external_node() {
        use std::cell::RefCell;
        use std::rc::Rc;

        let backend = Rc::new(RefCell::new(MockBackend::new()));

        // A pre-built "holder" node, as dev-client would hand in.
        let holder = {
            let mut b = backend.borrow_mut();
            <MockBackend as Backend>::create_view(&mut b, &Default::default())
        };

        // The SDK's leading_slot sentinel: an External with the marker
        // TypeId. Its payload would normally be downcast by a handler —
        // here the walker intercepts on type_id first.
        let sentinel = runtime_core::Element::External {
            type_id: TypeId::of::<AdoptMarker>(),
            type_name: std::any::type_name::<AdoptMarker>(),
            payload: Rc::new(AdoptMarker),
            children: Vec::new(),
            style: None,
            ref_fill: None,
            accessibility: Default::default(),
        };

        let (node, _scope) = runtime_core::build_detached(
            &backend,
            sentinel,
            Some((TypeId::of::<AdoptMarker>(), holder)),
        );

        // The adopted node IS the holder; no External was created.
        assert_eq!(node, holder, "build_detached returns the adopt holder node");
        let created = backend
            .borrow()
            .events()
            .iter()
            .filter(|e| matches!(e, Event::CreateExternal { .. }))
            .count();
        assert_eq!(
            created, 0,
            "adopt path skips create_external for the matching sentinel"
        );
    }

    /// When the adopt `TypeId` does NOT match the External's `type_id`,
    /// `build_detached` falls through to the normal `create_external`
    /// path — the adopt is scoped to the one sentinel kind, so a
    /// non-sentinel External in the same detached build still mounts
    /// normally. (Also covers `build_detached` with `adopt = None`.)
    #[test]
    fn build_detached_does_not_adopt_mismatched_external() {
        use std::cell::RefCell;
        use std::rc::Rc;

        let backend = Rc::new(RefCell::new(MockBackend::new()));
        let holder = {
            let mut b = backend.borrow_mut();
            <MockBackend as Backend>::create_view(&mut b, &Default::default())
        };

        // External whose TypeId is MapViewProps — different from the
        // adopt marker, so it must NOT be adopted.
        let (node, _scope) = runtime_core::build_detached(
            &backend,
            external(MapViewProps { lat: 1.0, lon: 2.0 }).into(),
            Some((TypeId::of::<AdoptMarker>(), holder)),
        );

        assert_ne!(node, holder, "mismatched external is not the holder");
        let created = backend
            .borrow()
            .events()
            .iter()
            .filter(|e| matches!(e, Event::CreateExternal { type_name }
                if type_name.contains("MapViewProps")))
            .count();
        assert_eq!(created, 1, "non-matching external mounts via create_external");
    }

    /// A childless external (the maps/webview leaf case) produces no
    /// Insert events — the framework doesn't assume children exist.
    #[test]
    fn external_without_children_inserts_nothing() {
        let rt = TestRuntime::new();
        let _owner = rt.render(external(MapViewProps { lat: 0.0, lon: 0.0 }).into());

        let insert_count = rt
            .backend()
            .count_matching(|e| matches!(e, Event::Insert { .. }));
        assert_eq!(insert_count, 0, "leaf external parents nothing");
    }

    /// Owner drop fires `release_external` for each mounted external.
    #[test]
    fn owner_drop_releases_externals() {
        let rt = TestRuntime::new();
        {
            let _owner = rt.render(external(MapViewProps { lat: 0.0, lon: 0.0 }).into());
            // owner dropped at block end
        }

        let release_count = rt
            .backend()
            .count_matching(|e| matches!(e, Event::ReleaseExternal { .. }));
        assert_eq!(release_count, 1, "exactly one external released on owner drop");
    }
}
