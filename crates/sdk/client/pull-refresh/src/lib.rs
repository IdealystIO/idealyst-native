//! Pull-to-refresh for any vertically scrolling child, drawn by the
//! platform's own refresh control.
//!
//! ```ignore
//! use pull_refresh::Refreshable;
//!
//! let loading = signal(false);
//! let reload: Rc<dyn Fn()> = Rc::new(move || {
//!     loading.set(true);
//!     spawn_reload(move || loading.set(false));
//! });
//! ui! {
//!     Refreshable(refreshing = loading, on_refresh = reload) {
//!         flat_list(data = rows, key = …, size = fixed_size(56.0), render = …)
//!     }
//! }
//! ```
//!
//! `Refreshable` wraps ONE child. It is not a scroll view: it gives the
//! scroll view its child renders the operating system's refresh control,
//! and the child keeps its own layout and style.
//!
//! - **iOS** — a `UIRefreshControl` set as `refreshControl` on the first
//!   vertical `UIScrollView` in the child's tree, the child itself first
//!   (a `scroll_view`, a `flat_list`'s collection view, a `virtual_grid`).
//!   UIKit draws the spinner, snaps, plays the haptic and holds the content
//!   down while refreshing.
//! - **Android** — androidx `SwipeRefreshLayout` around the child. A pull
//!   starts only while no scroller in the child's tree is scrolled away
//!   from its top.
//! - **Every other backend** (web, macOS, Windows, Linux, SSR, GPU, mocks)
//!   renders the child unchanged and attaches nothing.
//!
//! On every backend the wrapper is a layout-transparent node (an anchor:
//! web's `display: contents`, a Taffy contents node natively), so wrapping
//! a child never changes how it lays out.
//!
//! # The refreshing contract
//!
//! The control turns its spinner on when the user completes a pull, then
//! `on_refresh` runs. Set `refreshing` to `true` inside `on_refresh` and to
//! `false` when the work is done. If `refreshing` is still `false` once
//! `on_refresh` has returned and its writes are committed, the spinner ends
//! right away. Setting `refreshing` to `true` from elsewhere (an initial
//! load) shows the spinner too; on iOS the list scrolls down to reveal it
//! when it is at the top.
//!
//! # Limits
//!
//! - Top edge only.
//! - A child with no scroll view renders unchanged on iOS; on Android the
//!   layout still wraps it, and a pull on it refreshes.
//! - iOS finds the scroll view once, at mount. A child that swaps its
//!   scroller later (an `if` around the list) keeps the control on the
//!   first one; put the `Refreshable` inside the branch instead.
//! - `bounces = false` on the scroll view turns UIKit's pull off with it.

use std::rc::Rc;

use runtime_core::{component, Element};
use runtime_scene::{item, Host, MountCx, Registry};
use runtime_shared::Color;

#[cfg(all(target_os = "android", not(target_arch = "wasm32")))]
mod android;
// The binding and the policy drive only the native handlers; on every
// other target they are compiled for their host tests alone.
#[cfg_attr(not(any(target_os = "ios", target_os = "android")), allow(dead_code))]
mod binding;
#[cfg(all(target_os = "ios", not(target_arch = "wasm32")))]
mod ios;
#[cfg_attr(not(any(target_os = "ios", target_os = "android")), allow(dead_code))]
mod policy;

/// Pull-to-refresh around one scrolling child — see the crate docs for
/// what each platform draws.
///
/// ```ignore
/// ui! {
///     Refreshable(refreshing = loading, on_refresh = reload, tint = Some(Color("#2563eb".into()))) {
///         scroll_view() { … }
///     }
/// }
/// ```
///
/// The body returns the scene item directly rather than a `ui!` tree: the
/// item IS this SDK's registry payload, which `ui!` has no spelling for —
/// everything it renders is the child it was given.
#[component]
pub fn Refreshable(
    /// Whether a refresh is running. Set it `true` in `on_refresh` and
    /// `false` when the work finishes; setting it `true` from elsewhere
    /// shows the spinner too.
    refreshing: bool,
    /// Called when the user completes a pull.
    #[prop(default = Rc::new(|| {}) as Rc<dyn Fn()>)]
    on_refresh: Rc<dyn Fn()>,
    /// The spinner's color, as a CSS color. `None` keeps the platform's
    /// default; a color that does not parse is treated as `None`.
    tint: Option<Color>,
    /// The scrolling content. One child: on Android the refresh layout
    /// hit-tests and clips to the first one.
    children: Vec<Element>,
) -> Element {
    item(
        RefreshablePrim {
            props: binding::Props { refreshing, on_refresh, tint },
        },
        children,
    )
}

/// Scene payload — its `TypeId` is the registry key. Only the native
/// handlers read the props; the passthrough renders the child alone.
#[cfg_attr(not(any(target_os = "ios", target_os = "android")), allow(dead_code))]
struct RefreshablePrim {
    props: binding::Props,
}

/// The handler for hosts with no native refresh control: a layout-
/// transparent anchor with the child realized into it, so the child renders
/// exactly as it would unwrapped. The anchor needs no teardown of its own:
/// it leaves with the subtree.
fn mount_passthrough<H: Host>(
    cx: &mut MountCx<'_, H>,
    _prim: &Rc<RefreshablePrim>,
    children: Vec<Element>,
) -> H::Node {
    let mut node = cx.backend().borrow_mut().create_anchor();
    cx.realize_children_into(&mut node, children);
    node
}

/// Register the `Refreshable` handler on a scene registry — pass this with
/// the app's other SDK registrations to the boot entry.
///
/// One generic fn that type-dispatches once, at registration: a native
/// build also serves `Registry<HostMock>` for tests, so it downcasts to the
/// platform's concrete registry (`H: 'static` makes the registry `Any`),
/// installs the native handler on a hit and the passthrough on every other
/// host. Mount-path cost: zero.
pub fn register<H: Host + 'static>(registry: &mut Registry<H>) {
    #[cfg(all(target_os = "ios", not(target_arch = "wasm32")))]
    {
        let any: &mut dyn std::any::Any = registry;
        if let Some(reg) = any.downcast_mut::<Registry<backend_ios::IosBackend>>() {
            reg.register::<RefreshablePrim, _>(ios::mount);
            return;
        }
    }
    #[cfg(all(target_os = "android", not(target_arch = "wasm32")))]
    {
        let any: &mut dyn std::any::Any = registry;
        if let Some(reg) = any.downcast_mut::<Registry<backend_android::AndroidBackend>>() {
            reg.register::<RefreshablePrim, _>(android::mount);
            return;
        }
    }
    registry.register::<RefreshablePrim, _>(mount_passthrough::<H>);
}
