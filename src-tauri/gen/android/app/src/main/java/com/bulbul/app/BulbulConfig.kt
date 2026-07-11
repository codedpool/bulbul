// Shared reader for the config.json that the Rust `save_config` command
// writes (see mobile.rs). Two Kotlin components need the Groq key + chat
// model out of it — the foreground service (transcription) and the
// PROCESS_TEXT transform activity — and both must resolve the *same*
// on-disk location Tauri's app_data_dir maps to.
//
// That location is NOT guaranteed to be filesDir: on some devices Tauri
// writes config.json elsewhere under dataDir, which bit the transcription
// path once already. So we find where config.json actually lives rather
// than assuming, and cache it.

package com.bulbul.app

import android.content.Context
import android.util.Log
import org.json.JSONObject
import java.io.File

object BulbulConfig {
    private const val TAG = "BulbulConfig"
    private const val CONFIG_FILE = "config.json"
    private const val DICTIONARY_FILE = "dictionary.json"
    private const val SNIPPETS_FILE = "snippets.json"
    private const val OVERLAY_FILE = "overlay.json"
    private const val DEFAULT_CHAT_MODEL = "llama-3.1-8b-instant"

    private var cachedDir: File? = null

    /// The directory Tauri's app_data_dir resolves to — the one holding
    /// config.json. Resolved by probing the likely candidates, then a
    /// shallow walk, falling back to filesDir.
    fun dataDir(context: Context): File {
        cachedDir?.let { return it }
        val candidates = listOf(context.filesDir, context.dataDir, File(context.dataDir, "files"))
        for (c in candidates) {
            if (File(c, CONFIG_FILE).exists()) {
                cachedDir = c
                return c
            }
        }
        context.dataDir.walkTopDown().maxDepth(3)
            .firstOrNull { it.name == CONFIG_FILE }
            ?.parentFile?.let {
                cachedDir = it
                return it
            }
        return context.filesDir
    }

    private fun read(context: Context): JSONObject? = try {
        val f = File(dataDir(context), CONFIG_FILE)
        if (f.exists()) JSONObject(f.readText()) else null
    } catch (t: Throwable) {
        Log.w(TAG, "reading config.json failed", t)
        null
    }

    /// Theme preference the React app persists: "dark" | "light" | "system".
    /// Defaults to "light" to match theme.js's own default. Used to keep the
    /// system status-bar / nav-bar icon contrast in sync with the in-app
    /// theme (otherwise, in light mode over a dark OS theme, the white status
    /// icons vanish into the app's white top).
    fun theme(context: Context): String =
        read(context)?.optString("theme", "light").orEmpty().ifBlank { "light" }

    fun apiKey(context: Context): String =
        read(context)?.optString("groq_api_key", "").orEmpty()

    /// Opt-in telemetry gate (Settings ▸ Privacy / onboarding toggle).
    /// Defaults true to match the desktop Config default.
    fun telemetryEnabled(context: Context): Boolean =
        read(context)?.optBoolean("telemetry_enabled", true) ?: true

    /// Dictation language code ("auto" | "en" | "hi" | …). Sent (coarsely) in
    /// telemetry; defaults to "auto".
    fun language(context: Context): String =
        read(context)?.optString("language", "auto").orEmpty().ifBlank { "auto" }

    /// Whether the per-app Style feature is on (Style page master toggle).
    /// Defaults true to match the desktop Config default.
    fun styleEnabled(context: Context): Boolean =
        read(context)?.optBoolean("style_enabled", true) ?: true

    /// Resolves the tone style ("formal" | "casual" | "very_casual") to apply
    /// for the app being dictated into, or null if Style is off. Combines the
    /// per-category picks (style_personal/work/email/other) with the app's
    /// resolved category (see AppStyle), honoring the user's custom overrides.
    fun styleForApp(context: Context, pkg: String?, friendly: String?): String? {
        val cfg = read(context) ?: return null
        if (!cfg.optBoolean("style_enabled", true)) return null
        val category = AppStyle.categoryForApp(pkg, friendly, parseOverrides(cfg))
        val (field, fallback) = when (category) {
            "personal" -> "style_personal" to "casual"
            "work" -> "style_work" to "casual"
            "email" -> "style_email" to "formal"
            else -> "style_other" to "casual"
        }
        return cfg.optString(field, fallback).ifBlank { fallback }
    }

    /// Parses style_app_overrides ([{exe, category}, …]) into (key, category)
    /// pairs. "exe" is the desktop field name; on Android the user types an
    /// app name or package and AppStyle matches it loosely.
    private fun parseOverrides(cfg: JSONObject): List<Pair<String, String>> {
        val arr = cfg.optJSONArray("style_app_overrides") ?: return emptyList()
        val out = ArrayList<Pair<String, String>>(arr.length())
        for (i in 0 until arr.length()) {
            val o = arr.optJSONObject(i) ?: continue
            val key = o.optString("exe").trim()
            val cat = o.optString("category").trim()
            if (key.isNotEmpty() && cat.isNotEmpty()) out.add(key to cat)
        }
        return out
    }

    fun chatModel(context: Context): String =
        read(context)?.optString("chat_model", "").orEmpty().ifBlank { DEFAULT_CHAT_MODEL }

    /// Overlay bubble diameter in dp (Settings → Overlay). Clamped to a sane
    /// range so a stale/garbage value can't produce an invisible or
    /// screen-filling bubble.
    fun overlaySize(context: Context): Int =
        (read(context)?.optInt("overlay_size", 52) ?: 52).coerceIn(40, 120)

    /// Overlay bubble opacity 0.3–1.0 (Settings → Overlay).
    fun overlayOpacity(context: Context): Float =
        (read(context)?.optDouble("overlay_opacity", 0.65) ?: 0.65).toFloat().coerceIn(0.3f, 1.0f)

    /// How long the overlay stays snoozed when dropped on the snooze target,
    /// in minutes (Settings → Overlay). Default 1 hour; clamped 5 min–24 h.
    fun snoozeMinutes(context: Context): Int =
        (read(context)?.optInt("overlay_snooze_minutes", 60) ?: 60).coerceIn(5, 1440)

    /// Snooze deadline lives in overlay.json (NOT config.json), so the React
    /// settings save — which overwrites the whole config — can't clobber it.
    /// Rust's resume_overlay / get_overlay_snoozed_until read the same file.
    fun isSnoozed(context: Context): Boolean =
        snoozedUntilSecs(context) > System.currentTimeMillis() / 1000

    private fun snoozedUntilSecs(context: Context): Long {
        val f = File(dataDir(context), OVERLAY_FILE)
        return try {
            if (f.exists()) JSONObject(f.readText()).optLong("snoozed_until", 0L) else 0L
        } catch (t: Throwable) {
            0L
        }
    }

    /// Writes the snooze deadline (unix seconds; 0 = active) to overlay.json.
    fun setSnoozedUntil(context: Context, secs: Long) {
        val f = File(dataDir(context), OVERLAY_FILE)
        val obj = try {
            if (f.exists()) JSONObject(f.readText()) else JSONObject()
        } catch (t: Throwable) {
            JSONObject()
        }
        obj.put("snoozed_until", secs)
        try {
            f.writeText(obj.toString())
        } catch (t: Throwable) {
            Log.w(TAG, "writing overlay.json failed", t)
        }
    }

    /// Applies the user's dictionary to a transcript: whole-word substitution
    /// of each `from_word` → `to_word` (case-insensitive unless the entry is
    /// case-sensitive). This is the mobile equivalent of the desktop
    /// post-transcription substitution pass — the reason a Dictionary entry
    /// exists is to be applied here. Returns the corrected text plus the count
    /// of substitutions that actually changed something (so Insights can show
    /// "fixes made"). Reads dictionary.json, the same file the Rust side seeds
    /// and the React Dictionary page edits.
    fun applyDictionary(context: Context, text: String): Pair<String, Int> {
        val file = File(dataDir(context), DICTIONARY_FILE)
        val arr = try {
            if (!file.exists()) return text to 0
            org.json.JSONArray(file.readText())
        } catch (t: Throwable) {
            Log.w(TAG, "reading dictionary.json failed", t)
            return text to 0
        }

        var result = text
        var totalFixes = 0
        var changed = false
        for (i in 0 until arr.length()) {
            val e = arr.getJSONObject(i)
            val from = e.optString("from_word").trim()
            val to = e.optString("to_word").trim()
            // Identical from/to entries are capitalization hints for the STT
            // model, not substitutions — applying them would change nothing
            // yet inflate the fix count, so skip them.
            if (from.isEmpty() || to.isEmpty() || from == to) continue
            val opts = if (e.optBoolean("case_sensitive", false)) {
                emptySet()
            } else {
                setOf(RegexOption.IGNORE_CASE)
            }
            val re = try {
                Regex("\\b" + Regex.escape(from) + "\\b", opts)
            } catch (t: Throwable) {
                continue
            }
            // Lambda replacement so `to` is inserted literally (no $-group
            // interpretation) and we only count matches that truly change.
            var entryHits = 0
            result = re.replace(result) { m ->
                if (m.value != to) entryHits++
                to
            }
            if (entryHits > 0) {
                totalFixes += entryHits
                // Bump this entry's usage count so the Dictionary page's
                // "N uses" reflects how often the correction fired.
                e.put("hit_count", e.optInt("hit_count", 0) + entryHits)
                changed = true
            }
        }
        // Persist the incremented counts back to the same file the Rust side
        // reads (list_dictionary) and the React Dictionary page edits.
        if (changed) {
            try {
                file.writeText(arr.toString())
            } catch (t: Throwable) {
                Log.w(TAG, "writing dictionary.json hit counts failed", t)
            }
        }
        return result to totalFixes
    }

    /// Expands snippets in a transcript: replaces each trigger phrase (matched
    /// whole-word, case-insensitively) with its expansion. Applied after the
    /// dictionary, matching desktop. Bumps each snippet's hit_count so the
    /// Snippets page's "N uses" reflects usage. Not counted as a "fix" — a
    /// snippet expansion isn't a correction.
    fun applySnippets(context: Context, text: String): String {
        val file = File(dataDir(context), SNIPPETS_FILE)
        val arr = try {
            if (!file.exists()) return text
            org.json.JSONArray(file.readText())
        } catch (t: Throwable) {
            Log.w(TAG, "reading snippets.json failed", t)
            return text
        }

        var result = text
        var changed = false
        for (i in 0 until arr.length()) {
            val e = arr.getJSONObject(i)
            val trigger = e.optString("trigger").trim()
            val expansion = e.optString("expansion")
            if (trigger.isEmpty() || expansion.isEmpty()) continue
            val re = try {
                Regex("\\b" + Regex.escape(trigger) + "\\b", setOf(RegexOption.IGNORE_CASE))
            } catch (t: Throwable) {
                continue
            }
            var hits = 0
            result = re.replace(result) { hits++; expansion }
            if (hits > 0) {
                e.put("hit_count", e.optInt("hit_count", 0) + hits)
                changed = true
            }
        }
        if (changed) {
            try {
                file.writeText(arr.toString())
            } catch (t: Throwable) {
                Log.w(TAG, "writing snippets.json hit counts failed", t)
            }
        }
        return result
    }
}
