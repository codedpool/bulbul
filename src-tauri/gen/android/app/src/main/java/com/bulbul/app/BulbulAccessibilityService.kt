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
import android.provider.Settings
import android.util.Log
import android.view.accessibility.AccessibilityEvent
import android.view.accessibility.AccessibilityNodeInfo
import android.view.accessibility.AccessibilityWindowInfo

class BulbulAccessibilityService : AccessibilityService() {

    /// Tracks whether we last asked the foreground service to be up.
    /// Lets us skip redundant start/stop calls (which would otherwise
    /// flicker the notification icon every time the IME state ticks).
    private var bubbleRequested = false

    /// True while the focused field is a password/PIN input. The bubble
    /// stays down on these: dictating a secret aloud is never the right
    /// input path, and banking apps treat an overlay near a credential
    /// field as an attack. Set from TYPE_VIEW_FOCUSED (the event source
    /// is the only reliable carrier of isPassword — see findFocus notes
    /// below), refreshed on window changes. Volatile: written on the
    /// a11y thread, read wherever the injector checks it.
    @Volatile
    private var passwordFocused = false

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
        instance = this
        TextInjector.bind(this)
        // Initial evaluation — if the user enables Bulbul while
        // they're already in a text field with the keyboard up, we
        // need to bring the bubble up without waiting for a new event.
        reevaluateBubble()
    }

    override fun onUnbind(intent: android.content.Intent?): Boolean {
        instance = null
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
                if (event.eventType == AccessibilityEvent.TYPE_VIEW_FOCUSED) {
                    // The event source is the node that just took focus —
                    // the most reliable password signal we get (findFocus
                    // is null on many OEMs mid-typing, so we can't poll it
                    // on demand). Track it here and gate the bubble on it.
                    passwordFocused = try {
                        event.source?.isPassword == true
                    } catch (t: Throwable) {
                        false
                    }
                }
                if (event.eventType == AccessibilityEvent.TYPE_WINDOW_STATE_CHANGED) {
                    // New window/screen: the focus event that set the flag
                    // belongs to the old screen. Re-derive from the current
                    // focus if the platform gives it to us; if it won't
                    // (null), fail open — a wrongly-hidden bubble in normal
                    // apps is worse than a briefly-visible one here, and
                    // the next TYPE_VIEW_FOCUSED corrects it anyway.
                    passwordFocused = currentFocusIsPassword()
                }
                if (event.eventType == AccessibilityEvent.TYPE_WINDOW_STATE_CHANGED ||
                    event.eventType == AccessibilityEvent.TYPE_VIEW_FOCUSED
                ) {
                    trackForegroundApp(event.packageName?.toString())
                }
                reevaluateBubble()
            }
        }
    }

    private fun currentFocusIsPassword(): Boolean = try {
        findFocus(AccessibilityNodeInfo.FOCUS_INPUT)?.let {
            val pw = it.isPassword
            it.recycle()
            pw
        } ?: false
    } catch (t: Throwable) {
        false
    }

    /// Remembers the last real foreground app package so the foreground
    /// service can, at record time, tag the dictation with which app it
    /// went into (dashboard badge + per-app Style). We filter out our own
    /// package, the soft keyboard, and the system UI so the "target app"
    /// stays the thing the user is actually typing into.
    private fun trackForegroundApp(pkg: String?) {
        if (pkg.isNullOrBlank()) return
        if (pkg == packageName || pkg == "com.android.systemui" || pkg == currentImePackage()) return
        targetPackage = pkg
    }

    private fun currentImePackage(): String? = try {
        Settings.Secure.getString(contentResolver, Settings.Secure.DEFAULT_INPUT_METHOD)
            ?.substringBefore('/')
    } catch (t: Throwable) {
        null
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

    /// Bubble should be visible whenever the IME (soft keyboard) is on
    /// screen — in any app, including Bulbul's own scratchpad.
    ///
    /// We used to also require findFocus(FOCUS_INPUT) to return an
    /// editable node, but on many devices/IMEs findFocus returns null
    /// even while the user is actively typing — which meant the bubble
    /// never appeared at all. The IME being up is already the signal
    /// that the user is in a text context.
    ///
    /// We also used to hide over Bulbul's own package, but that blocked
    /// dictation into the in-app scratchpad — the one place users most
    /// expect the same bubble. So the bubble now shows over our own app
    /// too; the injector appends into whatever field is focused.
    ///
    /// Password/PIN fields are the exception: the bubble goes (and stays)
    /// down while one is focused, so we never draw over a credential
    /// prompt and never invite dictating a secret out loud.
    private fun shouldShowBubble(): Boolean {
        return isImeVisible() && !passwordFocused && !BulbulConfig.isSnoozed(this)
    }

    /// Called by the foreground service when the user snoozes: forget that
    /// we were showing the bubble so that, once the snooze expires, the next
    /// IME event cleanly re-shows it (rather than us thinking it's still up).
    private fun onSnoozed() {
        handler.removeCallbacks(hideRunnable)
        bubbleRequested = false
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

        /// The last real foreground-app package (not Bulbul, the IME, or
        /// system UI). Read by the foreground service at record time to tag
        /// the dictation with its target app. Volatile: written on the
        /// a11y thread, read on the recorder thread.
        @Volatile
        var targetPackage: String? = null

        /// Live instance, so the foreground service can notify us of a snooze.
        @Volatile
        private var instance: BulbulAccessibilityService? = null

        /// Foreground service → a11y service hook: reset our bubble state when
        /// the user snoozes, so it re-shows cleanly once the snooze expires.
        fun notifySnoozed() {
            instance?.onSnoozed()
        }

        // Just long enough to absorb a one-frame IME teardown/rebuild
        // flicker during a keyboard animation, but short enough that
        // closing the keyboard hides the bubble near-instantly. 500 ms
        // read as "the bubble lingers" — 120 ms feels immediate while
        // still swallowing the transient flap.
        private const val HIDE_GRACE_MS = 120L
    }
}
