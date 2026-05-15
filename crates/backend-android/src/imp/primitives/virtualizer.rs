//! `Primitive::Virtualizer` — `androidx.recyclerview.widget.RecyclerView`
//! + a Kotlin `RustListAdapter` that trampolines every lifecycle event
//! back to a leaked `VirtualizerCallbacks` box.
//!
//! The framework still owns mount/release ordering — it hands us
//! callbacks, we hand them to Kotlin, Kotlin calls back through JNI
//! on every `onBindViewHolder` / `onViewRecycled`.

use crate::imp::helpers::apply_default_layout_params;
use crate::imp::{with_env, AndroidBackend};
use framework_core::VirtualizerCallbacks;
use jni::objects::{GlobalRef, JValue};
use jni::sys::jlong;

pub(crate) fn create(
    b: &AndroidBackend,
    callbacks: VirtualizerCallbacks<GlobalRef>,
    overscan: f32,
    horizontal: bool,
) -> GlobalRef {
    // We leak the box to get a stable pointer; `nativeDrop` (called
    // from the adapter teardown path, if ever wired) frees it. The
    // Activity outlives the list in this demo so the leak is bounded.
    let boxed = Box::new(callbacks);
    let ptr = Box::into_raw(boxed) as jlong;

    with_env(|env| {
        // RecyclerView(Context).
        let rv_class = env
            .find_class("androidx/recyclerview/widget/RecyclerView")
            .expect("RecyclerView class — add androidx.recyclerview to the consuming app's Gradle deps");
        let rv = env
            .new_object(
                &rv_class,
                "(Landroid/content/Context;)V",
                &[JValue::Object(&b.context.as_obj())],
            )
            .unwrap();

        // RustLinearLayoutManager(Context, orientation, overscanFactor).
        // VERTICAL=1, HORIZONTAL=0 (matches LinearLayoutManager constants).
        let lm_class = env
            .find_class("io/idealyst/runtime/RustLinearLayoutManager")
            .unwrap();
        let orientation_int = if horizontal { 0 } else { 1 };
        let lm = env
            .new_object(
                &lm_class,
                "(Landroid/content/Context;IF)V",
                &[
                    JValue::Object(&b.context.as_obj()),
                    JValue::Int(orientation_int),
                    JValue::Float(overscan),
                ],
            )
            .unwrap();
        env.call_method(
            &rv,
            "setLayoutManager",
            "(Landroidx/recyclerview/widget/RecyclerView$LayoutManager;)V",
            &[JValue::Object(&lm)],
        )
        .unwrap();

        // RustListAdapter(nativePtr).
        let adapter_class = env
            .find_class("io/idealyst/runtime/RustListAdapter")
            .unwrap();
        let adapter = env
            .new_object(&adapter_class, "(J)V", &[JValue::Long(ptr)])
            .unwrap();
        env.call_method(
            &rv,
            "setAdapter",
            "(Landroidx/recyclerview/widget/RecyclerView$Adapter;)V",
            &[JValue::Object(&adapter)],
        )
        .unwrap();

        apply_default_layout_params(env, &rv);
        env.new_global_ref(rv).unwrap()
    })
}

pub(crate) fn data_changed(node: &GlobalRef) {
    // Fetch the RecyclerView's adapter and tell it the data changed.
    // The adapter computes a key diff against its last snapshot and
    // dispatches granular updates so surviving items don't rebind.
    with_env(|env| {
        let adapter = match env.call_method(
            node.as_obj(),
            "getAdapter",
            "()Landroidx/recyclerview/widget/RecyclerView$Adapter;",
            &[],
        ) {
            Ok(v) => v.l().unwrap(),
            Err(_) => return,
        };
        if adapter.is_null() {
            return;
        }
        // `dataChanged()` lives on RustListAdapter, not the base
        // Adapter type — Java dispatch finds it by the runtime class.
        let _ = env.call_method(&adapter, "dataChanged", "()V", &[]);
    });
}
