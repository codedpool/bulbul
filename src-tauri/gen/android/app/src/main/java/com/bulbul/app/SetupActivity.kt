// SPDX-License-Identifier: GPL-3.0-only
// Copyright (c) 2026 Romanch Roshan Singh

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
// missing. The walker shows ONE permission per screen, easiest
// first, and requires the current one to be granted before it
// advances — auto-advancing (with a brief success beat) the moment
// it detects the grant, rather than a manual Next button. It polls
// in onResume (since two of the three require leaving the app) and
// auto-finishes the instant all three are granted, so the user can't
// end up wedged on a setup screen after they're actually done.
//
// UI is intentionally built in code — no XML layout — to keep this
// flow self-contained and to avoid one more file in the gen tree
// that has to be force-added.

package com.bulbul.app

import android.Manifest
import android.app.Activity
import android.content.ComponentName
import android.content.Context
import android.content.Intent
import android.content.pm.PackageManager
import android.content.res.Configuration
import android.graphics.Typeface
import android.os.Build
import android.widget.Toast
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
import android.widget.FrameLayout
import android.widget.LinearLayout
import android.widget.ScrollView
import android.widget.TextView
import androidx.core.app.ActivityCompat
import androidx.core.content.ContextCompat

class SetupActivity : Activity() {

    /// Which of [steps] is on screen. Advances only when that step's
    /// permission is freshly granted (see [checkForAutoAdvance]); Back
    /// always moves freely since it can't un-grant anything.
    private var step = 0

    /// Snapshot of whether the current step was ALREADY granted at the
    /// moment it was rendered — the auto-advance trigger is the
    /// not-granted-to-granted transition, not "is granted", so that
    /// paging Back to an already-done step doesn't immediately bounce
    /// forward again.
    private var stepEnteredGranted = false

    /// True while a scheduled advance (success beat -> step++ -> render) is
    /// in flight. Granting mic fires BOTH onResume (the system dialog
    /// closing brings the activity back) and onRequestPermissionsResult
    /// (the actual result) in quick succession, and the real step++ doesn't
    /// happen until playSuccessBeat's delayed callback completes — so
    /// without this guard, both calls see "not yet advanced" and each
    /// schedules its own advance, net-advancing by two and skipping a
    /// screen. Reset once the in-flight advance's step++ actually runs.
    private var advancing = false

    private lateinit var dotsContainer: LinearLayout
    private lateinit var backButton: TextView
    private lateinit var contentContainer: FrameLayout

    private data class PermStep(
        val title: String,
        val blurb: String,
        val actionLabel: String,
        val onAction: () -> Unit,
        val isGranted: () -> Boolean,
        val extra: (() -> View)? = null,
    )

    private val steps: List<PermStep> by lazy {
        listOf(
            PermStep(
                title = "Microphone",
                blurb = "Used only while you hold or tap the floating bubble.",
                actionLabel = "Allow microphone",
                onAction = ::requestMic,
                isGranted = ::micGranted,
            ),
            PermStep(
                title = "Display over other apps",
                blurb = "Lets the floating bubble appear above your keyboard in any app.",
                actionLabel = "Open Display settings",
                onAction = ::openOverlaySettings,
                isGranted = ::overlayGranted,
            ),
            PermStep(
                title = "Accessibility",
                blurb = "Lets Bulbul see which text field you tapped into and paste cleaned-up transcripts there.",
                actionLabel = "Open Accessibility settings",
                onAction = ::openAccessibilitySettings,
                isGranted = ::accessibilityGranted,
                // The "restricted settings" block this card explains only
                // ever hits sideloaded installs — a real Play install never
                // trips it — so only show it when we're NOT running under
                // Play. Otherwise Play users see irrelevant sideload
                // troubleshooting on a build that will never need it.
                extra = if (!installedViaPlayStore()) {
                    { buildRestrictedHelpCard() }
                } else null,
            ),
        )
    }

    override fun onCreate(savedInstanceState: Bundle?) {
        super.onCreate(savedInstanceState)
        // Start at the FIRST permission that's actually missing, not always
        // step 0. Covers two cases: (a) a later launch after one permission
        // got revoked mid-use — the walker should only ask for that one, not
        // repeat ones already granted; (b) with auto-advance being the only
        // forward motion (no manual Next), always starting at 0 would leave
        // an already-granted step just sitting there showing "✓ Granted"
        // with nothing to trigger the advance. Already-granted steps stay
        // reachable via Back if the user wants to double-check them.
        val firstMissing = steps.indexOfFirst { !it.isGranted() }
        if (firstMissing < 0) {
            // Nothing is actually missing — e.g. reopened after everything
            // was already granted elsewhere. Nothing to walk through.
            finish()
            return
        }
        step = firstMissing
        setContentView(buildScaffold())
        renderStep(animate = false)
    }

    override fun onResume() {
        super.onResume()
        checkForAutoAdvance()
    }

    override fun onRequestPermissionsResult(
        requestCode: Int,
        permissions: Array<out String>,
        grantResults: IntArray,
    ) {
        super.onRequestPermissionsResult(requestCode, permissions, grantResults)
        if (requestCode == REQ_MIC) checkForAutoAdvance()
    }

    /// Disabled while the walker is up — reaching this Activity at all means
    /// at least one permission is still missing (allGranted() already
    /// finishes the Activity everywhere else), so exiting via system Back
    /// would just reveal MainActivity's onboarding underneath with setup
    /// incomplete. Per product decision: nothing proceeds until all three
    /// are granted.
    @Deprecated("Deprecated in Java")
    override fun onBackPressed() {
        Toast.makeText(
            this,
            "Please finish granting these permissions to continue.",
            Toast.LENGTH_SHORT,
        ).show()
    }

    // ---------------- Permission state ----------------

    /// Whether Bulbul was installed via the Play Store, vs. a sideloaded
    /// APK (GitHub direct download, adb install, etc.). Used to hide
    /// sideload-only troubleshooting (buildRestrictedHelpCard) from Play
    /// users, for whom it's never relevant.
    private fun installedViaPlayStore(): Boolean {
        val installer = if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.R) {
            try {
                packageManager.getInstallSourceInfo(packageName).installingPackageName
            } catch (t: Throwable) {
                null
            }
        } else {
            @Suppress("DEPRECATION")
            packageManager.getInstallerPackageName(packageName)
        }
        return installer == "com.android.vending"
    }

    private fun micGranted(): Boolean =
        ContextCompat.checkSelfPermission(this, Manifest.permission.RECORD_AUDIO) ==
            PackageManager.PERMISSION_GRANTED

    private fun overlayGranted(): Boolean =
        Settings.canDrawOverlays(this)

    private fun accessibilityGranted(): Boolean =
        isAccessibilityServiceEnabled(this, BulbulAccessibilityService::class.java)

    private fun allGranted(): Boolean =
        micGranted() && overlayGranted() && accessibilityGranted()

    /// Re-checks the CURRENT step's permission on every resume/permission
    /// result. If it just flipped from not-granted to granted during this
    /// visit, plays a brief success beat and advances to the next step
    /// (or finishes, on the last one). If all three end up granted — e.g.
    /// the user already had one from an earlier partial run — finishes
    /// immediately, same guarantee the old single-screen version made.
    private fun checkForAutoAdvance() {
        if (allGranted()) {
            finish()
            return
        }
        if (advancing) return
        if (step !in steps.indices) return
        val current = steps[step]
        if (current.isGranted() && !stepEnteredGranted) {
            advancing = true
            playSuccessBeat {
                advancing = false
                if (step < steps.lastIndex) {
                    step++
                    renderStep(animate = true)
                } else {
                    finish()
                }
            }
        }
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
    private val onAccentColor get() = col(0xFFFFFFFF.toInt(), 0xFF0B0E12.toInt())

    // ---------------- Scaffold (static chrome; only the step card swaps) ----------------

    private fun buildScaffold(): View {
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
            text = "Bulbul needs three quick permissions before the floating bubble can work. We'll walk through them one at a time."
            textSize = 14f
            setTextColor(bodyColor)
            setPadding(0, 0, 0, dp(20))
        })

        dotsContainer = LinearLayout(this).apply {
            orientation = LinearLayout.HORIZONTAL
            gravity = Gravity.CENTER_HORIZONTAL
        }
        root.addView(dotsContainer)

        backButton = TextView(this).apply {
            text = "← Back"
            textSize = 13f
            setTypeface(typeface, Typeface.BOLD)
            setTextColor(accentColor)
            setPadding(0, dp(16), 0, dp(4))
            isClickable = true
            isFocusable = true
            visibility = View.GONE
            setOnClickListener {
                if (step > 0) {
                    step--
                    renderStep(animate = true)
                }
            }
        }
        root.addView(backButton)

        contentContainer = FrameLayout(this).apply {
            layoutParams = LinearLayout.LayoutParams(
                ViewGroup.LayoutParams.MATCH_PARENT,
                ViewGroup.LayoutParams.WRAP_CONTENT,
            ).apply { topMargin = dp(8) }
        }
        root.addView(contentContainer)

        return ScrollView(this).apply {
            addView(root, ViewGroup.LayoutParams(
                ViewGroup.LayoutParams.MATCH_PARENT,
                ViewGroup.LayoutParams.WRAP_CONTENT,
            ))
            setBackgroundColor(bgColor)
        }
    }

    // ---------------- Step rendering ----------------

    private fun renderStep(animate: Boolean) {
        val s = steps[step]
        stepEnteredGranted = s.isGranted()
        renderDots()
        backButton.visibility = if (step > 0) View.VISIBLE else View.GONE

        val newContent = buildStepCard(s, stepEnteredGranted)
        val old = contentContainer.getChildAt(0)
        if (!animate || old == null) {
            contentContainer.removeAllViews()
            contentContainer.addView(newContent)
            return
        }
        // Quick slide-and-fade crossfade between steps — no extra deps,
        // just ViewPropertyAnimator. The illustrated per-permission loop
        // each screen should eventually show (see the motion-graphics
        // plan) layers on top of this same transition later.
        old.animate().alpha(0f).translationX(dp(-16).toFloat()).setDuration(140)
            .withEndAction {
                contentContainer.removeAllViews()
                newContent.alpha = 0f
                newContent.translationX = dp(16).toFloat()
                contentContainer.addView(newContent)
                newContent.animate().alpha(1f).translationX(0f).setDuration(200).start()
            }.start()
    }

    private fun renderDots() {
        dotsContainer.removeAllViews()
        for (i in steps.indices) {
            val active = i == step
            val size = if (active) dp(9) else dp(7)
            val dot = View(this).apply {
                background = GradientDrawable().apply {
                    shape = GradientDrawable.OVAL
                    setColor(if (i <= step) accentColor else mutedColor)
                }
            }
            val lp = LinearLayout.LayoutParams(size, size)
            lp.marginStart = dp(4)
            lp.marginEnd = dp(4)
            dot.layoutParams = lp
            dotsContainer.addView(dot)
        }
    }

    private fun buildStepCard(s: PermStep, granted: Boolean): View {
        val card = LinearLayout(this).apply {
            orientation = LinearLayout.VERTICAL
        }
        card.addView(TextView(this).apply {
            text = "Step ${step + 1} of ${steps.size}"
            textSize = 12f
            setTypeface(typeface, Typeface.BOLD)
            setTextColor(mutedColor)
            setPadding(0, 0, 0, dp(6))
        })
        card.addView(TextView(this).apply {
            text = s.title
            textSize = 26f
            setTypeface(typeface, Typeface.BOLD)
            setTextColor(headingColor)
            setPadding(0, 0, 0, dp(10))
        })
        card.addView(TextView(this).apply {
            text = s.blurb
            textSize = 15f
            setTextColor(bodyColor)
            setLineSpacing(dp(3).toFloat(), 1f)
            setPadding(0, 0, 0, dp(24))
        })

        if (granted) {
            card.addView(TextView(this).apply {
                text = "✓ Granted"
                textSize = 15f
                setTypeface(typeface, Typeface.BOLD)
                setTextColor(accentColor)
            })
        } else {
            card.addView(Button(this).apply {
                text = s.actionLabel
                isAllCaps = false
                textSize = 15f
                setTextColor(onAccentColor)
                background = GradientDrawable().apply {
                    setColor(accentColor)
                    cornerRadius = dp(999).toFloat()
                }
                stateListAnimator = null
                setPadding(dp(28), dp(14), dp(28), dp(14))
                setOnClickListener { s.onAction() }
                layoutParams = LinearLayout.LayoutParams(
                    ViewGroup.LayoutParams.WRAP_CONTENT,
                    ViewGroup.LayoutParams.WRAP_CONTENT,
                )
            })
        }

        s.extra?.let { card.addView(it()) }

        return card
    }

    /// A brief native placeholder for "permission granted" before the wizard
    /// auto-advances — a check mark pops in over the card, holds briefly,
    /// then [onDone] fires. This is the MECHANISM only; the illustrated
    /// per-permission motion graphic each screen should eventually carry
    /// (see the onboarding motion-graphics plan) layers on top of this same
    /// hook once those assets exist.
    private fun playSuccessBeat(onDone: () -> Unit) {
        val overlay = TextView(this).apply {
            text = "✓"
            textSize = 48f
            setTypeface(typeface, Typeface.BOLD)
            setTextColor(accentColor)
            gravity = Gravity.CENTER
            alpha = 0f
            scaleX = 0.6f
            scaleY = 0.6f
        }
        contentContainer.addView(
            overlay,
            FrameLayout.LayoutParams(
                ViewGroup.LayoutParams.MATCH_PARENT,
                ViewGroup.LayoutParams.WRAP_CONTENT,
                Gravity.CENTER,
            ),
        )
        overlay.animate().alpha(1f).scaleX(1f).scaleY(1f).setDuration(220)
            .withEndAction {
                overlay.postDelayed({
                    overlay.animate().alpha(0f).setDuration(160).withEndAction {
                        contentContainer.removeView(overlay)
                        onDone()
                    }.start()
                }, 500)
            }.start()
    }

    /// Accent-tinted note explaining the Android 13+ "restricted settings"
    /// block on the Accessibility toggle for sideloaded installs, and the
    /// two ways past it. Brief and step-numbered so a stuck user can act
    /// without leaving the screen to search for an answer.
    private fun buildRestrictedHelpCard(): View {
        val tint = col(0xFFECF6F4.toInt(), 0xFF15201F.toInt())
        val card = LinearLayout(this).apply {
            orientation = LinearLayout.VERTICAL
            setPadding(dp(16), dp(14), dp(16), dp(14))
            background = GradientDrawable().apply {
                setColor(tint)
                cornerRadius = dp(14).toFloat()
                setStroke(dp(1), accentColor)
            }
            layoutParams = LinearLayout.LayoutParams(
                ViewGroup.LayoutParams.MATCH_PARENT,
                ViewGroup.LayoutParams.WRAP_CONTENT,
            ).apply { topMargin = dp(16) }
        }
        card.addView(TextView(this).apply {
            text = "Accessibility greyed out or \"restricted\"?"
            textSize = 14f
            setTypeface(typeface, Typeface.BOLD)
            setTextColor(headingColor)
            setPadding(0, 0, 0, dp(6))
        })
        card.addView(TextView(this).apply {
            text = "Android blocks Accessibility for sideloaded apps.\n\n" +
                "Easiest way: install Bulbul with \"Split APKs Installer\" from the Play Store (you'll watch one short ad) — then you can allow every permission with no blocks.\n\n" +
                "Or do it once manually:\n" +
                "1.  Open App info → tap ⋮ (top-right) → Allow restricted settings.\n" +
                "2.  Come back here and tap \"Open Accessibility settings\" again.\n" +
                "3.  Allow Bulbul, then return to this screen."
            textSize = 13f
            setTextColor(bodyColor)
            setLineSpacing(dp(2).toFloat(), 1f)
        })
        return card
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

        /// Walks Settings.Secure.ENABLED_ACCESSIBILITY_SERVICES for
        /// [serviceClass]. There's no API that just answers "is my service
        /// on?" — the colon-separated string from Settings.Secure is what
        /// every accessibility-using app ends up parsing.
        ///
        /// Compare by ComponentName, NOT raw string. Android stores each
        /// enabled entry in either flattened form — "pkg/pkg.Class" (full)
        /// or "pkg/.Class" (short, leading dot) — and which one lands here
        /// depends on how the service was enabled: the system Settings
        /// toggle writes the SHORT form. A plain string equals against the
        /// full form then misses the short form and reports "not granted"
        /// even though the service is on (seen after a reinstall drops the
        /// grant and the user re-enables via Settings). unflattenFromString
        /// resolves the leading dot to the package, so ComponentName
        /// equality matches both forms.
        fun isAccessibilityServiceEnabled(
            context: Context,
            serviceClass: Class<*>,
        ): Boolean {
            val expected = ComponentName(context.packageName, serviceClass.name)
            val enabled = Settings.Secure.getString(
                context.contentResolver,
                Settings.Secure.ENABLED_ACCESSIBILITY_SERVICES,
            ) ?: return false
            val splitter = TextUtils.SimpleStringSplitter(':')
            splitter.setString(enabled)
            while (splitter.hasNext()) {
                if (ComponentName.unflattenFromString(splitter.next()) == expected) return true
            }
            return false
        }
    }
}
