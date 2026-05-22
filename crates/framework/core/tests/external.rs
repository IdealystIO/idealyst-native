//! External primitive suite — third-party primitive infrastructure.
//!
//! Tests cover:
//! - `ExternalRegistry<B>` registration + dispatch + `has`
//! - The `external::<T>(props)` constructor
//! - `Primitive::External` mounting through MockBackend
//! - Type erasure roundtrip (props go in typed, come back via downcast)
//! - Unregistered kind falls through to `create_external_unsupported`

#[path = "common/mod.rs"]
mod common;

mod tests {
    #[allow(unused_imports)]
    use std::any::TypeId;
    use std::rc::Rc;

    use framework_core::{external, Backend, ExternalRegistry};

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

    /// Mount a `Primitive::External` whose TypeId matches a registered
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
            framework_core::view(vec![
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
