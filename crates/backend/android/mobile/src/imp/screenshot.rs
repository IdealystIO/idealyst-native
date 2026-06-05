//! Native screen capture for the Android backend — the on-device side of
//! the Robot bridge's `"screenshot"` verb.
//!
//! Renders the root view's hierarchy into an `ARGB_8888` `Bitmap` via
//! `View.draw(Canvas)` and PNG-encodes it with `Bitmap.compress`. We walk
//! up to `getRootView()` so the snapshot covers the whole window content,
//! not just the framework's container subtree.
//!
//! ## Why `View.draw`, not `PixelCopy`
//!
//! `PixelCopy` is higher fidelity (it reads back the actual surface,
//! including `SurfaceView`/`TextureView`/GPU content) but is inherently
//! **asynchronous** — it delivers on a callback thread, which doesn't fit
//! the synchronous bridge reply path. `View.draw(Canvas)` is synchronous
//! on the UI thread and captures the standard view hierarchy, which is
//! what a debug UI screenshot needs. The documented gap — shared with the
//! iOS/macOS impls — is that content on a *separate* surface (a `Graphics`
//! primitive's GPU surface, video) won't appear; those need the async
//! `PixelCopy` path, deferred until the bridge supports async replies.
//!
//! Must run on the UI thread; the bridge polls there (the scheduler posts
//! to the main looper), so the caller already satisfies it.

use jni::objects::{GlobalRef, JByteArray, JObject, JValue};
use jni::JNIEnv;
use runtime_core::Screenshot;

fn jni_err(e: jni::errors::Error) -> String {
    format!("android screenshot JNI error: {e}")
}

/// Capture the window rooted at `root` as a PNG.
pub(crate) fn capture(root: &GlobalRef) -> Result<Screenshot, String> {
    super::with_env(|env| capture_with_env(env, root.as_obj()))
}

fn capture_with_env(env: &mut JNIEnv, root: &JObject) -> Result<Screenshot, String> {
    // target = root.getRootView() — the topmost view of the window.
    let target = env
        .call_method(root, "getRootView", "()Landroid/view/View;", &[])
        .and_then(|v| v.l())
        .map_err(jni_err)?;
    if target.is_null() {
        return Err("getRootView() returned null".into());
    }

    let width = env
        .call_method(&target, "getWidth", "()I", &[])
        .and_then(|v| v.i())
        .map_err(jni_err)?;
    let height = env
        .call_method(&target, "getHeight", "()I", &[])
        .and_then(|v| v.i())
        .map_err(jni_err)?;
    if width <= 0 || height <= 0 {
        return Err("root view has zero size (not laid out yet)".into());
    }

    // config = Bitmap.Config.ARGB_8888
    let config_cls = env
        .find_class("android/graphics/Bitmap$Config")
        .map_err(jni_err)?;
    let config = env
        .get_static_field(
            &config_cls,
            "ARGB_8888",
            "Landroid/graphics/Bitmap$Config;",
        )
        .and_then(|v| v.l())
        .map_err(jni_err)?;

    // bitmap = Bitmap.createBitmap(width, height, config)
    let bitmap_cls = env.find_class("android/graphics/Bitmap").map_err(jni_err)?;
    let bitmap = env
        .call_static_method(
            &bitmap_cls,
            "createBitmap",
            "(IILandroid/graphics/Bitmap$Config;)Landroid/graphics/Bitmap;",
            &[
                JValue::Int(width),
                JValue::Int(height),
                JValue::Object(&config),
            ],
        )
        .and_then(|v| v.l())
        .map_err(jni_err)?;

    // canvas = new Canvas(bitmap); target.draw(canvas)
    let canvas = env
        .new_object(
            "android/graphics/Canvas",
            "(Landroid/graphics/Bitmap;)V",
            &[JValue::Object(&bitmap)],
        )
        .map_err(jni_err)?;
    env.call_method(
        &target,
        "draw",
        "(Landroid/graphics/Canvas;)V",
        &[JValue::Object(&canvas)],
    )
    .map_err(jni_err)?;

    // baos = new ByteArrayOutputStream();
    // bitmap.compress(Bitmap.CompressFormat.PNG, 100, baos)
    let baos = env
        .new_object("java/io/ByteArrayOutputStream", "()V", &[])
        .map_err(jni_err)?;
    let fmt_cls = env
        .find_class("android/graphics/Bitmap$CompressFormat")
        .map_err(jni_err)?;
    let png_fmt = env
        .get_static_field(
            &fmt_cls,
            "PNG",
            "Landroid/graphics/Bitmap$CompressFormat;",
        )
        .and_then(|v| v.l())
        .map_err(jni_err)?;
    env.call_method(
        &bitmap,
        "compress",
        "(Landroid/graphics/Bitmap$CompressFormat;ILjava/io/OutputStream;)Z",
        &[JValue::Object(&png_fmt), JValue::Int(100), JValue::Object(&baos)],
    )
    .map_err(jni_err)?;

    // bytes = baos.toByteArray()
    let arr_obj = env
        .call_method(&baos, "toByteArray", "()[B", &[])
        .and_then(|v| v.l())
        .map_err(jni_err)?;
    let arr: JByteArray = arr_obj.into();
    let png = env.convert_byte_array(&arr).map_err(jni_err)?;

    // Release the bitmap's native pixels promptly rather than waiting for
    // GC — a full-window ARGB_8888 buffer is multiple MB.
    let _ = env.call_method(&bitmap, "recycle", "()V", &[]);

    Ok(Screenshot {
        png,
        width: width as u32,
        height: height as u32,
    })
}
