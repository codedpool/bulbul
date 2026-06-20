// Bulbul's accessibility service. Two jobs:
//
//   1. Detect when the user focuses a text input so the floating
//      bubble overlay (Phase 5) can pop up over the keyboard.
//   2. Inject finalized transcripts into the focused field via
//      AccessibilityNodeInfo.performAction(ACTION_SET_TEXT) (Phase 6).
//
// This file is the Phase 4 skeleton — it wires the lifecycle hooks
// and logs incoming events so we can see what the system actually
// sends on real devices, but does not yet talk to the bubble or
// touch any node. The interesting work lands incrementally:
//
//   - Phase 5: emit "input focused" / "input blurred" callbacks
//     into Rust via JNI so the Tauri side can show/hide the bubble.
//   - Phase 6: receive a string from Rust + the saved node ref,
//     call performAction(ACTION_SET_TEXT) on it.
//
// We intentionally do NOT subscribe to text-changed events here —
// they fire on every keystroke in every app and would burn battery
// for no benefit. Focus + window-state are enough to know "user is
// in an editable field right now."

package com.bulbul.app

import android.accessibilityservice.AccessibilityService
import android.util.Log
import android.view.accessibility.AccessibilityEvent

class BulbulAccessibilityService : AccessibilityService() {

    override fun onServiceConnected() {
        super.onServiceConnected()
        Log.i(TAG, "Bulbul accessibility service connected")
    }

    override fun onAccessibilityEvent(event: AccessibilityEvent?) {
        if (event == null) return
        when (event.eventType) {
            AccessibilityEvent.TYPE_VIEW_FOCUSED -> {
                val node = event.source
                val editable = node?.isEditable == true
                Log.d(
                    TAG,
                    "VIEW_FOCUSED pkg=${event.packageName} class=${event.className} editable=$editable"
                )
                // node.recycle() is no-op on API 33+, harmless on older.
                node?.recycle()
            }
            AccessibilityEvent.TYPE_WINDOW_STATE_CHANGED -> {
                Log.d(
                    TAG,
                    "WINDOW_STATE_CHANGED pkg=${event.packageName} class=${event.className}"
                )
            }
            else -> { /* swallow — we only care about focus + window state */ }
        }
    }

    override fun onInterrupt() {
        // System asks us to stop ongoing work. We have none yet — once
        // text injection lands we'll cancel any in-flight performAction
        // here.
        Log.d(TAG, "onInterrupt")
    }

    companion object {
        private const val TAG = "BulbulA11y"
    }
}
