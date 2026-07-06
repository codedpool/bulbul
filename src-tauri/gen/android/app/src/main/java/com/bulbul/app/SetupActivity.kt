// One-time permission walker.
//
// Bulbul needs three Android grants to do its job:
//   1. RECORD_AUDIO    — runtime permission, standard system dialog
//   2. SYSTEM_ALERT_WINDOW (overlay) — special permission, must be
//      granted from a Settings screen we open via Intent
//   3. Accessibility   — the user toggles Bulbul on inside
//      Settings → Accessibility; there's no programmatic grant
//
// MainActivity launches this activity if any of the three are
// missing. The user can grant them in any order. The activity polls
// in onResume (since two of the three require leaving the app), and
// auto-finishes the instant all three are granted so the user can't
// accidentally back into a wedged setup screen after they're done.
//
// UI is intentionally built in code — no XML layout — to keep this
// flow self-contained and to avoid one more file in the gen tree
// that has to be force-added.

package com.bulbul.app

import android.Manifest
import android.app.Activity
import android.content.Context
import android.content.Intent
import android.content.pm.PackageManager
import android.content.res.Configuration
import android.graphics.Color
import android.graphics.Typeface
import android.graphics.drawable.GradientDrawable
import android.net.Uri
import android.os.Bundle
import android.provider.Settings
import android.text.TextUtils
import android.util.TypedValue
import android.view.Gravity
import android.view.View
import android.view.ViewGroup
import android.widget.Button
import android.widget.LinearLayout
import android.widget.ScrollView
import android.widget.TextView
import androidx.core.app.ActivityCompat
import androidx.core.content.ContextCompat

class SetupActivity : Activity() {

    private lateinit var micRow: PermissionRow
    private lateinit var overlayRow: PermissionRow
    private lateinit var accessibilityRow: PermissionRow

    override fun onCreate(savedInstanceState: Bundle?) {
        super.onCreate(savedInstanceState)
        setContentView(buildLayout())
        refreshStatuses()
    }

    override fun onResume() {
        super.onResume()
        refreshStatuses()
        if (allGranted()) finish()
    }

    override fun onRequestPermissionsResult(
        requestCode: Int,
        permissions: Array<out String>,
        grantResults: IntArray,
    ) {
        super.onRequestPermissionsResult(requestCode, permissions, grantResults)
        if (requestCode == REQ_MIC) {
            refreshStatuses()
            if (allGranted()) finish()
        }
    }

    // ---------------- Permission state ----------------

    private fun micGranted(): Boolean =
        ContextCompat.checkSelfPermission(this, Manifest.permission.RECORD_AUDIO) ==
            PackageManager.PERMISSION_GRANTED

    private fun overlayGranted(): Boolean =
        Settings.canDrawOverlays(this)

    private fun accessibilityGranted(): Boolean =
        isAccessibilityServiceEnabled(this, BulbulAccessibilityService::class.java)

    private fun allGranted(): Boolean =
        micGranted() && overlayGranted() && accessibilityGranted()

    private fun refreshStatuses() {
        micRow.setGranted(micGranted())
        overlayRow.setGranted(overlayGranted())
        accessibilityRow.setGranted(accessibilityGranted())
    }

    // ---------------- Theme ----------------
    //
    // Mirrors Bulbul's own palette (mint accent, slate text) and follows the
    // system light/dark setting, so this native first-launch screen reads as
    // part of the app instead of a raw white system dialog.

    private val night: Boolean
        get() = (resources.configuration.uiMode and Configuration.UI_MODE_NIGHT_MASK) ==
            Configuration.UI_MODE_NIGHT_YES

    private fun col(light: Int, dark: Int): Int = if (night) dark else light

    private val bgColor get() = col(0xFFFFFFFF.toInt(), 0xFF101318.toInt())
    // Not "titleColor" — Activity already has a (deprecated) getTitleColor().
    private val headingColor get() = col(0xFF0F172A.toInt(), 0xFFF1F5F9.toInt())
    private val bodyColor get() = col(0xFF475569.toInt(), 0xFF94A3B8.toInt())
    private val mutedColor get() = col(0xFF94A3B8.toInt(), 0xFF6B7280.toInt())
    private val accentColor get() = col(0xFF12A594.toInt(), 0xFF5EC8C0.toInt())

    // ---------------- Layout (code-built) ----------------

    private fun buildLayout(): View {
        val root = LinearLayout(this).apply {
            orientation = LinearLayout.VERTICAL
            setPadding(dp(24), dp(40), dp(24), dp(24))
            setBackgroundColor(bgColor)
        }

        // Brand header — launcher icon + wordmark, so setup feels on-brand.
        val brand = LinearLayout(this).apply {
            orientation = LinearLayout.HORIZONTAL
            gravity = Gravity.CENTER_VERTICAL
            setPadding(0, 0, 0, dp(20))
        }
        brand.addView(android.widget.ImageView(this).apply {
            setImageResource(R.mipmap.ic_launcher)
            layoutParams = LinearLayout.LayoutParams(dp(36), dp(36)).apply {
                rightMargin = dp(10)
            }
        })
        brand.addView(TextView(this).apply {
            text = "bulbul"
            textSize = 22f
            setTypeface(Typeface.create(Typeface.SERIF, Typeface.ITALIC))
            setTextColor(headingColor)
        })
        root.addView(brand)

        root.addView(TextView(this).apply {
            text = "Set up Bulbul"
            textSize = 24f
            setTypeface(typeface, Typeface.BOLD)
            setTextColor(headingColor)
            setPadding(0, 0, 0, dp(8))
        })
        root.addView(TextView(this).apply {
            text = "Bulbul needs three permissions before the floating bubble can dictate into other apps. Grant them in any order — this screen closes itself once all three are on."
            textSize = 14f
            setTextColor(bodyColor)
            setPadding(0, 0, 0, dp(24))
        })

        micRow = PermissionRow(
            this, night,
            title = "Microphone",
            blurb = "Used only while you hold or tap the floating bubble.",
            actionLabel = "Allow microphone",
            onAction = ::requestMic,
        )
        overlayRow = PermissionRow(
            this, night,
            title = "Display over other apps",
            blurb = "Lets the floating bubble appear above your keyboard in any app.",
            actionLabel = "Open Display settings",
            onAction = ::openOverlaySettings,
        )
        accessibilityRow = PermissionRow(
            this, night,
            title = "Accessibility",
            blurb = "Lets Bulbul see which text field you tapped into and paste cleaned-up transcripts there.",
            actionLabel = "Open Accessibility settings",
            onAction = ::openAccessibilitySettings,
        )

        root.addView(micRow.view)
        root.addView(overlayRow.view)
        root.addView(accessibilityRow.view)

        root.addView(TextView(this).apply {
            text = "All three are required because of how Android isolates apps from each other — there's no privileged shortcut."
            textSize = 12f
            setTextColor(mutedColor)
            setPadding(0, dp(16), 0, 0)
        })

        return ScrollView(this).apply {
            addView(root, ViewGroup.LayoutParams(
                ViewGroup.LayoutParams.MATCH_PARENT,
                ViewGroup.LayoutParams.WRAP_CONTENT,
            ))
            setBackgroundColor(bgColor)
        }
    }

    // ---------------- Grant actions ----------------

    private fun requestMic() {
        if (micGranted()) return
        ActivityCompat.requestPermissions(
            this,
            arrayOf(Manifest.permission.RECORD_AUDIO),
            REQ_MIC,
        )
    }

    private fun openOverlaySettings() {
        val intent = Intent(
            Settings.ACTION_MANAGE_OVERLAY_PERMISSION,
            Uri.parse("package:$packageName"),
        )
        startActivity(intent)
    }

    private fun openAccessibilitySettings() {
        startActivity(Intent(Settings.ACTION_ACCESSIBILITY_SETTINGS))
    }

    private fun dp(value: Int): Int =
        TypedValue.applyDimension(
            TypedValue.COMPLEX_UNIT_DIP, value.toFloat(), resources.displayMetrics,
        ).toInt()

    companion object {
        private const val REQ_MIC = 1001

        /// Walks Settings.Secure.ENABLED_ACCESSIBILITY_SERVICES for the
        /// fully-qualified component name of [serviceClass]. There's no
        /// API that just answers "is my service on?" — the colon-
        /// separated string from Settings.Secure is what every
        /// accessibility-using app on Android ends up parsing.
        fun isAccessibilityServiceEnabled(
            context: Context,
            serviceClass: Class<*>,
        ): Boolean {
            val expected = "${context.packageName}/${serviceClass.name}"
            val enabled = Settings.Secure.getString(
                context.contentResolver,
                Settings.Secure.ENABLED_ACCESSIBILITY_SERVICES,
            ) ?: return false
            val splitter = TextUtils.SimpleStringSplitter(':')
            splitter.setString(enabled)
            while (splitter.hasNext()) {
                if (splitter.next().equals(expected, ignoreCase = true)) return true
            }
            return false
        }
    }
}

/// One row per permission: title, blurb, status pill, grant button. Themed
/// (light/dark) to match the Bulbul palette.
private class PermissionRow(
    context: Context,
    night: Boolean,
    title: String,
    blurb: String,
    actionLabel: String,
    onAction: () -> Unit,
) {
    val view: View
    private val status: TextView
    private val grantedColor = if (night) 0xFF5EC8C0.toInt() else 0xFF12A594.toInt()
    private val notGrantedColor = if (night) 0xFFF87171.toInt() else 0xFFDC2626.toInt()

    init {
        val cardBg = if (night) 0xFF1B1F27.toInt() else 0xFFF1F5F9.toInt()
        val titleColor = if (night) 0xFFF1F5F9.toInt() else 0xFF0F172A.toInt()
        val bodyColor = if (night) 0xFF94A3B8.toInt() else 0xFF475569.toInt()
        val accentColor = if (night) 0xFF5EC8C0.toInt() else 0xFF12A594.toInt()
        val onAccent = if (night) 0xFF0B0E12.toInt() else 0xFFFFFFFF.toInt()

        val card = LinearLayout(context).apply {
            orientation = LinearLayout.VERTICAL
            setPadding(dp(context, 16), dp(context, 16), dp(context, 16), dp(context, 16))
            background = GradientDrawable().apply {
                setColor(cardBg)
                cornerRadius = dp(context, 14).toFloat()
            }
            val lp = LinearLayout.LayoutParams(
                ViewGroup.LayoutParams.MATCH_PARENT,
                ViewGroup.LayoutParams.WRAP_CONTENT,
            )
            lp.bottomMargin = dp(context, 12)
            layoutParams = lp
        }
        val header = LinearLayout(context).apply {
            orientation = LinearLayout.HORIZONTAL
            gravity = Gravity.CENTER_VERTICAL
        }
        header.addView(TextView(context).apply {
            text = title
            textSize = 16f
            setTypeface(typeface, Typeface.BOLD)
            setTextColor(titleColor)
            layoutParams = LinearLayout.LayoutParams(0, ViewGroup.LayoutParams.WRAP_CONTENT, 1f)
        })
        status = TextView(context).apply {
            text = "Not granted"
            textSize = 12f
            setTextColor(notGrantedColor)
        }
        header.addView(status)
        card.addView(header)
        card.addView(TextView(context).apply {
            text = blurb
            textSize = 13f
            setTextColor(bodyColor)
            setPadding(0, dp(context, 6), 0, dp(context, 12))
        })
        card.addView(Button(context).apply {
            text = actionLabel
            isAllCaps = false
            setTextColor(onAccent)
            background = GradientDrawable().apply {
                setColor(accentColor)
                cornerRadius = dp(context, 999).toFloat()
            }
            stateListAnimator = null
            setPadding(dp(context, 20), dp(context, 10), dp(context, 20), dp(context, 10))
            setOnClickListener { onAction() }
            layoutParams = LinearLayout.LayoutParams(
                ViewGroup.LayoutParams.WRAP_CONTENT,
                ViewGroup.LayoutParams.WRAP_CONTENT,
            )
        })
        view = card
    }

    fun setGranted(granted: Boolean) {
        if (granted) {
            status.text = "✓ Granted"
            status.setTextColor(grantedColor)
        } else {
            status.text = "Not granted"
            status.setTextColor(notGrantedColor)
        }
    }

    companion object {
        private fun dp(context: Context, v: Int): Int =
            TypedValue.applyDimension(
                TypedValue.COMPLEX_UNIT_DIP, v.toFloat(), context.resources.displayMetrics,
            ).toInt()
    }
}
