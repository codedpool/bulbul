// Injects a transcript into the field the user is focused in.
//
// The trick: the foreground service does the work, but only the
// AccessibilityService has the privilege to call performAction on
// nodes in other apps. So we keep a process-wide reference to the
// running AccessibilityService and route inject() requests through
// it.
//
// AccessibilityService.findFocus(FOCUS_INPUT) walks the active window
// tree and returns the node currently holding input focus — works
// across app boundaries because canRetrieveWindowContent is on in
// the service config. We grab that node at inject time (rather than
// caching from the last TYPE_VIEW_FOCUSED event) because cached node
// refs go stale fast on Android and become AccessibilityNodeInfo
// instances that no longer back any view.
//
// performAction(ACTION_SET_TEXT, args) is the only way to put text
// into a field without a soft-keyboard hop. We pack the text into
// the standard ACTION_ARGUMENT_SET_TEXT_CHARSEQUENCE bundle key the
// platform expects.

package com.bulbul.app

import android.accessibilityservice.AccessibilityService
import android.os.Bundle
import android.util.Log
import android.view.accessibility.AccessibilityNodeInfo
import java.lang.ref.WeakReference

object TextInjector {

    private const val TAG = "BulbulInject"

    /// Weak so we don't pin the service after the user disables
    /// accessibility for Bulbul.
    @Volatile private var serviceRef: WeakReference<AccessibilityService>? = null

    fun bind(service: AccessibilityService) {
        serviceRef = WeakReference(service)
    }

    fun unbind() {
        serviceRef = null
    }

    /// Places [text] into the currently focused editable field.
    /// Returns true if a node was found AND the platform reported
    /// the action succeeded. False otherwise — caller logs / falls
    /// back to copy-to-clipboard.
    fun inject(text: String): Boolean {
        val svc = serviceRef?.get()
        if (svc == null) {
            Log.w(TAG, "no accessibility service bound; cannot inject")
            return false
        }
        // findFocus walks every active window — the active app's
        // input field, not the dashboard's. If the user moved focus
        // while we were transcribing, this picks up the new target,
        // which is the behaviour we want.
        val node = svc.findFocus(AccessibilityNodeInfo.FOCUS_INPUT) ?: run {
            Log.w(TAG, "no focused input — dropping transcript")
            return false
        }
        return try {
            val args = Bundle().apply {
                putCharSequence(
                    AccessibilityNodeInfo.ACTION_ARGUMENT_SET_TEXT_CHARSEQUENCE,
                    text,
                )
            }
            val ok = node.performAction(AccessibilityNodeInfo.ACTION_SET_TEXT, args)
            if (!ok) Log.w(TAG, "performAction(SET_TEXT) returned false")
            ok
        } finally {
            node.recycle()
        }
    }
}
