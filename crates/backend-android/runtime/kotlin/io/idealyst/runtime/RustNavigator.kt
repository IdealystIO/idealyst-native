package io.idealyst.runtime

import android.content.Context
import android.view.View
import android.view.ViewGroup
import android.widget.FrameLayout
import androidx.fragment.app.FragmentActivity
import androidx.fragment.app.FragmentManager

/**
 * Per-navigator-instance Kotlin controller. Wraps a [FrameLayout]
 * (the navigator's visible container) and the host
 * [FragmentManager], and exposes push / pop / replace / reset calls
 * that the Rust side invokes via JNI to commit fragment transactions.
 *
 * One controller per Rust `Primitive::Navigator` instance.
 *
 * Container id: every controller assigns a unique synthetic id to
 * its FrameLayout. `FragmentManager` needs a container id to commit
 * `add(R.id.X, fragment)`, but Android resources are static — we
 * generate at runtime with [View.generateViewId].
 */
class RustNavigator(
    context: Context,
    private val nativePtr: Long,
) {
    val container: FrameLayout = FrameLayout(context).apply {
        id = View.generateViewId()
        layoutParams = ViewGroup.LayoutParams(
            ViewGroup.LayoutParams.MATCH_PARENT,
            ViewGroup.LayoutParams.MATCH_PARENT,
        )
    }

    /** Resolved at attach time. The Activity hosting our container
     *  must be a [FragmentActivity] (AppCompatActivity is one). If
     *  not, the controller refuses to commit transactions and the
     *  navigator degrades to a one-screen container — there is no
     *  reasonable fallback for fragment-less hosts. */
    private val fragmentManager: FragmentManager? =
        (context as? FragmentActivity)?.supportFragmentManager

    /** Stack of fragment tags we've added, top-of-stack last. Used
     *  for `pop` (we pop the topmost tag) and for `reset` (we pop
     *  everything before adding the new root). */
    private val tagStack = mutableListOf<String>()
    private var nextTag = 0

    /**
     * Push a new screen. The framework has already built the view
     * (via `NavigatorCallbacks.mount_screen`) and allocated a scope
     * id for it; we wrap them in a [RustHostFragment] and commit.
     *
     * Adds to the fragment back-stack so the system back button pops
     * the stack — the framework's `onDestroyView` hook fires on pop
     * and releases the matching scope.
     */
    fun push(view: View, scopeId: Long) {
        val fm = fragmentManager ?: return
        val tag = "rust-nav-${nextTag++}"
        val fragment = RustHostFragment().apply {
            installView(nativePtr, scopeId, view)
        }
        val tx = fm.beginTransaction()
        if (tagStack.isNotEmpty()) {
            // Detach the current top so it stops drawing; pop will
            // re-attach it. `detach` preserves the fragment instance
            // (and through `RustHostFragment.installView`, the
            // already-built Rust view); `add` would create a parallel
            // mount stack.
            fm.findFragmentByTag(tagStack.last())?.let { tx.detach(it) }
        }
        tx.add(container.id, fragment, tag)
        tx.addToBackStack(tag)
        tx.commit()
        tagStack.add(tag)
    }

    /**
     * Pop the top screen. No-op if only the root is mounted.
     * Triggers `RustHostFragment.onDestroyView` → release-scope JNI
     * callback for the popped scope.
     */
    fun pop() {
        val fm = fragmentManager ?: return
        if (tagStack.size <= 1) {
            return
        }
        val poppedTag = tagStack.removeAt(tagStack.size - 1)
        fm.popBackStack(poppedTag, FragmentManager.POP_BACK_STACK_INCLUSIVE)
        // Re-attach the new top so its view is visible again. The
        // `detach` in push() removed the view but kept the fragment;
        // `attach` restores onCreateView -> our hosted view.
        if (tagStack.isNotEmpty()) {
            fm.findFragmentByTag(tagStack.last())?.let { topFrag ->
                fm.beginTransaction().attach(topFrag).commit()
            }
        }
    }

    /**
     * Replace the top screen. Equivalent to pop + push, but executed
     * in one transaction so the user doesn't see an intermediate
     * one-screen-less state.
     */
    fun replace(view: View, scopeId: Long) {
        val fm = fragmentManager ?: return
        if (tagStack.isEmpty()) {
            push(view, scopeId)
            return
        }
        val oldTag = tagStack.removeAt(tagStack.size - 1)
        fm.popBackStack(oldTag, FragmentManager.POP_BACK_STACK_INCLUSIVE)
        val newTag = "rust-nav-${nextTag++}"
        val fragment = RustHostFragment().apply { installView(nativePtr, scopeId, view) }
        val tx = fm.beginTransaction()
        tx.add(container.id, fragment, newTag)
        // Only `addToBackStack` if there was something below — the
        // root screen isn't on the back stack so the user can't pop
        // it.
        if (tagStack.isNotEmpty()) {
            tx.addToBackStack(newTag)
        }
        tx.commit()
        tagStack.add(newTag)
    }

    /**
     * Pop everything, then mount [view] as the new root.
     */
    fun reset(view: View, scopeId: Long) {
        val fm = fragmentManager ?: return
        // Pop the whole back stack — the framework's `onDestroyView`
        // hook fires per fragment so every popped scope gets
        // released.
        if (tagStack.isNotEmpty()) {
            val rootTag = tagStack.first()
            fm.popBackStack(rootTag, FragmentManager.POP_BACK_STACK_INCLUSIVE)
        }
        tagStack.clear()
        val tag = "rust-nav-${nextTag++}"
        val fragment = RustHostFragment().apply { installView(nativePtr, scopeId, view) }
        fm.beginTransaction()
            .add(container.id, fragment, tag)
            .commit()
        tagStack.add(tag)
    }

    /** Current stack depth (1 = only root). */
    fun depth(): Int = tagStack.size

    /**
     * Mount the very first screen as the root. Distinct from `push`
     * because the root isn't added to the back stack (the user can't
     * pop it).
     */
    fun mountRoot(view: View, scopeId: Long) {
        val fm = fragmentManager ?: return
        val tag = "rust-nav-${nextTag++}"
        val fragment = RustHostFragment().apply { installView(nativePtr, scopeId, view) }
        fm.beginTransaction()
            .add(container.id, fragment, tag)
            .commit()
        tagStack.add(tag)
    }
}
