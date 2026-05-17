package io.idealyst.runtime

import android.content.Context
import android.util.Log
import android.view.View
import android.view.ViewGroup
import android.widget.FrameLayout
import androidx.fragment.app.FragmentActivity
import androidx.fragment.app.FragmentManager
import androidx.fragment.app.FragmentTransaction

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
 *
 * # Attach ordering
 *
 * Fragment transactions only display correctly once the container
 * `FrameLayout` is attached to the activity's window — FragmentManager
 * uses `findViewById` on the activity's content view at commit time
 * to locate the container. Because the Rust framework calls
 * `mountRoot` *before* it inserts our container into its parent (the
 * standard create → attach flow used by every other primitive), we
 * queue mount operations until the container is attached. Pending
 * ops drain in [onContainerAttached], which fires from the
 * [View.OnAttachStateChangeListener] we install on the container.
 */
class RustNavigator(
    context: Context,
    private val nativePtr: Long,
) {
    val container: FrameLayout = FrameLayout(context).apply {
        id = View.generateViewId()
        // MATCH_PARENT in the cross-axis, WRAP_CONTENT in the main
        // axis. The container hosts one fragment view at a time; its
        // size should track the fragment's natural size so it lays
        // out correctly inside arbitrary parents (LinearLayout,
        // ScrollView, etc.). Hard-coding MATCH_PARENT on height would
        // collapse the container to 0 when its parent is a vertical
        // LinearLayout with WRAP_CONTENT height (the host
        // ScrollView pattern the framework's example uses).
        layoutParams = ViewGroup.LayoutParams(
            ViewGroup.LayoutParams.MATCH_PARENT,
            ViewGroup.LayoutParams.WRAP_CONTENT,
        )
    }

    /** Resolved at construction time. The Activity hosting our
     *  container must be a [FragmentActivity] (AppCompatActivity is
     *  one). If not, the controller refuses to commit transactions
     *  and the navigator degrades to a one-screen container — there
     *  is no reasonable fallback for fragment-less hosts. */
    private val fragmentManager: FragmentManager? =
        (context as? FragmentActivity)?.supportFragmentManager

    /** Stack of fragment tags we've added, top-of-stack last. Used
     *  for `pop` (we pop the topmost tag) and for `reset` (we pop
     *  everything before adding the new root). */
    private val tagStack = mutableListOf<String>()
    private var nextTag = 0

    /** Queued mount operations awaiting container attach. Drained in
     *  [onContainerAttached]. While empty, container is attached and
     *  operations run synchronously. */
    private val pending = mutableListOf<() -> Unit>()
    /** True once we've seen the container attach to a window. Stays
     *  true thereafter — we don't track detach because the navigator's
     *  lifetime is tied to its surrounding `Scope`, not the view's
     *  attach state. */
    private var attached = false

    init {
        if (fragmentManager == null) {
            Log.w(
                "idealyst",
                "RustNavigator: hosting context is not a FragmentActivity " +
                    "(got ${context.javaClass.name}). Fragment-backed navigation " +
                    "is disabled; navigator will render only its initial screen.",
            )
        } else {
            Log.i("idealyst", "RustNavigator init: fragmentManager resolved, container id=${container.id}")
        }
        container.addOnAttachStateChangeListener(object : View.OnAttachStateChangeListener {
            override fun onViewAttachedToWindow(v: View) {
                Log.i("idealyst", "RustNavigator container attached to window, draining ${pending.size} pending ops")
                attached = true
                drainPending()
            }
            override fun onViewDetachedFromWindow(v: View) {
                Log.i("idealyst", "RustNavigator container detached from window")
            }
        })
    }

    /** Run [op] now if the container is attached, queue it otherwise.
     *  The wrapper centralizes the "is the container ready" decision
     *  so each public method stays linear. */
    private fun runOrQueue(op: () -> Unit) {
        if (attached) {
            op()
        } else {
            pending.add(op)
        }
    }

    /** Flush every queued op in registration order. Each op runs
     *  exactly once; we clear the queue before iterating so an op
     *  that re-enters (unlikely but possible) doesn't see stale
     *  entries. */
    private fun drainPending() {
        if (pending.isEmpty()) return
        val drained = pending.toList()
        pending.clear()
        for (op in drained) op()
    }

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
        tagStack.add(tag)
        runOrQueue {
            val fragment = RustHostFragment().apply { installView(nativePtr, scopeId, view) }
            val tx = fm.beginTransaction()
            // Standard Android push animation — the new fragment
            // slides in from the right (open) and on pop the platform
            // automatically plays the reverse (close). The transit
            // type is the modern Material-aware replacement for
            // hard-coded `setCustomAnimations` calls and respects
            // the host theme's `windowAnimationStyle`.
            tx.setTransition(FragmentTransaction.TRANSIT_FRAGMENT_OPEN)
            // Hide (not detach) the previous top. `hide` just flips
            // visibility GONE; the fragment stays in RESUMED state
            // and its view stays alive. This matters for two reasons:
            //   1. `detach` triggers `onDestroyView`, which (in
            //      RustHostFragment) trampolines into Rust to drop
            //      the screen's scope — making the screen
            //      unrecoverable when popping back to it.
            //   2. `detach` also nulls out the cached `hosted` view,
            //      so on re-attach the fragment would inflate the
            //      fallback empty FrameLayout instead of our screen.
            // Both effects are reversed by popBackStack (it un-hides
            // the previous fragment automatically), and `hide` keeps
            // the screen's reactive scope alive underneath.
            if (tagStack.size >= 2) {
                fm.findFragmentByTag(tagStack[tagStack.size - 2])?.let { tx.hide(it) }
            }
            tx.add(container.id, fragment, tag)
            tx.addToBackStack(tag)
            // `commit()` (async) because `commitNow()` rejects
            // transactions that touch the back stack.
            tx.commit()
        }
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
        runOrQueue {
            // popBackStack reverses the matching `push` transaction —
            // the new top fragment is automatically un-hidden, the
            // popped fragment is removed (triggering onDestroyView →
            // nativeReleaseScreen). No follow-up transaction needed.
            fm.popBackStack(poppedTag, FragmentManager.POP_BACK_STACK_INCLUSIVE)
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
        val newTag = "rust-nav-${nextTag++}"
        tagStack.add(newTag)
        runOrQueue {
            // Pop the old top off the back stack. This un-hides the
            // fragment that was below it (if any), so we re-hide it
            // in the next transaction to keep the visual invariant
            // (only the topmost screen is visible).
            fm.popBackStack(oldTag, FragmentManager.POP_BACK_STACK_INCLUSIVE)
            val fragment = RustHostFragment().apply { installView(nativePtr, scopeId, view) }
            val tx = fm.beginTransaction()
            tx.setTransition(FragmentTransaction.TRANSIT_FRAGMENT_FADE)
            val wantsBackStack = tagStack.size > 1
            // If there's something below, hide it again so the
            // new replacement is the only visible fragment.
            if (wantsBackStack) {
                fm.findFragmentByTag(tagStack[tagStack.size - 2])?.let { tx.hide(it) }
            }
            tx.add(container.id, fragment, newTag)
            if (wantsBackStack) {
                tx.addToBackStack(newTag)
            }
            // commitNow() is rejected by transactions that touch the
            // back stack; fall back to commit() in that case.
            if (wantsBackStack) tx.commit() else tx.commitNow()
        }
    }

    /**
     * Pop everything, then mount [view] as the new root.
     */
    fun reset(view: View, scopeId: Long) {
        val fm = fragmentManager ?: return
        val firstTag = tagStack.firstOrNull()
        tagStack.clear()
        val tag = "rust-nav-${nextTag++}"
        tagStack.add(tag)
        runOrQueue {
            if (firstTag != null) {
                // Pop the whole back stack — the framework's
                // onDestroyView hook fires per fragment so every
                // popped scope gets released.
                fm.popBackStack(firstTag, FragmentManager.POP_BACK_STACK_INCLUSIVE)
            }
            val fragment = RustHostFragment().apply { installView(nativePtr, scopeId, view) }
            fm.beginTransaction()
                .add(container.id, fragment, tag)
                .commitNow()
        }
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
        tagStack.add(tag)
        Log.i("idealyst", "RustNavigator.mountRoot called (attached=$attached, view=$view)")
        runOrQueue {
            try {
                val fragment = RustHostFragment().apply { installView(nativePtr, scopeId, view) }
                fm.beginTransaction()
                    .add(container.id, fragment, tag)
                    .commitNow()
                Log.i("idealyst", "RustNavigator.mountRoot fragment committed tag=$tag")
            } catch (t: Throwable) {
                Log.e("idealyst", "RustNavigator.mountRoot failed", t)
            }
        }
    }
}
