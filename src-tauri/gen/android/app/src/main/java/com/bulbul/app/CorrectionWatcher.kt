// SPDX-License-Identifier: GPL-3.0-only
// Copyright (c) 2026 Romanch Roshan Singh

// Correction memory — the Android side of desktop's correction.rs. After
// injecting a transcript, watch the field the user dictated into for a
// short window; if they hand-edit what we typed, log the {injected,
// corrected} pair so the Dictionary page can suggest it and Insights can
// show it. Algorithm (extractCorrection/finalize/wordRetention below) is a
// deliberate line-for-line port of correction.rs's UIA-based watcher — same
// heuristics, same tunables, so "what counts as a correction" means the
// same thing on both platforms. KEEP IN SYNC with correction.rs.
//
// Observation is event-driven, not polled: BulbulAccessibilityService turns
// on TYPE_VIEW_TEXT_CHANGED for the duration of one watch (applyTextWatch /
// setTextWatchActive) and forwards each event here via onTextChanged. This
// matters because a correction that's typed and then IMMEDIATELY submitted
// (no pause before hitting Send) still needs to be caught — the edit and
// the submit-triggered clear are two separate, near-simultaneous text
// changes, and only an event stream (not a periodic poll, which can land on
// either side of a sub-second gap) is guaranteed to see the one in between.
// A single direct read is still used to establish the starting snapshot
// (before events are live) and as a fallback at the end if no event ever
// arrived (some custom editors don't fire standard a11y text-changed
// events). Deliberately short-lived either way: one watch per dictation,
// capped at WATCH_MS, and the event subscription goes back off the moment
// the watch ends — see BulbulAccessibilityService's header comment for why
// it isn't left on permanently.

package com.bulbul.app

import android.accessibilityservice.AccessibilityService
import android.util.Log
import android.view.accessibility.AccessibilityNodeInfo
import java.io.File

object CorrectionWatcher {
    private const val TAG = "BulbulCorrection"
    private const val CORRECTIONS_FILE = "corrections.json"
    private const val MAX_CORRECTIONS = 200

    // Mirrors correction.rs WATCH_SECS.
    private const val WATCH_MS = 12_000L
    // Only paces the "did the foreground app change" check once the event
    // stream is live — the actual text comes from onTextChanged, not this
    // timer, so it can be coarse without risking a missed edit.
    private const val APP_CHANGE_POLL_MS = 400L
    private const val MAX_FIELD_CHARS = 5_000

    /// Only the most recently started watch may write a correction — an
    /// older watch whose window hasn't expired yet abandons itself the
    /// moment a newer dictation starts one, same as desktop's watcher being
    /// scoped to a single spawned thread per dictation.
    @Volatile private var activeToken: Any? = null

    /// The most recent text seen via a live TYPE_VIEW_TEXT_CHANGED event for
    /// the active watch — null until the first event arrives, so the
    /// fallback direct read at teardown can tell "no event ever fired" from
    /// "an event confirmed nothing changed".
    @Volatile private var liveText: String? = null

    /// App the active watch belongs to, for onTextChanged's package filter —
    /// see its comment. Null when no watch is active.
    @Volatile private var watchedApp: String? = null

    /// Starts watching [injected] (the exact text just set into the focused
    /// field) for a hand-edit. Fire-and-forget, like correction.rs's
    /// watch_for_correction — never blocks the caller (TextInjector.inject,
    /// itself already off the main thread).
    fun watch(service: AccessibilityService, injected: String, foregroundApp: String?) {
        val text = injected.trim()
        if (text.isEmpty()) return
        val token = Any()
        activeToken = token
        Thread({ runWatch(token, service, text, foregroundApp) }, "BulbulCorrectionWatch")
            .apply { isDaemon = true; start() }
    }

    /// Called by BulbulAccessibilityService for every TYPE_VIEW_TEXT_CHANGED
    /// event while a watch has the subscription turned on. [pkg] is the
    /// event's own source app, checked against the app this watch belongs
    /// to — a coarse filter (not exact-node identity, which would mean
    /// holding a long-lived AccessibilityNodeInfo reference across the
    /// whole window) that's enough to ignore an unrelated field updating
    /// elsewhere on screen during the same few seconds.
    fun onTextChanged(pkg: String?, node: AccessibilityNodeInfo?) {
        if (activeToken == null || node == null) return
        if (pkg != null && watchedApp != null && pkg != watchedApp) return
        liveText = readNodeText(node)
    }

    /// A field showing its own placeholder ("Message", "Type a message…")
    /// can report that string as its text without isShowingHintText being
    /// set — same trap TextInjector.inject already guards against with the
    /// same three checks. Without this, sending the message (which clears
    /// the field back to its placeholder) reads as "replaced with unrelated
    /// text" instead of "field is empty now", relying on the length/
    /// retention guards to catch it by coincidence rather than by design.
    private fun readNodeText(node: AccessibilityNodeInfo): String {
        if (android.os.Build.VERSION.SDK_INT >= 26 && node.isShowingHintText) return ""
        var text = node.text?.toString() ?: ""
        if (text.isNotEmpty() && android.os.Build.VERSION.SDK_INT >= 26) {
            val hint = node.hintText?.toString()
            val matchesHint = hint != null && text.trim().equals(hint.trim(), ignoreCase = true)
            val cursorAtZero = node.textSelectionEnd <= 0 && node.textSelectionStart <= 0
            if (matchesHint || cursorAtZero) text = ""
        }
        return text
    }

    private fun fieldText(service: AccessibilityService): String? = try {
        val node = service.findFocus(AccessibilityNodeInfo.FOCUS_INPUT)
        val t = node?.let { readNodeText(it) }
        node?.recycle()
        t
    } catch (t: Throwable) {
        null
    }

    private fun runWatch(token: Any, service: AccessibilityService, injected: String, app: String?) {
        // Wait for the SET_TEXT/paste to land, then snapshot — retry a few
        // times until our text shows up (mirrors correction.rs's retry loop;
        // performAction can lag a frame behind on some OEMs). Events aren't
        // live yet at this point, so this is a direct read.
        var snapshot: String? = null
        for (i in 0 until 6) {
            Thread.sleep(150)
            if (activeToken !== token) return
            val t = fieldText(service) ?: continue
            snapshot = t
            if (t.contains(injected)) break
        }
        val before: String = snapshot?.takeIf { it.contains(injected) } ?: run {
            Log.d(TAG, "correction-watch: injected text not found in field; skipping")
            return
        }
        if (before.length > MAX_FIELD_CHARS) return

        // From here on, prefer the live event stream over polling — see the
        // file header for why. Only a lightweight app-change check runs on
        // a timer; the actual text comes from onTextChanged.
        watchedApp = app
        liveText = null
        BulbulAccessibilityService.setTextWatchActive(true)
        try {
            val deadline = System.currentTimeMillis() + WATCH_MS
            while (System.currentTimeMillis() < deadline) {
                Thread.sleep(APP_CHANGE_POLL_MS)
                if (activeToken !== token) return
                if (BulbulAccessibilityService.targetPackage != app) break
            }
            if (activeToken !== token) return

            // liveText is whatever the last real edit-in-place event showed;
            // null means no event ever arrived (this editor doesn't fire
            // standard text-changed events), so fall back to a direct read.
            val latest = liveText ?: fieldText(service) ?: before
            if (latest == before) return

            val corrected = extractCorrection(injected, before, latest) ?: run {
                // Debug-only diagnostic dump — truncated so a long field
                // doesn't flood logcat, but enough to see why a real edit
                // got rejected (span mismatch, too divergent, etc).
                fun clip(s: String) = if (s.length > 120) s.take(120) + "…(${s.length} chars)" else s
                Log.d(
                    TAG,
                    "correction-watch: field changed but no clean correction extractable\n" +
                        "  injected: ${clip(injected)}\n" +
                        "  before:   ${clip(before)}\n" +
                        "  after:    ${clip(latest)}",
                )
                return
            }
            record(service, injected, corrected, app)
        } finally {
            // Only the watch that's still current tears the shared state
            // down — a superseded watch's own finally must not clobber the
            // newer watch that replaced it (activeToken already points
            // elsewhere in that case).
            if (activeToken === token) {
                BulbulAccessibilityService.setTextWatchActive(false)
                liveText = null
                watchedApp = null
            }
        }
    }

    // ---------------------------------------------------------------------
    // Extraction — verbatim-logic port of correction.rs extract_correction /
    // finalize / word_retention.
    // ---------------------------------------------------------------------

    private fun extractCorrection(injected: String, before: String, after: String): String? {
        val start = before.indexOf(injected)
        if (start < 0) return null
        val end = start + injected.length
        val prefix = before.substring(0, start)
        val suffix = before.substring(end)

        if (after.startsWith(prefix) && after.endsWith(suffix) && after.length >= prefix.length + suffix.length) {
            val corrected = after.substring(prefix.length, after.length - suffix.length)
            return finalize(injected, corrected)
        }
        if (prefix.isBlank() && suffix.isBlank()) {
            return finalize(injected, after)
        }
        return null
    }

    private fun finalize(injectedRaw: String, correctedRaw: String): String? {
        val corrected = correctedRaw.trim()
        val injected = injectedRaw.trim()
        if (corrected.isEmpty() || corrected == injected) return null
        // "Kept typing" rather than corrected.
        if (corrected.startsWith(injected)) return null
        val injLen = injected.length.coerceAtLeast(1).toFloat()
        val corLen = corrected.length.toFloat()
        if (kotlin.math.abs(corLen - injLen) / injLen > 0.6f) return null
        if (wordRetention(injected, corrected) < 0.5f) return null
        return corrected
    }

    private fun wordRetention(injected: String, corrected: String): Float {
        fun words(s: String): List<String> =
            s.split(Regex("[^\\p{L}\\p{N}]+")).filter { it.isNotEmpty() }.map { it.lowercase() }
        val inj = words(injected)
        if (inj.isEmpty()) return 0f
        val cor = words(corrected).toHashSet()
        return inj.count { cor.contains(it) }.toFloat() / inj.size
    }

    // ---------------------------------------------------------------------
    // Storage — corrections.json, the Kotlin-writes/Rust-reads half of the
    // pair (dismissals go the other way: Rust writes correction_dismissals
    // .json from a UI action, never Kotlin — see mobile.rs). Same shape as
    // desktop's `corrections` table: exact-duplicate {injected, corrected}
    // pairs bump a hit_count and refresh ts instead of piling up, pruned to
    // the MAX_CORRECTIONS most recent.
    // ---------------------------------------------------------------------

    /// Resolves a package id to its launcher-visible label ("com.whatsapp" ->
    /// "WhatsApp"), same call BulbulForegroundService.friendlyAppName makes
    /// for history.jsonl — duplicated here (rather than shared) since that
    /// one is private to a different class, and it's a single PackageManager
    /// call. Falls back to the raw package id if resolution fails, same as
    /// the original.
    private fun friendlyAppName(context: android.content.Context, pkg: String?): String? {
        if (pkg.isNullOrBlank()) return null
        return try {
            val pm = context.packageManager
            pm.getApplicationLabel(pm.getApplicationInfo(pkg, 0)).toString()
        } catch (t: Throwable) {
            pkg
        }
    }

    @Synchronized
    private fun record(context: android.content.Context, injected: String, corrected: String, app: String?) {
        val inj = injected.trim()
        val cor = corrected.trim()
        if (inj.isEmpty() || cor.isEmpty() || inj == cor) return
        // Stored as the friendly label, matching history.jsonl's convention
        // (BulbulForegroundService resolves before writing there too) — the
        // UI shows this directly, it should never be a raw package id.
        val friendlyApp = friendlyAppName(context, app)

        val file = File(BulbulConfig.dataDir(context), CORRECTIONS_FILE)
        val arr = try {
            if (file.exists()) org.json.JSONArray(file.readText()) else org.json.JSONArray()
        } catch (t: Throwable) {
            org.json.JSONArray()
        }

        var found = false
        for (i in 0 until arr.length()) {
            val e = arr.optJSONObject(i) ?: continue
            if (e.optString("injected") == inj && e.optString("corrected") == cor) {
                e.put("ts", System.currentTimeMillis() / 1000)
                e.put("hit_count", e.optInt("hit_count", 0) + 1)
                found = true
                break
            }
        }
        if (!found) {
            val e = org.json.JSONObject().apply {
                put("ts", System.currentTimeMillis() / 1000)
                put("injected", inj)
                put("corrected", cor)
                if (!friendlyApp.isNullOrBlank()) put("foreground_app", friendlyApp)
                put("hit_count", 0)
            }
            arr.put(e)
        }

        val pruned = org.json.JSONArray(
            (0 until arr.length())
                .mapNotNull { arr.optJSONObject(it) }
                .sortedByDescending { it.optLong("ts", 0) }
                .take(MAX_CORRECTIONS),
        )
        try {
            file.writeText(pruned.toString())
            Log.i(TAG, "correction-watch: learned app=$app\n  injected:  $inj\n  corrected: $cor")
        } catch (t: Throwable) {
            Log.w(TAG, "writing corrections.json failed", t)
        }
    }
}
