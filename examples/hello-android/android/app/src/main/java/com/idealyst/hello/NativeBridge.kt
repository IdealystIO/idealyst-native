package com.idealyst.hello

import android.content.Context
import android.view.ViewGroup

/**
 * Thin facade in front of the Rust JNI entry points. Keeps the `external`
 * declarations off the Activity itself.
 */
object NativeBridge {
    @JvmStatic external fun attach(context: Context, root: ViewGroup)
    @JvmStatic external fun detach()
}
