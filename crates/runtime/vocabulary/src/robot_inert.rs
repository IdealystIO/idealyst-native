//! `robot` without the `robot` feature: the calls an app makes from its
//! own code, compiled to nothing, so they need no `cfg`.
//!
//! An app can't gate them on a feature of its own: `idealyst dev` and
//! `idealyst test` turn on the FRAMEWORK's `robot` features (the
//! `idealyst/robot` facade, the backends), never the app crate's, so an
//! app-side `#[cfg(feature = "robot")]` block is compiled out of every dev
//! build too. The signatures match the robot build's, bounds included, so
//! code that compiles here compiles there.

use std::fmt::Debug;

use runtime_world::{Memo, ReadSignal, Signal};

/// Anything [`watch_signal`] accepts: the unified handle, the read half or
/// a memo output.
pub trait WatchTarget<T> {}

impl<T: PartialEq + Clone + Debug + 'static> WatchTarget<T> for Signal<T> {}
impl<T: PartialEq + Clone + Debug + 'static> WatchTarget<T> for ReadSignal<T> {}
impl<T: PartialEq + Clone + Debug + 'static> WatchTarget<T> for Memo<T> {}

/// Expose a signal to the robot bridge by name. Does nothing in a build
/// without the robot.
pub fn watch_signal<T, S>(_name: impl Into<String>, _signal: S)
where
    T: PartialEq + Clone + Debug + 'static,
    S: WatchTarget<T> + Copy + 'static,
{
}

/// [`watch_signal`] for a signal the bridge may also set. Does nothing in
/// a build without the robot.
pub fn watch_signal_writable<T>(_name: impl Into<String>, _signal: Signal<T>)
where
    T: PartialEq + Clone + Debug + serde::de::DeserializeOwned + 'static,
{
}

/// Whether an external test driver owns this app. Never, in a build
/// without the robot: nothing can drive it.
pub fn is_test_driven() -> bool {
    false
}

/// Mark the app as owned by an external test driver. Does nothing in a
/// build without the robot.
pub fn set_test_driven(_on: bool) {}
