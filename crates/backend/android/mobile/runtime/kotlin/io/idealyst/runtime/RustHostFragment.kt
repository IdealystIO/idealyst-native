package io.idealyst.runtime

import android.content.Context
import android.os.Bundle
import android.view.LayoutInflater
import android.view.View
import android.view.ViewGroup
import android.widget.FrameLayout
import androidx.fragment.app.Fragment

/**
 * A Fragment whose view content is constructed by the Rust framework
 * and handed in via [installView]. The fragment doesn't inflate any
 * XML — it just exposes the pre-built View as its content.
 *
 * On `onDestroyView` we trampoline back to Rust so the framework can
 * drop the per-screen `Scope` (freeing every Signal/Effect/Ref nested
 * inside the screen). This hook fires for both programmatic pops
 * (`RustNavigator.pop`) and system-back / process-death paths, so the
 * scope cleanup is uniform.
 *
 * Lifetime: `nativePtr` is a leaked pointer to the navigator's
 * `NavigatorCallbacks`; `scopeId` identifies the per-screen scope.
 * The fragment instance must not outlive the navigator (the
 * Activity's Fragment back-stack handles this naturally).
 */
class RustHostFragment : Fragment() {
    /** Pointer to the navigator's leaked callbacks box. Stashed via
     *  [installView] before the fragment commits. */
    private var nativePtr: Long = 0
    /** Per-screen scope id, also stashed via [installView]. */
    private var scopeId: Long = 0
    /** The pre-built native view. Held until `onCreateView` hands it
     *  to the framework; cleared on destroy so we don't retain the
     *  View graph past the fragment's lifecycle. */
    private var hosted: View? = null

    fun installView(nativePtr: Long, scopeId: Long, view: View) {
        this.nativePtr = nativePtr
        this.scopeId = scopeId
        this.hosted = view
    }

    override fun onCreateView(
        inflater: LayoutInflater,
        container: ViewGroup?,
        savedInstanceState: Bundle?,
    ): View {
        val v = hosted
        if (v != null) {
            // Detach from any previous parent (defensive — should
            // normally have none).
            (v.parent as? ViewGroup)?.removeView(v)
            return v
        }
        // Fallback: the fragment was reconstructed by the platform
        // without `installView` ever being called (e.g. after process
        // death). We don't have a way to rebuild the screen here —
        // return an empty container so the fragment doesn't crash.
        return FrameLayout(requireContext())
    }

    override fun onDestroyView() {
        super.onDestroyView()
        // Distinguish permanent removal from transient teardown.
        // `onDestroyView` fires in three cases:
        //   1. Fragment popped off the back stack → `isRemoving` true.
        //   2. Activity finishing / config change → `activity?.isFinishing` true.
        //   3. `detach()` (we don't call it anymore, but the platform
        //      might): isRemoving false, activity not finishing.
        //
        // We only want to drop the Rust-side scope + cached view in
        // case 1 or 2. For case 3 the fragment will come back and
        // expects its hosted view + scope intact.
        val permanent = isRemoving || (activity?.isFinishing == true)
        if (permanent) {
            if (nativePtr != 0L && scopeId != 0L) {
                nativeReleaseScreen(nativePtr, scopeId)
            }
            hosted = null
            scopeId = 0
            nativePtr = 0
        }
    }

    private external fun nativeReleaseScreen(ptr: Long, scopeId: Long)
}
