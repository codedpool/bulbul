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
                val injected = TextInjector.inject(transcript)
                Log.i(TAG, "transcript len=${transcript.length} injected=$injected")
                // If we couldn't inject (no focused field, A11y not
                // bound), still save the WAV so the dictation isn't
                // lost — but only when there's no transcript to fall
                // back on either.
                if (!injected) writeRecording(wav)
            } else {
                Log.w(TAG, "transcription failed; saving WAV instead")
                writeRecording(wav)
            }
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
        return prefs.getInt(BUBBLE_X, 0) to prefs.getInt(BUBBLE_Y, 0)
    }

    /// Reads the Groq API key from the same config.json the Rust
    /// save_config command writes to. Both sides agree the file lives
    /// under filesDir / app_data_dir — they're the same Android path —
    /// so the React Settings UI writing through Tauri immediately
    /// becomes visible to this service without a JNI bridge.
    private fun getApiKey(): String {
        return try {
            val file = File(filesDir, CONFIG_FILE)
            if (!file.exists()) return ""
            val json = org.json.JSONObject(file.readText())
            json.optString("groq_api_key", "")
        } catch (t: Throwable) {
            Log.w(TAG, "reading config.json failed", t)
            ""
        }
    }

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
        if (bubbleView != null) return
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
        val params = WindowManager.LayoutParams(
            BUBBLE_SIZE_DP.dp(this),
            BUBBLE_SIZE_DP.dp(this),
            overlayType,
            WindowManager.LayoutParams.FLAG_NOT_FOCUSABLE or
                WindowManager.LayoutParams.FLAG_LAYOUT_NO_LIMITS,
            PixelFormat.TRANSLUCENT,
        ).apply {
            gravity = Gravity.TOP or Gravity.START
            // Prefer the last spot the user dragged the bubble to; on
            // a fresh install drop it at the right edge, roughly above
            // the IME top.
            val saved = loadBubblePosition()
            x = saved?.first
                ?: (resources.displayMetrics.widthPixels * 0.78).toInt()
            y = saved?.second
                ?: (resources.displayMetrics.heightPixels * 0.55).toInt()
        }

        view.bind(params, wm)
        wm.addView(view, params)
        bubbleView = view
        bubbleParams = params
    }

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
    }
    private val ringPaint = Paint(Paint.ANTI_ALIAS_FLAG).apply {
        color = Color.WHITE
        style = Paint.Style.STROKE
        strokeWidth = 4f
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
            fillPaint.color = if (active) COLOR_RECORDING else COLOR_IDLE
            invalidate()
        }
    }

    override fun onDraw(canvas: Canvas) {
        val r = width / 2f
        canvas.drawCircle(r, r, r - 2f, fillPaint)
        canvas.drawCircle(r, r, r - 2f, ringPaint)
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
