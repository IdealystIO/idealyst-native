//! Test fixtures: per-test setup (and teardown) declared once and asked for
//! by type — Playwright's fixtures, in Rust.
//!
//! A `#[robot_test]` parameter after the app handle is a fixture: any type
//! implementing [`Fixture`], taken by reference. The macro sets each one up
//! before the body, in parameter order, hands it to the body, and tears it
//! down afterwards in reverse order — also when the body (or a later
//! fixture's setup) panics, so a failed test can't leave a signed-in session
//! or seeded rows behind for the next one.
//!
//! ```ignore
//! use robot_test::{json, robot_test, App, Fixture};
//!
//! /// A clean server: the app's `test.reset` robot function wipes its data.
//! struct FreshServer;
//!
//! impl Fixture for FreshServer {
//!     fn setup(app: &mut App) -> Self {
//!         app.call_function("test.reset", json!({}));
//!         FreshServer
//!     }
//! }
//!
//! /// Signed in as a seeded user — on a fresh server.
//! struct SignedIn { email: String }
//!
//! impl Fixture for SignedIn {
//!     fn setup(app: &mut App) -> Self {
//!         FreshServer::setup(app); // a fixture can build on another
//!         let email = "ada@example.com".to_string();
//!         app.call_function("test.sign_in", json!({ "email": email }));
//!         SignedIn { email }
//!     }
//!
//!     fn teardown(&mut self, app: &mut App) {
//!         app.call_function("test.sign_out", json!({}));
//!     }
//! }
//!
//! #[robot_test]
//! fn shows_the_account(app: &mut App, user: &SignedIn) {
//!     app.test_id("account-email").assert_text(&user.email);
//! }
//!
//! #[robot_test]
//! fn starts_empty(app: &mut App, _: &FreshServer) {
//!     app.test_id("todo-row").assert_count(0);
//! }
//! ```

use crate::App;
use std::panic::{catch_unwind, resume_unwind, AssertUnwindSafe};

/// Per-test setup a `#[robot_test]` asks for by naming the type as a
/// parameter (`user: &SignedIn`). See the [module docs](self).
pub trait Fixture: Sized {
    /// Prepare the app for one test — reset data, seed rows, sign in,
    /// navigate — and return what the test needs to know about it. Panics
    /// fail the test, as an assertion does.
    fn setup(app: &mut App) -> Self;

    /// Undo what [`setup`](Self::setup) did that would leak into the next
    /// test. Runs after the body, whether it passed or panicked. Nothing,
    /// by default.
    fn teardown(&mut self, app: &mut App) {
        let _ = app;
    }
}

/// Set up `F`, run `body` with it, tear it down — the expansion of one
/// fixture parameter. A teardown that panics after the body already failed
/// is reported and swallowed, so the test fails with the body's message;
/// after a passing body it fails the test.
#[doc(hidden)]
pub fn __with_fixture<F: Fixture, R>(app: &mut App, body: impl FnOnce(&mut App, &mut F) -> R) -> R {
    let mut fixture = F::setup(app);
    let outcome = catch_unwind(AssertUnwindSafe(|| body(app, &mut fixture)));
    match outcome {
        Ok(value) => {
            fixture.teardown(app);
            value
        }
        Err(panic) => {
            if catch_unwind(AssertUnwindSafe(|| fixture.teardown(app))).is_err() {
                eprintln!(
                    "[robot_test] the teardown of {} panicked too, after the test failed",
                    std::any::type_name::<F>()
                );
            }
            resume_unwind(panic)
        }
    }
}
