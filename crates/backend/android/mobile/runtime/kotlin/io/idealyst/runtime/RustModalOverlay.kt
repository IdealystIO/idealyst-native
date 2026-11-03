package io.idealyst.runtime

import android.content.Context
import android.view.KeyEvent
import android.widget.FrameLayout

/**
 * The full-bleed overlay `View` a modal portal with an `on_dismiss` renders
 * into (`imp/primitives/overlay.rs`). Back (button, or the gesture's
 * synthesized `KEYCODE_BACK`) and Escape close it, as an `android.app.Dialog`
 * closes on cancel and as every other backend closes an overlay with an
 * `on_dismiss`.
 *
 * The handling is in [dispatchKeyEvent], after the children: the key first
 * goes down the focus path, so a focused field inside the modal gets it, and
 * only a key nothing below consumed dismisses. That covers both the overlay
 * itself holding focus (it takes focus when revealed) and a field inside it
 * holding focus — a `View.OnKeyListener` on the overlay would run only in the
 * first case. A field passes an unclaimed Escape on to here: it is a
 * [RustEditText], which declines the API 37+ `TextView` default of taking
 * Escape to blur itself.
 *
 * Ordering with the rest of Back handling: the view tree sees a key before
 * the Activity, so the topmost modal (the one holding focus) takes Back
 * ahead of the `OnBackPressedDispatcher` callbacks — stack presenters, then
 * `RustSystemBack`'s `on_back` handlers. While the soft keyboard is up, the
 * IME consumes the first Back (to hide itself) before any view sees it.
 *
 * Both phases of a dismiss key are consumed (the Activity would otherwise
 * track the down and act on the up); `on_dismiss` fires on the up, as a
 * dialog cancels, and not for a canceled key (an aborted back gesture) nor
 * for a key whose down a field below claimed ([downClaimedBelow]).
 *
 * [dismissPtr] is the `Box<OverlayDismissCallback>` from `create_portal`:
 * never freed while the view lives (`release_portal` blanks its closure).
 */
class RustModalOverlay(context: Context, private val dismissPtr: Long) : FrameLayout(context) {
    /**
     * Whether something below claimed the DOWN of the dismiss key in flight.
     * A field's `on_key_down` runs on the down; if it claimed the key
     * (`KeyOutcome::PreventDefault`, e.g. a field closing its own dropdown on
     * Escape) the matching up must not dismiss either — iOS, macOS and web
     * keep the overlay open for a prevented Escape.
     */
    private var downClaimedBelow = false

    override fun dispatchKeyEvent(event: KeyEvent): Boolean {
        val dismissKey = event.keyCode == KeyEvent.KEYCODE_BACK || event.keyCode == KeyEvent.KEYCODE_ESCAPE
        val claimed = super.dispatchKeyEvent(event)
        if (dismissKey && event.action == KeyEvent.ACTION_DOWN && event.repeatCount == 0) {
            downClaimedBelow = claimed
        }
        if (claimed) return true
        if (!dismissKey || dismissPtr == 0L) return false
        if (event.action == KeyEvent.ACTION_UP) {
            val claimedDown = downClaimedBelow
            downClaimedBelow = false
            if (claimedDown) return true
            if (!event.isCanceled) nativeDismiss(dismissPtr)
        }
        return true
    }

    private external fun nativeDismiss(ptr: Long)
}
