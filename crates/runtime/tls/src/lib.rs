//! A drop-in [`thread_local!`] that costs Android one pthread key per
//! process instead of one per static.
//!
//! # Why
//!
//! Rust's Android targets have no native thread-locals (`tls-model:
//! emulated`, no `target_thread_local`), so std backs every
//! `thread_local!` static with its own pthread key, created the first time
//! the static is touched — a `const`-initialised `Cell<bool>` as much as a
//! `RefCell<HashMap>`. bionic allows 128 keys per process. The framework
//! declares a few hundred thread-locals, so an app that reaches enough of
//! them (a real touch, then the async executor, in the robot-e2e demo)
//! aborts in `LazyKey::lazy_init`.
//!
//! # How
//!
//! Off Android, [`thread_local!`] IS std's: the same expansion, the same
//! [`std::thread::LocalKey`]. On Android it declares a [`table::Key`]
//! instead, with `LocalKey`'s methods: each static owns an index into one
//! per-thread table of boxed values, which std stores behind two keys
//! (the table pointer, and a guard whose destructor drops the values when
//! the thread exits). Values initialise lazily on first access, as std's
//! do, and an access during a value's destruction re-initialises it, as
//! std's os-key storage does.
//!
//! Use it by importing the macro, which then shadows std's:
//!
//! ```
//! use runtime_tls::thread_local;
//! use std::cell::Cell;
//!
//! thread_local! {
//!     static DEPTH: Cell<u32> = const { Cell::new(0) };
//! }
//! DEPTH.set(DEPTH.get() + 1);
//! assert_eq!(DEPTH.get(), 1);
//! ```
//!
//! Code that names `std::thread::LocalKey` in a signature can't take an
//! Android key; nothing in the Android-linked crates does.

pub mod table;

/// `std::thread_local!`, or on Android a [`table::Key`] per static. See
/// the crate docs.
#[cfg(not(target_os = "android"))]
#[macro_export]
macro_rules! thread_local {
    ($($t:tt)*) => { ::std::thread_local! { $($t)* } };
}

/// `std::thread_local!`, or on Android a [`table::Key`] per static. See
/// the crate docs.
#[cfg(target_os = "android")]
#[macro_export]
macro_rules! thread_local {
    ($($t:tt)*) => { $crate::table_thread_local! { $($t)* } };
}

/// The table-backed form of [`thread_local!`] on every target — what
/// Android gets. Public so its behaviour is tested on any host.
#[macro_export]
macro_rules! table_thread_local {
    () => {};
    ($(#[$attr:meta])* $vis:vis static $name:ident: $t:ty = const $init:block; $($rest:tt)*) => {
        $crate::table_thread_local!(@one $(#[$attr])* $vis $name, $t, $init);
        $crate::table_thread_local!($($rest)*);
    };
    ($(#[$attr:meta])* $vis:vis static $name:ident: $t:ty = const $init:block) => {
        $crate::table_thread_local!(@one $(#[$attr])* $vis $name, $t, $init);
    };
    ($(#[$attr:meta])* $vis:vis static $name:ident: $t:ty = $init:expr; $($rest:tt)*) => {
        $crate::table_thread_local!(@one $(#[$attr])* $vis $name, $t, $init);
        $crate::table_thread_local!($($rest)*);
    };
    ($(#[$attr:meta])* $vis:vis static $name:ident: $t:ty = $init:expr) => {
        $crate::table_thread_local!(@one $(#[$attr])* $vis $name, $t, $init);
    };
    (@one $(#[$attr:meta])* $vis:vis $name:ident, $t:ty, $init:expr) => {
        $(#[$attr])* $vis static $name: $crate::table::Key<$t> = {
            fn __init() -> $t {
                $init
            }
            $crate::table::Key::new(__init)
        };
    };
}
