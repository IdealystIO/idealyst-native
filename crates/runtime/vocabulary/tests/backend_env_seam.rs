//! The boot seam must wire a backend's environment capabilities into the
//! ambient thread-locals author code reads.
//!
//! ## The regression
//!
//! Five author-facing free functions — `platform()`, `color_scheme()`,
//! `open_url()`, `set_fullscreen()`, `announce()` — read thread-local
//! slots rather than taking a backend reference, so they work from any
//! component body, effect, or event handler. Under the pre-v2 core, a
//! central `mount()` filled those slots from the backend. `mount()` was
//! deleted with the walker, and nothing replaced it: every backend still
//! implemented `AppEnvOps::{platform, color_scheme, url_opener,
//! fullscreen_setter}` and `A11yOps::announce_for_accessibility`, but no
//! code path forwarded them. The installers had also been left
//! `#[doc(hidden)]` and documented as "not part of the public API", so a
//! backend author had no sanctioned way to call them.
//!
//! Net effect on EVERY backend: `platform()` returned `Custom("")`,
//! `color_scheme()` returned `Auto`, and `open_url()` /
//! `set_fullscreen()` / `announce()` were silent no-ops that logged at
//! debug level. It type-checked, mounted, and rendered — the failure was
//! only visible to an author who called one of the five and got nothing.
//! `backend_macos::newcore` documented the gap in a comment ("until the
//! migration gives those installs a public seam") rather than a test,
//! which is why it survived.
//!
//! `runtime_vocabulary::backend::install_env_services` is that seam, and
//! this file pins it. The companion source-scan in
//! `boot_seam_surface.rs` pins that every backend boot entry calls it —
//! a property no unit test can observe, since it is about code that does
//! not run rather than code that runs wrong.
//!
//! ## Second job: proving the public surface is sufficient
//!
//! `EnvBackend` below is written against `runtime_vocabulary::backend`
//! and nothing else — no `runtime-core`, no `#[doc(hidden)]` path, no
//! crate-internal helper. If a future refactor moves something a backend
//! author needs behind a private door, this file stops compiling.

use std::cell::RefCell;
use std::rc::Rc;

use runtime_vocabulary::backend::{self, caps, Host};

// ---------------------------------------------------------------------------
// Fixture: the smallest backend that declares every environment capability.
// ---------------------------------------------------------------------------

#[derive(Default)]
struct EnvBackend {
    opened: Rc<RefCell<Vec<String>>>,
    fullscreen: Rc<RefCell<Vec<bool>>>,
    announced: Rc<RefCell<Vec<String>>>,
    /// Flipped in `Drop` so the weak-capture test can prove the backend
    /// actually died rather than being pinned by the announcer slot.
    dropped: Option<Rc<RefCell<bool>>>,
}

impl Drop for EnvBackend {
    fn drop(&mut self) {
        if let Some(flag) = &self.dropped {
            *flag.borrow_mut() = true;
        }
    }
}

impl Host for EnvBackend {
    type Node = ();
    fn insert(&mut self, _parent: &mut (), _child: ()) {}
    fn insert_at(&mut self, _parent: &mut (), _child: (), _index: usize) {}
    fn remove_child(&mut self, _parent: &(), _child: &()) {}
    fn clear_children(&mut self, _node: &()) {}
    fn create_anchor(&mut self) {}
    fn supports_splice(&self) -> bool {
        false
    }
}

impl caps::AppEnvOps for EnvBackend {
    fn platform(&self) -> backend::Platform {
        backend::Platform::Custom("EnvTest")
    }

    fn color_scheme(&self) -> backend::ColorScheme {
        backend::ColorScheme::Dark
    }

    fn url_opener(&self) -> Option<Rc<dyn Fn(&str)>> {
        let sink = self.opened.clone();
        Some(Rc::new(move |url: &str| sink.borrow_mut().push(url.to_string())))
    }

    fn fullscreen_setter(&self) -> Option<Rc<dyn Fn(bool)>> {
        let sink = self.fullscreen.clone();
        Some(Rc::new(move |on: bool| sink.borrow_mut().push(on)))
    }
}

impl caps::A11yOps for EnvBackend {
    fn announce_for_accessibility(&mut self, msg: &str, _priority: backend::LiveRegionPriority) {
        self.announced.borrow_mut().push(msg.to_string());
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

/// The regression, end to end: all five ambient reads are dead before the
/// seam runs and live after it.
///
/// The "before" half is the part that actually failed — it is the state
/// every shipped backend was permanently stuck in. It is assertable here
/// because the harness gives each test its own thread, and these slots
/// are thread-locals that start empty.
#[test]
fn install_env_services_seeds_every_ambient_author_read() {
    let backend = Rc::new(RefCell::new(EnvBackend::default()));
    let opened = backend.borrow().opened.clone();
    let fullscreen = backend.borrow().fullscreen.clone();
    let announced = backend.borrow().announced.clone();

    // --- before: the shipped behavior this change fixes ---------------
    assert_eq!(
        runtime_shared::platform(),
        backend::Platform::Custom(""),
        "precondition: no platform installed on a fresh thread"
    );
    assert_eq!(runtime_shared::color_scheme(), backend::ColorScheme::Auto);
    runtime_shared::open_url("https://example.com/before");
    runtime_shared::set_fullscreen(true);
    runtime_shared::announce("before", backend::LiveRegionPriority::Polite);
    assert!(
        opened.borrow().is_empty() && fullscreen.borrow().is_empty() && announced.borrow().is_empty(),
        "precondition: the three routed services are no-ops until the seam runs"
    );

    // --- the seam -----------------------------------------------------
    runtime_vocabulary::backend::install_env_services(&backend);

    // --- after: every read reaches the backend ------------------------
    assert_eq!(
        runtime_shared::platform(),
        backend::Platform::Custom("EnvTest"),
        "platform() must report the booting backend's AppEnvOps::platform"
    );
    assert_eq!(
        runtime_shared::color_scheme(),
        backend::ColorScheme::Dark,
        "color_scheme() must report AppEnvOps::color_scheme"
    );

    runtime_shared::open_url("https://example.com/after");
    assert_eq!(
        opened.borrow().as_slice(),
        ["https://example.com/after"],
        "open_url() must route to AppEnvOps::url_opener"
    );

    runtime_shared::set_fullscreen(true);
    runtime_shared::set_fullscreen(false);
    assert_eq!(
        fullscreen.borrow().as_slice(),
        [true, false],
        "set_fullscreen() must route to AppEnvOps::fullscreen_setter"
    );

    runtime_shared::announce("saved", backend::LiveRegionPriority::Polite);
    assert_eq!(
        announced.borrow().as_slice(),
        ["saved"],
        "announce() must route to A11yOps::announce_for_accessibility"
    );
}

/// The announcer must hold the backend **weakly**.
///
/// It is the one env service that cannot be a self-contained closure —
/// `announce_for_accessibility` takes `&mut self`, so the closure has to
/// capture the backend handle. The `ANNOUNCER` thread-local is never
/// cleared on teardown, so a strong `Rc` there would pin the backend, its
/// `Node` graph, and every retained handle for the life of the thread.
/// On web that is the life of the page: a re-mount would leak an entire
/// prior view tree.
#[test]
fn announcer_captures_the_backend_weakly_and_degrades_after_teardown() {
    let dropped = Rc::new(RefCell::new(false));
    // Built field-by-field rather than with `..Default::default()`: the
    // functional-update form cannot move out of a `Drop` type.
    let mut b = EnvBackend::default();
    b.dropped = Some(dropped.clone());
    let backend = Rc::new(RefCell::new(b));

    runtime_vocabulary::backend::install_env_services(&backend);
    assert_eq!(
        Rc::strong_count(&backend),
        1,
        "the announcer must not add a strong reference to the backend"
    );

    // Teardown: the boot entry's handle goes away.
    drop(backend);
    assert!(
        *dropped.borrow(),
        "backend must actually drop — a strong capture in the ANNOUNCER \
         thread-local would pin it and its whole view tree"
    );

    // The stale slot degrades to the documented no-op rather than
    // panicking on a dangling upgrade.
    runtime_shared::announce("after teardown", backend::LiveRegionPriority::Assertive);
}

/// A backend that leaves a capability at its trait default gets the
/// documented no-op for that service and working behavior for the rest —
/// the seam must not require all-or-nothing.
#[test]
fn defaulted_capabilities_degrade_individually() {
    struct Bare;

    impl Host for Bare {
        type Node = ();
        fn insert(&mut self, _parent: &mut (), _child: ()) {}
        fn insert_at(&mut self, _parent: &mut (), _child: (), _index: usize) {}
        fn remove_child(&mut self, _parent: &(), _child: &()) {}
        fn clear_children(&mut self, _node: &()) {}
        fn create_anchor(&mut self) {}
        fn supports_splice(&self) -> bool {
            false
        }
    }

    // Everything defaulted except the platform identity.
    impl caps::AppEnvOps for Bare {
        fn platform(&self) -> backend::Platform {
            backend::Platform::Custom("Bare")
        }
    }
    impl caps::A11yOps for Bare {}

    let backend = Rc::new(RefCell::new(Bare));
    runtime_vocabulary::backend::install_env_services(&backend);

    assert_eq!(runtime_shared::platform(), backend::Platform::Custom("Bare"));
    // `url_opener`/`fullscreen_setter` defaulted to `None`; these must be
    // silent no-ops, not panics.
    runtime_shared::open_url("https://example.com");
    runtime_shared::set_fullscreen(true);
    runtime_shared::announce("x", backend::LiveRegionPriority::Polite);
}
