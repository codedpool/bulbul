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
        // Service-level findFocus is unreliable on several OEMs (returns
        // null mid-typing — same failure that hid the bubble), so walk a
        // fallback chain until we find an editable target.
        val node = findTarget(svc) ?: run {
            Log.w(TAG, "no editable target found — dropping transcript")
            return false
        }
        return try {
            // SET_TEXT-first: it never touches the clipboard, so the user
            // doesn't get Android 13's "app copied to clipboard" toast on
            // every dictation (paste-first did). Two traps handled:
            //   - apps like WhatsApp report their hint ("Message") through
            //     node.text without setting isShowingHintText — text that
            //     matches the hint counts as an empty field, else every
            //     dictation started with the placeholder word;
            //   - SET_TEXT replaces the whole field, so we append to the
            //     real existing content rather than wiping it.
            // Fields that refuse SET_TEXT (custom editors) fall back to
            // clipboard + ACTION_PASTE, toast and all — better than
            // dropping the transcript.
            var existing =
                if (android.os.Build.VERSION.SDK_INT >= 26 && node.isShowingHintText) ""
                else node.text?.toString() ?: ""
            if (existing.isNotEmpty()) {
                val hint =
                    if (android.os.Build.VERSION.SDK_INT >= 26) node.hintText?.toString() else null
                val matchesHint =
                    hint != null && existing.trim().equals(hint.trim(), ignoreCase = true)
                // Strongest hint signal: a field showing only its
                // placeholder reports a cursor at 0 (or -1) while still
                // claiming text. Real content keeps the cursor inside
                // the text (usually at its end). WhatsApp trips the
                // text-equals-hint check, so we need this too.
                val cursorAtZero =
                    node.textSelectionEnd <= 0 && node.textSelectionStart <= 0
                Log.d(
                    TAG,
                    "target text.len=${existing.length} hint=$hint sel=" +
                        "${node.textSelectionStart}..${node.textSelectionEnd}",
                )
                if (matchesHint || cursorAtZero) existing = ""
            }
            val combined =
                if (existing.isBlank()) text else existing.trimEnd() + " " + text
            val args = Bundle().apply {
                putCharSequence(
                    AccessibilityNodeInfo.ACTION_ARGUMENT_SET_TEXT_CHARSEQUENCE,
                    combined,
                )
            }
            var ok = node.performAction(AccessibilityNodeInfo.ACTION_SET_TEXT, args)
            if (!ok) ok = pasteInto(svc, node, text)
            if (!ok) Log.w(TAG, "SET_TEXT and PASTE both failed")
            ok
        } finally {
            node.recycle()
        }
    }

    /// Focused-editable discovery, most-reliable first:
    ///   1. service findFocus  2. per-window-root findFocus
    ///   3. DFS for a focused editable  4. DFS for any editable.
    private fun findTarget(svc: AccessibilityService): AccessibilityNodeInfo? {
        try {
            svc.findFocus(AccessibilityNodeInfo.FOCUS_INPUT)?.let {
                if (it.isEditable) return it else it.recycle()
            }
        } catch (_: Throwable) {}
        return try {
            val windows = svc.windows ?: return null
            for (w in windows) {
                val root = w.root ?: continue
                root.findFocus(AccessibilityNodeInfo.FOCUS_INPUT)?.let {
                    if (it.isEditable) return it else it.recycle()
                }
            }
            for (w in windows) {
                val root = w.root ?: continue
                dfs(root, 0) { it.isEditable && it.isFocused }?.let { return it }
            }
            for (w in windows) {
                val root = w.root ?: continue
                dfs(root, 0) { it.isEditable }?.let { return it }
            }
            null
        } catch (t: Throwable) {
            Log.w(TAG, "window walk failed", t)
            null
        }
    }

    private fun dfs(
        node: AccessibilityNodeInfo,
        depth: Int,
        match: (AccessibilityNodeInfo) -> Boolean,
    ): AccessibilityNodeInfo? {
        if (depth > 24) return null // runaway-tree guard
        if (match(node)) return node
        for (i in 0 until node.childCount) {
            val child = node.getChild(i) ?: continue
            dfs(child, depth + 1, match)?.let { return it }
            child.recycle()
        }
        return null
    }

    private fun pasteInto(
        svc: AccessibilityService,
        node: AccessibilityNodeInfo,
        text: String,
    ): Boolean {
        return try {
            val cm = svc.getSystemService(android.content.Context.CLIPBOARD_SERVICE)
                as android.content.ClipboardManager
            cm.setPrimaryClip(android.content.ClipData.newPlainText("Bulbul", text))
            if (!node.isFocused) node.performAction(AccessibilityNodeInfo.ACTION_FOCUS)
            node.performAction(AccessibilityNodeInfo.ACTION_PASTE)
        } catch (t: Throwable) {
            Log.w(TAG, "paste fallback failed", t)
            false
        }
    }
}
