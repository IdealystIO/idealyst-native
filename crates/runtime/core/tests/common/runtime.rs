//! `TestRuntime` — wires a `MockBackend` into the framework with a
//! synchronous scheduler so tests don't need a real platform.
//!
//! Most reactive tests don't need this — they construct signals /
//! effects directly via `signal!` / `watch` and never call
//! `render`. Walker / primitive / lifecycle tests do: they need to
//! call `runtime_core::render(backend, primitive_tree)` and then
//! drive the resulting `Owner` to mount + unmount things.
//!
//! Usage:
//!
//! ```ignore
//! let rt = TestRuntime::new();
//! let _owner = rt.render(view(vec![text("hi").into()]).into());
//! rt.backend().assert_events(&[ /* ... */ ]);
//! ```

#![allow(dead_code)]

use std::cell::RefCell;
use std::rc::Rc;

use runtime_core::{render, Owner, Element};

use super::mock_backend::{MockBackend, MockBackendConfig};

pub struct TestRuntime {
    backend: Rc<RefCell<MockBackend>>,
}

impl TestRuntime {
    pub fn new() -> Self {
        Self {
            backend: Rc::new(RefCell::new(MockBackend::new())),
        }
    }

    /// Construct a runtime with a custom mock-backend config — used by
    /// tests that want to opt into the batched-Repeat fast path
    /// (`MockBackendConfig { supports_batched_repeat: true }`).
    pub fn with_config(config: MockBackendConfig) -> Self {
        Self {
            backend: Rc::new(RefCell::new(MockBackend::with_config(config))),
        }
    }

    /// Render a primitive tree into the test runtime. Returns the
    /// `Owner` that keeps the render tree alive — drop it to unmount
    /// the whole tree (each release_* fires through the backend).
    pub fn render(&self, root: Element) -> Owner {
        render(self.backend.clone(), root)
    }

    /// Render via a closure that runs INSIDE the root reactive scope, so
    /// `provide(...)` / signals declared in it are adopted by the `Owner`.
    /// Lets a test stand in for a navigator screen (which `provide`s its
    /// `ScreenNav` before building the screen subtree).
    pub fn render_with(&self, f: impl FnOnce() -> Element + 'static) -> Owner {
        runtime_core::mount(self.backend.clone(), f)
    }

    /// Borrow the backend (immutable) for event inspection.
    pub fn backend(&self) -> std::cell::Ref<'_, MockBackend> {
        self.backend.borrow()
    }

    /// Borrow the backend mutably (for clear_events between phases).
    pub fn backend_mut(&self) -> std::cell::RefMut<'_, MockBackend> {
        self.backend.borrow_mut()
    }

    /// Convenience: read the event log.
    pub fn events(&self) -> Vec<super::mock_backend::Event> {
        self.backend.borrow().events()
    }

    /// Drive every mounted `Element::Virtualizer`'s mount/release
    /// callbacks to match its current item count. Real backends do this
    /// from scroll / rAF — OUTSIDE the framework's `backend.borrow_mut()`
    /// — so the mock stores the callbacks and this helper invokes them
    /// with no borrow held (it clones the shared core, dropping the
    /// backend borrow before calling any callback).
    ///
    /// Non-windowing: it mounts every index in `0..item_count()` and
    /// releases any previously-mounted index that's now out of range.
    /// That's enough to unit-test row content, incremental mount on
    /// growth, and per-row `Scope` teardown on shrink (`release_item`
    /// drops the row's scope, firing its `on_cleanup`s).
    pub fn sync_virtualizers(&self) {
        use std::collections::HashSet;

        // Clone the shared core (Rc-clone) and DROP the backend borrow
        // before invoking any callback — `mount_item` re-borrows the
        // backend internally.
        let core = self.backend.borrow().inspector();
        let nodes: Vec<super::mock_backend::NodeId> =
            core.virtualizers.borrow().keys().copied().collect();

        for node in nodes {
            let (count_fn, mount_fn, release_fn, mut mounted) = {
                let vs = core.virtualizers.borrow();
                let sv = vs.get(&node).expect("virtualizer entry present");
                (
                    sv.item_count.clone(),
                    sv.mount_item.clone(),
                    sv.release_item.clone(),
                    sv.mounted.clone(),
                )
            };

            let count = count_fn();

            // Release rows that fell outside `0..count` (shrink).
            mounted.retain(|(idx, scope_id, _node)| {
                if *idx >= count {
                    release_fn(*scope_id);
                    false
                } else {
                    true
                }
            });

            // Mount any in-range index not already mounted (growth).
            let have: HashSet<usize> = mounted.iter().map(|(i, _, _)| *i).collect();
            for idx in 0..count {
                if !have.contains(&idx) {
                    let (n, sid) = mount_fn(idx);
                    mounted.push((idx, sid, n));
                }
            }
            mounted.sort_by_key(|(i, _, _)| *i);

            core.virtualizers
                .borrow_mut()
                .get_mut(&node)
                .expect("virtualizer entry present")
                .mounted = mounted;
        }
    }
}

impl Default for TestRuntime {
    fn default() -> Self {
        Self::new()
    }
}
