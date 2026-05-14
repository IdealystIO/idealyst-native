package com.idealyst.hello

import android.os.Bundle
import android.view.ViewGroup
import android.widget.LinearLayout
import android.widget.ScrollView
import androidx.appcompat.app.AppCompatActivity

/**
 * Single Activity that hands a Context + parent ViewGroup down to the
 * Rust side and lets the framework render the shared `hello::app()`
 * tree underneath it.
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

        // Wrap the framework root in a ScrollView so the demo tree (which
        // can exceed a phone's screen height) is scrollable instead of
        // clipped.
        val scroll = ScrollView(this).apply {
            layoutParams = ViewGroup.LayoutParams(
                ViewGroup.LayoutParams.MATCH_PARENT,
                ViewGroup.LayoutParams.MATCH_PARENT,
            )
        }

        val root = LinearLayout(this).apply {
            orientation = LinearLayout.VERTICAL
            layoutParams = ViewGroup.LayoutParams(
                ViewGroup.LayoutParams.MATCH_PARENT,
                ViewGroup.LayoutParams.WRAP_CONTENT,
            )
        }
        scroll.addView(root)
        setContentView(scroll)

        NativeBridge.attach(this, root)
    }

    override fun onDestroy() {
        super.onDestroy()
        NativeBridge.detach()
    }
}
