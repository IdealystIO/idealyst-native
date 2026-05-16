package io.idealyst.hello

import android.os.Bundle
import android.view.ViewGroup
import android.widget.FrameLayout
import androidx.appcompat.app.AppCompatActivity

/**
 * Single Activity that hands a Context + parent ViewGroup down to the
 * Rust side and lets the framework render the shared `hello::app()`
 * tree underneath it.
 *
 * The root is a `FrameLayout` sized to fill the activity window
 * (`MATCH_PARENT × MATCH_PARENT`). A real fill-the-screen parent is
 * required for `DrawerLayout` to mount inside it — that widget
 * insists on being measured with `MeasureSpec.EXACTLY` and throws
 * if its parent doesn't pass an exact height/width. An ancestor
 * `ScrollView` would give the drawer `UNSPECIFIED` height and the
 * screen would render as 0px tall.
 *
 * App-level screens that need internal scrolling can wrap their
 * content in a framework `ScrollView` primitive — that's the right
 * place for "this particular screen has more content than fits."
 */
class MainActivity : AppCompatActivity() {

    companion object {
        init {
            // Loading the Rust cdylib also fires its `JNI_OnLoad`, which
            // caches the `JavaVM` so the backend can attach threads to
            // it on demand.
            System.loadLibrary("hello_android")
        }
    }

    override fun onCreate(savedInstanceState: Bundle?) {
        super.onCreate(savedInstanceState)

        val root = FrameLayout(this).apply {
            layoutParams = ViewGroup.LayoutParams(
                ViewGroup.LayoutParams.MATCH_PARENT,
                ViewGroup.LayoutParams.MATCH_PARENT,
            )
        }
        setContentView(root)

        NativeBridge.attach(this, root)
    }

    override fun onDestroy() {
        super.onDestroy()
        NativeBridge.detach()
    }
}
