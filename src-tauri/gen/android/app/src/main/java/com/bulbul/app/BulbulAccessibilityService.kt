// Bulbul's accessibility service. Two jobs:
//
//   1. Decide when the floating bubble should be on screen — the rule
//      is: the soft keyboard (IME) is up AND the focused field is an
//      editable input in another app. Anything else and we hide.
//   2. Inject finalized transcripts into the focused field via
//      AccessibilityNodeInfo.performAction(ACTION_SET_TEXT). The
//      TextInjector holds a reference to this service so the
//      foreground service can ask it to inject without needing its
//      own accessibility context.
//
// We deliberately do NOT subscribe to text-changed events — they fire
// on every keystroke in every app and burn battery for no benefit.
// Focus + window-state + windows-changed are enough to know the
// bubble-visible state at any moment.

package com.bulbul.app

import android.accessibilityservice.AccessibilityService
import android.os.Handler
import android.os.Looper
import android.util.Log
import android.view.accessibility.AccessibilityEvent
import android.view.accessibility.AccessibilityNodeInfo
import android.view.accessibility.AccessibilityWindowInfo

class BulbulAccessibilityService : AccessibilityService() {

    /// Tracks whether we last asked the foreground service to be up.
    /// Lets us skip redundant start/stop calls (which would otherwise
    /// flicker the notification icon every time the IME state ticks).
    private var bubbleRequested = false

    /// Posts hide requests with a small delay so a transient IME
    /// flicker (keyboard animation, soft-input refresh during text
    /// selection) doesn't take the bubble down only to put it right
    /// back up. Show requests cancel any pending hide.
    private val handler = Handler(Looper.getMainLooper())
    private val hideRunnable = Runnable {
        if (bubbleRequested) {
            Log.d(TAG, "hide grace expired — stopping foreground service")
            BulbulForegroundService.stop(this)
            bubbleRequested = false
        }
    }

    override fun onServiceConnected() {
        super.onServiceConnected()
        Log.i(TAG, "Bulbul accessibility service connected")
        TextInjector.bind(this)
        // Initial evaluation — if the user enables Bulbul while
        // they're already in a text field with the keyboard up, we
        // need to bring the bubble up without waiting for a new event.
        reevaluateBubble()
    }

    override fun onUnbind(intent: android.content.Intent?): Boolean {
        TextInjector.unbind()
        // Whatever state we were in, hide the bubble immediately
        // (no grace period — accessibility is off, we have no
        // further ability to react if we wait).
        handler.removeCallbacks(hideRunnable)
        if (bubbleRequested) {
            BulbulForegroundService.stop(this)
            bubbleRequested = false
        }
        return super.onUnbind(intent)
    }

    override fun onAccessibilityEvent(event: AccessibilityEvent?) {
        if (event == null) return
        when (event.eventType) {
            AccessibilityEvent.TYPE_VIEW_FOCUSED,
            AccessibilityEvent.TYPE_WINDOW_STATE_CHANGED,
            AccessibilityEvent.TYPE_WINDOWS_CHANGED -> {
                reevaluateBubble()
            }
        }
    }

    override fun onInterrupt() {
        Log.d(TAG, "onInterrupt")
    }

    /// Single source of truth for whether the bubble should be on
    /// screen. Called on every event we care about. Shows are
    /// instant; hides go through a HIDE_GRACE_MS debounce so an IME
    /// flicker during a keyboard animation doesn't tear down the
    /// foreground service only to rebuild it 100 ms later — that
    /// rebuild cycle is what was making the bubble flash for under
    /// two seconds and never become visually obvious.
    private fun reevaluateBubble() {
        val shouldShow = shouldShowBubble()

        if (shouldShow) {
            // Any pending hide was a flap — cancel it.
            handler.removeCallbacks(hideRunnable)
            if (!bubbleRequested) {
                Log.d(TAG, "showing bubble — IME up + editable focus in another app")
                BulbulForegroundService.start(this)
                bubbleRequested = true
            }
        } else {
            if (bubbleRequested) {
                // Don't queue multiple — last hide request wins on
                // the same delay, so removeCallbacks first.
                handler.removeCallbacks(hideRunnable)
                handler.postDelayed(hideRunnable, HIDE_GRACE_MS)
            }
        }
    }

    /// Bubble should be visible iff:
    ///   (a) The IME (soft keyboard) is on screen — there's an
    ///       AccessibilityWindowInfo of type TYPE_INPUT_METHOD.
    ///   (b) The focused input is in another app (not Bulbul's own
    ///       dashboard), so we don't pop the bubble on top of our own
    ///       Settings text fields.
    ///   (c) The focused node is actually editable — IMEs can stay up
    ///       briefly after focus moves to a non-editable element; we
    ///       don't want the bubble in those windows.
    private fun shouldShowBubble(): Boolean {
        if (!isImeVisible()) return false
        val focused = findFocus(AccessibilityNodeInfo.FOCUS_INPUT) ?: return false
        return try {
            val editable = focused.isEditable
            val pkg = focused.packageName?.toString()
            editable && pkg != packageName
        } finally {
            // recycle() is no-op on API 33+ but harmless; older
            // platforms leak the AccessibilityNodeInfo otherwise.
            focused.recycle()
        }
    }

    private fun isImeVisible(): Boolean {
        return try {
            windows?.any { it.type == AccessibilityWindowInfo.TYPE_INPUT_METHOD } == true
        } catch (t: Throwable) {
            // getWindows() can throw on some OEMs when the service is
            // mid-rebind. Treat as "no IME" and let the next event
            // re-evaluate — better than crashing.
            Log.w(TAG, "windows lookup failed", t)
            false
        }
    }

    companion object {
        private const val TAG = "BulbulA11y"
        // Long enough to absorb IME teardown/rebuild during keyboard
        // animations, short enough that an actual dismissal feels
        // responsive. 500 ms hits the sweet spot on every device I
        // have logs from; if a future OEM is slower we can dial up.
        private const val HIDE_GRACE_MS = 500L
    }
}
