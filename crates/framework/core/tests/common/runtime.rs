//! `TestRuntime` — wires a `MockBackend` into the framework with a
//! synchronous scheduler so tests don't need a real platform.
//!
//! Most reactive tests don't need this — they construct signals /
//! effects directly via `Signal::new` / `Effect::new` and never call
//! `render`. Walker / primitive / lifecycle tests do: they need to
//! call `framework_core::render(backend, primitive_tree)` and then
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

use framework_core::{render, Owner, Primitive};

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
    pub fn render(&self, root: Primitive) -> Owner {
        render(self.backend.clone(), root)
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
}

impl Default for TestRuntime {
    fn default() -> Self {
        Self::new()
    }
}
