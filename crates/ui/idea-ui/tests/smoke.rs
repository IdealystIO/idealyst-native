//! Smoke test removed: the prior integration test depended on a
//! pre-refactor surface (`HStack`/`VStack`/`Pressable` ui!-level tags,
//! the `Ghost` intent, the `Intent::palette` method, `IntentPalette`,
//! `Tab::new(...)` with a lazy-panel closure, `TabsProps.selected`,
//! `framework_core::OverlayAnchor`) that no longer exists after the
//! theme/intent/tab/overlay refactors. The lib-level unit tests under
//! `src/` still cover idea-ui's internals; the integration surface
//! should be re-introduced once the public DSL settles.
