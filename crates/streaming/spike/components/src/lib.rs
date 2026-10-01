//! The remote components themselves — an ordinary crate of `#[component]`s.
//!
//! The app links it directly (the in-binary fallback, and the native
//! baseline the spike measures against); `spike-fullguest` wraps it as a
//! bundle. Same source both ways. Kept separate from the bundle's `cdylib`
//! because a crate built as `cdylib` AND `rlib` loses link-time
//! optimization: the bundle measured 668 KB that way against 545 KB.

use runtime_core::{component, inject, signal, ui, Element, ReadSignal};

/// Context the app provides: who is signed in. Reactive — the app can
/// change it and the remote component follows.
#[derive(Clone)]
pub struct CurrentUser(pub ReadSignal<String>);

/// A remote component, written exactly like a native one.
#[component]
pub fn RemoteCounter(title: String, external: ReadSignal<i64>) -> Element {
    let clicks = signal(0i64);
    let user = inject::<CurrentUser>().map(|u| u.0);
    ui! {
        view() {
            text { "{title}" }
            text { "external: {external}" }
            text { "clicks: {clicks}" }
            button(label = "add", on_click = move || clicks.update(|c| c + 1))
            if let Some(user) = user {
                text { "signed in as {user}" }
            }
        }
    }
}
