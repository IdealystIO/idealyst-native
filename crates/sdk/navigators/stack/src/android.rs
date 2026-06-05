//! Android-backend handler for the Stack navigator SDK.
//!
//! The FragmentManager + `RustNavigator` machinery (per-instance state,
//! dispatcher closures, JNI exports) lives in the
//! `android-navigator-helpers` crate, shared with tab + drawer. This
//! module's `AndroidStackHandler` is a thin wrapper: it constructs an
//! `AndroidNavCallbacks` from the framework-supplied `NavigatorHost`,
//! drives the helpers crate's `create_stack()` at init time, retains
//! the returned container `GlobalRef`, and forwards subsequent post-init
//! dispatch (`attach_initial` / `release` / `make_handle`) to the
//! matching helpers entry point.

use crate::{StackPresentation, StackScreenOptions};
use android_navigator_helpers::{AndroidNavCallbacks, AndroidScreenOptions, BarButton};
use backend_android::AndroidBackend;
use jni::objects::GlobalRef;
use runtime_core::{
    primitives::navigator::{MountResult, NavigatorHandler, NavigatorHost, NavigatorOps},
    NavigatorHandle,
};
use std::any::Any;
use std::rc::Rc;

pub struct AndroidStackHandler {
    container: Option<GlobalRef>,
}

impl AndroidStackHandler {
    pub fn new() -> Self {
        Self { container: None }
    }
}

impl Default for AndroidStackHandler {
    fn default() -> Self {
        Self::new()
    }
}

impl NavigatorHandler<AndroidBackend> for AndroidStackHandler {
    fn init(
        &mut self,
        backend: &mut AndroidBackend,
        host: NavigatorHost<GlobalRef>,
        _presentation: Rc<dyn Any>,
    ) -> GlobalRef {
        let NavigatorHost {
            initial_route,
            initial_path,
            defer_initial_mount,
            mount_screen,
            release_screen,
            match_path,
            nav_state,
            depth_changed,
            active_changed: _,
            control,
            build_node: _,
            build_node_into: _,
            build_in_screen: _,
            // `resolve_entry` + `base`: framework/web deep-link plumbing; the
            // Android stack handler doesn't read them.
            ..
        } = host;

        // Adapter: helpers-crate `mount_screen` is 2-arg `(name, params)`;
        // the substrate's host is 3-arg `(name, params, state)`. Discard
        // `state` for the stack-on-Android path — the helpers crate
        // doesn't currently thread per-screen state through the
        // fragment transaction.
        //
        // We also repack the SDK-side `StackScreenOptions` into the
        // helper-side `AndroidScreenOptions` here (mirroring the iOS
        // handler), so the helper's command dispatcher — which only
        // knows the helper type — can read `back_enabled` off the
        // mounted screen without depending on this SDK crate.
        let mount_2arg: Rc<dyn Fn(&'static str, Box<dyn Any>) -> MountResult<GlobalRef>> = {
            let m = mount_screen;
            Rc::new(move |name, params| {
                let result = m(name, params, None);
                let options: Box<dyn Any> =
                    if let Some(opts) = result.options.downcast_ref::<StackScreenOptions>() {
                        Box::new(translate_stack_options(opts))
                    } else if result.options.downcast_ref::<AndroidScreenOptions>().is_some() {
                        result.options
                    } else {
                        Box::new(AndroidScreenOptions::default())
                    };
                MountResult { node: result.node, scope_id: result.scope_id, options }
            })
        };

        let callbacks = AndroidNavCallbacks {
            initial_route,
            initial_path,
            mount_screen: mount_2arg,
            release_screen,
            match_path,
            depth_changed,
            nav_state,
            defer_initial_mount,
        };

        let node = android_navigator_helpers::create_stack(backend, callbacks, control);
        self.container = Some(node.clone());
        node
    }

    fn attach_initial(
        &mut self,
        _backend: &mut AndroidBackend,
        screen: GlobalRef,
        scope_id: u64,
        options: Box<dyn Any>,
    ) {
        let Some(container) = self.container.clone() else { return };
        let android_options = stack_options_to_android(options.downcast::<StackScreenOptions>().ok());
        android_navigator_helpers::attach_initial(&container, screen, scope_id, &android_options);
    }

    fn on_command(&mut self, _cmd: runtime_core::NavCommand) {
        unreachable!(
            "AndroidStackHandler::on_command — helpers::create_stack owns the \
             control-plane dispatcher"
        );
    }

    fn release(&mut self, _backend: &mut AndroidBackend) {
        if let Some(container) = self.container.take() {
            android_navigator_helpers::release(&container);
        }
    }

    fn make_handle(&self) -> NavigatorHandle {
        match self.container.as_ref() {
            Some(c) => android_navigator_helpers::make_handle(c),
            None => NavigatorHandle::new(Rc::new(()), &NoopStackOps),
        }
    }

    fn apply_slot_style(
        &mut self,
        _backend: &mut AndroidBackend,
        slot: &'static str,
        style: &Rc<runtime_core::StyleRules>,
    ) {
        let Some(container) = self.container.clone() else { return };
        match slot {
            "header" => android_navigator_helpers::apply_header_style(&container, style),
            "title" => android_navigator_helpers::apply_title_style(&container, style),
            "button" => android_navigator_helpers::apply_button_style(&container, style),
            "body" => android_navigator_helpers::apply_body_style(&container, style),
            _ => {}
        }
    }
}

struct NoopStackOps;
impl NavigatorOps for NoopStackOps {}

/// Translate the SDK's typed `StackScreenOptions` to the helpers
/// crate's `AndroidScreenOptions`. Returns the default empty options
/// when the downcast failed (which happens for screens that didn't
/// set any stack options via `.title(...)` / `.header_*(...)`).
fn stack_options_to_android(opts: Option<Box<StackScreenOptions>>) -> AndroidScreenOptions {
    match opts {
        Some(opts) => translate_stack_options(&opts),
        None => AndroidScreenOptions::default(),
    }
}

/// Translate the SDK's `StackScreenOptions` to the helper-side
/// `AndroidScreenOptions`. Shared by [`stack_options_to_android`] (the
/// `attach_initial` path) and the `mount_2arg` repack (the push /
/// replace / reset path), so `back_enabled` reaches the helper
/// dispatcher uniformly regardless of how the screen was mounted.
fn translate_stack_options(opts: &StackScreenOptions) -> AndroidScreenOptions {
    AndroidScreenOptions {
        title: opts.title.clone(),
        header_shown: opts.header_shown,
        header_left: opts.header_left.as_ref().map(|btn| BarButton {
            icon: btn.icon.clone(),
            on_press: btn.on_press.clone(),
        }),
        header_right: opts.header_right.as_ref().map(|btn| BarButton {
            icon: btn.icon.clone(),
            on_press: btn.on_press.clone(),
        }),
        header_background: opts.header_background.clone(),
        header_tint: opts.header_tint.clone(),
        title_color: opts.title_color.clone(),
        back_enabled: opts.back_enabled,
        fullscreen: opts.fullscreen,
        // `unmount_on_blur` is a stack-only knob; AndroidScreenOptions
        // shares its `mount_policy` slot with drawer/tab. The Android
        // stack helper today doesn't honor unmount-on-push semantics —
        // same blocker as iOS (`crates/sdk/navigators/stack/src/ios.rs`,
        // `translate_options`): `NavCommand::Push.params` is
        // non-`Clone` `Box<dyn Any>`, so the dispatcher has no way to
        // remember mount params for a remount-on-pop pass. The Android
        // helper would additionally need a "before the popped fragment
        // is revealed" hook similar to UIKit's `willShow:animated:`
        // delegate — likely a custom `FragmentTransaction.runOnCommit`
        // wedge so the fragment whose scope was released gets a fresh
        // mount before its view becomes visible. The field rides here
        // for surface symmetry; honoring it lands in the same patch
        // that lands iOS's path.
        mount_policy: None,
    }
}

/// Install the stack navigator handler on an Android backend. Call once at
/// startup so `Element::Navigator`s carrying a [`StackPresentation`]
/// resolve to this backend's chrome.
pub fn register(backend: &mut AndroidBackend) {
    backend.register_navigator::<StackPresentation, _>(|| Box::new(AndroidStackHandler::new()));
}

// Self-register at backend construction (no app-side `register` call needed).
// See [[project_inventory_self_registration]].
inventory::submit! {
    backend_android::AndroidNavigatorRegistrar(register)
}
