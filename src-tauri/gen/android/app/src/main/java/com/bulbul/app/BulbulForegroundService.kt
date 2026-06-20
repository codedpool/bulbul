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

class BulbulForegroundService : Service() {

    private var windowManager: WindowManager? = null
    private var bubbleView: BubbleView? = null
    private var bubbleParams: WindowManager.LayoutParams? = null

    override fun onBind(intent: Intent?): IBinder? = null

    override fun onCreate() {
        super.onCreate()
        Log.i(TAG, "service onCreate")
        createNotificationChannel()
        startInForeground()
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
        hideBubble()
        super.onDestroy()
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

        val view = BubbleView(this)
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
            // Start at the right edge, roughly above where the IME
            // top will be. The user can drag from here; we'll save
            // the last position once SharedPreferences is wired.
            x = (resources.displayMetrics.widthPixels * 0.78).toInt()
            y = (resources.displayMetrics.heightPixels * 0.55).toInt()
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

/// The floating bubble itself. Draws a filled circle in onDraw and
/// owns the touch logic: tap → toggle, long-press → start hold-mode,
/// drag → reposition. For Phase 5 every action just logs; Phase 6
/// wires them into the audio path.
private class BubbleView(context: Context) : View(context) {
    private val fillPaint = Paint(Paint.ANTI_ALIAS_FLAG).apply {
        color = Color.parseColor("#3B82F6") // matches Bulbul's primary blue
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

    private val gestures = GestureDetector(context, object : GestureDetector.SimpleOnGestureListener() {
        override fun onSingleTapConfirmed(e: MotionEvent): Boolean {
            Log.d(TAG, "tap")
            return true
        }

        override fun onLongPress(e: MotionEvent) {
            Log.d(TAG, "long-press")
        }
    })

    fun bind(p: WindowManager.LayoutParams, w: WindowManager) {
        params = p
        wm = w
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
                if (dragging) {
                    Log.d(TAG, "drag end at (${p.x}, ${p.y})")
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
    }
}

private fun Int.dp(context: Context): Int =
    (this * context.resources.displayMetrics.density).toInt()
