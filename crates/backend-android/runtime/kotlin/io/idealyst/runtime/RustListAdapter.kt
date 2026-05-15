package io.idealyst.runtime

import android.content.Context
import android.view.View
import android.view.ViewGroup
import android.widget.FrameLayout
import androidx.recyclerview.widget.DiffUtil
import androidx.recyclerview.widget.LinearLayoutManager
import androidx.recyclerview.widget.RecyclerView

/**
 * RecyclerView adapter whose data lives on the Rust side. Every
 * lifecycle event trampolines through a small set of `native*` calls
 * which the framework backend implements as JNI exports.
 *
 * Item identity: keys come from Rust as `Long` (the framework's `u64`
 * item key, narrowed). `setHasStableIds(true)` is required so
 * RecyclerView can match positions to keys across data changes.
 *
 * Recycling: every holder owns a stable `FrameLayout` container. On
 * bind, the previous child (if any) is released back to Rust and
 * removed; the new child returned by `nativeMountItem` is added.
 * RecyclerView doesn't see Rust scope ids — it only sees the
 * holder's container view.
 */
class RustListAdapter(private val nativePtr: Long) :
    RecyclerView.Adapter<RustListAdapter.RustViewHolder>() {

    init {
        setHasStableIds(true)
    }

    /** Snapshot of item keys at the last `notifyDataSetChanged` /
     *  diff-driven update. Used as the "old list" input to DiffUtil
     *  on the next `dataChanged` call. */
    private var lastKeys: LongArray = LongArray(0)

    override fun getItemCount(): Int = nativeItemCount(nativePtr)

    override fun getItemViewType(position: Int): Int = 0

    override fun getItemId(position: Int): Long = nativeItemKey(nativePtr, position)

    override fun onCreateViewHolder(parent: ViewGroup, viewType: Int): RustViewHolder {
        val container = FrameLayout(parent.context)
        // RecyclerView wraps with its own LayoutParams; the container
        // itself just needs MATCH_PARENT in the cross-axis so child
        // views can fill the row/column.
        container.layoutParams = RecyclerView.LayoutParams(
            ViewGroup.LayoutParams.MATCH_PARENT,
            ViewGroup.LayoutParams.WRAP_CONTENT,
        )
        return RustViewHolder(container)
    }

    override fun onBindViewHolder(holder: RustViewHolder, position: Int) {
        releasePreviousChild(holder)
        val mount = nativeMountItem(nativePtr, position)
        holder.scopeId = mount.scopeId
        holder.container.addView(mount.view)
        // Hook a layout listener that pushes the rendered size back to
        // Rust on every layout pass. The framework's measured-size
        // cache only updates if the size actually changed, so the
        // listener firing on every layout is safe.
        val listener = View.OnLayoutChangeListener { v, _, top, _, bottom, _, oldTop, _, oldBottom ->
            // We always pin to the main-axis size of the holder's
            // container. The adapter doesn't know the layout
            // orientation here, so push the *height* unconditionally;
            // horizontal lists tell the framework "this is the width"
            // via the orientation set on construction — framework
            // side picks the axis it cares about.
            val size = (bottom - top).toFloat()
            val oldSize = (oldBottom - oldTop).toFloat()
            if (kotlin.math.abs(size - oldSize) > 0.5f) {
                nativeSetMeasuredSize(nativePtr, holder.scopeId, size)
            }
        }
        holder.layoutListener = listener
        holder.container.addOnLayoutChangeListener(listener)
    }

    override fun onViewRecycled(holder: RustViewHolder) {
        releasePreviousChild(holder)
    }

    private fun releasePreviousChild(holder: RustViewHolder) {
        if (holder.scopeId != 0L) {
            nativeReleaseItem(nativePtr, holder.scopeId)
            holder.scopeId = 0L
        }
        holder.layoutListener?.let { holder.container.removeOnLayoutChangeListener(it) }
        holder.layoutListener = null
        holder.container.removeAllViews()
    }

    /**
     * Called from Rust when the underlying data changes. Computes a
     * key diff against the previous snapshot and dispatches granular
     * RecyclerView updates so insertions/removals animate and
     * surviving items don't rebind.
     */
    fun dataChanged() {
        val newCount = nativeItemCount(nativePtr)
        val newKeys = LongArray(newCount) { nativeItemKey(nativePtr, it) }

        val oldKeys = lastKeys
        lastKeys = newKeys

        val diff = DiffUtil.calculateDiff(object : DiffUtil.Callback() {
            override fun getOldListSize(): Int = oldKeys.size
            override fun getNewListSize(): Int = newKeys.size
            override fun areItemsTheSame(oldItemPosition: Int, newItemPosition: Int): Boolean =
                oldKeys[oldItemPosition] == newKeys[newItemPosition]

            // We don't compare contents on the Kotlin side — if the
            // key is the same we let Rust's reactive subtree decide
            // whether the bound view needs visual updates. Returning
            // true here means "same key, no rebind" which is what we
            // want for stable-key updates.
            override fun areContentsTheSame(oldItemPosition: Int, newItemPosition: Int): Boolean =
                true
        })
        diff.dispatchUpdatesTo(this)
    }

    class RustViewHolder(val container: FrameLayout) : RecyclerView.ViewHolder(container) {
        var scopeId: Long = 0
        var layoutListener: View.OnLayoutChangeListener? = null
    }

    private external fun nativeItemCount(ptr: Long): Int
    private external fun nativeItemKey(ptr: Long, position: Int): Long
    private external fun nativeMountItem(ptr: Long, position: Int): MountResult
    private external fun nativeReleaseItem(ptr: Long, scopeId: Long)
    private external fun nativeSetMeasuredSize(ptr: Long, scopeId: Long, size: Float)

    /** Cleanup hook so the framework can drop the leaked callbacks box.
     *  Called from the adapter's `onDetachedFromRecyclerView` or from
     *  application teardown. */
    external fun nativeDrop(ptr: Long)

    /** Result of `nativeMountItem`. Bundles the freshly-built native
     *  View with the scope id Rust allocated for it. */
    class MountResult(val view: View, val scopeId: Long)
}

/**
 * `LinearLayoutManager` that pre-lays-out extra pixels off-screen so
 * scrolling has work to do before the next bind cycle. The framework
 * passes `overscan` as a fraction-of-viewport multiplier; we compute
 * the pixel extent against the parent's measured size and clamp to a
 * sensible floor.
 */
class RustLinearLayoutManager(
    context: Context,
    orientation: Int,
    private val overscanFactor: Float,
) : LinearLayoutManager(context, orientation, false) {
    override fun calculateExtraLayoutSpace(state: RecyclerView.State, extraLayoutSpace: IntArray) {
        val extent = if (orientation == HORIZONTAL) width else height
        val px = (extent * overscanFactor).toInt().coerceAtLeast(0)
        extraLayoutSpace[0] = px
        extraLayoutSpace[1] = px
    }
}
