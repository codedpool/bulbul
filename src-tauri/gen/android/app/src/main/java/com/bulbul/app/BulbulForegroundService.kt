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
import android.app.Service
import android.content.Context
import android.content.Intent
import android.content.pm.ServiceInfo
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
                bubbleView?.setRecording(true)
            } else {
                Log.w(TAG, "recorder failed to start — RECORD_AUDIO probably not granted")
            }
        }
    }

    /// Long-press: hold-to-talk. Press starts; release stops + sends.
    /// BubbleView fires the release once it sees ACTION_UP after a
    /// long-press has fired.
    private fun onBubbleLongPress() {
        val r = recorder ?: return
        if (!r.isRecording()) {
            if (r.start()) bubbleView?.setRecording(true)
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
                val finalText = BulbulConfig.applySnippets(this, corrected)
                val injected = TextInjector.inject(finalText)
                Log.i(TAG, "transcript len=${finalText.length} fixes=$fixes injected=$injected")
                recordHistory(finalText, wavDurationMs(wav), fixes)
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

    /// Appends one dictation to filesDir/history.jsonl. The Rust side
    /// (mobile.rs get_recent_dictations / get_home_stats) reads the same
    /// file — file-as-IPC, same trick as config.json.
    private fun recordHistory(text: String, durationMs: Long, fixCount: Int) {
        try {
            val words = text.trim().split(Regex("\\s+")).count { it.isNotEmpty() }
            val line = org.json.JSONObject().apply {
                put("ts", System.currentTimeMillis() / 1000)
                put("cleaned_text", text)
                put("word_count", words)
                put("mode", "clean")
                put("duration_ms", durationMs)
                put("fix_count", fixCount)
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
        val notification: Notification = NotificationCompat.Builder(this, CHANNEL_ID)
            .setContentTitle("Bulbul")
            .setContentText("Tap the bubble to dictate")
            .setSmallIcon(android.R.drawable.ic_btn_speak_now)
            .setOngoing(true)
            .setPriority(NotificationCompat.PRIORITY_LOW)
            .build()

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
            onDragEnd = ::saveBubblePosition,
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
        } catch (t: Throwable) {
            // BadTokenException — overlay permission was revoked
            // mid-session. Surface it so we know what's wrong.
            Log.e(TAG, "showBubble: addView FAILED", t)
        }
    }

    private fun dp(value: Int): Int =
        (value * resources.displayMetrics.density).toInt()

    private fun hideBubble() {
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

    companion object {
        private const val TAG = "BulbulFG"
        private const val CHANNEL_ID = "bulbul.bubble"
        private const val NOTIFICATION_ID = 1001
        private const val BUBBLE_SIZE_DP = 56
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
    val onDragEnd: (x: Int, y: Int) -> Unit,
)

/// The floating bubble itself. Draws a filled circle in onDraw and
/// owns the touch logic: tap → toggle, long-press → start hold-mode
/// (fires onLongPressDown, then onLongPressUp on release), drag →
/// reposition. The "recording" visual state swaps the fill to red so
/// the user has unambiguous feedback while audio is being captured.
private class BubbleView(context: Context, private val cb: BubbleCallbacks) : View(context) {
    private val fillPaint = Paint(Paint.ANTI_ALIAS_FLAG).apply {
        color = COLOR_IDLE
        style = Paint.Style.FILL
        // Soft drop shadow so the bubble reads as floating above any
        // background. Needs a software layer to render on all APIs.
        setShadowLayer(10f, 0f, 4f, Color.argb(90, 0, 0, 0))
    }
    private val micPaint = Paint(Paint.ANTI_ALIAS_FLAG).apply {
        color = Color.WHITE
        style = Paint.Style.STROKE
        strokeWidth = 5f
        strokeCap = Paint.Cap.ROUND
    }
    private val micFillPaint = Paint(Paint.ANTI_ALIAS_FLAG).apply {
        color = Color.WHITE
        style = Paint.Style.FILL
    }
    /// Animated ring drawn while recording — alpha/radius driven by a
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
            fillPaint.color = if (active) COLOR_RECORDING else COLOR_IDLE
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
        val cx = width / 2f
        val cy = height / 2f
        val r = width / 2f - 8f // leave room for shadow + pulse ring
        if (recording) {
            pulsePaint.alpha = ((1f - pulse) * 160).toInt()
            canvas.drawCircle(cx, cy, r + pulse * 7f, pulsePaint)
        }
        canvas.drawCircle(cx, cy, r, fillPaint)

        // Mic glyph, scaled to the bubble: capsule body, cradle arc,
        // stem and base — the universal "dictate here" symbol.
        val u = r / 22f
        val bodyW = 9f * u
        val bodyTop = cy - 12f * u
        val bodyBottom = cy + 1f * u
        canvas.drawRoundRect(
            cx - bodyW / 2, bodyTop, cx + bodyW / 2, bodyBottom,
            bodyW / 2, bodyW / 2, micFillPaint,
        )
        micPaint.strokeWidth = 2.6f * u
        val arc = android.graphics.RectF(cx - 8f * u, cy - 7f * u, cx + 8f * u, cy + 5f * u)
        canvas.drawArc(arc, 20f, 140f, false, micPaint)
        canvas.drawLine(cx, cy + 5f * u, cx, cy + 9f * u, micPaint)
        canvas.drawLine(cx - 4.5f * u, cy + 10f * u, cx + 4.5f * u, cy + 10f * u, micPaint)
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
                }
                if (dragging) {
                    p.x = dragInitialX + dx.toInt()
                    p.y = dragInitialY + dy.toInt()
                    try {
                        w.updateViewLayout(this, p)
                    } catch (t: Throwable) {
                        Log.w(TAG, "updateViewLayout failed", t)
                    }
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
        // Bulbul's primary blue when idle; cherry red when capturing,
        // so a glance at the screen tells you whether audio is live.
        private val COLOR_IDLE = Color.parseColor("#3B82F6")
        private val COLOR_RECORDING = Color.parseColor("#EF4444")
    }
}

private fun Int.dp(context: Context): Int =
    (this * context.resources.displayMetrics.density).toInt()
