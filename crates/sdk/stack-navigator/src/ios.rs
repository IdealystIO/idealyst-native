//! iOS-backend handler for the Stack navigator SDK.
//!
//! The UIKit machinery (`UINavigationController`, push/pop dispatch,
//! interactive-pop delegate, header chrome) lives in the
//! `ios-navigator-helpers` crate, shared with tab + drawer. This
//! module's `IosStackHandler` is a thin wrapper: it constructs an
//! `IosNavCallbacks` from the framework-supplied `NavigatorHost`,
//! drives the helpers crate's `create_stack()` at init time, retains
//! the returned container `IosNode`, and forwards subsequent post-init
//! dispatch (`attach_initial` / `release` / `make_handle` /
//! `apply_slot_style`) to the matching helpers entry point.

use crate::{BarButton, StackPresentation, StackScreenOptions, STACK_OPS};
use backend_ios::{IosBackend, IosNode};
use ios_navigator_helpers::{
    self as helpers, BarButton as HelpersBarButton, IosNavCallbacks, IosScreenOptions,
};
use runtime_core::primitives::navigator::{MountResult, NavigatorHandler, NavigatorHost};
use std::any::Any;
use std::rc::Rc;

pub struct IosStackHandler {
    container: Option<IosNode>,
}

impl IosStackHandler {
    pub fn new() -> Self {
        Self { container: None }
    }
}
impl Default for IosStackHandler {
    fn default() -> Self {
        Self::new()
    }
}

fn translate_bar_button(btn: &BarButton) -> HelpersBarButton {
    HelpersBarButton {
        icon: btn.icon.clone(),
        on_press: btn.on_press.clone(),
        tint: btn.tint.clone(),
    }
}

fn translate_options(opts: &StackScreenOptions) -> IosScreenOptions {
    IosScreenOptions {
        title: opts.title.clone(),
        header_shown: opts.header_shown,
        header_left: opts.header_left.as_ref().map(translate_bar_button),
        header_right: opts.header_right.as_ref().map(translate_bar_button),
        header_background: opts.header_background.clone(),
        header_tint: opts.header_tint.clone(),
        title_color: opts.title_color.clone(),
        // `unmount_on_blur` is currently a no-op on the iOS stack.
        // The field is plumbed through `StackScreenOptions` for API
        // surface symmetry with drawer/tab `MountPolicy`, but
        // honoring it requires three things this layer doesn't have
        // yet:
        //
        //   1. **Mount-params snapshot.** `NavCommand::Push.params`
        //      is `Box<dyn Any>` — owned, non-`Clone`, consumed by
        //      the first `mount_screen` call. Remounting after a
        //      pop needs a stored copy. Easiest fix is to switch
        //      `Box<dyn Any>` → `Rc<dyn Any>` on the command type
        //      so the dispatcher can keep a clone alongside the
        //      `ScreenEntry`; alternatively, expose a framework
        //      `remount_screen(scope_id)` that re-runs the
        //      route's original builder with the original payload
        //      transparently.
        //   2. **Pop-completion hook that fires BEFORE the
        //      revealed VC's `viewWillAppear`.** UIKit's
        //      `UINavigationControllerDelegate::didShow` runs AFTER
        //      the pop animation — too late to swap the revealed
        //      VC's content view without a visible flash. The
        //      cleanest hook is the navigation controller's
        //      `willShow:animated:` delegate method (already
        //      implementable since we own the delegate at
        //      `crates/sdk/ios-navigator-helpers/src/stack.rs:79`).
        //   3. **Per-`ScreenEntry` remount-needed marker.** The
        //      helper's `Vec<ScreenEntry>` would need to track a
        //      `mount_policy: MountPolicy` (or equivalent) per
        //      entry so the `willShow` hook knows which screens
        //      to rebuild and which to leave cached.
        //
        // Once (1) lands the rest is a straight refactor of
        // `stack.rs::create_stack`'s `Push`/`Pop` arms. Until then
        // the field rides as documentation of intent.
        mount_policy: None,
    }
}

impl NavigatorHandler<IosBackend> for IosStackHandler {
    fn init(
        &mut self,
        backend: &mut IosBackend,
        host: NavigatorHost<IosNode>,
        _presentation: Rc<dyn Any>,
    ) -> IosNode {
        let NavigatorHost {
            initial_route,
            initial_path,
            defer_initial_mount,
            mount_screen,
            release_screen,
            match_path: _,
            nav_state,
            depth_changed,
            active_changed: _,
            control,
            build_node: _,
            build_node_into: _,
            build_in_screen: _,
        } = host;

        // Adapter: the helpers-crate's `mount_screen` is 2-arg
        // `(name, params)`; the substrate's host is 3-arg
        // `(name, params, state)`. Discard `state` — the iOS stack
        // engine doesn't currently thread per-screen state through
        // UINavigationController, and no first-party iOS screen
        // reads `current_screen_state()`. The closure rewraps the
        // returned `MountResult.options` into an `IosScreenOptions`
        // so the helper engine can downcast it cleanly inside the
        // dispatcher.
        let mount_2arg: Rc<dyn Fn(&'static str, Box<dyn Any>) -> MountResult<IosNode>> = {
            let m = mount_screen;
            Rc::new(move |name, params| {
                let result = m(name, params, None);
                // If the screen carried `StackScreenOptions`, repack as
                // `IosScreenOptions` so the helper engine doesn't have
                // to know about SDK-side typed options.
                let new_options: Box<dyn Any> =
                    if let Some(opts) = result.options.downcast_ref::<StackScreenOptions>() {
                        Box::new(translate_options(opts))
                    } else if result.options.downcast_ref::<IosScreenOptions>().is_some() {
                        result.options
                    } else {
                        // No SDK-side options attached. Hand the helper
                        // a default `IosScreenOptions` so its
                        // downcast-and-apply path is a no-op.
                        Box::new(IosScreenOptions::default())
                    };
                MountResult {
                    node: result.node,
                    scope_id: result.scope_id,
                    options: new_options,
                }
            })
        };

        let callbacks = IosNavCallbacks {
            initial_route,
            initial_path,
            mount_screen: mount_2arg,
            release_screen,
            depth_changed,
            nav_state,
            defer_initial_mount,
        };

        let node = helpers::create_stack(backend.mtm(), callbacks, control);
        self.container = Some(node.clone());
        node
    }

    fn attach_initial(
        &mut self,
        backend: &mut IosBackend,
        screen: IosNode,
        scope_id: u64,
        options: Box<dyn Any>,
    ) {
        let Some(container) = self.container.clone() else { return };
        let ios_opts = options
            .downcast_ref::<StackScreenOptions>()
            .map(translate_options)
            .unwrap_or_default();
        helpers::stack_attach_initial(backend.mtm(), &container, screen, scope_id, &ios_opts);
    }

    fn on_command(&mut self, _cmd: runtime_core::NavCommand) {
        // `helpers::create_stack` installs the dispatcher closure on
        // the control plane at init time; commands route directly
        // through that closure and never reach the handler.
        unreachable!(
            "IosStackHandler::on_command — helpers::create_stack owns the \
             control-plane dispatcher"
        );
    }

    fn release(&mut self, _backend: &mut IosBackend) {
        if let Some(container) = self.container.take() {
            helpers::release_stack(&container);
        }
    }

    fn make_handle(&self) -> runtime_core::NavigatorHandle {
        match self.container.as_ref() {
            Some(c) => helpers::make_stack_handle(c),
            None => runtime_core::NavigatorHandle::new(Rc::new(()), &STACK_OPS),
        }
    }

    fn apply_slot_style(
        &mut self,
        _backend: &mut IosBackend,
        slot: &'static str,
        style: &Rc<runtime_core::StyleRules>,
    ) {
        let Some(container) = self.container.clone() else { return };
        match slot {
            "header" => helpers::apply_stack_header_style(&container, style),
            "title" => helpers::apply_stack_title_style(&container, style),
            "button" => helpers::apply_stack_button_style(&container, style),
            // `body` paints the `UINavigationController`'s root view —
            // the screen outlet that push/pop swap content inside.
            // Same role as Android's `apply_body_style` and web's
            // `apply_body_style`; without this, themed
            // `HeaderStyle.body_background` is silently dropped.
            "body" => helpers::apply_stack_body_style(&container, style),
            _ => {}
        }
    }
}

/// Install the stack navigator handler on an iOS backend. Call once at
/// startup so `Element::Navigator`s carrying a [`StackPresentation`]
/// resolve to this backend's chrome.
pub fn register(backend: &mut IosBackend) {
    backend.register_navigator::<StackPresentation, _>(|| Box::new(IosStackHandler::new()));
}
