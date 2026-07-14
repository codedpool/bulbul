// The foreground service that owns Bulbul's floating bubble + (later)
// the mic stream.
//
// Why a foreground service?
//   - Android kills background processes that hold the mic; a fg
//     service with FOREGROUND_SERVICE_MICROPHONE is the only way to
//     keep capturing while the user is in another app.
//   - The same lifecycle is the natural owner of the WindowManager
//     overlay that hosts the bubble — when the service dies, the
//     bubble goes with it (no leaked windows).
//
// Phase 5 scope (this file): the service starts up, surfaces a
// persistent notification ("Bulbul is listening"), and adds a
// draggable circular bubble to the system overlay layer. Tap,
// long-press, and drag are detected and logged — they don't yet
// trigger dictation. Audio capture + JNI bridge to Rust + transcript
// injection all land in Phase 6.
//
// Lifecycle:
//   - BulbulAccessibilityService starts us with startForegroundService
//     the first time the user focuses an editable field after the
//     service is enabled.
//   - We stop ourselves once the focused field goes away for more
//     than a short grace period, so the notification doesn't linger
//     when the user is just reading. (Grace logic lands with the
//     focus-tracking improvements.)
//   - The bubble is drawn in TYPE_APPLICATION_OVERLAY which sits
//     above the IME — that's the layer system overlays live in. The
//     keyboard underneath stays interactive because the bubble's
//     LayoutParams flags don't include FLAG_NOT_FOCUSABLE clears.

package com.bulbul.app

import android.app.Notification
import android.app.NotificationChannel
import android.app.NotificationManager
import android.app.PendingIntent
import android.app.Service
import android.content.Context
import android.content.Intent
import android.content.pm.ServiceInfo
import android.graphics.Bitmap
import android.graphics.BitmapFactory
import android.graphics.Canvas
import android.graphics.Color
import android.graphics.Paint
import android.graphics.PixelFormat
import android.os.Build
import android.os.IBinder
import android.util.Log
import android.view.GestureDetector
import android.view.Gravity
import android.view.MotionEvent
import android.view.View
import android.view.WindowManager
import androidx.core.app.NotificationCompat
import java.io.File
import java.io.FileOutputStream
import kotlin.concurrent.thread

class BulbulForegroundService : Service() {

    private var windowManager: WindowManager? = null
    private var bubbleView: BubbleView? = null
    private var bubbleParams: WindowManager.LayoutParams? = null
    private var recorder: AudioRecorder? = null

    /// True while the fg notification is showing the permission-recovery
    /// CTA (a mic/overlay permission was revoked mid-use). Lets us repaint
    /// back to the normal notification exactly once when it recovers.
    private var inRecovery = false

    /// The foreground app captured when the current recording started, so
    /// the transcript is tagged with the app it was dictated into even if
    /// focus shifts by the time transcription returns.
    private var pendingAppPackage: String? = null

    /// The snooze drop-target shown at the bottom of the screen while the
    /// bubble is being dragged; dropping the bubble onto it snoozes Bulbul.
    /// Its center (screen px) is cached so the drag-end hit-test is cheap.
    private var snoozeTargetView: SnoozeTargetView? = null
    private var snoozeCenterX = 0
    private var snoozeCenterY = 0

    override fun onBind(intent: Intent?): IBinder? = null

    override fun onCreate() {
        super.onCreate()
        Log.i(TAG, "service onCreate")
        createNotificationChannel()
        startInForeground()
        recorder = AudioRecorder(this)
        showBubble()
    }

    override fun onStartCommand(intent: Intent?, flags: Int, startId: Int): Int {
        // Sticky so the system restarts us if we're killed for memory
        // pressure while the user has Bulbul enabled. The
        // AccessibilityService will re-trigger us anyway, but sticky
        // smooths over edge cases.
        return START_STICKY
    }

    override fun onDestroy() {
        Log.i(TAG, "service onDestroy")
        // Drop any in-flight recording on the floor so the mic
        // releases — we don't want a half-finished clip lingering
        // after the service dies.
        try { recorder?.stop() } catch (_: Throwable) {}
        recorder = null
        hideBubble()
        super.onDestroy()
    }

    /// Tap-to-toggle: first tap starts the recorder, second tap stops
    /// it, transcribes via Groq, and injects the result into the
    /// focused field. If Groq fails for any reason (no API key, no
    /// network, server error) we fall back to writing the WAV to
    /// filesDir/recordings/ so the user's audio isn't lost.
    private fun onBubbleTap() {
        val r = recorder ?: return
        if (r.isRecording()) {
            val wav = r.stop()
            bubbleView?.setRecording(false)
            if (wav != null) processCapturedAudio(wav)
        } else {
            val ok = r.start()
            if (ok) {
                pendingAppPackage = BulbulAccessibilityService.targetPackage
                bubbleView?.setRecording(true)
                restoreNormalNotification()
            } else {
                // Mic revoked mid-use: the recorder can't start. Surface it
                // via the tappable recovery notification instead of failing
                // silently (the bubble would otherwise just do nothing).
                Log.w(TAG, "recorder failed to start — RECORD_AUDIO probably not granted")
                showPermissionRecovery("Microphone is off")
            }
        }
    }

    /// Long-press: hold-to-talk. Press starts; release stops + sends.
    /// BubbleView fires the release once it sees ACTION_UP after a
    /// long-press has fired.
    private fun onBubbleLongPress() {
        val r = recorder ?: return
        if (!r.isRecording()) {
            if (r.start()) {
                pendingAppPackage = BulbulAccessibilityService.targetPackage
                bubbleView?.setRecording(true)
                restoreNormalNotification()
            } else {
                Log.w(TAG, "recorder failed to start on long-press — RECORD_AUDIO probably not granted")
                showPermissionRecovery("Microphone is off")
            }
        }
    }

    private fun onBubbleHoldReleased() {
        val r = recorder ?: return
        if (r.isRecording()) {
            val wav = r.stop()
            bubbleView?.setRecording(false)
            if (wav != null) processCapturedAudio(wav)
        }
    }

    /// Off-main-thread: ship the WAV to Groq Whisper, then inject the
    /// transcript into the focused field via the AccessibilityService.
    /// On any failure, write the WAV to disk so the user's words
    /// survive — they'll lose the convenience of auto-paste but not
    /// the audio itself.
    private fun processCapturedAudio(wav: ByteArray) {
        val appPkg = pendingAppPackage
        thread(name = "BulbulTranscribe", isDaemon = true) {
            val apiKey = getApiKey()
            val transcript = if (apiKey.isNotBlank()) {
                GroqClient.transcribe(apiKey, wav)
            } else null

            if (transcript != null) {
                // Apply the user's dictionary (whole-word substitutions) then
                // expand snippets — same order as desktop — before injecting.
                // Count dictionary fixes so Insights can report them.
                val (corrected, fixes) = BulbulConfig.applyDictionary(this, transcript)
                var finalText = BulbulConfig.applySnippets(this, corrected)

                // Per-app Style: reformat the tone to match the app being
                // dictated into (WhatsApp → casual, Outlook → formal, …).
                // Deterministic string transform — no LLM, so it's instant
                // and can never "reply" to the dictation.
                val friendly = friendlyAppName(appPkg)
                val styleId = BulbulConfig.styleForApp(this, appPkg, friendly)
                if (styleId != null) {
                    finalText = AppStyle.applyStyle(styleId, finalText)
                }

                val injected = TextInjector.inject(finalText)
                Log.i(TAG, "transcript len=${finalText.length} fixes=$fixes app=$friendly injected=$injected")
                val durationMs = wavDurationMs(wav)
                recordHistory(finalText, durationMs, fixes, friendly)

                // Opt-in telemetry: coarse buckets only — no text, no app name.
                val words = finalText.trim().split(Regex("\\s+")).count { it.isNotEmpty() }
                Telemetry.track(this, "dictation_completed", org.json.JSONObject().apply {
                    put("mode", "clean")
                    put("language", BulbulConfig.language(this@BulbulForegroundService))
                    put("duration_bucket", Telemetry.durationBucket(durationMs))
                    put("word_count_bucket", Telemetry.wordCountBucket(words))
                    put("had_fixes", fixes > 0)
                })
                // Couldn't type it in (focus gone, A11y unbound) — put
                // the words on the clipboard so they're one long-press
                // away instead of silently lost.
                if (!injected) clipboardFallback(finalText)
            } else {
                Log.w(TAG, "transcription failed; saving WAV instead")
                writeRecording(wav)
            }
        }
    }

    /// Resolves a package name to its user-visible app label ("com.whatsapp"
    /// → "WhatsApp") via PackageManager. Falls back to the raw package if the
    /// label can't be read, or null if we never captured a package.
    private fun friendlyAppName(pkg: String?): String? {
        if (pkg.isNullOrBlank()) return null
        return try {
            val pm = packageManager
            pm.getApplicationLabel(pm.getApplicationInfo(pkg, 0)).toString()
        } catch (t: Throwable) {
            pkg
        }
    }

    /// Appends one dictation to filesDir/history.jsonl. The Rust side
    /// (mobile.rs get_recent_dictations / get_home_stats) reads the same
    /// file — file-as-IPC, same trick as config.json.
    private fun recordHistory(text: String, durationMs: Long, fixCount: Int, app: String?) {
        try {
            val words = text.trim().split(Regex("\\s+")).count { it.isNotEmpty() }
            val line = org.json.JSONObject().apply {
                put("ts", System.currentTimeMillis() / 1000)
                put("cleaned_text", text)
                put("word_count", words)
                put("mode", "clean")
                put("duration_ms", durationMs)
                put("fix_count", fixCount)
                // Which app the dictation landed in — powers the dashboard's
                // per-row app badge (Rust get_recent_dictations reads this key).
                if (!app.isNullOrBlank()) put("foreground_app", app)
            }
            // Same dir the Rust side reads (app_data_dir) — resolved, not
            // assumed, for the same reason as getApiKey.
            File(tauriDataDir(), HISTORY_FILE).appendText(line.toString() + "\n")
        } catch (t: Throwable) {
            Log.w(TAG, "history write failed", t)
        }
    }

    /// Clip length from the WAV header's byte-rate field (offset 28,
    /// little-endian) — avoids hardcoding the recorder's format here.
    private fun wavDurationMs(wav: ByteArray): Long {
        if (wav.size < 44) return 0
        val byteRate = java.nio.ByteBuffer.wrap(wav, 28, 4)
            .order(java.nio.ByteOrder.LITTLE_ENDIAN).int
        return if (byteRate > 0) (wav.size - 44) * 1000L / byteRate else 0
    }

    private fun clipboardFallback(text: String) {
        try {
            val cm = getSystemService(Context.CLIPBOARD_SERVICE)
                as android.content.ClipboardManager
            cm.setPrimaryClip(android.content.ClipData.newPlainText("Bulbul", text))
            android.os.Handler(android.os.Looper.getMainLooper()).post {
                android.widget.Toast.makeText(
                    this,
                    "Bulbul: transcript copied — long-press the field to paste",
                    android.widget.Toast.LENGTH_LONG,
                ).show()
            }
        } catch (t: Throwable) {
            Log.w(TAG, "clipboard fallback failed", t)
        }
    }

    /// Persists the bubble's last on-screen position so it doesn't
    /// jump back to the right edge after a system kill or reboot.
    /// SharedPreferences (not config.json) because this is a UI
    /// concern of the foreground service — the user never sees it,
    /// it doesn't belong in the shared Config schema.
    private fun saveBubblePosition(x: Int, y: Int) {
        getSharedPreferences(BUBBLE_PREFS, Context.MODE_PRIVATE).edit().apply {
            putInt(BUBBLE_X, x)
            putInt(BUBBLE_Y, y)
            apply()
        }
    }

    private fun loadBubblePosition(): Pair<Int, Int>? {
        val prefs = getSharedPreferences(BUBBLE_PREFS, Context.MODE_PRIVATE)
        if (!prefs.contains(BUBBLE_X)) return null
        val x = prefs.getInt(BUBBLE_X, 0)
        val y = prefs.getInt(BUBBLE_Y, 0)
        val sizePx = BUBBLE_SIZE_DP.dp(this)
        val screenW = resources.displayMetrics.widthPixels
        val screenH = resources.displayMetrics.heightPixels
        // If the saved position would put the bubble entirely or
        // mostly off-screen (rotation change, switch to a smaller
        // display, stale data from a different default), drop it and
        // fall back to the default — better to surprise the user with
        // a relocated bubble than a missing one.
        if (x < 0 || y < 0 || x + sizePx > screenW || y + sizePx > screenH) {
            Log.w(
                TAG,
                "saved bubble position ($x, $y) is off-screen on " +
                    "${screenW}x${screenH} — discarding"
            )
            prefs.edit().remove(BUBBLE_X).remove(BUBBLE_Y).apply()
            return null
        }
        return x to y
    }

    /// Reads the Groq API key from the same config.json the Rust
    /// save_config command writes to. Both sides agree the file lives
    /// under filesDir / app_data_dir — they're the same Android path —
    /// so the React Settings UI writing through Tauri immediately
    /// becomes visible to this service without a JNI bridge.
    private fun getApiKey(): String {
        return try {
            val file = File(tauriDataDir(), CONFIG_FILE)
            if (!file.exists()) {
                Log.w(TAG, "config.json not found at ${file.absolutePath}")
                return ""
            }
            val json = org.json.JSONObject(file.readText())
            json.optString("groq_api_key", "")
        } catch (t: Throwable) {
            Log.w(TAG, "reading config.json failed", t)
            ""
        }
    }

    /// Tauri's app_data_dir on Android is NOT guaranteed to be filesDir —
    /// on this device the Rust side writes config.json somewhere else
    /// under dataDir. Rather than hardcode an assumption that already
    /// bit us once, find where config.json actually lives and use that
    /// directory for everything we share with the Rust side.
    private fun tauriDataDir(): File {
        cachedDataDir?.let { return it }
        val candidates = listOf(filesDir, dataDir, File(dataDir, "files"))
        for (c in candidates) {
            if (File(c, CONFIG_FILE).exists()) {
                Log.i(TAG, "tauri data dir resolved: ${c.absolutePath}")
                cachedDataDir = c
                return c
            }
        }
        // Shallow walk as a last resort (covers future Tauri layouts).
        dataDir.walkTopDown().maxDepth(3)
            .firstOrNull { it.name == CONFIG_FILE }
            ?.parentFile?.let {
                Log.i(TAG, "tauri data dir found by walk: ${it.absolutePath}")
                cachedDataDir = it
                return it
            }
        Log.w(TAG, "config.json not found anywhere under ${dataDir.absolutePath}")
        return filesDir
    }

    private var cachedDataDir: File? = null

    private fun writeRecording(wav: ByteArray) {
        try {
            val dir = File(filesDir, "recordings").apply { mkdirs() }
            val file = File(dir, "${System.currentTimeMillis()}.wav")
            FileOutputStream(file).use { it.write(wav) }
            Log.i(TAG, "wrote ${wav.size} bytes to ${file.absolutePath}")
        } catch (t: Throwable) {
            Log.w(TAG, "writing recording failed", t)
        }
    }

    private fun startInForeground() {
        val notification = buildNotification(NORMAL_TEXT, recovery = false)

        if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.Q) {
            // FOREGROUND_SERVICE_TYPE_MICROPHONE is required from
            // Android 14 onward for any fg service that uses the mic;
            // we declare it now so the audio path works the moment
            // RECORD_AUDIO is granted in Phase 6.
            startForeground(
                NOTIFICATION_ID,
                notification,
                ServiceInfo.FOREGROUND_SERVICE_TYPE_MICROPHONE,
            )
        } else {
            startForeground(NOTIFICATION_ID, notification)
        }
    }

    /// Builds the fg-service notification in one of two states:
    ///  - normal: "Tap the bubble to dictate"; tapping opens the app.
    ///  - recovery: shown when a permission Bulbul needs was revoked
    ///    mid-use (mic can't record, or the overlay can't be drawn).
    ///    Tapping jumps straight to SetupActivity to re-grant. Without
    ///    this the failures are logcat-only and the app looks dead.
    private fun buildNotification(text: String, recovery: Boolean): Notification {
        val target = if (recovery) SetupActivity::class.java else MainActivity::class.java
        val intent = Intent(this, target).apply {
            flags = Intent.FLAG_ACTIVITY_NEW_TASK or Intent.FLAG_ACTIVITY_CLEAR_TOP
        }
        val piFlags = PendingIntent.FLAG_UPDATE_CURRENT or
            (if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.M) PendingIntent.FLAG_IMMUTABLE else 0)
        val contentIntent = PendingIntent.getActivity(this, if (recovery) 1 else 0, intent, piFlags)
        return NotificationCompat.Builder(this, CHANNEL_ID)
            .setContentTitle(if (recovery) "Bulbul is paused" else "Bulbul")
            .setContentText(text)
            .setSmallIcon(android.R.drawable.ic_btn_speak_now)
            .setOngoing(true)
            .setContentIntent(contentIntent)
            .setPriority(NotificationCompat.PRIORITY_LOW)
            .build()
    }

    /// Flip the notification to the tappable permission-recovery CTA. Called
    /// from the silent-failure sites (recorder can't start, overlay addView
    /// throws) so a revoked permission has a one-tap path back to setup
    /// instead of the bubble just quietly not working.
    private fun showPermissionRecovery(reason: String) {
        inRecovery = true
        val nm = getSystemService(Context.NOTIFICATION_SERVICE) as NotificationManager
        nm.notify(NOTIFICATION_ID, buildNotification("$reason — tap to fix.", recovery = true))
    }

    /// Restore the normal notification once the previously failing action
    /// succeeds again (permission re-granted). No-op unless we were in
    /// recovery, so the happy path doesn't repaint on every bubble tap.
    private fun restoreNormalNotification() {
        if (!inRecovery) return
        inRecovery = false
        val nm = getSystemService(Context.NOTIFICATION_SERVICE) as NotificationManager
        nm.notify(NOTIFICATION_ID, buildNotification(NORMAL_TEXT, recovery = false))
    }

    private fun createNotificationChannel() {
        if (Build.VERSION.SDK_INT < Build.VERSION_CODES.O) return
        val channel = NotificationChannel(
            CHANNEL_ID,
            "Bulbul bubble",
            NotificationManager.IMPORTANCE_LOW,
        ).apply {
            description = "Persistent notification for the floating bubble"
            setShowBadge(false)
        }
        val nm = getSystemService(Context.NOTIFICATION_SERVICE) as NotificationManager
        nm.createNotificationChannel(channel)
    }

    private fun showBubble() {
        if (bubbleView != null) {
            Log.d(TAG, "showBubble called but bubbleView already exists — skipping")
            return
        }
        val wm = getSystemService(Context.WINDOW_SERVICE) as WindowManager
        windowManager = wm

        val view = BubbleView(this, BubbleCallbacks(
            onTap = ::onBubbleTap,
            onLongPressDown = ::onBubbleLongPress,
            onLongPressUp = ::onBubbleHoldReleased,
            onDragStart = ::showSnoozeTarget,
            onDragMove = ::onBubbleDragMove,
            onDragEnd = ::onBubbleDragEnd,
        ))
        val overlayType = if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.O) {
            WindowManager.LayoutParams.TYPE_APPLICATION_OVERLAY
        } else {
            @Suppress("DEPRECATION")
            WindowManager.LayoutParams.TYPE_PHONE
        }
        // Size + opacity come from Settings → Overlay (config.json), read
        // each time the bubble is drawn so a change applies the next time it
        // appears. View alpha makes the whole bubble see-through.
        val sizePx = BulbulConfig.overlaySize(this).dp(this)
        view.alpha = BulbulConfig.overlayOpacity(this)
        val params = WindowManager.LayoutParams(
            sizePx,
            sizePx,
            overlayType,
            WindowManager.LayoutParams.FLAG_NOT_FOCUSABLE or
                WindowManager.LayoutParams.FLAG_LAYOUT_NO_LIMITS,
            PixelFormat.TRANSLUCENT,
        ).apply {
            gravity = Gravity.TOP or Gravity.START
            // Anchored near the top-left of the screen for first-run
            // discoverability — the previous "right edge, mid-height"
            // default was sometimes landing under the IME or off-
            // screen on certain devices and people couldn't find the
            // bubble at all. Saved drag positions still win after the
            // first move.
            val saved = loadBubblePosition()
            val screenW = resources.displayMetrics.widthPixels
            val screenH = resources.displayMetrics.heightPixels
            // Even after loadBubblePosition's validation, clamp once
            // more here so a future rotation or screen-size change
            // can't put us off-screen between load and addView.
            // Leaves a one-bubble-width margin so the user always sees
            // the full circle.
            x = (saved?.first ?: dp(40)).coerceIn(0, screenW - sizePx)
            y = (saved?.second ?: dp(120)).coerceIn(0, screenH - sizePx)
        }

        Log.i(
            TAG,
            "showBubble: addView size=${sizePx}px pos=(${params.x}, ${params.y}) " +
                "screen=${resources.displayMetrics.widthPixels}x${resources.displayMetrics.heightPixels} " +
                "overlayType=$overlayType"
        )
        try {
            view.bind(params, wm)
            wm.addView(view, params)
            bubbleView = view
            bubbleParams = params
            Log.i(TAG, "showBubble: addView succeeded")
            restoreNormalNotification()
        } catch (t: Throwable) {
            // BadTokenException — overlay permission was revoked mid-session.
            // Surface it to the USER via the tappable recovery notification
            // (not just logcat) so the missing bubble has an explanation and
            // a one-tap path back to setup.
            Log.e(TAG, "showBubble: addView FAILED", t)
            showPermissionRecovery("Screen overlay is off")
        }
    }

    private fun dp(value: Int): Int =
        (value * resources.displayMetrics.density).toInt()

    private fun hideBubble() {
        hideSnoozeTarget()
        val v = bubbleView ?: return
        try {
            windowManager?.removeView(v)
        } catch (t: Throwable) {
            // View may already be gone if the system reaped the
            // window — swallow to avoid crashing during shutdown.
            Log.w(TAG, "removeView failed", t)
        }
        bubbleView = null
        bubbleParams = null
        windowManager = null
    }

    // ---- Snooze: drag the bubble onto the bottom target to mute it ----

    /// Reveals the snooze drop-target at the bottom-center of the screen
    /// when a drag begins. It's a display-only overlay (not touchable) —
    /// the hit-test lives in the bubble's drag-end, so the keyboard beneath
    /// stays fully usable and the target can safely render above it.
    private fun showSnoozeTarget() {
        if (snoozeTargetView != null) return
        val wm = getSystemService(Context.WINDOW_SERVICE) as WindowManager
        val size = dp(SNOOZE_TARGET_DP)
        val screenW = resources.displayMetrics.widthPixels
        val screenH = resources.displayMetrics.heightPixels
        snoozeCenterX = screenW / 2
        snoozeCenterY = screenH - dp(96) - size / 2
        val overlayType = if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.O) {
            WindowManager.LayoutParams.TYPE_APPLICATION_OVERLAY
        } else {
            @Suppress("DEPRECATION")
            WindowManager.LayoutParams.TYPE_PHONE
        }
        val params = WindowManager.LayoutParams(
            size, size, overlayType,
            WindowManager.LayoutParams.FLAG_NOT_FOCUSABLE or
                WindowManager.LayoutParams.FLAG_NOT_TOUCHABLE or
                WindowManager.LayoutParams.FLAG_LAYOUT_NO_LIMITS,
            PixelFormat.TRANSLUCENT,
        ).apply {
            gravity = Gravity.TOP or Gravity.START
            x = snoozeCenterX - size / 2
            y = snoozeCenterY - size / 2
        }
        try {
            val view = SnoozeTargetView(this)
            wm.addView(view, params)
            snoozeTargetView = view
        } catch (t: Throwable) {
            Log.w(TAG, "snooze target addView failed", t)
        }
    }

    private fun onBubbleDragMove(x: Int, y: Int) {
        snoozeTargetView?.setActive(isOverSnooze(x, y))
    }

    private fun onBubbleDragEnd(x: Int, y: Int) {
        val over = isOverSnooze(x, y)
        hideSnoozeTarget()
        if (over) snoozeNow() else saveBubblePosition(x, y)
    }

    /// True when the dragged bubble's center is within the snooze target.
    private fun isOverSnooze(bubbleX: Int, bubbleY: Int): Boolean {
        if (snoozeTargetView == null) return false
        val bs = bubbleParams?.width ?: dp(BUBBLE_SIZE_DP)
        val bcx = bubbleX + bs / 2
        val bcy = bubbleY + bs / 2
        val dx = (bcx - snoozeCenterX).toDouble()
        val dy = (bcy - snoozeCenterY).toDouble()
        val dist = kotlin.math.sqrt(dx * dx + dy * dy)
        return dist < dp(SNOOZE_TARGET_DP) / 2.0 + bs / 2.0 + dp(12)
    }

    private fun hideSnoozeTarget() {
        val v = snoozeTargetView ?: return
        try {
            (getSystemService(Context.WINDOW_SERVICE) as WindowManager).removeView(v)
        } catch (t: Throwable) {
            Log.w(TAG, "snooze target removeView failed", t)
        }
        snoozeTargetView = null
    }

    /// Snooze for the configured duration: persist the deadline, tell the
    /// a11y service to forget it was showing the bubble, and stop — the
    /// bubble stays gone until the deadline passes (shouldShowBubble checks
    /// isSnoozed on the next IME event).
    private fun snoozeNow() {
        val minutes = BulbulConfig.snoozeMinutes(this)
        val untilSecs = System.currentTimeMillis() / 1000 + minutes * 60L
        BulbulConfig.setSnoozedUntil(this, untilSecs)
        android.widget.Toast.makeText(
            this, "Bulbul snoozed for ${snoozeLabel(minutes)}", android.widget.Toast.LENGTH_SHORT,
        ).show()
        BulbulAccessibilityService.notifySnoozed()
        stopSelf()
    }

    private fun snoozeLabel(min: Int): String = when {
        min < 60 -> "${min}m"
        min % 60 == 0 -> "${min / 60}h"
        else -> "${min / 60}h ${min % 60}m"
    }

    companion object {
        private const val TAG = "BulbulFG"
        private const val CHANNEL_ID = "bulbul.bubble"
        private const val NOTIFICATION_ID = 1001
        private const val NORMAL_TEXT = "Tap the bubble to dictate"
        private const val BUBBLE_SIZE_DP = 56
        private const val SNOOZE_TARGET_DP = 68
        // Mirrors MOBILE_CONFIG_FILE on the Rust side — same file,
        // both processes read/write JSON shaped like `Config`.
        private const val CONFIG_FILE = "config.json"
        // Dictation history, one JSON object per line. Read by
        // mobile.rs for the dashboard's Recent activity + stats.
        private const val HISTORY_FILE = "history.jsonl"
        // Bubble position cache — separate from config.json because
        // it's a transient UI concern, not a setting the user
        // configures through the React Settings page.
        private const val BUBBLE_PREFS = "bulbul_bubble"
        private const val BUBBLE_X = "x"
        private const val BUBBLE_Y = "y"

        /// Start the foreground service if it isn't already running.
        /// Safe to call repeatedly.
        fun start(context: Context) {
            val intent = Intent(context, BulbulForegroundService::class.java)
            if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.O) {
                context.startForegroundService(intent)
            } else {
                context.startService(intent)
            }
        }

        fun stop(context: Context) {
            context.stopService(Intent(context, BulbulForegroundService::class.java))
        }
    }
}

/// Callbacks the bubble fires into the foreground service. Kept as a
/// data class with named lambdas so the wiring at the call site reads
/// declaratively (`onTap = ...`) and so the service can pass method
/// references in directly.
private data class BubbleCallbacks(
    val onTap: () -> Unit,
    val onLongPressDown: () -> Unit,
    val onLongPressUp: () -> Unit,
    val onDragStart: () -> Unit,
    val onDragMove: (x: Int, y: Int) -> Unit,
    val onDragEnd: (x: Int, y: Int) -> Unit,
)

/// The floating bubble itself. Draws a filled circle in onDraw and
/// owns the touch logic: tap → toggle, long-press → start hold-mode
/// (fires onLongPressDown, then onLongPressUp on release), drag →
/// reposition. The "recording" visual state swaps the fill to red so
/// the user has unambiguous feedback while audio is being captured.
private class BubbleView(context: Context, private val cb: BubbleCallbacks) : View(context) {
    /// The bubble IS the app icon: a black rounded-square tile with the
    /// gold Bulbul bird centered on it — identical to the launcher icon.
    /// The whole view's alpha (set from the user's overlay-opacity
    /// setting in showBubble) is what makes it see-through; nothing here
    /// dims the icon itself.
    private val tilePaint = Paint(Paint.ANTI_ALIAS_FLAG).apply {
        color = Color.BLACK
        style = Paint.Style.FILL
        // Soft drop shadow so the tile reads as floating above any
        // background. Needs a software layer to render on all APIs.
        setShadowLayer(10f, 0f, 4f, Color.argb(90, 0, 0, 0))
    }
    /// The Bulbul bird, keyed to a transparent PNG (drawable-nodpi/
    /// bulbul_bird.png) — same art the launcher foreground uses.
    private val birdBitmap: Bitmap? = try {
        BitmapFactory.decodeResource(context.resources, R.drawable.bulbul_bird)
    } catch (t: Throwable) {
        Log.w(TAG, "bird bitmap decode failed", t)
        null
    }
    private val birdPaint = Paint(Paint.ANTI_ALIAS_FLAG or Paint.FILTER_BITMAP_FLAG)
    private val birdDst = android.graphics.RectF()
    private val tileRect = android.graphics.RectF()
    /// Animated border drawn while recording — alpha/inset driven by a
    /// repeating animator so "live mic" is unmissable at a glance.
    private val pulsePaint = Paint(Paint.ANTI_ALIAS_FLAG).apply {
        color = COLOR_RECORDING
        style = Paint.Style.STROKE
        strokeWidth = 6f
    }
    private var recording = false
    private var pulse = 0f
    private var pulser: android.animation.ValueAnimator? = null

    init {
        setLayerType(LAYER_TYPE_SOFTWARE, null)
    }

    private var params: WindowManager.LayoutParams? = null
    private var wm: WindowManager? = null

    private var dragInitialX = 0
    private var dragInitialY = 0
    private var dragInitialTouchX = 0f
    private var dragInitialTouchY = 0f
    private var dragging = false

    /// True between the long-press detector firing and ACTION_UP.
    /// Used to suppress the singleTapConfirmed callback for the same
    /// gesture and to know to fire onLongPressUp on release.
    private var inLongPress = false

    private val gestures = GestureDetector(context, object : GestureDetector.SimpleOnGestureListener() {
        override fun onSingleTapConfirmed(e: MotionEvent): Boolean {
            // GestureDetector calls this even if a long-press already
            // fired; gate it so hold gestures don't also toggle.
            if (!inLongPress) cb.onTap()
            return true
        }

        override fun onLongPress(e: MotionEvent) {
            inLongPress = true
            cb.onLongPressDown()
        }
    })

    fun bind(p: WindowManager.LayoutParams, w: WindowManager) {
        params = p
        wm = w
    }

    /// Flip the fill colour so the user can see when capture is active.
    /// Posted to the view's handler so it's safe to call from any
    /// thread (the recorder lives off the main thread).
    fun setRecording(active: Boolean) {
        post {
            recording = active
            pulser?.cancel(); pulser = null
            if (active) {
                pulser = android.animation.ValueAnimator.ofFloat(0f, 1f).apply {
                    duration = 1100
                    repeatCount = android.animation.ValueAnimator.INFINITE
                    addUpdateListener { pulse = it.animatedValue as Float; invalidate() }
                    start()
                }
            }
            invalidate()
        }
    }

    override fun onDraw(canvas: Canvas) {
        val pad = 8f // room for the drop shadow + recording border
        tileRect.set(pad, pad, width - pad, height - pad)
        val radius = tileRect.width() * TILE_CORNER_FRACTION

        // Black rounded-square tile — the app-icon ground.
        canvas.drawRoundRect(tileRect, radius, radius, tilePaint)

        // The gold bird, centered and scaled to the same proportion the
        // launcher foreground uses, so the bubble is literally the app
        // icon.
        val bmp = birdBitmap
        if (bmp != null && bmp.width > 0 && bmp.height > 0) {
            val maxSide = tileRect.width() * BIRD_TILE_FRACTION
            val scale = maxSide / maxOf(bmp.width, bmp.height)
            val hw = bmp.width * scale / 2f
            val hh = bmp.height * scale / 2f
            val cx = tileRect.centerX()
            val cy = tileRect.centerY()
            birdDst.set(cx - hw, cy - hh, cx + hw, cy + hh)
            canvas.drawBitmap(bmp, null, birdDst, birdPaint)
        }

        // Recording: a pulsing red border hugging the tile — keeps the
        // icon itself black while making "live mic" unmissable.
        if (recording) {
            pulsePaint.alpha = (120 + (1f - pulse) * 135).toInt().coerceIn(0, 255)
            val grow = pulse * 3f
            val rr = android.graphics.RectF(
                tileRect.left - grow, tileRect.top - grow,
                tileRect.right + grow, tileRect.bottom + grow,
            )
            canvas.drawRoundRect(rr, radius + grow, radius + grow, pulsePaint)
        }
    }

    override fun onTouchEvent(event: MotionEvent): Boolean {
        // Drag handling is manual because GestureDetector's onScroll
        // doesn't move WindowManager params. Tap + long-press still
        // route through GestureDetector.
        val p = params ?: return super.onTouchEvent(event)
        val w = wm ?: return super.onTouchEvent(event)

        when (event.actionMasked) {
            MotionEvent.ACTION_DOWN -> {
                dragInitialX = p.x
                dragInitialY = p.y
                dragInitialTouchX = event.rawX
                dragInitialTouchY = event.rawY
                dragging = false
                inLongPress = false
            }
            MotionEvent.ACTION_MOVE -> {
                val dx = event.rawX - dragInitialTouchX
                val dy = event.rawY - dragInitialTouchY
                if (!dragging && (kotlin.math.abs(dx) > TOUCH_SLOP_PX || kotlin.math.abs(dy) > TOUCH_SLOP_PX)) {
                    dragging = true
                    cb.onDragStart() // reveal the snooze drop-target
                }
                if (dragging) {
                    p.x = dragInitialX + dx.toInt()
                    p.y = dragInitialY + dy.toInt()
                    try {
                        w.updateViewLayout(this, p)
                    } catch (t: Throwable) {
                        Log.w(TAG, "updateViewLayout failed", t)
                    }
                    cb.onDragMove(p.x, p.y)
                }
            }
            MotionEvent.ACTION_UP, MotionEvent.ACTION_CANCEL -> {
                if (inLongPress) {
                    cb.onLongPressUp()
                    inLongPress = false
                }
                if (dragging) {
                    Log.d(TAG, "drag end at (${p.x}, ${p.y})")
                    cb.onDragEnd(p.x, p.y)
                    dragging = false
                }
            }
        }
        // Always feed the event into GestureDetector — it figures out
        // whether the gesture qualifies as a tap or long-press based
        // on the actual movement / timing.
        gestures.onTouchEvent(event)
        return true
    }

    companion object {
        private const val TAG = "BulbulBubble"
        private const val TOUCH_SLOP_PX = 12
        // Corner radius of the black tile as a fraction of its width —
        // tuned to read like the launcher icon's rounded square.
        private const val TILE_CORNER_FRACTION = 0.26f
        // Bird size as a fraction of the tile — matches the launcher
        // foreground's padding so the bubble is the same icon.
        private const val BIRD_TILE_FRACTION = 0.56f
        // Cherry red for the recording border, so a glance tells you
        // whether audio is live.
        private val COLOR_RECORDING = Color.parseColor("#EF4444")
    }
}

/// The snooze drop-target: a soft circle with a crescent-moon glyph that
/// sits at the bottom of the screen while the bubble is dragged. Idle it's
/// a muted dark disc with a teal moon; when the bubble hovers over it, it
/// grows and flips to a teal disc with a dark moon, so the drop is obvious.
private class SnoozeTargetView(context: Context) : View(context) {
    private var active = false
    private val bgPaint = Paint(Paint.ANTI_ALIAS_FLAG).apply {
        style = Paint.Style.FILL
        setShadowLayer(12f, 0f, 4f, Color.argb(120, 0, 0, 0))
    }
    private val ringPaint = Paint(Paint.ANTI_ALIAS_FLAG).apply {
        style = Paint.Style.STROKE
    }
    private val moonPaint = Paint(Paint.ANTI_ALIAS_FLAG).apply {
        style = Paint.Style.FILL
    }

    init {
        setLayerType(LAYER_TYPE_SOFTWARE, null)
    }

    fun setActive(a: Boolean) {
        if (a == active) return
        active = a
        invalidate()
    }

    override fun onDraw(canvas: Canvas) {
        val cx = width / 2f
        val cy = height / 2f
        val r = (width / 2f - 10f) * (if (active) 1f else 0.86f)

        bgPaint.color = if (active) COLOR_ACTIVE else COLOR_IDLE
        canvas.drawCircle(cx, cy, r, bgPaint)
        ringPaint.color = if (active) Color.WHITE else COLOR_ACTIVE
        ringPaint.strokeWidth = if (active) 4f else 3f
        canvas.drawCircle(cx, cy, r, ringPaint)

        // Crescent moon = "snooze". Carve a circle out of a circle.
        moonPaint.color = if (active) COLOR_ON_ACTIVE else COLOR_ACTIVE
        val mr = r * 0.42f
        val moon = android.graphics.Path().apply {
            addCircle(cx, cy, mr, android.graphics.Path.Direction.CW)
        }
        val cut = android.graphics.Path().apply {
            addCircle(cx + mr * 0.55f, cy - mr * 0.35f, mr * 0.95f, android.graphics.Path.Direction.CW)
        }
        moon.op(cut, android.graphics.Path.Op.DIFFERENCE)
        canvas.drawPath(moon, moonPaint)
    }

    companion object {
        private val COLOR_IDLE = Color.parseColor("#22262B")
        private val COLOR_ACTIVE = Color.parseColor("#5EC8C0")
        private val COLOR_ON_ACTIVE = Color.parseColor("#0A1716")
    }
}

private fun Int.dp(context: Context): Int =
    (this * context.resources.displayMetrics.density).toInt()
