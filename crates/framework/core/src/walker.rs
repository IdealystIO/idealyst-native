//! The render walker.
//!
//! `render(backend, primitive_tree)` is the entry point: it sets up a
//! reactive `Scope`, walks the primitive tree via `build`, hands the
//! resulting backend node off to `Backend::finish`, and returns an
//! `Owner` whose `Drop` tears down everything reactive that was
//! created.
//!
//! Internally this file owns:
//! - the `build` walker (dispatches on `Primitive` variant);
//! - per-primitive builders for everything mountable (Text, View,
//!   Button, Image, TextInput, Toggle, ScrollView, Slider, Video,
//!   WebView, ActivityIndicator, Graphics, Virtualizer, Navigator,
//!   Link, Overlay, Presence) plus the reactive-branching builders
//!   (`build_when`, `build_switch`);
//! - the style-attach machinery (static + reactive paths) and the
//!   theme cohort, so a `set_theme` re-applies every styled node in
//!   one pass instead of firing per-node effects;
//! - RAII cleanup wrappers (`GraphicsHandleCleanup`,
//!   `VirtualizerHandleCleanup`, `NavigatorHandleCleanup`,
//!   `OverlayHandleCleanup`, `StyleHandle`) that tell the backend to
//!   release a node when its surrounding `Scope` drops.
//!
//! Public surface from this module: just `render` and `Owner`. The
//! rest is implementation detail.

use crate::backend::{Backend, VirtualizerCallbacks};
use crate::handles::{RefFill, StateBits};
use crate::primitive::Primitive;
use crate::primitives;
use crate::reactive::{self, untrack, Effect, Ref, Signal};
use crate::scheduling::schedule_microtask;
use crate::sources::{StyleSource, TextSource};
use crate::style::{self, resolve as resolve_style, StyleApplication, StyleRules};
use std::any::Any;
use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;

#[cfg(feature = "debug-stats")]
use crate::debug;

// `pkind!` produces a `PrimitiveKind` tag when the debug feature is
// on, and `()` when off. Paired with `time_backend_create`, this keeps
// call sites identical between build modes without scattering
// `#[cfg]` attributes through the walker.
#[cfg(feature = "debug-stats")]
macro_rules! pkind {
    ($variant:ident) => { $crate::debug::PrimitiveKind::$variant };
}
#[cfg(not(feature = "debug-stats"))]
macro_rules! pkind {
    ($variant:ident) => { () };
}

/// Owns the reactive state created by a render call. Dropping the `Owner`
/// drops its `Scope`, which frees every signal and effect created during
/// rendering — no leaks across the boundary.
pub struct Owner {
    // Boxed so we can hand out a `&mut Scope` to `with_scope` calls inside
    // reactive subtree rebuilds without invalidating other references.
    // Field is dropped-only: it's never read, but its `Drop` impl is what
    // actually frees the arena slots.
    #[allow(dead_code)]
    scope: Box<reactive::Scope>,
}

#[must_use = "drop the Owner to dispose the UI; keep it alive to keep the UI reactive"]
pub fn render<B: Backend + 'static>(backend: Rc<RefCell<B>>, tree: Primitive) -> Owner {
    let mut scope = Box::new(reactive::Scope::new());
    let root = reactive::with_scope(&mut scope, || build(&backend, tree));
    backend.borrow_mut().finish(root);
    Owner { scope }
}

fn build<B: Backend + 'static>(backend: &Rc<RefCell<B>>, node: Primitive) -> B::Node {

    // Walker-level timing. Record the kind once on entry; the matching
    // exit fires after the match returns. Tag covers the full subtree
    // build (children inclusive). Each backend create call below
    // records its own narrower BackendCreate pair.
    #[cfg(feature = "debug-stats")]
    let _debug_kind = debug_kind_of(&node);
    #[cfg(feature = "debug-stats")]
    debug::record_build_enter(_debug_kind);

    // Robot: extract metadata and pre-register so children see us as parent.
    #[cfg(feature = "robot")]
    let robot_id = {
        if let Some(meta) = robot_extract_meta(&node) {
            use crate::robot::{self, RegistryEntry};
            let parent = robot::current_parent();
            let id = robot::register(RegistryEntry {
                kind: meta.kind,
                test_id: meta.test_id,
                label: meta.label,
                actions: meta.actions,
                parent,
                children: Vec::new(),
            });
            // Link child → parent.
            if let Some(pid) = parent {
                robot::add_child(pid, id);
            }
            robot::push_parent(id);
            Some(id)
        } else {
            None
        }
    };

    let result = match node {
        Primitive::Text { source, style, ref_fill, .. } => {
            let n = build_text(backend, source);
            if let Some(s) = style {
                attach_style(backend, &n, s);
            }
            if let Some(RefFill::Text(fill)) = ref_fill {
                let handle = backend.borrow().make_text_handle(&n);
                fill(handle);
            }
            n
        }
        Primitive::View { children, style, ref_fill, safe_area_sides, .. } => {
            let n = build_view(backend, children);
            if let Some(s) = style {
                attach_style(backend, &n, s);
            }
            if !safe_area_sides.is_empty() {
                attach_safe_area(backend, &n, safe_area_sides);
            }
            if let Some(RefFill::View(fill)) = ref_fill {
                let handle = backend.borrow().make_view_handle(&n);
                fill(handle);
            }
            n
        }
        Primitive::Pressable { children, on_click, style, ref_fill, disabled, .. } => {
            // Backend creates a bare tappable container with the
            // click handler bound. Children are inserted just like
            // View — the visual is entirely subtree-driven, no UA
            // chrome (no `<button>` border on web; no system
            // styling on native).
            let mut n = time_backend_create(pkind!(Pressable), || {
                backend.borrow_mut().create_pressable(on_click)
            });
            insert_children(backend, &mut n, children);
            // Same attach_style/disabled wiring as Button so
            // `state hovered`/`state pressed`/`state disabled`
            // overlays apply and the disabled-bit/native-inert
            // sync works identically.
            let state_setter = style.map(|s| attach_style(backend, &n, s));
            if let Some(RefFill::Pressable(fill)) = ref_fill {
                let handle = backend.borrow().make_pressable_handle(&n);
                fill(handle);
            }
            if let Some(d) = disabled {
                attach_disabled(backend, &n, d, state_setter);
            }
            n
        }
        Primitive::Button { label, on_click, leading_icon, trailing_icon, style, ref_fill, disabled, .. } => {
            // Pull the initial label from the source and create the
            // native widget with it. For reactive labels we install
            // an Effect below that calls `update_button_label` on
            // every signal change the closure subscribes to —
            // mirroring how Image's `src` works.
            let (initial_label, reactive_label) = match label {
                TextSource::Static(s) => (s, None),
                TextSource::Bound(d) => ((d.compute)(), Some(d.compute.clone())),
            };
            // `on_click` is an `Action` carrying both the runtime
            // callable (`fire`) and the structured metadata
            // (`method` + `inputs` + `output`). Backends pick what
            // they need from it — runtime backends call `fire`,
            // generator backends serialize the metadata.
            let n = time_backend_create(pkind!(Button), || {
                backend.borrow_mut().create_button(
                    &initial_label,
                    &on_click,
                    leading_icon.as_ref(),
                    trailing_icon.as_ref(),
                )
            });
            // attach_style returns the state setter so we can drive
            // the DISABLED bit reactively from `disabled` below. If
            // there's no style, we still need to react to disabled to
            // toggle the native widget's inert state, so allocate a
            // no-op-style setter route in that case.
            let state_setter = style.map(|s| attach_style(backend, &n, s));
            if let Some(RefFill::Button(fill)) = ref_fill {
                let handle = backend.borrow().make_button_handle(&n);
                fill(handle);
            }
            if let Some(d) = disabled {
                attach_disabled(backend, &n, d, state_setter);
            }
            // Reactive label effect. The first invocation re-reads
            // the closure (so the initial label and the first
            // effect run produce the same string), but signal reads
            // inside the closure subscribe this effect for future
            // updates.
            if let Some(f) = reactive_label {
                let backend = backend.clone();
                let node = n.clone();
                let _e = Effect::new(move || {
                    let s = f();
                    backend.borrow_mut().update_button_label(&node, &s);
                });
            }
            n
        }
        Primitive::Image { src, alt, style, ref_fill, .. } => {
            // Initial mount: call the source closure once for the
            // initial URL, then wrap it in an effect that updates the
            // image whenever signals it reads change.
            let initial = src();
            let n = time_backend_create(pkind!(Image), || backend.borrow_mut().create_image(&initial, alt.as_deref()));
            if let Some(s) = style {
                attach_style(backend, &n, s);
            }
            // Reactive src: if `src()` re-reads on subsequent fires,
            // the Effect subscribes and `update_image_src` re-runs.
            {
                let backend = backend.clone();
                let node = n.clone();
                let _e = Effect::new(move || {
                    let url = src();
                    backend.borrow_mut().update_image_src(&node, &url);
                });
            }
            if let Some(RefFill::Image(fill)) = ref_fill {
                let handle = backend.borrow().make_image_handle(&n);
                fill(handle);
            }
            n
        }
        Primitive::Icon { data, color, stroke, draw_in, style, ref_fill } => {
            let initial_color = color.as_ref().map(|f| f());
            let n = time_backend_create(pkind!(Icon), || {
                backend.borrow_mut().create_icon(&data, initial_color.as_ref())
            });
            if let Some(s) = style {
                attach_style(backend, &n, s);
            }
            // Reactive color effect.
            if let Some(f) = color {
                let backend = backend.clone();
                let node = n.clone();
                let _e = Effect::new(move || {
                    let c = f();
                    backend.borrow_mut().update_icon_color(&node, &c);
                });
            }
            // Reactive stroke progress effect.
            if let Some(f) = stroke {
                let initial = f();
                backend.borrow_mut().update_icon_stroke(&n, initial);
                let backend = backend.clone();
                let node = n.clone();
                let _e = Effect::new(move || {
                    let progress = f();
                    backend.borrow_mut().update_icon_stroke(&node, progress);
                });
            }
            // Mount draw-in animation: snap to `from`, then animate
            // to `to` on the next microtask.
            if let Some(anim) = draw_in {
                backend.borrow_mut().update_icon_stroke(&n, anim.from);
                let backend = backend.clone();
                let node = n.clone();
                let autoreverses = anim.autoreverses;
                schedule_microtask(move || {
                    backend.borrow_mut().animate_icon_stroke(
                        &node, anim.from, anim.to, anim.duration_ms, anim.easing,
                        anim.infinite, autoreverses,
                    );
                });
            }
            if let Some(RefFill::Icon(fill)) = ref_fill {
                let handle = backend.borrow().make_icon_handle(&n);
                fill(handle);
            }
            n
        }
        Primitive::TextInput { value, on_change, placeholder, style, ref_fill, .. } => {
            let initial = value.get();
            let n = time_backend_create(pkind!(TextInput), || {
                backend.borrow_mut().create_text_input(
                    &initial,
                    placeholder.as_deref(),
                    on_change,
                )
            });
            if let Some(s) = style {
                attach_style(backend, &n, s);
            }
            // Reactive: whenever the controlled signal changes, push
            // the new value into the widget. Setting to the same
            // value is a no-op on most platforms (web ignores no-change
            // sets on inputs).
            {
                let backend = backend.clone();
                let node = n.clone();
                let _e = Effect::new(move || {
                    let v = value.get();
                    backend.borrow_mut().update_text_input_value(&node, &v);
                });
            }
            if let Some(RefFill::TextInput(fill)) = ref_fill {
                let handle = backend.borrow().make_text_input_handle(&n);
                fill(handle);
            }
            n
        }
        Primitive::Toggle { value, on_change, style, ref_fill, .. } => {
            let initial = value.get();
            let n = time_backend_create(pkind!(Toggle), || backend.borrow_mut().create_toggle(initial, on_change));
            if let Some(s) = style {
                attach_style(backend, &n, s);
            }
            {
                let backend = backend.clone();
                let node = n.clone();
                let _e = Effect::new(move || {
                    let v = value.get();
                    backend.borrow_mut().update_toggle_value(&node, v);
                });
            }
            if let Some(RefFill::Toggle(fill)) = ref_fill {
                let handle = backend.borrow().make_toggle_handle(&n);
                fill(handle);
            }
            n
        }
        Primitive::ScrollView { children, horizontal, style, ref_fill, safe_area_sides } => {
            let mut n = time_backend_create(pkind!(ScrollView), || backend.borrow_mut().create_scroll_view(horizontal));
            insert_children(backend, &mut n, children);
            if let Some(s) = style {
                attach_style(backend, &n, s);
            }
            // ScrollViews route safe-area opt-in through the
            // *contentInset* path rather than padding the scroll view
            // itself. The scroll surface keeps its background
            // edge-to-edge; the content origin insets by the
            // safe-area amount and the user can scroll *under* the
            // bar (the iOS-native scroll pattern). Views use
            // `attach_safe_area` (outer padding); this is the only
            // place the two paths diverge.
            if !safe_area_sides.is_empty() {
                attach_scroll_view_safe_area_inset(backend, &n, safe_area_sides);
            }
            if let Some(RefFill::ScrollView(fill)) = ref_fill {
                let handle = backend.borrow().make_scroll_view_handle(&n);
                fill(handle);
            }
            n
        }
        Primitive::Slider { value, on_change, min, max, step, style, ref_fill, .. } => {
            let initial = value.get();
            // Wrap the user's on_change to snap to `step` first, so all
            // backends produce identical values regardless of native
            // step handling.
            let on_change_snap: Rc<dyn Fn(f32)> = if let Some(s) = step {
                let user = on_change.clone();
                let min_c = min;
                Rc::new(move |v| {
                    let snapped = min_c + ((v - min_c) / s).round() * s;
                    user(snapped);
                })
            } else {
                on_change.clone()
            };
            let n = time_backend_create(pkind!(Slider), || {
                backend.borrow_mut().create_slider(initial, min, max, step, on_change_snap)
            });
            if let Some(s) = style {
                attach_style(backend, &n, s);
            }
            // Reactive: write the controlled value back to the widget
            // whenever the signal changes.
            {
                let backend = backend.clone();
                let node = n.clone();
                let _e = Effect::new(move || {
                    let v = value.get();
                    backend.borrow_mut().update_slider_value(&node, v);
                });
            }
            if let Some(RefFill::Slider(fill)) = ref_fill {
                let handle = backend.borrow().make_slider_handle(&n);
                fill(handle);
            }
            n
        }
        Primitive::WebView { url, style, ref_fill } => {
            let initial = url();
            let n = time_backend_create(pkind!(WebView), || backend.borrow_mut().create_web_view(&initial));
            if let Some(s) = style {
                attach_style(backend, &n, s);
            }
            {
                let backend = backend.clone();
                let node = n.clone();
                let _e = Effect::new(move || {
                    let u = url();
                    backend.borrow_mut().update_web_view_url(&node, &u);
                });
            }
            if let Some(RefFill::WebView(fill)) = ref_fill {
                let handle = backend.borrow().make_web_view_handle(&n);
                fill(handle);
            }
            n
        }
        Primitive::Video { src, autoplay, controls, loop_playback, style, ref_fill } => {
            let initial = src();
            let n = time_backend_create(pkind!(Video), || {
                backend.borrow_mut().create_video(&initial, autoplay, controls, loop_playback)
            });
            if let Some(s) = style {
                attach_style(backend, &n, s);
            }
            {
                let backend = backend.clone();
                let node = n.clone();
                let _e = Effect::new(move || {
                    let s = src();
                    backend.borrow_mut().update_video_src(&node, &s);
                });
            }
            if let Some(RefFill::Video(fill)) = ref_fill {
                let handle = backend.borrow().make_video_handle(&n);
                fill(handle);
            }
            n
        }
        Primitive::ActivityIndicator { size, color, style, ref_fill } => {
            let n = time_backend_create(pkind!(ActivityIndicator), || {
                backend.borrow_mut().create_activity_indicator(size, color.as_ref())
            });
            if let Some(s) = style {
                attach_style(backend, &n, s);
            }
            if let Some(RefFill::ActivityIndicator(fill)) = ref_fill {
                let handle = backend.borrow().make_activity_indicator_handle(&n);
                fill(handle);
            }
            n
        }
        Primitive::Virtualizer {
            item_count,
            item_key,
            item_size,
            render_item,
            row_template,
            row_index_signal_id,
            overscan,
            horizontal,
            style,
            ref_fill,
        } => {
            // Dispatch on whether the backend opts into the
            // structured / slot-capture path AND whether the
            // Virtualizer carries the structured metadata
            // (row_template) needed for that path. Otherwise fall
            // through to the runtime closure path that drives
            // native virtualization on iOS/Android/Web.
            let lazy = backend.borrow().supports_lazy_slot_capture();
            let n = if lazy && row_template.is_some() && !item_count.is_opaque() {
                build_virtualizer_declarative(
                    backend,
                    item_count,
                    *row_template.unwrap(),
                    row_index_signal_id,
                    horizontal,
                )
            } else {
                build_virtualizer(
                    backend,
                    item_count,
                    item_key,
                    item_size,
                    render_item,
                    row_template,
                    row_index_signal_id,
                    overscan,
                    horizontal,
                )
            };
            if let Some(s) = style {
                attach_style(backend, &n, s);
            }
            // Cleanup hook: when the surrounding scope drops, this
            // Effect drops, dropping `cleanup`, which calls
            // `release_virtualizer`. Without this, the backend's
            // queued scroll/resize events keep firing into
            // user-supplied callbacks whose captured `Signal`s have
            // been freed → "signal used after its scope was
            // dropped" panic. Same shape as the Graphics cleanup
            // below.
            {
                let cleanup = VirtualizerHandleCleanup {
                    backend: backend.clone(),
                    node: n.clone(),
                };
                let _e = Effect::new(move || {
                    let _ = &cleanup.node;
                });
            }
            if let Some(RefFill::Virtualizer(fill)) = ref_fill {
                let handle = backend.borrow().make_virtualizer_handle(&n);
                fill(handle);
            }
            n
        }
        Primitive::Graphics { on_ready, on_resize, on_lost, style, ref_fill } => {
            let n = time_backend_create(pkind!(Graphics), || {
                backend.borrow_mut().create_graphics(on_ready, on_resize, on_lost)
            });
            if let Some(s) = style {
                attach_style(backend, &n, s);
            }
            // Install an unconditional cleanup hook. The empty Effect
            // captures a `GraphicsHandleCleanup` whose Drop calls
            // `release_graphics`. Independent of the style effect so
            // unstyled Graphics still get torn down. Same scope-drop
            // mechanics: `when()` branch flips, list recycling, and
            // `Owner` drop all cascade through here.
            {
                let cleanup = GraphicsHandleCleanup {
                    backend: backend.clone(),
                    node: n.clone(),
                };
                let _e = Effect::new(move || {
                    let _ = &cleanup.node;
                });
            }
            if let Some(RefFill::Graphics(fill)) = ref_fill {
                let handle = backend.borrow().make_graphics_handle(&n);
                fill(handle);
            }
            n
        }
        Primitive::Navigator(nav) => {
            let primitives::navigator::Navigator {
                initial,
                initial_path,
                screens,
                layout,
                default_options,
                style,
                header_style,
                title_style,
                button_style,
                ref_fill,
            } = *nav;
            let n = build_navigator(backend, initial, initial_path, screens, layout, default_options, ref_fill);
            if let Some(s) = style {
                attach_style(backend, &n, s);
            }
            if let Some(s) = header_style {
                attach_navigator_slot_style(backend, &n, s, |b, node, rules| {
                    b.borrow_mut().apply_navigator_header_style(node, rules);
                });
            }
            if let Some(s) = title_style {
                attach_navigator_slot_style(backend, &n, s, |b, node, rules| {
                    b.borrow_mut().apply_navigator_title_style(node, rules);
                });
            }
            if let Some(s) = button_style {
                attach_navigator_slot_style(backend, &n, s, |b, node, rules| {
                    b.borrow_mut().apply_navigator_button_style(node, rules);
                });
            }
            // Cleanup: when the surrounding scope drops, this empty
            // Effect drops, dropping the `NavigatorHandleCleanup`,
            // which tells the backend to tear down its native stack.
            // Same pattern as Virtualizer / Graphics.
            {
                let cleanup = NavigatorHandleCleanup {
                    backend: backend.clone(),
                    node: n.clone(),
                };
                let _e = Effect::new(move || {
                    let _ = &cleanup.node;
                });
            }
            n
        }
        Primitive::TabNavigator(nav) => {
            let primitives::navigator::TabNavigator {
                initial,
                initial_path,
                tab_order,
                screens,
                layout,
                placement,
                mount_policy,
                default_options,
                style,
                header_style,
                title_style,
                button_style,
                tab_bar_style,
                tab_icon_style,
                tab_label_style,
                ref_fill,
            } = *nav;
            let n = build_tab_navigator(
                backend,
                initial,
                initial_path,
                tab_order,
                screens,
                layout,
                placement,
                mount_policy,
                default_options,
                ref_fill,
            );
            if let Some(s) = style {
                attach_style(backend, &n, s);
            }
            if let Some(s) = header_style {
                attach_navigator_slot_style(backend, &n, s, |b, node, rules| {
                    b.borrow_mut().apply_navigator_header_style(node, rules);
                });
            }
            if let Some(s) = title_style {
                attach_navigator_slot_style(backend, &n, s, |b, node, rules| {
                    b.borrow_mut().apply_navigator_title_style(node, rules);
                });
            }
            if let Some(s) = button_style {
                attach_navigator_slot_style(backend, &n, s, |b, node, rules| {
                    b.borrow_mut().apply_navigator_button_style(node, rules);
                });
            }
            if let Some(s) = tab_bar_style {
                attach_navigator_slot_style(backend, &n, s, |b, node, rules| {
                    b.borrow_mut().apply_tab_bar_style(node, rules);
                });
            }
            if let Some(s) = tab_icon_style {
                attach_navigator_slot_style(backend, &n, s, |b, node, rules| {
                    b.borrow_mut().apply_tab_icon_style(node, rules);
                });
            }
            if let Some(s) = tab_label_style {
                attach_navigator_slot_style(backend, &n, s, |b, node, rules| {
                    b.borrow_mut().apply_tab_label_style(node, rules);
                });
            }
            {
                let cleanup = TabNavigatorHandleCleanup {
                    backend: backend.clone(),
                    node: n.clone(),
                };
                let _e = Effect::new(move || {
                    let _ = &cleanup.node;
                });
            }
            n
        }
        Primitive::DrawerNavigator(nav) => {
            let primitives::navigator::DrawerNavigator {
                initial,
                initial_path,
                screens,
                layout,
                content,
                side,
                drawer_type,
                drawer_width,
                swipe_to_open,
                mount_policy,
                default_options,
                style,
                header_style,
                title_style,
                button_style,
                sidebar_style,
                scrim_style,
                ref_fill,
            } = *nav;
            let n = build_drawer_navigator(
                backend,
                initial,
                initial_path,
                screens,
                layout,
                content,
                side,
                drawer_type,
                drawer_width,
                swipe_to_open,
                mount_policy,
                default_options,
                ref_fill,
            );
            if let Some(s) = style {
                attach_style(backend, &n, s);
            }
            if let Some(s) = header_style {
                attach_navigator_slot_style(backend, &n, s, |b, node, rules| {
                    b.borrow_mut().apply_navigator_header_style(node, rules);
                });
            }
            if let Some(s) = title_style {
                attach_navigator_slot_style(backend, &n, s, |b, node, rules| {
                    b.borrow_mut().apply_navigator_title_style(node, rules);
                });
            }
            if let Some(s) = button_style {
                attach_navigator_slot_style(backend, &n, s, |b, node, rules| {
                    b.borrow_mut().apply_navigator_button_style(node, rules);
                });
            }
            if let Some(s) = sidebar_style {
                attach_navigator_slot_style(backend, &n, s, |b, node, rules| {
                    b.borrow_mut().apply_drawer_sidebar_style(node, rules);
                });
            }
            if let Some(s) = scrim_style {
                attach_navigator_slot_style(backend, &n, s, |b, node, rules| {
                    b.borrow_mut().apply_drawer_scrim_style(node, rules);
                });
            }
            {
                let cleanup = DrawerNavigatorHandleCleanup {
                    backend: backend.clone(),
                    node: n.clone(),
                };
                let _e = Effect::new(move || {
                    let _ = &cleanup.node;
                });
            }
            n
        }
        Primitive::When { cond, then, otherwise, style } => {
            // Two paths: declarative (the backend wants structured
            // metadata + both pre-built branches) or closure-based
            // (the existing Effect path that rebuilds the active
            // branch on signal change). The choice depends on (a)
            // whether `cond` carries structured metadata at all and
            // (b) whether the backend opts into the slot-capture
            // path for generator-style realization.
            let lazy = backend.borrow().supports_lazy_slot_capture();
            let n = if !cond.is_opaque() && lazy {
                build_when_declarative(backend, cond, then, otherwise)
            } else {
                build_when(backend, cond, then, otherwise)
            };
            if let Some(s) = style {
                attach_style(backend, &n, s);
            }
            n
        }
        Primitive::Switch { discriminant, arms, default, style } => {
            let lazy = backend.borrow().supports_lazy_slot_capture();
            let n = if !discriminant.is_opaque() && lazy && !arms.is_empty() {
                build_switch_declarative(backend, discriminant, arms, default)
            } else {
                build_switch(backend, discriminant, arms, default)
            };
            if let Some(s) = style {
                attach_style(backend, &n, s);
            }
            n
        }
        Primitive::Link {
            children,
            route,
            url,
            make_params,
            kind,
            target,
            style,
            ref_fill,
        } => {
            let on_activate = primitives::link::make_on_activate(
                target,
                route,
                url.clone(),
                kind,
                make_params,
            );
            let config = primitives::link::LinkConfig {
                route,
                url,
                on_activate,
            };
            let mut n = time_backend_create(pkind!(Link), || {
                backend.borrow_mut().create_link(config)
            });
            // Children are built recursively (same shape as View)
            // and inserted into the link's native container. The
            // backend is responsible for making the container
            // tappable / clickable as a whole; children are just
            // visual content.
            insert_children(backend, &mut n, children);
            if let Some(s) = style {
                attach_style(backend, &n, s);
            }
            if let Some(RefFill::Link(fill)) = ref_fill {
                let handle = backend.borrow().make_link_handle(&n);
                fill(handle);
            }
            n
        }
        Primitive::Overlay {
            children,
            placement,
            backdrop,
            backdrop_style,
            on_dismiss,
            trap_focus,
            style,
            ref_fill,
        } => {
            // Hand the platform-side floating layer everything it
            // needs to position itself + wire system dismissal at
            // creation time. The framework drives mount/unmount via
            // the surrounding scope (the host's open-state signal
            // flipping a `when` triggers our release wrapper);
            // backends don't have to manage the open state
            // themselves.
            let dismiss_for_backend = on_dismiss.clone();
            let mut n = time_backend_create(pkind!(Overlay), || {
                backend.borrow_mut().create_overlay(
                    placement,
                    backdrop,
                    dismiss_for_backend,
                    trap_focus,
                )
            });

            insert_children(backend, &mut n, children);

            if let Some(s) = style {
                attach_style(backend, &n, s);
            }
            if let Some(bs) = backdrop_style {
                let backend_clone = backend.clone();
                let node_for_backdrop = n.clone();
                let _e = Effect::new(move || {
                    let app = match &bs {
                        StyleSource::Static(a) => a.clone(),
                        StyleSource::Reactive(f) => f(),
                    };
                    let resolved = style::resolve(&app);
                    backend_clone
                        .borrow_mut()
                        .apply_overlay_backdrop_style(&node_for_backdrop, &resolved);
                });
            }

            if let Some(RefFill::Overlay(fill)) = ref_fill {
                let handle = backend.borrow().make_overlay_handle(&n);
                fill(handle);
            }

            let cleanup = OverlayHandleCleanup {
                backend: backend.clone(),
                node: n.clone(),
            };
            let _cleanup_effect = Effect::new(move || {
                let _ = &cleanup;
            });

            n
        }
        Primitive::AnchoredOverlay {
            children,
            target,
            side,
            align,
            offset,
            backdrop,
            backdrop_style,
            on_dismiss,
            trap_focus,
            style,
            ref_fill,
        } => {
            // Same lifecycle pattern as `Overlay` above — the only
            // difference is the backend hook we route to (so backends
            // can dispatch element-anchored cases to a different
            // native presentation API). Cleanup goes through the
            // separate `release_anchored_overlay` so backends can
            // tear down the anchor tracker / native popup distinctly
            // from their viewport-overlay machinery.
            let dismiss_for_backend = on_dismiss.clone();
            let mut n = time_backend_create(pkind!(AnchoredOverlay), || {
                backend.borrow_mut().create_anchored_overlay(
                    target,
                    side,
                    align,
                    offset,
                    backdrop,
                    dismiss_for_backend,
                    trap_focus,
                )
            });

            insert_children(backend, &mut n, children);

            if let Some(s) = style {
                attach_style(backend, &n, s);
            }
            if let Some(bs) = backdrop_style {
                let backend_clone = backend.clone();
                let node_for_backdrop = n.clone();
                let _e = Effect::new(move || {
                    let app = match &bs {
                        StyleSource::Static(a) => a.clone(),
                        StyleSource::Reactive(f) => f(),
                    };
                    let resolved = style::resolve(&app);
                    backend_clone
                        .borrow_mut()
                        .apply_anchored_overlay_backdrop_style(&node_for_backdrop, &resolved);
                });
            }

            if let Some(RefFill::AnchoredOverlay(fill)) = ref_fill {
                let handle = backend.borrow().make_anchored_overlay_handle(&n);
                fill(handle);
            }

            let cleanup = AnchoredOverlayHandleCleanup {
                backend: backend.clone(),
                node: n.clone(),
            };
            let _cleanup_effect = Effect::new(move || {
                let _ = &cleanup;
            });

            n
        }
        Primitive::Presence { child, present, enter, exit, ref_fill } => {
            let n = build_presence(backend, child, present, enter, exit);
            if let Some(RefFill::Presence(fill)) = ref_fill {
                let handle = backend.borrow().make_presence_handle(&n);
                fill(handle);
            }
            n
        }
        Primitive::Repeat { .. } => {
            // `Repeat` represents N sibling nodes, not a single
            // node. It can only appear inside a parent's children
            // list, where `insert_children` expands it inline.
            // Reaching this arm means a `Repeat` was used outside
            // a children context — author or macro bug.
            panic!(
                "Primitive::Repeat encountered as a standalone subtree root. \
                 Repeat is a children-list primitive (used for `for` loops \
                 inside `ui!`); it cannot be the result of a `build()` call \
                 on its own. Wrap it in a View / ScrollView / fragment."
            );
        }
    };

    #[cfg(feature = "debug-stats")]
    debug::record_build_exit(_debug_kind);

    // Robot: wire frame-reading closures now that the backend node
    // exists. Each closure captures the node + backend Rc; they're
    // called on demand by `Robot::frame` / `Robot::absolute_frame`
    // via the bridge or in-app paths.
    #[cfg(feature = "robot")]
    if let Some(id) = robot_id {
        let node_for_frame = result.clone();
        let node_for_abs = result.clone();
        let backend_for_frame = backend.clone();
        let backend_for_abs = backend.clone();
        crate::robot::attach_frame_actions(
            id,
            Rc::new(move || backend_for_frame.borrow().frame(&node_for_frame)),
            Rc::new(move || backend_for_abs.borrow().absolute_frame(&node_for_abs)),
        );
    }

    // Robot: pop parent stack now that children are built.
    #[cfg(feature = "robot")]
    if robot_id.is_some() {
        crate::robot::pop_parent();
    }

    result
}

/// Wrap a backend create call with BackendCreate enter/exit recording.
/// When `debug-stats` is off this is a transparent passthrough — both
/// the kind argument and the wrapper itself become no-ops the compiler
/// inlines away.
#[inline(always)]
#[cfg(feature = "debug-stats")]
fn time_backend_create<R>(kind: debug::PrimitiveKind, f: impl FnOnce() -> R) -> R {
    debug::record_backend_create_enter(kind);
    let r = f();
    debug::record_backend_create_exit(kind);
    r
}

/// No-op variant: the `kind` parameter doesn't even exist, so call
/// sites pass `()` instead. Keeps the call-site shape identical to the
/// debug-on path while emitting nothing when off.
#[inline(always)]
#[cfg(not(feature = "debug-stats"))]
fn time_backend_create<R>(_kind: (), f: impl FnOnce() -> R) -> R {
    f()
}

// (`pkind!` is defined near the top of this module so it's in scope
// for all callers below.)

/// Map a primitive to the coarse-grained `PrimitiveKind` tag used by
/// debug events. Only compiled when `debug-stats` is enabled.
#[cfg(feature = "debug-stats")]
fn debug_kind_of(node: &Primitive) -> debug::PrimitiveKind {
    use debug::PrimitiveKind;
    match node {
        Primitive::Text { .. } => PrimitiveKind::Text,
        Primitive::View { .. } => PrimitiveKind::View,
        Primitive::Button { .. } => PrimitiveKind::Button,
        Primitive::Pressable { .. } => PrimitiveKind::Pressable,
        Primitive::Image { .. } => PrimitiveKind::Image,
        Primitive::Icon { .. } => PrimitiveKind::Icon,
        Primitive::TextInput { .. } => PrimitiveKind::TextInput,
        Primitive::Toggle { .. } => PrimitiveKind::Toggle,
        Primitive::ScrollView { .. } => PrimitiveKind::ScrollView,
        Primitive::Slider { .. } => PrimitiveKind::Slider,
        Primitive::WebView { .. } => PrimitiveKind::WebView,
        Primitive::Video { .. } => PrimitiveKind::Video,
        Primitive::ActivityIndicator { .. } => PrimitiveKind::ActivityIndicator,
        Primitive::Virtualizer { .. } => PrimitiveKind::Virtualizer,
        Primitive::Graphics { .. } => PrimitiveKind::Graphics,
        Primitive::Navigator(_) => PrimitiveKind::Navigator,
        Primitive::TabNavigator(_) => PrimitiveKind::TabNavigator,
        Primitive::DrawerNavigator(_) => PrimitiveKind::DrawerNavigator,
        Primitive::When { .. } => PrimitiveKind::When,
        Primitive::Switch { .. } => PrimitiveKind::Switch,
        Primitive::Link { .. } => PrimitiveKind::Link,
        Primitive::Overlay { .. } => PrimitiveKind::Overlay,
        Primitive::AnchoredOverlay { .. } => PrimitiveKind::AnchoredOverlay,
        Primitive::Presence { .. } => PrimitiveKind::Presence,
        // Repeat is expanded into siblings by `insert_children`
        // and never reaches the build walker as a standalone
        // subtree, so this arm is dead in practice. Tag as View
        // to keep the debug timing breakdown defined.
        Primitive::Repeat { .. } => PrimitiveKind::View,
    }
}

// =============================================================================
// Robot: metadata extraction from Primitive before destructuring.
// =============================================================================

#[cfg(feature = "robot")]
struct RobotMeta {
    kind: crate::robot::ElementKind,
    test_id: Option<&'static str>,
    label: Option<String>,
    actions: crate::robot::ElementActions,
}

/// Extract robot-relevant metadata from a primitive *before* the move
/// match destructures it. Only interactive primitives (buttons,
/// inputs, etc.) produce a `Some`; structural primitives that aren't
/// useful to query (When, Switch, Repeat) produce `None`.
#[cfg(feature = "robot")]
fn robot_extract_meta(node: &Primitive) -> Option<RobotMeta> {
    use crate::robot::{ElementActions, ElementKind};
    use crate::sources::TextSource;

    match node {
        Primitive::View { test_id, .. } => Some(RobotMeta {
            kind: ElementKind::View,
            test_id: *test_id,
            label: None,
            actions: ElementActions::empty(),
        }),
        Primitive::Text { source, test_id, .. } => {
            let label = match source {
                TextSource::Static(s) => Some(s.clone()),
                TextSource::Bound(d) => Some((d.compute)()),
            };
            Some(RobotMeta {
                kind: ElementKind::Text,
                test_id: *test_id,
                label,
                actions: ElementActions::empty(),
            })
        }
        Primitive::Button { label, on_click, test_id, .. } => {
            let label_text = match label {
                TextSource::Static(s) => Some(s.clone()),
                TextSource::Bound(d) => Some((d.compute)()),
            };
            let click = on_click.clone();
            Some(RobotMeta {
                kind: ElementKind::Button,
                test_id: *test_id,
                label: label_text,
                actions: ElementActions {
                    click: Some(click),
                    ..ElementActions::empty()
                },
            })
        }
        Primitive::Pressable { on_click, test_id, .. } => {
            let click = on_click.clone();
            Some(RobotMeta {
                kind: ElementKind::Pressable,
                test_id: *test_id,
                label: None,
                actions: ElementActions {
                    click: Some(click),
                    ..ElementActions::empty()
                },
            })
        }
        Primitive::Image { test_id, .. } => Some(RobotMeta {
            kind: ElementKind::Image,
            test_id: *test_id,
            label: None,
            actions: ElementActions::empty(),
        }),
        Primitive::TextInput { on_change, test_id, .. } => {
            let set_text = on_change.clone();
            Some(RobotMeta {
                kind: ElementKind::TextInput,
                test_id: *test_id,
                label: None,
                actions: ElementActions {
                    set_text: Some(set_text),
                    ..ElementActions::empty()
                },
            })
        }
        Primitive::Toggle { on_change, test_id, .. } => {
            let set_toggle = on_change.clone();
            Some(RobotMeta {
                kind: ElementKind::Toggle,
                test_id: *test_id,
                label: None,
                actions: ElementActions {
                    set_toggle: Some(set_toggle),
                    ..ElementActions::empty()
                },
            })
        }
        Primitive::Slider { on_change, test_id, .. } => {
            let set_slider = on_change.clone();
            Some(RobotMeta {
                kind: ElementKind::Slider,
                test_id: *test_id,
                label: None,
                actions: ElementActions {
                    set_slider: Some(set_slider),
                    ..ElementActions::empty()
                },
            })
        }
        Primitive::Link { route, url, make_params, kind, target, .. } => {
            // Build the same on_activate the backend wires onto the
            // native tap target so the robot's `click` triggers the
            // navigator just like a real tap would.
            let click = primitives::link::make_on_activate(
                target.clone(),
                route,
                url.clone(),
                *kind,
                make_params.clone(),
            );
            Some(RobotMeta {
                kind: ElementKind::Link,
                test_id: None,
                label: None,
                actions: ElementActions {
                    click: Some(click),
                    ..ElementActions::empty()
                },
            })
        }
        Primitive::Navigator(_) => Some(RobotMeta {
            kind: ElementKind::Navigator,
            test_id: None,
            label: None,
            actions: ElementActions::empty(),
        }),
        Primitive::TabNavigator(_) => Some(RobotMeta {
            kind: ElementKind::TabNavigator,
            test_id: None,
            label: None,
            actions: ElementActions::empty(),
        }),
        Primitive::DrawerNavigator(_) => Some(RobotMeta {
            kind: ElementKind::DrawerNavigator,
            test_id: None,
            label: None,
            actions: ElementActions::empty(),
        }),
        // Structural/reactive primitives don't get registered.
        _ => None,
    }
}

/// Builds a Text primitive (static or reactive). Style application is
/// handled by the caller via `attach_style` so the content effect and
/// the style effect stay independent.
fn build_text<B: Backend + 'static>(
    backend: &Rc<RefCell<B>>,
    source: TextSource,
) -> B::Node {
    match source {
        TextSource::Static(content) => {
            time_backend_create(pkind!(Text), || backend.borrow_mut().create_text(&content))
        }
        TextSource::Bound(d) => {
            // Create the text node (empty initially — the Effect's
            // first run populates it). Inform the backend of any
            // structured metadata BEFORE the Effect kicks in so
            // generator backends can declare the binding before
            // the runtime path emits an `UpdateText`. Then the
            // Effect drives the closure path identically to before
            // the refactor.
            let node = time_backend_create(pkind!(Text), || {
                backend.borrow_mut().create_text("")
            });
            // Only surface structured metadata when the binding
            // actually has any. Opaque Deriveds (closure-only
            // coercions) skip this entirely — generator backends
            // wouldn't be able to do anything useful with an empty
            // method name.
            if !d.is_opaque() {
                let mut b = backend.borrow_mut();
                for (sid, val) in d.inputs.iter().zip(d.initial.iter()) {
                    b.note_signal_initial(*sid, val);
                }
                b.note_text_binding(&node, &d.inputs, d.method);
            }
            let compute = d.compute.clone();
            let node_for_effect = node.clone();
            let backend_for_effect = backend.clone();
            let _e = Effect::new(move || {
                let value = (compute)();
                backend_for_effect
                    .borrow_mut()
                    .update_text(&node_for_effect, &value);
            });
            node
        }
    }
}

fn build_view<B: Backend + 'static>(
    backend: &Rc<RefCell<B>>,
    children: Vec<Primitive>,
) -> B::Node {
    let mut parent = time_backend_create(pkind!(View), || backend.borrow_mut().create_view());
    insert_children(backend, &mut parent, children);
    parent
}

/// Walk a children vec and append each child to `parent`. Expands
/// `Primitive::Repeat` inline: instead of `count` individual `insert`
/// calls, builds all `count` child nodes first and hands them to the
/// backend's `insert_many` for batched DOM insertion (typically via
/// a `DocumentFragment` on web). For non-Repeat children this is the
/// same `build + insert` loop as before.
///
/// Why expand Repeat here and not as a regular Primitive in the
/// match: Repeat doesn't correspond to a single backend node — it
/// stands for N sibling nodes. So it can only appear inside a
/// children list, never as the root of a subtree.
fn insert_children<B: Backend + 'static>(
    backend: &Rc<RefCell<B>>,
    parent: &mut B::Node,
    children: Vec<Primitive>,
) {
    for child in children {
        match child {
            Primitive::Repeat { count, row_builder } => {
                // Build every row first, then hand the lot to the
                // backend for one batched insert. Building eagerly
                // means each row's own subtree may have done its own
                // backend FFI calls (createElement etc.) — those
                // can't be batched further at this layer, but the
                // *parent insert* is.
                let mut rows: Vec<B::Node> = Vec::with_capacity(count);
                for i in 0..count {
                    let row_prim = row_builder(i);
                    rows.push(build(backend, row_prim));
                }
                backend.borrow_mut().insert_many(parent, rows);
            }
            other => {
                let child_node = build(backend, other);
                backend.borrow_mut().insert(parent, child_node);
            }
        }
    }
}

/// RAII wrapper that calls `Backend::on_node_unstyled` when dropped.
/// Captured by the styled effect's closure so backend per-node state
/// (e.g. the web backend's dynamic CSS class slot) gets cleaned up
/// when the effect's scope drops — which happens on `when()` rebuilds
/// and on `Owner` teardown.
struct StyleHandle<B: Backend + 'static> {
    backend: Rc<RefCell<B>>,
    node: B::Node,
    /// For nodes attached via the static-style path: id into the
    /// theme cohort. `None` for reactive-style nodes (those re-apply
    /// via their own `Effect`'s theme subscription, not the cohort).
    cohort_id: Option<CohortId>,
}

impl<B: Backend + 'static> Drop for StyleHandle<B> {
    fn drop(&mut self) {
        // Remove from the theme cohort first, if registered. The
        // cohort holds a `Box<dyn Any>` that owns a clone of the
        // node; dropping it triggers the JS-side decref. Doing it
        // before `on_node_unstyled` keeps the backend's per-node
        // maps consistent during the unwind.
        if let Some(id) = self.cohort_id.take() {
            theme_cohort_unregister(id);
        }
        self.backend.borrow_mut().on_node_unstyled(&self.node);
    }
}

/// Opaque id for a cohort entry. Returned by
/// [`theme_cohort_register`] and consumed by
/// [`theme_cohort_unregister`].
#[derive(Copy, Clone, Debug, PartialEq, Eq, Hash)]
pub struct CohortId(u32);

/// One entry in the theme cohort. The framework doesn't know how to
/// re-apply on its own — backends are type-erased. So each entry
/// carries the typed re-apply closure inside, and the cohort just
/// iterates and calls them.
///
/// The closure captures everything it needs (backend, node, app),
/// so dropping the entry tears down those captures. A 10 000-row
/// cohort holds 10 000 closures — but each is small (Rc clones +
/// one Node clone + one `StyleApplication` clone) and we never
/// allocate `Effect` slots / arena entries for them.
struct CohortEntry {
    reapply: Box<dyn Fn()>,
}

thread_local! {
    /// Theme cohort: every static-style-attached node lives in this
    /// dense slab. A single framework-installed Effect subscribes
    /// to the active theme and iterates the slab on every fire,
    /// calling each entry's `reapply` closure. So we pay one Effect
    /// for the whole app instead of one per styled node.
    ///
    /// Layout: `Vec<Option<CohortEntry>>` indexed by the `CohortId`'s
    /// inner `u32`. Freed slots become `None` and their ids go on
    /// the freelist. Same shape as the reactive arena's signal /
    /// effect storage — and chosen for the same reason: a HashMap
    /// keyed by the same `u32` paid a ~30 ms hashing cost during a
    /// 10k-row mount that the slab avoids entirely.
    static THEME_COHORT: RefCell<Vec<Option<CohortEntry>>> = const { RefCell::new(Vec::new()) };

    /// Recycled slot ids. Popped on register, pushed on unregister.
    /// Without this, monotonic ids would grow per rebuild and the
    /// `Vec<Option<_>>` would balloon with None slots over time —
    /// same issue we fixed in the reactive arena.
    static THEME_COHORT_FREE: RefCell<Vec<u32>> = const { RefCell::new(Vec::new()) };

    /// Has the cohort driver effect been installed? Set on first
    /// register; never cleared. The effect lives in the root
    /// `Owner`'s scope and is dropped when that scope drops.
    static THEME_COHORT_DRIVER_INSTALLED: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
}

fn theme_cohort_register(reapply: Box<dyn Fn()>) -> CohortId {
    let entry = CohortEntry { reapply };
    let id = THEME_COHORT.with(|slab| {
        let mut slab = slab.borrow_mut();
        if let Some(idx) = THEME_COHORT_FREE.with(|f| f.borrow_mut().pop()) {
            slab[idx as usize] = Some(entry);
            idx
        } else {
            let idx = slab.len() as u32;
            slab.push(Some(entry));
            idx
        }
    });
    CohortId(id)
}

fn theme_cohort_unregister(id: CohortId) {
    THEME_COHORT.with(|slab| {
        if let Some(slot) = slab.borrow_mut().get_mut(id.0 as usize) {
            if slot.take().is_some() {
                THEME_COHORT_FREE.with(|f| f.borrow_mut().push(id.0));
            }
        }
    });
}

/// Install (idempotently) the cohort driver effect: subscribes to
/// the active theme signal and re-applies every cohort entry when
/// the theme changes. Created lazily on the first
/// `theme_cohort_register` call so we only pay for it when the
/// static-style path is actually used.
///
/// The driver registers with the currently-active `Scope` (the
/// root `Owner`'s scope at first call). When that scope drops, the
/// driver effect drops and we clear the flag so a subsequent
/// render reinstalls. The cohort map itself is also cleared on
/// driver drop — its entries' `reapply` closures captured Rcs to
/// the old backend, which is gone.
fn install_theme_cohort_driver() {
    if THEME_COHORT_DRIVER_INSTALLED.with(|c| c.get()) {
        return;
    }
    THEME_COHORT_DRIVER_INSTALLED.with(|c| c.set(true));

    // RAII guard captured by the driver closure. On drop (scope
    // teardown), clears the installed flag and drops every cohort
    // entry. Putting the cleanup on a captured guard rather than a
    // separate cleanup effect avoids ordering hazards.
    struct DriverGuard;
    impl Drop for DriverGuard {
        fn drop(&mut self) {
            THEME_COHORT_DRIVER_INSTALLED.with(|c| c.set(false));
            THEME_COHORT.with(|m| m.borrow_mut().clear());
            THEME_COHORT_FREE.with(|f| f.borrow_mut().clear());
        }
    }
    let _guard = DriverGuard;

    let _e = Effect::new(move || {
        // Anchor the guard inside the effect closure so it lives
        // exactly as long as the effect.
        let _ = &_guard;
        // Subscribe to the active theme. We don't use the value
        // directly — the cohort entries' `reapply` closures each
        // call `active_theme()` themselves through `resolve_style`.
        let _ = style::active_theme();
        // Iterate the slab under a single immutable borrow. Skip
        // empty slots. The `reapply` closure does DOM/backend work
        // only — never touches the cohort slab — so the long
        // borrow is safe.
        THEME_COHORT.with(|slab| {
            for entry in slab.borrow().iter().flatten() {
                (entry.reapply)();
            }
        });
    });
    let _ = _e;
}

/// RAII wrapper that calls `Backend::release_graphics` when dropped.
/// Installed unconditionally per Graphics primitive (i.e. doesn't
/// depend on a user-supplied style) by a dedicated cleanup `Effect`
/// in the build walker. When the surrounding scope drops — `when()`
/// branch flip, list-item recycling, `Owner` teardown — the effect
/// drops, this handle drops, and the backend tears down its wgpu
/// state.
struct GraphicsHandleCleanup<B: Backend + 'static> {
    backend: Rc<RefCell<B>>,
    node: B::Node,
}

impl<B: Backend + 'static> Drop for GraphicsHandleCleanup<B> {
    fn drop(&mut self) {
        self.backend.borrow_mut().release_graphics(&self.node);
    }
}

/// RAII wrapper that calls `Backend::release_virtualizer` when
/// dropped. Same lifecycle shape as `GraphicsHandleCleanup`:
/// installed per Virtualizer primitive by the walker via an empty
/// `Effect`; when that effect's scope drops, the backend detaches
/// listeners + drops the closures it handed the JS shim. Critical
/// for preventing "signal used after its scope was dropped"
/// panics from late-firing scroll/resize events whose Rust
/// callbacks captured the now-freed `Signal`.
struct VirtualizerHandleCleanup<B: Backend + 'static> {
    backend: Rc<RefCell<B>>,
    node: B::Node,
}

impl<B: Backend + 'static> Drop for VirtualizerHandleCleanup<B> {
    fn drop(&mut self) {
        self.backend.borrow_mut().release_virtualizer(&self.node);
    }
}

/// RAII wrapper that calls `Backend::release_navigator` when dropped.
/// Same shape as Virtualizer / Graphics cleanup. The navigator owns a
/// stack of per-screen scopes; when the cleanup fires, the backend's
/// `release_navigator` impl is responsible for releasing every still-
/// mounted scope via the `release_screen` callback the framework
/// handed it at create time.
struct NavigatorHandleCleanup<B: Backend + 'static> {
    backend: Rc<RefCell<B>>,
    node: B::Node,
}

impl<B: Backend + 'static> Drop for NavigatorHandleCleanup<B> {
    fn drop(&mut self) {
        self.backend.borrow_mut().release_navigator(&self.node);
    }
}

/// RAII wrapper that calls `Backend::release_tab_navigator` when
/// dropped. Same shape as `NavigatorHandleCleanup`.
struct TabNavigatorHandleCleanup<B: Backend + 'static> {
    backend: Rc<RefCell<B>>,
    node: B::Node,
}

impl<B: Backend + 'static> Drop for TabNavigatorHandleCleanup<B> {
    fn drop(&mut self) {
        self.backend.borrow_mut().release_tab_navigator(&self.node);
    }
}

/// RAII wrapper that calls `Backend::release_drawer_navigator` when
/// dropped. Same shape as `NavigatorHandleCleanup`.
struct DrawerNavigatorHandleCleanup<B: Backend + 'static> {
    backend: Rc<RefCell<B>>,
    node: B::Node,
}

impl<B: Backend + 'static> Drop for DrawerNavigatorHandleCleanup<B> {
    fn drop(&mut self) {
        self.backend.borrow_mut().release_drawer_navigator(&self.node);
    }
}

/// RAII wrapper that calls `Backend::release_overlay` when dropped.
/// Installed unconditionally per Overlay primitive by a dedicated
/// `Effect` in the build walker. When the surrounding scope drops —
/// host's open-state signal flips, `when` rebuilds the surrounding
/// branch, this scope drops — the backend tears down its floating
/// layer (detaches the portal node, removes Escape/back listeners,
/// drops the wasm-bindgen / JNI closure handles wired to system
/// dismiss events).
///
/// Without this, browser-queued dismissal events or anchor-tracking
/// observers firing after the scope dropped would invoke Rust
/// callbacks against freed `Signal` / `Effect` slots — same failure
/// mode `release_virtualizer` was added to prevent.
struct OverlayHandleCleanup<B: Backend + 'static> {
    backend: Rc<RefCell<B>>,
    node: B::Node,
}

impl<B: Backend + 'static> Drop for OverlayHandleCleanup<B> {
    fn drop(&mut self) {
        self.backend.borrow_mut().release_overlay(&self.node);
    }
}

struct AnchoredOverlayHandleCleanup<B: Backend + 'static> {
    backend: Rc<RefCell<B>>,
    node: B::Node,
}

impl<B: Backend + 'static> Drop for AnchoredOverlayHandleCleanup<B> {
    fn drop(&mut self) {
        self.backend.borrow_mut().release_anchored_overlay(&self.node);
    }
}

/// Build a Navigator. Stands up the per-screen scope registry, builds
/// the `NavigatorCallbacks` bundle, wires the user-facing handle's
/// control plane, mounts the initial screen, and returns the native
/// container node. Mirrors `build_virtualizer` — both manage a set of
/// nested scopes that map 1:1 with a backend-owned UI container.
fn build_navigator<B: Backend + 'static>(
    backend: &Rc<RefCell<B>>,
    initial: &'static str,
    initial_path: &'static str,
    screens: HashMap<&'static str, primitives::navigator::RouteEntry>,
    layout: Option<primitives::navigator::LayoutBuilder>,
    _default_options: Option<primitives::navigator::ScreenOptions>,
    ref_fill: Option<RefFill>,
) -> B::Node {
    use primitives::navigator::{
        match_pattern, LayoutPlan, LayoutProps, MountResult, NavState, NavigatorCallbacks,
        NavigatorControl,
    };

    // Per-screen scope registry. The framework owns the scopes — the
    // backend stores opaque scope ids alongside its native cells and
    // calls `release_screen(id)` to drop the matching scope. Same
    // discipline as Virtualizer.
    let scopes: Rc<RefCell<HashMap<u64, Box<reactive::Scope>>>> =
        Rc::new(RefCell::new(HashMap::new()));
    let next_scope_id: Rc<RefCell<u64>> = Rc::new(RefCell::new(0));

    // Screen table is `Rc`'d so the mount + match closures can clone it.
    // Each entry holds the route's path pattern + typed builder + segment-parser
    // (see `RouteEntry`).
    let screens = Rc::new(screens);

    // Control plane — handed to the handle now; populated by the
    // backend's `create_navigator` impl.
    let control = Rc::new(NavigatorControl::new());

    // mount_screen: look up the screen builder, build the screen
    // inside a fresh per-screen Scope, return (node, scope_id).
    // Panics on unregistered route — declaring routes is the
    // navigator's contract.
    let mount_screen: Rc<dyn Fn(&'static str, Box<dyn Any>) -> MountResult<B::Node>> = {
        let scopes = scopes.clone();
        let next_id = next_scope_id.clone();
        let screens = screens.clone();
        let backend = backend.clone();
        let control_for_mount = control.clone();
        Rc::new(move |name, params| {
            let entry = screens
                .get(name)
                .unwrap_or_else(|| panic!("Navigator: route '{}' is not registered", name));
            let builder = entry.build.clone();
            let mut scope = Box::new(reactive::Scope::new());
            // Wrap BOTH `builder(...)` and the subsequent `build(...)`
            // inside `with_scope`. Any Effects that the build walker
            // creates (e.g. switch/when/style/data_changed effects)
            // must register with this screen's scope so they stay
            // alive until the screen is released. Without this,
            // those Effects get `owns: true` and free immediately
            // when their handle drops at end of `build` —
            // unintentionally dropping shared `Rc<RefCell<...>>`
            // state the framework's microtasks depend on.
            //
            // Also push this navigator's control plane onto the
            // ambient stack so any `Link` primitives built inside
            // the screen capture it as their target. RAII guard
            // pops on drop, so nested navigators (each pushing in
            // turn) stack correctly.
            let _ambient_guard =
                primitives::navigator::AmbientNavGuard::push(control_for_mount.clone());
            // `builder` now returns a `Screen` (the render closure's
            // return value, type-erased through `ScreenBuilder`). Split
            // it into the primitive (built into a node) and the
            // options (handed to the backend via `MountResult`).
            let (node, options) = reactive::with_scope(&mut scope, || {
                let screen = builder(params);
                let n = build(&backend, screen.primitive);
                (n, screen.options)
            });
            let scope_id = {
                let mut n = next_id.borrow_mut();
                let v = *n;
                *n = n.checked_add(1).unwrap_or(0);
                v
            };
            scopes.borrow_mut().insert(scope_id, scope);
            MountResult { node, scope_id, options }
        })
    };

    // release_screen: drop the scope. The Drop impl on `Scope` frees
    // every signal/effect/ref scoped to the screen, including the
    // child subtree's `Effect`s.
    let release_screen: Rc<dyn Fn(u64)> = {
        let scopes = scopes.clone();
        Rc::new(move |id| {
            scopes.borrow_mut().remove(&id);
        })
    };

    // match_path: pure function from URL → (route name, typed params).
    // Walks the screen table and tries each pattern in registration
    // order; returns the first match whose segments parse cleanly.
    // The web backend calls this on mount + popstate; an SSR backend
    // would call it once per request.
    let match_path: Rc<dyn Fn(&str) -> Option<(&'static str, Box<dyn Any>)>> = {
        let screens = screens.clone();
        Rc::new(move |path| {
            for (name, entry) in screens.iter() {
                if let Some(segs) = match_pattern(path, entry.path) {
                    if let Some(params) = (entry.from_segments)(&segs) {
                        return Some((*name, params));
                    }
                }
            }
            None
        })
    };

    // Reactive nav-state signals. The dispatcher updates them on
    // every commit; layout effects subscribe to whichever they care
    // about. Initial values match the about-to-mount initial route.
    let nav_state = NavState {
        active_route: Signal::new(initial),
        active_path: Signal::new(initial_path.to_string()),
        depth: Signal::new(1),
        can_go_back: Signal::new(false),
    };
    // Hand the state to the control plane so `dispatch(...)` can
    // update the signals before the backend's dispatcher runs.
    control.attach_nav_state(nav_state.clone());

    // depth_changed: backend reports stack depth after each commit.
    // We update both the control plane (so `handle.depth()` is a
    // cheap probe) and the `nav_state.depth` signal (so reactive
    // layouts re-render). `can_go_back` is derived from depth.
    let depth_changed: Rc<dyn Fn(usize)> = {
        let control = control.clone();
        let depth_sig = nav_state.depth;
        let back_sig = nav_state.can_go_back;
        Rc::new(move |d| {
            control.set_depth(d);
            depth_sig.set(d);
            back_sig.set(d > 1);
        })
    };

    // Layout-scope. Layouts contain reactive effects (e.g. a
    // `Text { format!("{}", active_route.get()) }` in the chrome)
    // that must keep firing on every navigation. Without a scope
    // owner, those effects would free immediately when the
    // `Effect` handle drops at the end of `build()` — because the
    // layout is built from a microtask (web) which runs detached
    // from the navigator's enclosing render scope, the
    // thread-local active-scope stack is empty at build time.
    //
    // The fix: give the layout its own long-lived scope. We own
    // it here in `build_navigator`; it stays alive as long as the
    // navigator does, and effects registered during the layout
    // build attach to it. Dropping the scope tears down every
    // layout effect — handled by the cleanup `Effect` the walker
    // installs around `Primitive::Navigator` (it lives in the
    // surrounding scope; when *that* drops, this navigator and
    // its layout_scope go with it).
    let layout_scope: Rc<RefCell<Option<Box<reactive::Scope>>>> =
        Rc::new(RefCell::new(None));

    // build_layout: invoked by backends that render through a
    // user-supplied layout (web). The framework runs the layout
    // closure with a freshly-created outlet `View` (whose ref the
    // backend later uses to find the outlet's native node), builds
    // the resulting `Primitive` into a native node via the standard
    // build walker — wrapped in `with_scope(layout_scope)` so
    // layout effects survive past the build call.
    //
    // **Borrow safety**: this closure calls `build(&backend, ...)`
    // which does `backend.borrow_mut()`. Backends must only invoke
    // build_layout *outside* the `create_navigator` borrow window —
    // typically from a microtask scheduled during create, the same
    // pattern web uses for `mount_screen`.
    let build_layout: Option<Rc<dyn Fn() -> LayoutPlan<B::Node>>> = layout.map(|layout_fn| {
        let nav_state = nav_state.clone();
        let control = control.clone();
        let layout_fn = layout_fn.clone();
        let backend = backend.clone();
        let layout_scope_slot = layout_scope.clone();
        let f: Rc<dyn Fn() -> LayoutPlan<B::Node>> = Rc::new(move || {
            let outlet_ref: Ref<crate::ViewHandle> = Ref::new();
            let outlet_primitive: Primitive = crate::view(Vec::new()).bind(outlet_ref).into();
            let on_back: Rc<dyn Fn()> = {
                let control = control.clone();
                Rc::new(move || control.pop())
            };
            let props = LayoutProps {
                outlet: outlet_primitive,
                // Stack navigators don't have sidebars. Hand the
                // layout an empty View so authors don't have to
                // write a None-case branch — they can embed it
                // unconditionally or ignore it.
                sidebar: crate::view(Vec::new()).into(),
                active_route: nav_state.active_route,
                active_path: nav_state.active_path,
                depth: nav_state.depth,
                can_go_back: nav_state.can_go_back,
                on_back,
            };
            // Layouts may contain `Link`s in their chrome (a nav bar
            // with a "Home" link, etc.). Push this navigator's
            // control plane onto the ambient stack BEFORE invoking
            // the layout closure — the `link()` constructor calls
            // `ambient_navigator()` at construction time, which is
            // *during* the layout closure's run. If the guard fires
            // after layout_fn returns, every Link in the chrome
            // captures `None` and clicking them silently no-ops.
            let _ambient_guard =
                primitives::navigator::AmbientNavGuard::push(control.clone());
            let root_primitive = layout_fn(props);
            // Build the layout subtree inside its dedicated scope.
            // Every Effect created during the build (reactive
            // text, button state, etc.) attaches to this scope and
            // stays alive across navigation. Without this wrap,
            // those effects would drop immediately because the
            // layout build runs detached from any active scope.
            let mut scope = Box::new(reactive::Scope::new());
            let root = reactive::with_scope(&mut scope, || {
                build(&backend, root_primitive)
            });
            // Stash the scope on the slot so it stays alive for the
            // navigator's lifetime. The slot itself is dropped in
            // `release_navigator` via the cleanup effect, which
            // drops `layout_scope` along with everything else.
            *layout_scope_slot.borrow_mut() = Some(scope);
            LayoutPlan { root, outlet_ref }
        });
        f
    });

    let callbacks = NavigatorCallbacks {
        initial_route: initial,
        initial_path,
        mount_screen,
        release_screen,
        match_path,
        build_layout,
        nav_state: nav_state.clone(),
        depth_changed,
        // Framework-driven build: the walker calls `mount_screen` +
        // `navigator_attach_initial` directly below, and backends
        // that auto-mount on URL match (web) keep doing so.
        defer_initial_mount: false,
    };

    // Create the native navigator. The backend stores the callbacks,
    // installs a dispatcher on `control`, but DOES NOT call
    // `mount_screen` synchronously (would re-enter the backend's
    // borrow_mut → panic). The framework handles initial mount below.
    let mount_screen_for_initial = callbacks.mount_screen.clone();
    let node = time_backend_create(pkind!(Navigator), || {
        backend.borrow_mut().create_navigator(callbacks, control.clone())
    });

    // Mount the initial screen *after* `create_navigator` returns —
    // i.e. outside the borrow_mut window. The screen build
    // re-enters the build walker which itself does `borrow_mut`, so
    // it MUST run outside any active backend borrow. The result is
    // handed to the backend via `navigator_attach_initial`, which
    // is a thin "stick this screen into the container" hook with no
    // borrow contention (it doesn't call back into build).
    let initial_result = mount_screen_for_initial(initial, Box::new(()));
    backend
        .borrow_mut()
        .navigator_attach_initial(&node, initial_result.node, initial_result.scope_id, initial_result.options);

    if let Some(RefFill::Navigator(fill)) = ref_fill {
        // The default handle the trait builds is a no-op (`control: None`).
        // For backends that override `make_navigator_handle` and wire up
        // the control plane, the user gets the live handle. Default-no-op
        // backends produce a handle whose calls are silent no-ops —
        // matching every other "primitive ref that the backend doesn't
        // support yet" path in the framework.
        let handle = backend.borrow().make_navigator_handle(&node);
        fill(handle);
    }

    // See `build_drawer_navigator` for the rationale — backends that
    // don't store `build_layout` in their callbacks would otherwise
    // drop the layout scope (and every reactive style closure in the
    // layout) as soon as this function returns.
    let _layout_scope_keepalive = layout_scope.clone();
    let _keepalive_effect = Effect::new(move || {
        let _ = &_layout_scope_keepalive;
    });

    node
}

/// Build a TabNavigator. Shares the per-screen scope registry and
/// ambient-nav wiring with `build_navigator`; differs in the
/// callbacks bundle (carries tab metadata + mount policy) and the
/// backend hook called (`create_tab_navigator`).
#[allow(clippy::too_many_arguments)]
fn build_tab_navigator<B: Backend + 'static>(
    backend: &Rc<RefCell<B>>,
    initial: &'static str,
    initial_path: &'static str,
    tab_order: Vec<(&'static str, primitives::navigator::TabSpec)>,
    screens: HashMap<&'static str, primitives::navigator::RouteEntry>,
    layout: Option<primitives::navigator::LayoutBuilder>,
    placement: primitives::navigator::TabPlacement,
    mount_policy: primitives::navigator::MountPolicy,
    _default_options: Option<primitives::navigator::ScreenOptions>,
    ref_fill: Option<RefFill>,
) -> B::Node {
    use primitives::navigator::{
        match_pattern, DefaultLinkKind, LayoutPlan, LayoutProps, MountResult, NavState,
        NavigatorCallbacks, NavigatorControl, TabNavigatorCallbacks, TabRegistration,
    };

    // Per-screen scope registry — same discipline as stack.
    let scopes: Rc<RefCell<HashMap<u64, Box<reactive::Scope>>>> =
        Rc::new(RefCell::new(HashMap::new()));
    let next_scope_id: Rc<RefCell<u64>> = Rc::new(RefCell::new(0));
    let screens = Rc::new(screens);
    let control = Rc::new(NavigatorControl::new());
    control.set_default_link_kind(DefaultLinkKind::Select);

    let mount_screen: Rc<dyn Fn(&'static str, Box<dyn Any>) -> MountResult<B::Node>> = {
        let scopes = scopes.clone();
        let next_id = next_scope_id.clone();
        let screens = screens.clone();
        let backend = backend.clone();
        let control_for_mount = control.clone();
        Rc::new(move |name, params| {
            let entry = screens
                .get(name)
                .unwrap_or_else(|| panic!("TabNavigator: route '{}' is not registered", name));
            let builder = entry.build.clone();
            let mut scope = Box::new(reactive::Scope::new());
            let _ambient_guard =
                primitives::navigator::AmbientNavGuard::push(control_for_mount.clone());
            let (node, options) = reactive::with_scope(&mut scope, || {
                let screen = builder(params);
                let n = build(&backend, screen.primitive);
                (n, screen.options)
            });
            let scope_id = {
                let mut n = next_id.borrow_mut();
                let v = *n;
                *n = n.checked_add(1).unwrap_or(0);
                v
            };
            scopes.borrow_mut().insert(scope_id, scope);
            MountResult { node, scope_id, options }
        })
    };

    let release_screen: Rc<dyn Fn(u64)> = {
        let scopes = scopes.clone();
        Rc::new(move |id| {
            scopes.borrow_mut().remove(&id);
        })
    };

    let match_path: Rc<dyn Fn(&str) -> Option<(&'static str, Box<dyn Any>)>> = {
        let screens = screens.clone();
        Rc::new(move |path| {
            for (name, entry) in screens.iter() {
                if let Some(segs) = match_pattern(path, entry.path) {
                    if let Some(params) = (entry.from_segments)(&segs) {
                        return Some((*name, params));
                    }
                }
            }
            None
        })
    };

    let nav_state = NavState {
        active_route: Signal::new(initial),
        active_path: Signal::new(initial_path.to_string()),
        // Tabs don't have stack depth; pin to 1 so layouts that
        // read `depth` see a sensible value (effectively "we're at
        // the root of the active tab"). Backends with nested stacks
        // inside tabs report the *active tab's* stack depth via
        // their own depth_changed; until then, 1 is correct.
        depth: Signal::new(1),
        can_go_back: Signal::new(false),
    };
    control.attach_nav_state(nav_state.clone());

    let depth_changed: Rc<dyn Fn(usize)> = {
        let control = control.clone();
        let depth_sig = nav_state.depth;
        let back_sig = nav_state.can_go_back;
        Rc::new(move |d| {
            control.set_depth(d);
            depth_sig.set(d);
            back_sig.set(d > 1);
        })
    };

    // Active-changed callback. Backends fire this after the tap
    // commits (e.g. for analytics); the reactive nav-state signals
    // have already been updated by `control.dispatch(...)`.
    let active_changed: Rc<dyn Fn(&'static str)> = Rc::new(|_name| {});

    // Layout slot — same shape as stack's. Tabs may want a top app
    // bar that spans tabs (e.g. a search field that lives above the
    // tab bar); the layout closure renders the chrome and embeds
    // the outlet where the active tab's content goes.
    let layout_scope: Rc<RefCell<Option<Box<reactive::Scope>>>> =
        Rc::new(RefCell::new(None));
    let build_layout: Option<Rc<dyn Fn() -> LayoutPlan<B::Node>>> = layout.map(|layout_fn| {
        let nav_state = nav_state.clone();
        let control = control.clone();
        let layout_fn = layout_fn.clone();
        let backend = backend.clone();
        let layout_scope_slot = layout_scope.clone();
        let f: Rc<dyn Fn() -> LayoutPlan<B::Node>> = Rc::new(move || {
            let outlet_ref: Ref<crate::ViewHandle> = Ref::new();
            let outlet_primitive: Primitive = crate::view(Vec::new()).bind(outlet_ref).into();
            // Tabs don't have a back-button — `on_back` is a no-op.
            // Layout authors who need one should hide it via the
            // `can_go_back` signal, which stays false for pure tab
            // navigators.
            let on_back: Rc<dyn Fn()> = Rc::new(|| {});
            let props = LayoutProps {
                outlet: outlet_primitive,
                // Tab navigators don't have sidebars either.
                sidebar: crate::view(Vec::new()).into(),
                active_route: nav_state.active_route,
                active_path: nav_state.active_path,
                depth: nav_state.depth,
                can_go_back: nav_state.can_go_back,
                on_back,
            };
            let _ambient_guard =
                primitives::navigator::AmbientNavGuard::push(control.clone());
            let root_primitive = layout_fn(props);
            let mut scope = Box::new(reactive::Scope::new());
            let root = reactive::with_scope(&mut scope, || {
                build(&backend, root_primitive)
            });
            *layout_scope_slot.borrow_mut() = Some(scope);
            LayoutPlan { root, outlet_ref }
        });
        f
    });

    // Translate the `Vec<(name, TabSpec)>` author input into the
    // `Vec<TabRegistration>` shape backends receive. Same data,
    // flat structure (no nested tuples).
    let tabs: Vec<TabRegistration> = tab_order
        .into_iter()
        .map(|(route, spec)| TabRegistration {
            route,
            label: spec.label,
            icon: spec.icon,
            badge: spec.badge,
        })
        .collect();

    let nav_callbacks = NavigatorCallbacks {
        initial_route: initial,
        initial_path,
        mount_screen,
        release_screen,
        match_path,
        build_layout,
        nav_state,
        depth_changed,
        defer_initial_mount: false,
    };
    let callbacks = TabNavigatorCallbacks {
        navigator: nav_callbacks,
        tabs,
        placement,
        mount_policy,
        active_changed,
    };

    let mount_screen_for_initial = callbacks.navigator.mount_screen.clone();
    let node = time_backend_create(pkind!(TabNavigator), || {
        backend.borrow_mut().create_tab_navigator(callbacks, control.clone())
    });

    // Mount the initial tab's screen after create_tab_navigator
    // returns (outside the borrow_mut window). Same pattern as the
    // stack navigator's `navigator_attach_initial`. Backends that
    // defer initial mount to a microtask (web) leave the default
    // no-op; backends that mount synchronously (Android) implement
    // `tab_navigator_attach_initial`.
    let initial_result = mount_screen_for_initial(initial, Box::new(()));
    backend
        .borrow_mut()
        .tab_navigator_attach_initial(&node, initial_result.node, initial_result.scope_id, initial_result.options);

    if let Some(RefFill::TabNavigator(fill)) = ref_fill {
        let handle = backend.borrow().make_tab_navigator_handle(&node);
        fill(handle);
    }

    // See `build_drawer_navigator` for the rationale.
    let _layout_scope_keepalive = layout_scope.clone();
    let _keepalive_effect = Effect::new(move || {
        let _ = &_layout_scope_keepalive;
    });

    node
}

/// Build a DrawerNavigator. Same per-screen scope machinery as the
/// stack and tab navigators; additionally exposes an `is_open`
/// signal the backend's dispatcher flips on
/// `OpenDrawer`/`CloseDrawer`/`ToggleDrawer` commands.
#[allow(clippy::too_many_arguments)]
fn build_drawer_navigator<B: Backend + 'static>(
    backend: &Rc<RefCell<B>>,
    initial: &'static str,
    initial_path: &'static str,
    screens: HashMap<&'static str, primitives::navigator::RouteEntry>,
    layout: Option<primitives::navigator::LayoutBuilder>,
    content: Option<primitives::navigator::ContentBuilder>,
    side: primitives::navigator::DrawerSide,
    drawer_type: primitives::navigator::DrawerType,
    drawer_width: f32,
    swipe_to_open: bool,
    mount_policy: primitives::navigator::MountPolicy,
    _default_options: Option<primitives::navigator::ScreenOptions>,
    ref_fill: Option<RefFill>,
) -> B::Node {
    use primitives::navigator::{
        match_pattern, DefaultLinkKind, DrawerContentProps, DrawerNavigatorCallbacks, LayoutPlan,
        LayoutProps, MountResult, NavState, NavigatorCallbacks, NavigatorControl,
    };

    let scopes: Rc<RefCell<HashMap<u64, Box<reactive::Scope>>>> =
        Rc::new(RefCell::new(HashMap::new()));
    let next_scope_id: Rc<RefCell<u64>> = Rc::new(RefCell::new(0));
    let screens = Rc::new(screens);
    let control = Rc::new(NavigatorControl::new());
    control.set_default_link_kind(DefaultLinkKind::Select);

    let mount_screen: Rc<dyn Fn(&'static str, Box<dyn Any>) -> MountResult<B::Node>> = {
        let scopes = scopes.clone();
        let next_id = next_scope_id.clone();
        let screens = screens.clone();
        let backend = backend.clone();
        let control_for_mount = control.clone();
        Rc::new(move |name, params| {
            let entry = screens
                .get(name)
                .unwrap_or_else(|| panic!("DrawerNavigator: route '{}' is not registered", name));
            let builder = entry.build.clone();
            let mut scope = Box::new(reactive::Scope::new());
            let _ambient_guard =
                primitives::navigator::AmbientNavGuard::push(control_for_mount.clone());
            let (node, mut options) = reactive::with_scope(&mut scope, || {
                let screen = builder(params);
                let n = build(&backend, screen.primitive);
                (n, screen.options)
            });
            // Default drawer-toggle hamburger when the author didn't
            // specify a left header button. Drawer-rooted screens
            // almost always want this, so making it the default
            // means the per-screen wiring stays focused on the
            // screen's body — the framework knows the screen sits
            // under a drawer and knows what icon + action belongs
            // there. Override is still trivial: pass any
            // `header_left(...)` from the author closure.
            if options.header_left.is_none() {
                let control = control_for_mount.clone();
                options.header_left = Some(primitives::navigator::HeaderButton::new(
                    "line.3.horizontal",
                    move || {
                        control.dispatch(primitives::navigator::NavCommand::ToggleDrawer);
                    },
                ));
            }
            let scope_id = {
                let mut n = next_id.borrow_mut();
                let v = *n;
                *n = n.checked_add(1).unwrap_or(0);
                v
            };
            scopes.borrow_mut().insert(scope_id, scope);
            MountResult { node, scope_id, options }
        })
    };

    let release_screen: Rc<dyn Fn(u64)> = {
        let scopes = scopes.clone();
        Rc::new(move |id| {
            scopes.borrow_mut().remove(&id);
        })
    };

    let match_path: Rc<dyn Fn(&str) -> Option<(&'static str, Box<dyn Any>)>> = {
        let screens = screens.clone();
        Rc::new(move |path| {
            for (name, entry) in screens.iter() {
                if let Some(segs) = match_pattern(path, entry.path) {
                    if let Some(params) = (entry.from_segments)(&segs) {
                        return Some((*name, params));
                    }
                }
            }
            None
        })
    };

    let nav_state = NavState {
        active_route: Signal::new(initial),
        active_path: Signal::new(initial_path.to_string()),
        depth: Signal::new(1),
        can_go_back: Signal::new(false),
    };
    control.attach_nav_state(nav_state.clone());

    // Reactive drawer-open signal. The backend's dispatcher flips
    // this in response to `OpenDrawer`/`CloseDrawer`/`ToggleDrawer`
    // commands; layout closures subscribe to it to drive the
    // hamburger icon's open/close visual.
    let is_open = Signal::new(false);

    let depth_changed: Rc<dyn Fn(usize)> = {
        let control = control.clone();
        let depth_sig = nav_state.depth;
        let back_sig = nav_state.can_go_back;
        Rc::new(move |d| {
            control.set_depth(d);
            depth_sig.set(d);
            back_sig.set(d > 1);
        })
    };

    let active_changed: Rc<dyn Fn(&'static str)> = Rc::new(|_name| {});
    let open_changed: Rc<dyn Fn(bool)> = Rc::new(move |open| is_open.set(open));

    // Wrap the user's `.content(...)` closure in a factory that
    // constructs a fresh `DrawerContentProps` each call and returns
    // the rendered Primitive. We need the factory shape because
    // both `build_layout` (web) and `build_content` (native) call
    // the closure, and they may be invoked at different times in
    // different scopes.
    let make_content_primitive: Option<Rc<dyn Fn() -> Primitive>> = content.map(|content_fn| {
        let active_route = nav_state.active_route;
        let is_open = is_open;
        let control_for_select = control.clone();
        let control_for_close = control.clone();
        let f: Rc<dyn Fn() -> Primitive> = Rc::new(move || {
            let on_select: Rc<dyn Fn(&'static str)> = {
                let control = control_for_select.clone();
                Rc::new(move |name: &'static str| {
                    // The `Select` URL doesn't matter to native
                    // (native ignores URLs); on web the dispatch
                    // path resolves it from the route's `path()`,
                    // but we don't have a Route<()> here — pass
                    // an empty URL. The web dispatcher reads URL
                    // from the active_path signal at activation
                    // time, so this is fine.
                    control.dispatch(primitives::navigator::NavCommand::Select {
                        name,
                        url: String::new(),
                        params: Box::new(()),
                    });
                })
            };
            let on_close: Rc<dyn Fn()> = {
                let control = control_for_close.clone();
                Rc::new(move || {
                    control.dispatch(primitives::navigator::NavCommand::CloseDrawer)
                })
            };
            let props = DrawerContentProps {
                active_route,
                is_open,
                on_select,
                on_close,
            };
            content_fn(props)
        });
        f
    });

    let layout_scope: Rc<RefCell<Option<Box<reactive::Scope>>>> =
        Rc::new(RefCell::new(None));
    let build_layout: Option<Rc<dyn Fn() -> LayoutPlan<B::Node>>> = layout.map(|layout_fn| {
        let nav_state = nav_state.clone();
        let control = control.clone();
        let layout_fn = layout_fn.clone();
        let backend = backend.clone();
        let layout_scope_slot = layout_scope.clone();
        let make_content = make_content_primitive.clone();
        let f: Rc<dyn Fn() -> LayoutPlan<B::Node>> = Rc::new(move || {
            let outlet_ref: Ref<crate::ViewHandle> = Ref::new();
            let outlet_primitive: Primitive = crate::view(Vec::new()).bind(outlet_ref).into();
            // Drawer's `on_back` toggles the drawer — back action
            // semantics on a drawer-rooted screen. Layout authors
            // who want stack-style back can hold a separate handle.
            let on_back: Rc<dyn Fn()> = {
                let control = control.clone();
                Rc::new(move || control.dispatch(primitives::navigator::NavCommand::ToggleDrawer))
            };
            // Push the ambient nav BEFORE building the drawer
            // content so any `Link`s in it capture this drawer.
            // The guard covers both the content build and the
            // layout closure's run; dropped at end of this scope.
            let _ambient_guard =
                primitives::navigator::AmbientNavGuard::push(control.clone());
            // Build the drawer-content Primitive (or empty View if
            // no `.content(...)` was registered). Either way,
            // LayoutProps carries a Primitive — the layout author
            // embeds it unconditionally.
            let sidebar_primitive: Primitive = match make_content.as_ref() {
                Some(f) => f(),
                None => crate::view(Vec::new()).into(),
            };
            let props = LayoutProps {
                outlet: outlet_primitive,
                sidebar: sidebar_primitive,
                active_route: nav_state.active_route,
                active_path: nav_state.active_path,
                depth: nav_state.depth,
                can_go_back: nav_state.can_go_back,
                on_back,
            };
            let root_primitive = layout_fn(props);
            let mut scope = Box::new(reactive::Scope::new());
            let root = reactive::with_scope(&mut scope, || {
                build(&backend, root_primitive)
            });
            *layout_scope_slot.borrow_mut() = Some(scope);
            LayoutPlan { root, outlet_ref }
        });
        f
    });

    // `build_content` — used by native backends that render the
    // drawer panel themselves (iOS/Android drawer shells). The
    // closure builds the user's content Primitive into a backend
    // Node inside a dedicated scope so reactive effects in the
    // panel survive across drawer state changes.
    let content_scope: Rc<RefCell<Option<Box<reactive::Scope>>>> =
        Rc::new(RefCell::new(None));
    let build_content: Option<Rc<dyn Fn() -> B::Node>> = make_content_primitive
        .as_ref()
        .map(|make_content| {
            let make_content = make_content.clone();
            let backend = backend.clone();
            let control = control.clone();
            let content_scope_slot = content_scope.clone();
            let f: Rc<dyn Fn() -> B::Node> = Rc::new(move || {
                // Same ambient-nav posture as build_layout: Links
                // in the panel capture this drawer's control.
                let _ambient_guard =
                    primitives::navigator::AmbientNavGuard::push(control.clone());
                let primitive = make_content();
                let mut scope = Box::new(reactive::Scope::new());
                let node = reactive::with_scope(&mut scope, || {
                    build(&backend, primitive)
                });
                *content_scope_slot.borrow_mut() = Some(scope);
                node
            });
            f
        });

    let nav_callbacks = NavigatorCallbacks {
        initial_route: initial,
        initial_path,
        mount_screen,
        release_screen,
        match_path,
        build_layout,
        nav_state,
        depth_changed,
        defer_initial_mount: false,
    };
    let callbacks = DrawerNavigatorCallbacks {
        navigator: nav_callbacks,
        side,
        drawer_type,
        drawer_width,
        swipe_to_open,
        mount_policy,
        is_open,
        build_content,
        active_changed,
        open_changed,
    };

    let mount_screen_for_initial = callbacks.navigator.mount_screen.clone();
    // Capture the content builder before moving `callbacks` into
    // the backend's `create_drawer_navigator` — we need it after
    // the create call returns (when the backend's borrow_mut is
    // released) to build the content Node and hand it back via
    // `drawer_navigator_attach_sidebar`. Web backends ignore this
    // path (they build the content via `build_layout`).
    let build_content_after_create = callbacks.build_content.clone();
    let node = time_backend_create(pkind!(DrawerNavigator), || {
        backend.borrow_mut().create_drawer_navigator(callbacks, control.clone())
    });

    // Build the drawer panel content (if registered) and hand the
    // resulting Node to the backend. Runs outside any active
    // borrow_mut window because the build re-enters the walker,
    // which also borrow_muts.
    if let Some(build_content) = build_content_after_create {
        let content_node = build_content();
        backend
            .borrow_mut()
            .drawer_navigator_attach_sidebar(&node, content_node);
    }

    // Mount the initial drawer screen — same pattern as the tab
    // navigator. Backends that mount via microtask (web) leave the
    // default no-op; backends that mount synchronously (Android)
    // implement `drawer_navigator_attach_initial`.
    let initial_result = mount_screen_for_initial(initial, Box::new(()));
    backend
        .borrow_mut()
        .drawer_navigator_attach_initial(&node, initial_result.node, initial_result.scope_id, initial_result.options);

    if let Some(RefFill::DrawerNavigator(fill)) = ref_fill {
        let handle = backend.borrow().make_drawer_navigator_handle(&node);
        fill(handle);
    }

    // Keep the sidebar's and layout's reactive scopes alive for as
    // long as the navigator stays mounted. The build closures stash
    // a `Box<Scope>` into these Rcs; without this keepalive Effect
    // the only Rc references are the local vars (dropped on return)
    // and the closures' captures (dropped when `callbacks` and
    // `build_content_after_create` go out of scope), freeing the
    // scope and unsubscribing every reactive style closure in the
    // content/layout (e.g. content item highlights stop updating on
    // Select). Capturing the Rcs in an Effect that registers with
    // the surrounding render scope ties their lifetime to the
    // navigator's enclosing scope.
    let _content_scope_keepalive = content_scope.clone();
    let _layout_scope_keepalive = layout_scope.clone();
    let _keepalive_effect = Effect::new(move || {
        let _ = &_content_scope_keepalive;
        let _ = &_layout_scope_keepalive;
    });

    node
}

/// Attach a style to a navigator sub-component (header, title, etc.).
/// Same cohort-based theme reactivity as `attach_style_static`, but
/// calls a custom apply function instead of `backend.apply_style`.
fn attach_navigator_slot_style<B, F>(
    backend: &Rc<RefCell<B>>,
    node: &B::Node,
    style: StyleSource,
    apply_fn: F,
) where
    B: Backend + 'static,
    F: Fn(&Rc<RefCell<B>>, &B::Node, &Rc<StyleRules>) + Clone + 'static,
{
    match style {
        StyleSource::Static(app) => {
            let resolved = resolve_style(&app);
            apply_fn(backend, node, &resolved);

            install_theme_cohort_driver();
            let backend_c = backend.clone();
            let node_c = node.clone();
            let app_rc = Rc::new(app);
            let apply_fn_c = apply_fn.clone();
            let cohort_id = theme_cohort_register(Box::new(move || {
                let resolved = resolve_style(&app_rc);
                apply_fn_c(&backend_c, &node_c, &resolved);
            }));
            struct SlotGuard(CohortId);
            impl Drop for SlotGuard {
                fn drop(&mut self) {
                    theme_cohort_unregister(self.0);
                }
            }
            reactive::adopt_guard_into_active_scope(SlotGuard(cohort_id));
        }
        StyleSource::Reactive(f) => {
            let backend_c = backend.clone();
            let node_c = node.clone();
            let _e = Effect::new(move || {
                let app = f();
                let resolved = resolve_style(&app);
                apply_fn(&backend_c, &node_c, &resolved);
            });
        }
    }
}

/// Attaches a style to an already-constructed node by spawning an
/// independent reactive Effect that re-applies on each signal change.
/// The effect captures a `StyleHandle` so that when its scope drops
/// the backend gets `on_node_unstyled` notification for per-node
/// cleanup (e.g. dropping the web backend's dynamic CSS rule).
///
/// Independent of any content effect on the same node — a content
/// signal change doesn't re-fire the style effect, and vice versa.
fn attach_style<B: Backend + 'static>(
    backend: &Rc<RefCell<B>>,
    node: &B::Node,
    style: StyleSource,
) -> Rc<dyn Fn(StateBits, bool)> {
    match style {
        StyleSource::Static(app) => attach_style_static(backend, node, app),
        StyleSource::Reactive(f) => attach_style_reactive(backend, node, f),
    }
}

/// Wire `safe_area_sides` to the backend reactively. Subscribes to
/// the framework's global insets signal so the backend re-applies
/// padding on every change (orientation flip, sheet adaptation,
/// dynamic island). The Effect lives until this primitive's scope
/// drops — same RAII model as `attach_style`'s reactive path.
fn attach_safe_area<B: Backend + 'static>(
    backend: &Rc<RefCell<B>>,
    node: &B::Node,
    sides: crate::SafeAreaSides,
) {
    use crate::reactive::Effect;
    let backend = backend.clone();
    let node = node.clone();
    let _effect = Effect::new(move || {
        // Touch the insets signal so this effect re-runs whenever
        // the platform reports new values. We don't need the value
        // here — the backend reads its own platform source inside
        // `apply_safe_area_padding`. The subscription is the point.
        let _ = crate::safe_area::safe_area_insets().get();
        backend
            .borrow_mut()
            .apply_safe_area_padding(&node, sides);
    });
}

/// Sibling of `attach_safe_area` for `Primitive::ScrollView`. Routes
/// the safe-area opt-in through `apply_scroll_view_safe_area_inset`
/// so backends apply *contentInset* semantics (background bleeds
/// edge-to-edge, content origin insets) rather than the outer
/// padding mode that fits a `View`.
fn attach_scroll_view_safe_area_inset<B: Backend + 'static>(
    backend: &Rc<RefCell<B>>,
    node: &B::Node,
    sides: crate::SafeAreaSides,
) {
    use crate::reactive::Effect;
    let backend = backend.clone();
    let node = node.clone();
    let _effect = Effect::new(move || {
        let _ = crate::safe_area::safe_area_insets().get();
        backend
            .borrow_mut()
            .apply_scroll_view_safe_area_inset(&node, sides);
    });
}

/// Static-style fast path: no per-node `Effect`, no signal
/// subscription. The style is applied inline at mount, and the node
/// is registered with the framework's theme cohort so a `set_theme`
/// call re-applies it in bulk via a single shared `Effect`. Saves
/// 10k arena slots + 10k closure boxes for a 10k-row scoreboard
/// vs. the reactive path. RAII guard inside the build walker (via
/// the returned `StyleHandle` captured by the cleanup effect)
/// removes the cohort entry on teardown.
fn attach_style_static<B: Backend + 'static>(
    backend: &Rc<RefCell<B>>,
    node: &B::Node,
    app: StyleApplication,
) -> Rc<dyn Fn(StateBits, bool)> {
    // Make sure the cohort driver is alive before we register.
    install_theme_cohort_driver();

    let handles_states_natively = backend.borrow().handles_states_natively();

    // Inline first apply. Identical work to what the reactive
    // path's Effect would do on its first run — just without
    // wrapping it in an Effect closure.
    apply_one(backend, node, &app, handles_states_natively);

    // Register the node with the theme cohort. We wrap the
    // `StyleApplication` in an `Rc` so the cohort closure pays
    // only a pointer-clone on registration — `StyleApplication`
    // itself transitively owns a `StyleRules` overrides struct
    // that's ~1 KB, and at 10k rows the per-row clone of that
    // was the dominant new allocation cost vs. the reactive path.
    let backend_for_cohort = backend.clone();
    let node_for_cohort = node.clone();
    let app_for_cohort = Rc::new(app);
    let cohort_id = theme_cohort_register(Box::new(move || {
        apply_one(&backend_for_cohort, &node_for_cohort, &app_for_cohort, handles_states_natively);
    }));

    // Attach the cleanup guard directly to the active scope —
    // bypasses the arena entirely (no `Effect` slot, no subscriber
    // set entry, no dependency set entry). The guard is held in
    // `Scope::guards`, dropped in the same batch as effects when
    // the scope tears down. For a 10k-row scope this is the
    // difference between 10k arena allocs and ~10k cheap Vec
    // pushes — the underlying `Box<dyn Any>` and the `StyleHandle`
    // contents are the same shape either way, but we save the
    // arena bookkeeping.
    let cleanup_handle = StyleHandle {
        backend: backend.clone(),
        node: node.clone(),
        cohort_id: Some(cohort_id),
    };
    let adopted = reactive::adopt_guard_into_active_scope(cleanup_handle);
    debug_assert!(
        adopted,
        "attach_style_static called outside an active Scope — \
         StyleHandle would leak (cohort entry + per-node backend state \
         never cleaned). The renderer's `Owner` always sets a scope, \
         so this fires only for ad-hoc top-level use."
    );

    // The setter is a no-op on natively-handling backends — `setter`
    // is exposed for `attach_disabled` etc., but with no Signal in
    // play it has nothing to flip. For event-driven backends the
    // static path doesn't apply (we'd lose state reactivity), but
    // those backends would route through `attach_style_reactive`
    // anyway because the macro emits a closure for state-bearing
    // styles. Returning a no-op keeps the return type aligned.
    //
    // TODO: revisit when adding native iOS/Android backends. The
    // static path may need to keep a Signal<StateBits> after all.
    Rc::new(|_, _| {})
}

/// Apply a style to a single node. Pulled out as a free function
/// so both the static path (called inline at mount) and the cohort
/// driver (called on theme change) can re-use it.
fn apply_one<B: Backend + 'static>(
    backend: &Rc<RefCell<B>>,
    node: &B::Node,
    app: &StyleApplication,
    handles_states_natively: bool,
) {
    {
        let backend_for_register = backend.clone();
        let backend_for_unregister = backend.clone();
        let backend_for_tokens = backend.clone();
        style::ensure_registered_with(
            &app.sheet,
            |rules| {
                backend_for_register.borrow_mut().register_stylesheet(rules);
            },
            |rules| {
                backend_for_unregister
                    .borrow_mut()
                    .unregister_stylesheet(rules);
            },
            |tokens| {
                backend_for_tokens
                    .borrow_mut()
                    .install_theme_variables(tokens);
            },
        );
        // Drain any pending multi-variant theme registrations + the
        // active-theme-signal binding. Idempotent — the drain
        // helpers take from their queues and leave them empty, so
        // subsequent stylesheet registrations during the same
        // walker pass are no-ops.
        let backend_for_variants = backend.clone();
        style::drain_pending_theme_variants(|name, tokens| {
            backend_for_variants
                .borrow_mut()
                .register_theme_variant(name, tokens);
        });
        let backend_for_active = backend.clone();
        style::drain_pending_active_theme_signal(|sig, name| {
            backend_for_active
                .borrow_mut()
                .bind_active_theme_signal(sig, name);
        });
    }
    if handles_states_natively {
        let base = resolve_style(app);
        let overlays = resolve_state_overlays(app);
        backend
            .borrow_mut()
            .apply_styled_states(node, &base, &overlays);
    } else {
        let resolved = resolve_style(app);
        backend.borrow_mut().apply_style(node, &resolved);
    }
}

fn attach_style_reactive<B: Backend + 'static>(
    backend: &Rc<RefCell<B>>,
    node: &B::Node,
    style: Box<dyn Fn() -> StyleApplication>,
) -> Rc<dyn Fn(StateBits, bool)> {
    // Per-phase timing of attach_style. The point is to separate
    // "framework overhead per styled node" (Effect alloc, Signal
    // alloc, scope registration, clones) from "actual style work"
    // (resolve, apply, register stylesheet) so a high-row-count
    // render's overhead can be measured rather than guessed at.
    //
    // Phases emitted (all only when `debug-stats` is on):
    //   attach_style_total          wraps the whole call
    //   attach_style_setup          pre-Effect setup (clones, Signal::new, borrow for caps)
    //   attach_style_effect_alloc   Effect::new — alloc slot AND first run
    //   attach_style_first_run      just the closure body inside Effect::new's first run
    //   attach_style_post_effect    Rc<setter>, backend.attach_states
    //   attach_style_resolve        resolve_style + resolve_state_overlays per run
    //   attach_style_apply_call     the backend's apply_styled_states / apply_style call
    //
    // The interesting quantity is (effect_alloc - first_run) — the
    // pure arena/scope-registration cost per styled node.
    #[cfg(feature = "debug-stats")]
    let _t_total_start = debug::now_micros();

    #[cfg(feature = "debug-stats")]
    let _t_setup_start = debug::now_micros();

    // StyleHandle owns the node-handle the effect closure needs. The
    // closure body reads `handle.node` directly, so we don't clone
    // the node twice per row — one Node clone per row is the floor,
    // and each clone is a wasm-bindgen JsValue (decref runs a JS-side
    // FFI call on drop, ~3μs in practice). At 10k rows that's the
    // difference between ~60ms and ~120ms of teardown cost.
    let backend_for_effect = backend.clone();

    let handle = StyleHandle {
        backend: backend.clone(),
        node: node.clone(),
        cohort_id: None,
    };

    let handles_states_natively = backend.borrow().handles_states_natively();

    // Per-node active interaction states. For backends that don't
    // handle states natively (Android, iOS), we keep a Signal<StateBits>
    // that flips on native events; the style effect re-resolves on
    // each flip and merges the relevant `__state_*` axes.
    //
    // For backends that DO handle states natively (web), no signal is
    // needed — `apply_styled_states` pre-emits all state overlays as
    // CSS pseudo-class rules, so the browser drives state tracking
    // without a Rust round-trip. Skipping the alloc is worth ~10k
    // arena slot creations per 10k-row rebuild.
    let states_signal: Option<Signal<StateBits>> = if handles_states_natively {
        None
    } else {
        Some(Signal::new(StateBits::NONE))
    };

    #[cfg(feature = "debug-stats")]
    debug::record_apply_phase(
        "attach_style_setup",
        debug::now_micros().saturating_sub(_t_setup_start),
    );

    #[cfg(feature = "debug-stats")]
    let _t_effect_alloc_start = debug::now_micros();

    let _e = Effect::new(move || {
        #[cfg(feature = "debug-stats")]
        let _t_first_run_start = debug::now_micros();

        // `handle` is captured by-move so its Drop runs iff the
        // effect is dropped — that's how `on_node_unstyled` fires
        // exactly once per styled node when its scope tears down.

        #[cfg(feature = "debug-stats")]
        debug::record_apply_style_enter();
        #[cfg(feature = "debug-stats")]
        debug::record_effect_fired();

        let app = style();

        let backend_for_register = backend_for_effect.clone();
        let backend_for_unregister = backend_for_effect.clone();
        let backend_for_tokens = backend_for_effect.clone();
        style::ensure_registered_with(
            &app.sheet,
            |rules| {
                backend_for_register.borrow_mut().register_stylesheet(rules);
            },
            |rules| {
                backend_for_unregister
                    .borrow_mut()
                    .unregister_stylesheet(rules);
            },
            |tokens| {
                backend_for_tokens
                    .borrow_mut()
                    .install_theme_variables(tokens);
            },
        );
        let backend_for_variants = backend_for_effect.clone();
        style::drain_pending_theme_variants(|name, tokens| {
            backend_for_variants
                .borrow_mut()
                .register_theme_variant(name, tokens);
        });
        let backend_for_active = backend_for_effect.clone();
        style::drain_pending_active_theme_signal(|sig, name| {
            backend_for_active
                .borrow_mut()
                .bind_active_theme_signal(sig, name);
        });

        if handles_states_natively {
            // Resolve the base (no state axes) and each declared state
            // overlay separately. The backend will emit CSS rules
            // scoped to each pseudo-class so the browser does the
            // state switching natively.
            //
            // We deliberately do NOT subscribe to `states_signal` here:
            // CSS handles all transitions, so the style effect should
            // re-fire only on theme/variant/override changes, not on
            // hover/press.
            #[cfg(feature = "debug-stats")]
            let _t_resolve_start = debug::now_micros();
            let base = resolve_style(&app);
            #[cfg(feature = "debug-stats")]
            debug::record_apply_phase(
                "attach_style_resolve_base",
                debug::now_micros().saturating_sub(_t_resolve_start),
            );
            #[cfg(feature = "debug-stats")]
            let _t_overlays_start = debug::now_micros();
            let overlays = resolve_state_overlays(&app);
            #[cfg(feature = "debug-stats")]
            debug::record_apply_phase(
                "attach_style_resolve_overlays",
                debug::now_micros().saturating_sub(_t_overlays_start),
            );
            #[cfg(feature = "debug-stats")]
            debug::record_apply_phase(
                "attach_style_resolve",
                debug::now_micros().saturating_sub(_t_resolve_start),
            );

            #[cfg(feature = "debug-stats")]
            let _t_apply_start = debug::now_micros();
            backend_for_effect
                .borrow_mut()
                .apply_styled_states(&handle.node, &base, &overlays);
            #[cfg(feature = "debug-stats")]
            debug::record_apply_phase(
                "attach_style_apply_call",
                debug::now_micros().saturating_sub(_t_apply_start),
            );
        } else {
            // Event-driven path: merge active-state axes into the
            // resolved application. Reading the signal subscribes this
            // effect to state changes, so a hover/press flip re-resolves
            // and re-applies through the regular apply_style path.
            //
            // Unwrap is safe: `states_signal` is only `None` when
            // `handles_states_natively == true`, in which case the
            // other branch above runs.
            let bits = states_signal.unwrap().get();
            let mut app = app;
            for axis in bits.active_axes() {
                app = app.with(axis, "on");
            }
            #[cfg(feature = "debug-stats")]
            let _t_resolve_start = debug::now_micros();
            let resolved = resolve_style(&app);
            #[cfg(feature = "debug-stats")]
            debug::record_apply_phase(
                "attach_style_resolve",
                debug::now_micros().saturating_sub(_t_resolve_start),
            );

            #[cfg(feature = "debug-stats")]
            let _t_apply_start = debug::now_micros();
            backend_for_effect
                .borrow_mut()
                .apply_style(&handle.node, &resolved);
            #[cfg(feature = "debug-stats")]
            debug::record_apply_phase(
                "attach_style_apply_call",
                debug::now_micros().saturating_sub(_t_apply_start),
            );
        }

        #[cfg(feature = "debug-stats")]
        debug::record_apply_style_exit();

        #[cfg(feature = "debug-stats")]
        debug::record_apply_phase(
            "attach_style_first_run",
            debug::now_micros().saturating_sub(_t_first_run_start),
        );
    });

    #[cfg(feature = "debug-stats")]
    debug::record_apply_phase(
        "attach_style_effect_alloc",
        debug::now_micros().saturating_sub(_t_effect_alloc_start),
    );

    #[cfg(feature = "debug-stats")]
    let _t_post_effect_start = debug::now_micros();

    // Hand the backend a setter so it can flip state bits from native
    // event listeners. The setter is `Rc<dyn Fn(StateBits, bool)>`
    // so the backend can clone it into per-event closures, and also
    // returned to the caller so it can wire prop-driven states like
    // `disabled` from the same signal.
    //
    // On natively-handling backends we have no `states_signal`, but
    // callers (e.g. `attach_disabled`) still hold the returned setter
    // and may invoke it from prop-driven flows. The setter is a no-op
    // in that case — `set_disabled` directly toggles the DOM
    // attribute, which is what activates `:disabled` CSS; we don't
    // need a Rust signal in between.
    let setter: Rc<dyn Fn(StateBits, bool)> = match states_signal {
        Some(sig) => Rc::new(move |bit, on| {
            sig.update(|bits| {
                *bits = if on { bits.with(bit) } else { bits.without(bit) };
            });
        }),
        None => Rc::new(|_, _| {}),
    };
    backend.borrow_mut().attach_states(node, setter.clone());

    #[cfg(feature = "debug-stats")]
    debug::record_apply_phase(
        "attach_style_post_effect",
        debug::now_micros().saturating_sub(_t_post_effect_start),
    );
    #[cfg(feature = "debug-stats")]
    debug::record_apply_phase(
        "attach_style_total",
        debug::now_micros().saturating_sub(_t_total_start),
    );

    setter
}

/// For backends that handle states natively, resolve each declared
/// state overlay against the application's variants + theme. Walks
/// the stylesheet's variant keys looking for `__state_*` axes,
/// resolves each one with the corresponding axis set to `"on"`, and
/// returns `(StateBits, Rc<StyleRules>)` pairs the backend can emit
/// as pseudo-class CSS.
fn resolve_state_overlays(app: &StyleApplication) -> Vec<(StateBits, Rc<StyleRules>)> {
    // Fast path: most stylesheets declare zero state blocks. The
    // cached slice is empty for them, so we skip both the
    // `variant_keys()` walk (which clones every axis/value String
    // out of the BTreeMap) AND any per-state resolve work.
    //
    // For 10k styled rows with no `state` blocks, this drops
    // `attach_style_resolve` from ~13μs per row to ~3μs — about a
    // 100ms total saving on the 10k-row case.
    let state_axes = app.sheet.state_axes();
    if state_axes.is_empty() {
        return Vec::new();
    }
    let mut out: Vec<(StateBits, Rc<StyleRules>)> = Vec::with_capacity(state_axes.len());
    for (bit, axis) in state_axes {
        // Resolve with this single state axis added on top of the
        // application's existing variants.
        let mut state_app = app.clone();
        state_app = state_app.with(axis.clone(), "on");
        let resolved = resolve_style(&state_app);
        out.push((*bit, resolved));
    }
    out
}

/// Reactive disabled-state wiring. Runs the user's closure inside an
/// `Effect` so the result tracks any signals it reads. On each
/// firing: (1) calls `Backend::set_disabled` so the native widget
/// is marked inert (web `disabled` attr, Android `setEnabled`); and
/// (2) flips the `DISABLED` state bit on the styled node so any
/// `state disabled { ... }` overlay applies via the existing state
/// machinery. If the button has no styled effect, `state_setter` is
/// `None` and step 2 is skipped.
fn attach_disabled<B: Backend + 'static>(
    backend: &Rc<RefCell<B>>,
    node: &B::Node,
    disabled: Box<dyn Fn() -> bool>,
    state_setter: Option<Rc<dyn Fn(StateBits, bool)>>,
) {
    let node_for_effect = node.clone();
    let backend_for_effect = backend.clone();
    let _e = Effect::new(move || {
        let d = disabled();
        backend_for_effect
            .borrow_mut()
            .set_disabled(&node_for_effect, d);
        if let Some(setter) = state_setter.as_ref() {
            setter(StateBits::DISABLED, d);
        }
    });
}

/// Renders a `When` primitive as a placeholder container whose subtree is
/// swapped each time `cond()` flips.
///
/// Lifecycle: the outer effect (registered with the surrounding scope)
/// reads `cond()` to track its dependencies. On every change it drops
/// the previous branch's nested `Scope` — freeing every signal and effect
/// in the old subtree atomically — and builds the new branch inside a
/// fresh nested scope.
/// Build a Virtualizer node. Sets up the callback bundle the
/// backend uses to query data + mount/release items, wraps each
/// `render_item(idx)` call in a fresh per-item Scope so signals
/// and effects nested inside an item are freed when the item is
/// released, and installs an Effect on the data so the backend
/// gets notified when item_count / keys / sizes change.
fn build_virtualizer<B: Backend + 'static>(
    backend: &Rc<RefCell<B>>,
    item_count: crate::derive::Derived<usize>,
    item_key: Box<dyn Fn(usize) -> primitives::virtualizer::ItemKey>,
    item_size: primitives::virtualizer::ItemSize,
    render_item: Rc<dyn Fn(usize) -> Primitive>,
    _row_template: Option<Box<Primitive>>,
    _row_index_signal_id: Option<u64>,
    overscan: f32,
    horizontal: bool,
) -> B::Node {
    // Per-item scope registry, owned by an Rc so the mount/release
    // closures (which live in the backend) share it. The framework
    // hands out monotonically-increasing u64 ids to identify each
    // mounted item; the backend stores the id alongside its cell so
    // it can release later.
    //
    // Also store measured sizes here. Backends that measure (web
    // ResizeObserver, native layout listeners) push updates via
    // `set_measured_size`; the framework keeps the canonical map.
    let scopes: Rc<RefCell<HashMap<u64, Box<reactive::Scope>>>> =
        Rc::new(RefCell::new(HashMap::new()));
    let measured_sizes: Rc<RefCell<HashMap<u64, f32>>> =
        Rc::new(RefCell::new(HashMap::new()));
    let next_scope_id: Rc<RefCell<u64>> = Rc::new(RefCell::new(0));

    // Shareable closures for the data side. `Rc` so the backend can
    // clone them into per-event handlers. For the count we pull
    // `Derived<usize>`'s compute closure; runtime backends call it
    // directly. Generator backends would consume the structured
    // metadata via a separate code path (TODO when Roku grows real
    // Virtualizer support).
    let item_count_rc: Rc<dyn Fn() -> usize> = item_count.compute.clone();
    let item_key_rc: Rc<dyn Fn(usize) -> primitives::virtualizer::ItemKey> = Rc::from(item_key);

    let measure_sizes = item_size.is_measured();
    let item_size_rc: Rc<dyn Fn(usize) -> f32> = match item_size {
        primitives::virtualizer::ItemSize::Known(f)
        | primitives::virtualizer::ItemSize::Measured(f) => f,
    };

    // `item_size` callback wraps the user's known/estimate with the
    // measured-override store: if we have a measured size, use it;
    // otherwise fall back to the user's value.
    let item_size_with_override: Rc<dyn Fn(usize) -> f32> = {
        let user = item_size_rc.clone();
        let measured = measured_sizes.clone();
        let key_fn = item_key_rc.clone();
        Rc::new(move |idx| {
            let key = key_fn(idx);
            // Measured cache is keyed by item key (not index) so it
            // survives reorderings.
            if let Some(v) = measured.borrow().get(&key) {
                return *v;
            }
            user(idx)
        })
    };

    // mount_item: build the subtree for `idx` inside a fresh Scope,
    // return its native node + the scope id.
    let mount_item: Rc<dyn Fn(usize) -> (B::Node, u64)> = {
        let scopes = scopes.clone();
        let next_id = next_scope_id.clone();
        let render = render_item.clone();
        let backend = backend.clone();
        Rc::new(move |idx| {
            let mut scope = Box::new(reactive::Scope::new());
            // Build inside the scope so any Effects the walker creates
            // (switch/when/style/etc.) register with this per-item
            // scope and stay alive for the item's lifetime. See the
            // matching comment in `build_navigator`'s `mount_screen`
            // for why this matters — Effects built outside any scope
            // get `owns: true` and free immediately when the handle
            // drops at end of `build`, taking their shared
            // `Rc<RefCell<...>>` state with them.
            let node = reactive::with_scope(&mut scope, || {
                let primitive = render(idx);
                build(&backend, primitive)
            });
            let id = {
                let mut n = next_id.borrow_mut();
                let v = *n;
                *n = n.checked_add(1).unwrap_or(0);
                v
            };
            scopes.borrow_mut().insert(id, scope);
            #[cfg(feature = "debug-stats")]
            debug::record_virtualizer_mount(idx, id);
            (node, id)
        })
    };

    // release_item: drop the scope, freeing every signal/effect/ref
    // scoped to the item.
    let release_item: Rc<dyn Fn(u64)> = {
        let scopes = scopes.clone();
        let measured = measured_sizes.clone();
        Rc::new(move |id| {
            #[cfg(feature = "debug-stats")]
            debug::record_virtualizer_release(id);
            // Drop the scope. Its Drop impl frees the reactive slots.
            scopes.borrow_mut().remove(&id);
            // We can't safely free the measured-size entry here
            // because the entry is keyed by item *key*, not scope
            // id. The measured cache survives unmount intentionally
            // — when the item re-enters the window, we want to use
            // the previously-measured size rather than start over
            // with an estimate.
            let _ = measured;
        })
    };

    // set_measured_size: backend tells us "this scope's rendered
    // size is X." We store it by item key so the cache survives
    // unmount/remount.
    //
    // Backend identifies the item by scope id; we look up the key
    // by walking which idx this scope was mounted for. Simpler:
    // have the backend pass the *index* too. But scope_id is what
    // it stored, and it doesn't know the current index after
    // reorders. So we maintain a scope_id -> key reverse map.
    let scope_id_to_key: Rc<RefCell<HashMap<u64, primitives::virtualizer::ItemKey>>> =
        Rc::new(RefCell::new(HashMap::new()));
    let set_measured_size: Rc<dyn Fn(u64, f32)> = {
        let measured = measured_sizes.clone();
        let map = scope_id_to_key.clone();
        Rc::new(move |scope_id, size| {
            if let Some(key) = map.borrow().get(&scope_id) {
                measured.borrow_mut().insert(*key, size);
            }
        })
    };

    // Augment mount_item to also record scope_id -> key.
    let mount_item: Rc<dyn Fn(usize) -> (B::Node, u64)> = {
        let inner = mount_item.clone();
        let key_fn = item_key_rc.clone();
        let map = scope_id_to_key.clone();
        Rc::new(move |idx| {
            let (node, id) = inner(idx);
            let k = key_fn(idx);
            map.borrow_mut().insert(id, k);
            (node, id)
        })
    };

    // Augment release_item to clean up the scope_id -> key entry.
    let release_item: Rc<dyn Fn(u64)> = {
        let inner = release_item.clone();
        let map = scope_id_to_key.clone();
        Rc::new(move |id| {
            map.borrow_mut().remove(&id);
            inner(id);
        })
    };

    let callbacks = VirtualizerCallbacks {
        item_count: item_count_rc.clone(),
        item_key: item_key_rc.clone(),
        item_size: item_size_with_override,
        measure_sizes,
        mount_item,
        release_item,
        set_measured_size,
    };

    let node = time_backend_create(pkind!(Virtualizer), || {
        backend.borrow_mut().create_virtualizer(callbacks, overscan, horizontal)
    });

    // Effect: re-fires whenever the data signal changes (any reads
    // inside item_count / item_key / etc. subscribe). We tell the
    // backend to re-diff its mounted set.
    {
        let backend = backend.clone();
        let node = node.clone();
        let count = item_count_rc.clone();
        let _e = Effect::new(move || {
            // Touch item_count so we subscribe to the data signal.
            // (item_count's body calls data.get().) We don't use the
            // value here directly — the backend re-queries.
            let _ = count();
            backend.borrow_mut().virtualizer_data_changed(&node);
        });
    }

    node
}

/// Build a `Primitive::Virtualizer` for the structured /
/// generator-backend path (Roku). Captures the row template's
/// commands as a single slot the backend stashes for per-row
/// replay on the device. Skips the closure-driven Virtualizer
/// machinery entirely — there's no Effect, no per-cell scope
/// registry, no item_size measurement. Generator backends own
/// the cell lifecycle.
fn build_virtualizer_declarative<B: Backend + 'static>(
    backend: &Rc<RefCell<B>>,
    item_count: crate::derive::Derived<usize>,
    row_template: Primitive,
    row_index_signal_id: Option<u64>,
    horizontal: bool,
) -> B::Node {
    let anchor = time_backend_create(pkind!(View), || {
        backend.borrow_mut().create_reactive_anchor()
    });

    // Capture the row template's construction commands as a slot.
    backend.borrow_mut().begin_slot_capture();
    let template_node = build(backend, row_template);
    backend.borrow_mut().end_slot_capture(&template_node);

    {
        let mut b = backend.borrow_mut();
        for (sid, val) in item_count.inputs.iter().zip(item_count.initial.iter()) {
            b.note_signal_initial(*sid, val);
        }
        b.note_virtualizer_binding(
            &anchor,
            &item_count.inputs,
            item_count.method,
            &template_node,
            row_index_signal_id,
            horizontal,
        );
    }

    anchor
}

fn build_when<B: Backend + 'static>(
    backend: &Rc<RefCell<B>>,
    cond: crate::derive::Derived<bool>,
    then: Box<dyn Fn() -> Primitive>,
    otherwise: Box<dyn Fn() -> Primitive>,
) -> B::Node {
    let placeholder = time_backend_create(pkind!(View), || {
        backend.borrow_mut().create_reactive_anchor()
    });
    let backend_for_effect = backend.clone();
    let placeholder_for_effect = placeholder.clone();

    // The branch scope lives across effect re-runs. Rc<RefCell<Option<…>>>
    // so we can replace it atomically when the condition flips.
    let branch_scope: Rc<RefCell<Option<Box<reactive::Scope>>>> = Rc::new(RefCell::new(None));
    let branch_scope_for_effect = branch_scope.clone();

    let compute = cond.compute.clone();
    let _e = Effect::new(move || {
        let active = (compute)();

        // Drop the previous branch's scope before building the new one,
        // freeing its signals + effects atomically.
        *branch_scope_for_effect.borrow_mut() = None;
        backend_for_effect
            .borrow_mut()
            .clear_children(&placeholder_for_effect);

        // Build inside a fresh nested scope. `untrack` keeps inner setup
        // reads from subscribing to *this* outer effect — inner effects
        // subscribe themselves when they run.
        let mut new_scope = Box::new(reactive::Scope::new());
        untrack(|| {
            reactive::with_scope(&mut new_scope, || {
                let branch = if active { then() } else { otherwise() };
                let child_node = build(&backend_for_effect, branch);
                let mut placeholder_mut = placeholder_for_effect.clone();
                backend_for_effect
                    .borrow_mut()
                    .insert(&mut placeholder_mut, child_node);
            });
        });
        *branch_scope_for_effect.borrow_mut() = Some(new_scope);
    });

    placeholder
}

/// Build a `Primitive::When` for backends that opt into
/// declarative conditional rendering via
/// `handles_when_natively()`. Both branches are constructed
/// eagerly, both attached to the same anchor, and the binding
/// metadata is handed to the backend so it can ship the
/// "which-branch-is-active" decision over its wire format.
///
/// No `Effect` is set up here. The remote runtime is responsible
/// for evaluating the condition + toggling subtree visibility on
/// signal change. Closures + signal IDs both flow through the
/// binding so closure-driven backends still have everything they
/// need if they ever want to dual-path.
fn build_when_declarative<B: Backend + 'static>(
    backend: &Rc<RefCell<B>>,
    cond: crate::derive::Derived<bool>,
    then: Box<dyn Fn() -> Primitive>,
    otherwise: Box<dyn Fn() -> Primitive>,
) -> B::Node {
    let anchor = time_backend_create(pkind!(View), || {
        backend.borrow_mut().create_reactive_anchor()
    });

    // Backends taking the declarative path always pair this with
    // lazy slot capture today (Roku). The capture brackets each
    // branch's subtree build so the backend stashes the commands
    // separately for play/teardown on the device.
    backend.borrow_mut().begin_slot_capture();
    let then_node = build(backend, then());
    backend.borrow_mut().end_slot_capture(&then_node);

    backend.borrow_mut().begin_slot_capture();
    let otherwise_node = build(backend, otherwise());
    backend.borrow_mut().end_slot_capture(&otherwise_node);

    // Declare signals + the when binding to the backend.
    {
        let mut b = backend.borrow_mut();
        for (sid, val) in cond.inputs.iter().zip(cond.initial.iter()) {
            b.note_signal_initial(*sid, val);
        }
        b.note_when_binding(
            &anchor,
            &cond.inputs,
            cond.method,
            &then_node,
            &otherwise_node,
        );
    }

    anchor
}

/// Build a `Primitive::Switch` for backends that opted into lazy
/// slot capture (Roku). Each arm's subtree is captured as a slot
/// (a self-contained command list); the backend stashes those
/// keyed by their root node id and the runtime plays / tears them
/// down on the device based on the discriminant's match.
fn build_switch_declarative<B: Backend + 'static>(
    backend: &Rc<RefCell<B>>,
    discriminant: crate::derive::Derived<crate::__serde_json::Value>,
    arms: Vec<(crate::__serde_json::Value, Box<dyn Fn() -> Primitive>)>,
    default: Box<dyn Fn() -> Primitive>,
) -> B::Node {
    let anchor = time_backend_create(pkind!(View), || {
        backend.borrow_mut().create_reactive_anchor()
    });

    // Capture each arm's commands as a slot.
    let mut arm_node_pairs: Vec<(crate::__serde_json::Value, B::Node)> =
        Vec::with_capacity(arms.len());
    for (value, builder) in arms.iter() {
        backend.borrow_mut().begin_slot_capture();
        let arm_node = build(backend, builder());
        backend.borrow_mut().end_slot_capture(&arm_node);
        arm_node_pairs.push((value.clone(), arm_node));
    }
    backend.borrow_mut().begin_slot_capture();
    let default_node = build(backend, default());
    backend.borrow_mut().end_slot_capture(&default_node);

    {
        let mut b = backend.borrow_mut();
        for (sid, val) in discriminant.inputs.iter().zip(discriminant.initial.iter()) {
            b.note_signal_initial(*sid, val);
        }
        b.note_switch_binding(
            &anchor,
            &discriminant.inputs,
            discriminant.method,
            &arm_node_pairs,
            &default_node,
        );
    }

    anchor
}

/// Build a `Primitive::Presence`. Manages mount/unmount timing so
/// the child's enter/exit animations actually have a window to
/// play.
///
/// State machine:
///
/// - **Initially absent** (`present()` is false): the child isn't
///   built. The placeholder sits empty.
/// - **Mounting (false → true)**: build the child in a fresh
///   scope. If `enter` is set: apply `enter.state` synchronously
///   (snap pre-paint), then schedule one animation frame later to
///   apply the rest state with the enter transition.
/// - **Mounted, present remains true**: the effect re-runs on
///   signal changes inside `present`, but if the bool didn't flip
///   we leave everything alone.
/// - **Exiting (true → false)**: if `exit` is set, apply
///   `exit.state` with the exit transition, schedule a timer for
///   `exit.duration_ms` that drops the scope. If `exit` is None,
///   drop the scope immediately.
/// - **Reversal (exiting → true)**: cancel the pending drop timer,
///   re-apply rest state with the enter transition (so the in-
///   flight animation reverses smoothly). The scope is reused.
///
/// All scope storage + scheduled task storage is in `Rc<RefCell>`
/// fields shared between the outer effect and the per-frame timers.
/// Drop semantics: when the surrounding scope drops (e.g. parent
/// `when` rebuilds), our owned `child_scope` drops, which drops the
/// child's subtree; the `ScheduledTask` drops at the same time,
/// cancelling any in-flight timer.
fn build_presence<B: Backend + 'static>(
    backend: &Rc<RefCell<B>>,
    child_fn: Box<dyn Fn() -> Primitive>,
    present: Box<dyn Fn() -> bool>,
    enter: Option<primitives::presence::PresenceAnim>,
    exit: Option<primitives::presence::PresenceAnim>,
) -> B::Node {
    use crate::primitives::presence::PresenceState;

    let placeholder = time_backend_create(pkind!(Presence), || backend.borrow_mut().create_view());

    // Shared state across the effect + scheduled tasks. `Rc<RefCell>`
    // so the outer Effect and the timer closures all reach the same
    // entry. `child_node` is `Option<Self::Node>` so we can tell
    // "currently mounted" apart from "absent."
    let child_scope: Rc<RefCell<Option<Box<reactive::Scope>>>> = Rc::new(RefCell::new(None));
    let child_node: Rc<RefCell<Option<B::Node>>> = Rc::new(RefCell::new(None));
    let pending_exit: Rc<RefCell<Option<crate::scheduling::ScheduledTask>>> =
        Rc::new(RefCell::new(None));
    let pending_enter: Rc<RefCell<Option<crate::scheduling::ScheduledTask>>> =
        Rc::new(RefCell::new(None));
    let last_present: Rc<RefCell<bool>> = Rc::new(RefCell::new(false));

    let backend_for_effect = backend.clone();
    let placeholder_for_effect = placeholder.clone();
    let child_scope_for_effect = child_scope.clone();
    let child_node_for_effect = child_node.clone();
    let pending_exit_for_effect = pending_exit.clone();
    let pending_enter_for_effect = pending_enter.clone();
    let last_present_for_effect = last_present.clone();

    let _e = Effect::new(move || {
        let want_present = present();
        let was_present = *last_present_for_effect.borrow();
        *last_present_for_effect.borrow_mut() = want_present;

        if want_present && !was_present {
            // ---- Mount (off → on) ----
            // Cancel any in-flight exit timer (shouldn't be one if
            // was_present == false, but be defensive against the
            // case where we mount-exit-mount within a single tick).
            *pending_exit_for_effect.borrow_mut() = None;

            // Build the child in a fresh nested scope. `untrack` so
            // inner setup signal reads don't subscribe back to this
            // effect — they wire their own per-node effects.
            let mut new_scope = Box::new(reactive::Scope::new());
            let built_node: Rc<RefCell<Option<B::Node>>> = Rc::new(RefCell::new(None));
            let built_node_inner = built_node.clone();
            let backend_inner = backend_for_effect.clone();
            let child_fn_call = || child_fn();
            untrack(|| {
                reactive::with_scope(&mut new_scope, || {
                    let prim = child_fn_call();
                    let node = build(&backend_inner, prim);
                    *built_node_inner.borrow_mut() = Some(node);
                });
            });
            let node = built_node.borrow_mut().take().expect("presence child built");
            // Insert into the placeholder.
            let mut placeholder_mut = placeholder_for_effect.clone();
            backend_for_effect
                .borrow_mut()
                .insert(&mut placeholder_mut, node.clone());
            *child_scope_for_effect.borrow_mut() = Some(new_scope);
            *child_node_for_effect.borrow_mut() = Some(node.clone());

            // If `enter` is set, snap to the enter state pre-paint,
            // then schedule the animate-to-rest one frame later.
            if let Some(anim) = enter {
                backend_for_effect
                    .borrow_mut()
                    .apply_presence(&node, anim.state, None);
                // Schedule the resting-state apply. Holding the
                // ScheduledTask in `pending_enter` lets a quick
                // unmount cancel it (otherwise we'd race the timer
                // against a freshly-applied exit state).
                let backend_for_frame = backend_for_effect.clone();
                let pending_enter_for_clear = pending_enter_for_effect.clone();
                let task = crate::scheduling::after_animation_frame(move || {
                    backend_for_frame.borrow_mut().apply_presence(
                        &node,
                        PresenceState::rest(),
                        Some((anim.duration_ms, anim.easing)),
                    );
                    // Self-clear: once the frame fires, the task is
                    // spent. Drop our handle so subsequent state
                    // checks see `None`.
                    *pending_enter_for_clear.borrow_mut() = None;
                });
                *pending_enter_for_effect.borrow_mut() = Some(task);
            }
        } else if !want_present && was_present {
            // ---- Unmount (on → off) ----
            // Cancel any pending enter timer — the child shouldn't
            // animate toward "rest" if we're about to exit it.
            *pending_enter_for_effect.borrow_mut() = None;

            let node_opt = child_node_for_effect.borrow().clone();
            let node = match node_opt {
                Some(n) => n,
                None => return,
            };

            if let Some(anim) = exit {
                backend_for_effect.borrow_mut().apply_presence(
                    &node,
                    anim.state,
                    Some((anim.duration_ms, anim.easing)),
                );
                // Schedule scope drop after the animation completes.
                let child_scope_for_timer = child_scope_for_effect.clone();
                let child_node_for_timer = child_node_for_effect.clone();
                let backend_for_timer = backend_for_effect.clone();
                let placeholder_for_timer = placeholder_for_effect.clone();
                let pending_exit_for_clear = pending_exit_for_effect.clone();
                let task = crate::scheduling::after_ms(anim.duration_ms as i32, move || {
                    // Tear down the child: drop its scope (which
                    // frees every signal/effect/ref inside) and
                    // remove its node from the placeholder.
                    *child_scope_for_timer.borrow_mut() = None;
                    *child_node_for_timer.borrow_mut() = None;
                    backend_for_timer
                        .borrow_mut()
                        .clear_children(&placeholder_for_timer);
                    *pending_exit_for_clear.borrow_mut() = None;
                });
                *pending_exit_for_effect.borrow_mut() = Some(task);
            } else {
                // No exit animation — drop the scope immediately.
                *child_scope_for_effect.borrow_mut() = None;
                *child_node_for_effect.borrow_mut() = None;
                backend_for_effect
                    .borrow_mut()
                    .clear_children(&placeholder_for_effect);
            }
        } else if want_present && was_present {
            // ---- Reversal mid-exit ----
            // If a pending exit task is alive, the user just
            // flipped back to present *during* the exit animation.
            // Cancel the timer (dropping the task) and re-animate
            // toward rest from wherever the interpolation currently
            // is.
            if pending_exit_for_effect.borrow().is_some() {
                *pending_exit_for_effect.borrow_mut() = None;
                if let Some(anim) = enter {
                    if let Some(node) = child_node_for_effect.borrow().clone() {
                        backend_for_effect.borrow_mut().apply_presence(
                            &node,
                            PresenceState::rest(),
                            Some((anim.duration_ms, anim.easing)),
                        );
                    }
                } else if let Some(node) = child_node_for_effect.borrow().clone() {
                    // No enter animation declared — snap back to
                    // rest with no transition.
                    backend_for_effect
                        .borrow_mut()
                        .apply_presence(&node, PresenceState::rest(), None);
                }
            }
        }
    });

    placeholder
}

/// Build a `Primitive::Switch` via the closure-driven Effect path.
/// On each signal change inside `discriminant.compute`, the Effect
/// re-evaluates the discriminant, dedupes against the previously
/// seen JSON value, and (if changed) tears down the prior branch
/// scope and builds the new active arm. State inside the old
/// subtree is freed atomically. When `arms` is empty (the typed
/// `switch()` builder's degenerate shape) every fire just calls
/// `default()`, which internally dispatches against the typed
/// scrutinee.
fn build_switch<B: Backend + 'static>(
    backend: &Rc<RefCell<B>>,
    discriminant: crate::derive::Derived<crate::__serde_json::Value>,
    arms: Vec<(crate::__serde_json::Value, Box<dyn Fn() -> Primitive>)>,
    default: Box<dyn Fn() -> Primitive>,
) -> B::Node {
    let placeholder = time_backend_create(pkind!(View), || {
        backend.borrow_mut().create_reactive_anchor()
    });
    let backend_for_effect = backend.clone();
    let placeholder_for_effect = placeholder.clone();

    let branch_scope: Rc<RefCell<Option<Box<reactive::Scope>>>> = Rc::new(RefCell::new(None));
    let last_key: Rc<RefCell<Option<crate::__serde_json::Value>>> = Rc::new(RefCell::new(None));
    let branch_scope_for_effect = branch_scope.clone();
    let last_key_for_effect = last_key.clone();

    // Opaque discriminant (closure-driven path) skips the JSON-key
    // dedup — its `compute` always returns `Null` and the
    // `default` closure does its own arm dispatch using a cached
    // typed scrutinee value, so we always need to re-fire after
    // every signal change.
    let opaque = discriminant.is_opaque();
    let compute = discriminant.compute.clone();
    let arms: Rc<Vec<(crate::__serde_json::Value, Box<dyn Fn() -> Primitive>)>> = Rc::new(arms);
    let default: Rc<dyn Fn() -> Primitive> = default.into();

    let _e = Effect::new(move || {
        let new_key = (compute)();

        if !opaque {
            let same_as_last = last_key_for_effect
                .borrow()
                .as_ref()
                .map(|prev| prev == &new_key)
                .unwrap_or(false);
            if same_as_last {
                return;
            }
        }

        let placeholder_for_microtask = placeholder_for_effect.clone();
        let backend_for_microtask = backend_for_effect.clone();
        let branch_scope_for_microtask = branch_scope_for_effect.clone();
        let last_key_for_microtask = last_key_for_effect.clone();
        let arms_for_microtask = arms.clone();
        let default_for_microtask = default.clone();

        schedule_microtask(move || {
            if !opaque {
                let same_as_last = last_key_for_microtask
                    .borrow()
                    .as_ref()
                    .map(|prev| prev == &new_key)
                    .unwrap_or(false);
                if same_as_last {
                    return;
                }
            }

            *branch_scope_for_microtask.borrow_mut() = None;
            backend_for_microtask
                .borrow_mut()
                .clear_children(&placeholder_for_microtask);

            let mut new_scope = Box::new(reactive::Scope::new());
            untrack(|| {
                reactive::with_scope(&mut new_scope, || {
                    // Pick the matching arm by JSON equality, or
                    // fall through to default. Empty arms vec
                    // (the typed builder's degenerate shape)
                    // always falls through.
                    let branch = arms_for_microtask
                        .iter()
                        .find(|(pat, _)| pat == &new_key)
                        .map(|(_, builder)| builder())
                        .unwrap_or_else(|| default_for_microtask());
                    let child_node = build(&backend_for_microtask, branch);
                    let mut placeholder_mut = placeholder_for_microtask.clone();
                    backend_for_microtask
                        .borrow_mut()
                        .insert(&mut placeholder_mut, child_node);
                });
            });
            *branch_scope_for_microtask.borrow_mut() = Some(new_scope);
            *last_key_for_microtask.borrow_mut() = Some(new_key);
        });
    });

    placeholder
}
