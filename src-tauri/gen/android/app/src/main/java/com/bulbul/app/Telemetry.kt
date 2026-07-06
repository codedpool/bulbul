// Opt-in anonymous telemetry — the Android counterpart of telemetry.rs.
//
// The Android dictation pipeline lives in Kotlin (the foreground service),
// not in the Rust mobile.rs that only backs the dashboard, so telemetry is
// posted from here. It writes to the SAME Supabase events table as desktop
// with the SAME event shape, so both platforms land in one stream.
//
// Rules mirror telemetry.rs exactly:
//   - Opt-in gate: track() no-ops unless telemetry_enabled is set. The gate
//     is read from config.json (the toggle the onboarding wizard + Settings
//     ▸ Privacy write).
//   - No content, ever. Allowed: counts, coarse duration/word buckets, mode,
//     language, app version, os, an anonymous UUID. Never: transcripts,
//     audio, dictionary, snippets, or WHICH app you're typing into.
//   - Never blocks the user: every send is on a daemon thread; failures are
//     logged at debug and dropped (no retries — a side-project stream that
//     retries forever just burns battery).
//   - Anon id is a durable v4 UUID in SharedPreferences; clearing it (or the
//     app's data) mints a fresh identity.

package com.bulbul.app

import android.content.Context
import android.util.Log
import org.json.JSONArray
import org.json.JSONObject
import java.net.HttpURLConnection
import java.net.URL
import java.util.UUID
import kotlin.concurrent.thread

object Telemetry {
    private const val TAG = "BulbulTelemetry"

    // Public infrastructure, not secrets — the events table is INSERT-only
    // RLS for the anon role, so this key can only add rows. Same values as
    // telemetry.rs so Android events join the desktop stream.
    private const val PROJECT_URL = "https://mpzuaarkdhdykpbkgyrs.supabase.co"
    private const val PUBLISHABLE_KEY = "sb_publishable_rGLwve2RSBeHG7mZRXYgSw_YUeJcF0x"

    private const val PREFS = "bulbul_telemetry"
    private const val ANON_ID = "anon_id"

    /// Enqueue-and-send one event, if telemetry is enabled. Posts a single
    /// event per call (dictations are infrequent enough that batching isn't
    /// worth the complexity). Off the main thread; best-effort.
    fun track(context: Context, eventName: String, props: JSONObject) {
        if (!BulbulConfig.telemetryEnabled(context)) return
        val anon = anonId(context)
        val version = appVersion(context)
        thread(name = "BulbulTelemetry", isDaemon = true) {
            try {
                val event = JSONObject().apply {
                    put("anon_id", anon)
                    put("app_version", version)
                    put("os", "android")
                    put("event_name", eventName)
                    put("props", props)
                }
                val body = JSONArray().put(event).toString()
                val conn = (URL("$PROJECT_URL/rest/v1/events").openConnection() as HttpURLConnection).apply {
                    requestMethod = "POST"
                    doOutput = true
                    connectTimeout = 10_000
                    readTimeout = 10_000
                    setRequestProperty("apikey", PUBLISHABLE_KEY)
                    setRequestProperty("Authorization", "Bearer $PUBLISHABLE_KEY")
                    setRequestProperty("Content-Type", "application/json")
                    setRequestProperty("Prefer", "return=minimal")
                }
                conn.outputStream.use { it.write(body.toByteArray(Charsets.UTF_8)) }
                val code = conn.responseCode
                if (code !in 200..299) Log.d(TAG, "telemetry POST returned $code")
                conn.disconnect()
            } catch (t: Throwable) {
                Log.d(TAG, "telemetry post failed", t)
            }
        }
    }

    private fun anonId(context: Context): String {
        val prefs = context.getSharedPreferences(PREFS, Context.MODE_PRIVATE)
        prefs.getString(ANON_ID, null)?.let { return it }
        val id = UUID.randomUUID().toString()
        prefs.edit().putString(ANON_ID, id).apply()
        return id
    }

    private fun appVersion(context: Context): String = try {
        context.packageManager.getPackageInfo(context.packageName, 0).versionName ?: "unknown"
    } catch (t: Throwable) {
        "unknown"
    }

    // Buckets match telemetry.rs so the numbers are comparable across OSes.
    fun durationBucket(ms: Long): String = when {
        ms < 2_000 -> "<2s"
        ms < 5_000 -> "2-5s"
        ms < 10_000 -> "5-10s"
        ms < 30_000 -> "10-30s"
        else -> "30s+"
    }

    fun wordCountBucket(n: Int): String = when {
        n <= 5 -> "1-5"
        n <= 20 -> "6-20"
        n <= 50 -> "21-50"
        n <= 100 -> "51-100"
        else -> "100+"
    }
}
