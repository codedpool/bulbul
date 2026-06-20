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
import android.graphics.Color
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

    // ---------------- Layout (code-built) ----------------

    private fun buildLayout(): View {
        val root = LinearLayout(this).apply {
            orientation = LinearLayout.VERTICAL
            setPadding(dp(24), dp(36), dp(24), dp(24))
            setBackgroundColor(Color.WHITE)
        }

        root.addView(TextView(this).apply {
            text = "Set up Bulbul"
            textSize = 24f
            setTextColor(Color.BLACK)
            setPadding(0, 0, 0, dp(8))
        })
        root.addView(TextView(this).apply {
            text = "Bulbul needs three permissions before the floating bubble can dictate into other apps. Grant them in any order — this screen closes itself once all three are on."
            textSize = 14f
            setTextColor(Color.parseColor("#475569"))
            setPadding(0, 0, 0, dp(24))
        })

        micRow = PermissionRow(
            this,
            title = "Microphone",
            blurb = "Used only while you hold or tap the floating bubble.",
            actionLabel = "Allow microphone",
            onAction = ::requestMic,
        )
        overlayRow = PermissionRow(
            this,
            title = "Display over other apps",
            blurb = "Lets the floating bubble appear above your keyboard in any app.",
            actionLabel = "Open Display settings",
            onAction = ::openOverlaySettings,
        )
        accessibilityRow = PermissionRow(
            this,
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
            setTextColor(Color.parseColor("#94a3b8"))
            setPadding(0, dp(16), 0, 0)
        })

        return ScrollView(this).apply {
            addView(root, ViewGroup.LayoutParams(
                ViewGroup.LayoutParams.MATCH_PARENT,
                ViewGroup.LayoutParams.WRAP_CONTENT,
            ))
            setBackgroundColor(Color.WHITE)
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

/// One row per permission: title, blurb, status pill, grant button.
private class PermissionRow(
    context: Context,
    title: String,
    blurb: String,
    actionLabel: String,
    onAction: () -> Unit,
) {
    val view: View
    private val status: TextView

    init {
        val card = LinearLayout(context).apply {
            orientation = LinearLayout.VERTICAL
            setPadding(dp(context, 16), dp(context, 16), dp(context, 16), dp(context, 16))
            setBackgroundColor(Color.parseColor("#F8FAFC"))
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
            setTextColor(Color.BLACK)
            layoutParams = LinearLayout.LayoutParams(0, ViewGroup.LayoutParams.WRAP_CONTENT, 1f)
        })
        status = TextView(context).apply {
            text = "Not granted"
            textSize = 12f
            setTextColor(Color.parseColor("#dc2626"))
        }
        header.addView(status)
        card.addView(header)
        card.addView(TextView(context).apply {
            text = blurb
            textSize = 13f
            setTextColor(Color.parseColor("#475569"))
            setPadding(0, dp(context, 6), 0, dp(context, 12))
        })
        card.addView(Button(context).apply {
            text = actionLabel
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
            status.text = "Granted"
            status.setTextColor(Color.parseColor("#16a34a"))
        } else {
            status.text = "Not granted"
            status.setTextColor(Color.parseColor("#dc2626"))
        }
    }

    companion object {
        private fun dp(context: Context, v: Int): Int =
            TypedValue.applyDimension(
                TypedValue.COMPLEX_UNIT_DIP, v.toFloat(), context.resources.displayMetrics,
            ).toInt()
    }
}
