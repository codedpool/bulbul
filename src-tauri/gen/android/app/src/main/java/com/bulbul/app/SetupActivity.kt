// SPDX-License-Identifier: GPL-3.0-only
// Copyright (c) 2026 Romanch Roshan Singh

// One-screen-at-a-time setup walker.
//
// Shows a one-time hero screen ("Bulbul is faster than typing") before
// asking for anything, then walks three Android grants one per screen,
// easiest first:
//   1. RECORD_AUDIO    — runtime permission, standard system dialog
//   2. SYSTEM_ALERT_WINDOW (overlay) — special permission, must be
//      granted from a Settings screen we open via Intent
//   3. Accessibility   — the user toggles Bulbul on inside
//      Settings → Accessibility; there's no programmatic grant
//
// MainActivity launches this activity if the hero hasn't been seen yet
// OR any of the three permissions are missing. Each permission screen
// auto-advances (with a brief success beat) the moment it detects a
// FRESH grant. It polls in onResume (since two of the three require
// leaving the app) and auto-finishes the instant all three are granted,
// so the user can't end up wedged on a setup screen after they're
// actually done.
//
// Every screen (hero + the three permission screens) shares one
// template: illustration + headline scroll in the middle; a FIXED
// footer (Back + one big primary action) sits below them, outside the
// scrolling area, so it's never pushed off-screen by a tall image —
// see buildScaffold. If the current permission is already granted (the
// user pressed Back to revisit an earlier, completed screen), the
// footer's primary button reads "Continue" instead of the grant action,
// since auto-advance only fires on a FRESH not-granted → granted
// transition and would otherwise never fire again for a revisit,
// leaving no way forward.
//
// This is the native half of a 10-screen onboarding journey that
// continues into the React wizard (name, Groq key, language, overlay
// tuning, dictate test, done) once this activity finishes — see
// onboarding-motion-plan.md for the full sequence.
//
// UI is intentionally built in code — no XML layout — to keep this
// flow self-contained and to avoid one more file in the gen tree that
// has to be force-added.

package com.bulbul.app

import android.Manifest
import android.app.Activity
import android.content.ComponentName
import android.content.Context
import android.content.Intent
import android.content.pm.PackageManager
import android.graphics.Typeface
import android.os.Build
import android.widget.ImageView
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

    /// Which of [steps] is on screen, once past the hero. Advances only
    /// when that step's permission is freshly granted (see
    /// [checkForAutoAdvance]) or the footer's "Continue" is tapped on an
    /// already-granted revisit (see [advanceManually]).
    private var step = 0

    /// True until the one-time hero screen has been dismissed via its
    /// own "Get started" button. Gates whether onCreate shows the hero
    /// or jumps straight to the first missing permission.
    private var showingHero = false

    /// Snapshot of whether the current step was ALREADY granted at the
    /// moment it was rendered — the auto-advance trigger is the
    /// not-granted-to-granted transition, not "is granted", so that
    /// paging Back to an already-done step doesn't immediately bounce
    /// forward again (it shows a "Continue" button instead — see
    /// [updateFooterForStep]).
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

    private lateinit var progressContainer: LinearLayout
    private lateinit var scrollContent: FrameLayout
    private lateinit var footerBack: TextView
    private lateinit var footerPrimary: Button

    private data class PermStep(
        val title: String,
        val blurb: String,
        val actionLabel: String,
        val onAction: () -> Unit,
        val isGranted: () -> Boolean,
        val imageRes: Int,
        val extra: (() -> View)? = null,
    )

    private val steps: List<PermStep> by lazy {
        listOf(
            PermStep(
                title = "Allow microphone access",
                blurb = "Used only while you hold or tap the floating bubble.",
                actionLabel = "Allow microphone",
                onAction = ::requestMic,
                isGranted = ::micGranted,
                imageRes = R.drawable.onboard_mic,
            ),
            PermStep(
                title = "Display over other apps",
                blurb = "Lets the floating bubble appear above your keyboard in any app.",
                actionLabel = "Open Display settings",
                onAction = ::openOverlaySettings,
                isGranted = ::overlayGranted,
                imageRes = R.drawable.onboard_overlay,
            ),
            PermStep(
                title = "Turn on Accessibility",
                blurb = "Lets Bulbul see which text field you tapped into and paste cleaned-up transcripts there.",
                actionLabel = "Open Accessibility settings",
                onAction = ::openAccessibilitySettings,
                isGranted = ::accessibilityGranted,
                imageRes = R.drawable.onboard_accessibility,
            ),
        )
    }

    override fun onCreate(savedInstanceState: Bundle?) {
        super.onCreate(savedInstanceState)
        setContentView(buildScaffold())
        if (!heroSeen()) {
            showingHero = true
            renderHero(animate = false)
            return
        }
        enterPermissionFlow(animate = false)
    }

    /// Computes the first ungranted permission and either renders it or,
    /// if nothing is missing, finishes the activity outright. Called once
    /// from onCreate (hero already seen) and once from the hero's own
    /// "Get started" button.
    private fun enterPermissionFlow(animate: Boolean) {
        showingHero = false
        val firstMissing = steps.indexOfFirst { !it.isGranted() }
        if (firstMissing < 0) {
            finish()
            return
        }
        step = firstMissing
        renderStep(animate = animate)
    }

    override fun onResume() {
        super.onResume()
        if (!showingHero) checkForAutoAdvance()
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
    /// either the hero hasn't been dismissed yet or at least one permission
    /// is still missing (allGranted() already finishes the Activity
    /// everywhere else), so exiting via system Back would just reveal
    /// MainActivity's onboarding underneath with setup incomplete. Per
    /// product decision: nothing proceeds until all three are granted. This
    /// only guards the Activity-level exit — moving BACKWARD between
    /// already-visited screens (the footer's own Back button) is always
    /// allowed, since that can't un-grant anything.
    @Deprecated("Deprecated in Java")
    override fun onBackPressed() {
        if (showingHero) {
            super.onBackPressed()
            return
        }
        Toast.makeText(
            this,
            "Please finish granting these permissions to continue.",
            Toast.LENGTH_SHORT,
        ).show()
    }

    // ---------------- Hero (once-only) state ----------------

    private fun heroPrefs() = getSharedPreferences("bulbul_setup", MODE_PRIVATE)

    private fun heroSeen(): Boolean = heroPrefs().getBoolean("hero_seen", false)

    private fun markHeroSeen() {
        heroPrefs().edit().putBoolean("hero_seen", true).apply()
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

    /// The only way forward from a step the user reached via Back that's
    /// ALREADY granted: auto-advance (above) only fires on a fresh
    /// not-granted → granted transition, which a mere revisit never
    /// triggers, so without this the footer's "Continue" button would be
    /// the only escape from an otherwise dead-end screen.
    private fun advanceManually() {
        if (step < steps.lastIndex) {
            step++
            renderStep(animate = true)
        } else {
            finish()
        }
    }

    // ---------------- Theme ----------------
    //
    // All ten onboarding screens (this activity's four, plus the six that
    // follow in the React wizard) share one set of illustrations generated
    // specifically for Bulbul's LIGHT theme. Forcing light here — rather
    // than following the system/app theme like the rest of the app does —
    // is deliberate: a dark-mode device would otherwise show a jarring
    // light-background image inside an otherwise-dark screen. This is a
    // one-time setup flow, not a surface the user lives in, so a fixed
    // light presentation is the right trade.
    private val bgColor = 0xFFEEF0F3.toInt()
    private val headingColor = 0xFF181B21.toInt()
    private val bodyColor = 0xFF5A6371.toInt()
    private val mutedColor = 0xFF7D8693.toInt()
    private val accentColor = 0xFF1F8C82.toInt()
    private val accentFillColor = 0xFF5EC8C0.toInt()
    private val onAccentColor = 0xFFFFFFFF.toInt()

    // ---------------- Scaffold ----------------
    //
    // Three fixed vertical zones: header (brand + progress, never
    // scrolls), a ScrollView that takes whatever space is left and
    // scrolls ONLY the illustration + headline, and a footer (Back +
    // one big primary button) pinned below it — never inside the
    // scrolling area, so a tall illustration can never push the action
    // button out of reach the way a single all-in-one ScrollView did.

    private fun buildScaffold(): View {
        val root = LinearLayout(this).apply {
            orientation = LinearLayout.VERTICAL
            setPadding(dp(20), dp(32), dp(20), dp(16))
            setBackgroundColor(bgColor)
            layoutParams = ViewGroup.LayoutParams(
                ViewGroup.LayoutParams.MATCH_PARENT,
                ViewGroup.LayoutParams.MATCH_PARENT,
            )
        }

        // Brand header — launcher icon + wordmark, so setup feels on-brand.
        val brand = LinearLayout(this).apply {
            orientation = LinearLayout.HORIZONTAL
            gravity = Gravity.CENTER_VERTICAL
            setPadding(0, 0, 0, dp(14))
        }
        brand.addView(ImageView(this).apply {
            setImageResource(R.mipmap.ic_launcher)
            layoutParams = LinearLayout.LayoutParams(dp(32), dp(32)).apply {
                rightMargin = dp(10)
            }
        })
        brand.addView(TextView(this).apply {
            text = "bulbul"
            textSize = 20f
            setTypeface(Typeface.create(Typeface.SERIF, Typeface.ITALIC))
            setTextColor(headingColor)
        })
        root.addView(brand)

        // Continuous progress across ALL ten onboarding screens (this
        // activity's four, then six more in the React wizard) — one bar
        // the user reads as a single journey, not two wizards each
        // restarting their own counter.
        progressContainer = LinearLayout(this).apply {
            orientation = LinearLayout.HORIZONTAL
            layoutParams = LinearLayout.LayoutParams(
                ViewGroup.LayoutParams.MATCH_PARENT,
                dp(3),
            ).apply { bottomMargin = dp(4) }
        }
        root.addView(progressContainer)

        scrollContent = FrameLayout(this).apply {
            layoutParams = FrameLayout.LayoutParams(
                ViewGroup.LayoutParams.MATCH_PARENT,
                ViewGroup.LayoutParams.WRAP_CONTENT,
            )
        }
        val scroll = ScrollView(this).apply {
            isFillViewport = true
            addView(scrollContent)
            layoutParams = LinearLayout.LayoutParams(
                ViewGroup.LayoutParams.MATCH_PARENT,
                0,
                1f,
            ).apply { topMargin = dp(8) }
        }
        root.addView(scroll)

        // Fixed footer — always visible regardless of scroll position or
        // how tall the illustration renders. Back is compact; the primary
        // button fills whatever width is left (the whole row, when Back
        // is hidden on the hero and on the first permission screen).
        val footer = LinearLayout(this).apply {
            orientation = LinearLayout.HORIZONTAL
            gravity = Gravity.CENTER_VERTICAL
            layoutParams = LinearLayout.LayoutParams(
                ViewGroup.LayoutParams.MATCH_PARENT,
                ViewGroup.LayoutParams.WRAP_CONTENT,
            ).apply { topMargin = dp(12) }
        }
        footerBack = TextView(this).apply {
            text = "← Back"
            textSize = 14f
            setTypeface(typeface, Typeface.BOLD)
            setTextColor(accentColor)
            setPadding(dp(4), dp(18), dp(16), dp(18))
            isClickable = true
            isFocusable = true
            visibility = View.GONE
            setOnClickListener {
                if (!showingHero && step > 0) {
                    step--
                    renderStep(animate = true)
                }
            }
        }
        footer.addView(footerBack)
        footerPrimary = bigButton("") {}
        footer.addView(footerPrimary, LinearLayout.LayoutParams(
            0,
            ViewGroup.LayoutParams.WRAP_CONTENT,
            1f,
        ))
        root.addView(footer)

        return root
    }

    /// 1-based position of whatever's on screen right now within the full
    /// ten-screen journey (hero=1, the three permissions=2..4 — the React
    /// wizard's name/Groq/language/overlay/dictate-test/done steps continue
    /// 5..10 on the other side of this activity finishing).
    private fun globalStepNumber(): Int = if (showingHero) 1 else step + 2

    private fun renderProgress() {
        progressContainer.removeAllViews()
        val current = globalStepNumber()
        for (i in 1..TOTAL_ONBOARDING_STEPS) {
            val seg = View(this).apply {
                background = GradientDrawable().apply {
                    cornerRadius = dp(2).toFloat()
                    setColor(if (i <= current) accentFillColor else 0xFFD4D8E0.toInt())
                }
            }
            val lp = LinearLayout.LayoutParams(0, dp(3), 1f)
            lp.marginStart = if (i > 1) dp(3) else 0
            seg.layoutParams = lp
            progressContainer.addView(seg)
        }
    }

    // ---------------- Hero screen ----------------

    private fun renderHero(animate: Boolean) {
        renderProgress()
        footerBack.visibility = View.GONE
        footerPrimary.text = "Get started"
        footerPrimary.setOnClickListener {
            markHeroSeen()
            enterPermissionFlow(animate = true)
        }
        swapScrollContent(buildHeroIllustration(), animate)
    }

    private fun buildHeroIllustration(): View {
        val card = FrameLayout(this)
        card.addView(ImageView(this).apply {
            setImageResource(R.drawable.onboard_hero)
            adjustViewBounds = true
            scaleType = ImageView.ScaleType.FIT_CENTER
            layoutParams = FrameLayout.LayoutParams(
                ViewGroup.LayoutParams.MATCH_PARENT,
                ViewGroup.LayoutParams.WRAP_CONTENT,
            )
        })
        card.addView(TextView(this).apply {
            text = "Bulbul is faster than typing"
            textSize = 25f
            setTypeface(typeface, Typeface.BOLD)
            setTextColor(headingColor)
            gravity = Gravity.CENTER
            layoutParams = FrameLayout.LayoutParams(
                ViewGroup.LayoutParams.MATCH_PARENT,
                ViewGroup.LayoutParams.WRAP_CONTENT,
                Gravity.TOP,
            ).apply { topMargin = dp(28) }
        })
        return card
    }

    // ---------------- Step rendering ----------------

    private fun renderStep(animate: Boolean) {
        val s = steps[step]
        stepEnteredGranted = s.isGranted()
        renderProgress()
        footerBack.visibility = if (step > 0) View.VISIBLE else View.GONE
        updateFooterForStep(s, stepEnteredGranted)
        swapScrollContent(buildStepIllustration(s), animate)
    }

    /// Sets the footer's primary button for the CURRENT step. Granted
    /// (a fresh grant, or a revisit via Back) always shows "Continue" so
    /// there's never a dead end; not-yet-granted shows the actual grant
    /// action.
    private fun updateFooterForStep(s: PermStep, granted: Boolean) {
        if (granted) {
            footerPrimary.text = "Continue →"
            footerPrimary.setOnClickListener { advanceManually() }
        } else {
            footerPrimary.text = s.actionLabel
            footerPrimary.setOnClickListener { s.onAction() }
        }
    }

    /// Quick slide-and-fade crossfade — no extra deps, just
    /// ViewPropertyAnimator. Shared by permission-to-permission transitions
    /// and the one hero-to-first-permission transition.
    private fun swapScrollContent(newContent: View, animate: Boolean) {
        if (!animate) {
            scrollContent.removeAllViews()
            scrollContent.addView(newContent)
            return
        }
        val old = scrollContent.getChildAt(0)
        if (old == null) {
            scrollContent.addView(newContent)
            return
        }
        old.animate().alpha(0f).translationX(dp(-16).toFloat()).setDuration(140)
            .withEndAction {
                scrollContent.removeAllViews()
                newContent.alpha = 0f
                newContent.translationX = dp(16).toFloat()
                scrollContent.addView(newContent)
                newContent.animate().alpha(1f).translationX(0f).setDuration(200).start()
            }.start()
    }

    private fun buildStepIllustration(s: PermStep): View {
        val card = FrameLayout(this)

        card.addView(ImageView(this).apply {
            setImageResource(s.imageRes)
            adjustViewBounds = true
            scaleType = ImageView.ScaleType.FIT_CENTER
            layoutParams = FrameLayout.LayoutParams(
                ViewGroup.LayoutParams.MATCH_PARENT,
                ViewGroup.LayoutParams.WRAP_CONTENT,
            )
        })

        val topText = LinearLayout(this).apply {
            orientation = LinearLayout.VERTICAL
            layoutParams = FrameLayout.LayoutParams(
                ViewGroup.LayoutParams.MATCH_PARENT,
                ViewGroup.LayoutParams.WRAP_CONTENT,
                Gravity.TOP,
            ).apply { topMargin = dp(24) }
        }
        topText.addView(TextView(this).apply {
            text = "Step ${globalStepNumber()} of $TOTAL_ONBOARDING_STEPS"
            textSize = 12f
            setTypeface(typeface, Typeface.BOLD)
            setTextColor(mutedColor)
            gravity = Gravity.CENTER
            setPadding(0, 0, 0, dp(4))
        })
        topText.addView(TextView(this).apply {
            text = s.title
            textSize = 22f
            setTypeface(typeface, Typeface.BOLD)
            setTextColor(headingColor)
            gravity = Gravity.CENTER
            setPadding(dp(12), 0, dp(12), dp(6))
        })
        topText.addView(TextView(this).apply {
            text = s.blurb
            textSize = 14f
            setTextColor(bodyColor)
            gravity = Gravity.CENTER
            setLineSpacing(dp(2).toFloat(), 1f)
            setPadding(dp(20), 0, dp(20), 0)
        })
        card.addView(topText)

        val outer = LinearLayout(this).apply { orientation = LinearLayout.VERTICAL }
        outer.addView(card)
        s.extra?.let { outer.addView(it()) }
        return outer
    }

    /// Full-width, generously-padded pill button — the "big button" style
    /// used throughout this journey (matching the reference onboarding
    /// flow this redesign was modeled on). Created once for the footer's
    /// persistent primary slot; callers mutate its text/click-listener
    /// per screen rather than recreating it, so its position never moves.
    private fun bigButton(label: String, onClick: () -> Unit): Button =
        Button(this).apply {
            text = label
            isAllCaps = false
            textSize = 17f
            setTypeface(typeface, Typeface.BOLD)
            setTextColor(onAccentColor)
            background = GradientDrawable().apply {
                setColor(accentColor)
                cornerRadius = dp(999).toFloat()
            }
            stateListAnimator = null
            setPadding(dp(28), dp(18), dp(28), dp(18))
            setOnClickListener { onClick() }
        }

    /// A brief native placeholder for "permission granted" before the wizard
    /// auto-advances — a check mark pops in over the scrollable content,
    /// holds briefly, then [onDone] fires.
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
        scrollContent.addView(
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
                        scrollContent.removeView(overlay)
                        onDone()
                    }.start()
                }, 500)
            }.start()
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

        /// Static read of the same "hero seen" flag [heroSeen] checks from
        /// an Activity instance — MainActivity needs this too, to decide
        /// whether to launch the walker even when all permissions already
        /// happen to be granted (e.g. a reinstall that retained grants but
        /// never showed the hero screen).
        fun heroSeenStatic(context: Context): Boolean =
            context.getSharedPreferences("bulbul_setup", Context.MODE_PRIVATE)
                .getBoolean("hero_seen", false)

        /// Total screens across the whole onboarding journey: this
        /// activity's hero + 3 permissions, then the React wizard's name,
        /// Groq key, language, overlay tuning, dictate test, and done — 10
        /// in all. Kept as a plain shared constant (not literally synced
        /// across the native/webview boundary) so both halves number
        /// consistently; see onboarding-motion-plan.md. Must match
        /// GLOBAL_TOTAL_STEPS in OnboardingWizard.jsx.
        const val TOTAL_ONBOARDING_STEPS = 10

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
