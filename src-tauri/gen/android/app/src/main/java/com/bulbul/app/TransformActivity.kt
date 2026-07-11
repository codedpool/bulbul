// The text-selection transform sheet.
//
// This is the Android-native answer to "let me transform selected text
// without leaving the app I'm in." When the user selects text in ANY
// app, the system floating toolbar (Copy / Cut / Paste / …) shows a
// "Bulbul" entry — that's this activity's PROCESS_TEXT intent-filter
// (see AndroidManifest.xml). Tapping it hands us the selected text.
//
// Crucially this is NOT a full-screen page: the activity uses a
// translucent theme (Theme.bulbul.Transparent) so the app underneath
// stays visible, and we draw a bottom sheet over it — the same shape as
// the system share sheet. The user picks a transform, we run it through
// Groq, and hand the result back via setResult(EXTRA_PROCESS_TEXT). The
// OS then replaces the selection in place. No screen change, ≤2 taps,
// and — unlike the dictation bubble — it never touches the
// AccessibilityService injection path, so it sidesteps that whole class
// of "wrong field / hint text leaked" bugs.
//
// UI is built in code (no XML layout), matching SetupActivity — keeps
// the flow self-contained and avoids force-adding layout files to the
// gen tree.

package com.bulbul.app

import android.animation.ObjectAnimator
import android.app.Activity
import android.content.ClipData
import android.content.ClipboardManager
import android.content.Context
import android.content.Intent
import android.content.res.ColorStateList
import android.content.res.Configuration
import android.graphics.Typeface
import android.graphics.drawable.ColorDrawable
import android.graphics.drawable.Drawable
import android.graphics.drawable.GradientDrawable
import android.os.Bundle
import android.text.TextUtils
import android.util.TypedValue
import android.view.Gravity
import android.view.View
import android.view.ViewGroup
import android.widget.FrameLayout
import android.widget.LinearLayout
import android.widget.ProgressBar
import android.widget.ScrollView
import android.widget.TextView
import android.widget.Toast
import kotlin.concurrent.thread

class TransformActivity : Activity() {

    private var selectedText: CharSequence = ""
    private var readOnly = false
    private var busy = false

    private lateinit var sheet: LinearLayout
    private lateinit var listContainer: LinearLayout

    private val night: Boolean
        get() = (resources.configuration.uiMode and Configuration.UI_MODE_NIGHT_MASK) ==
            Configuration.UI_MODE_NIGHT_YES

    override fun onCreate(savedInstanceState: Bundle?) {
        super.onCreate(savedInstanceState)

        if (intent?.action != Intent.ACTION_PROCESS_TEXT) {
            finish(); return
        }
        selectedText = intent.getCharSequenceExtra(Intent.EXTRA_PROCESS_TEXT) ?: ""
        readOnly = intent.getBooleanExtra(Intent.EXTRA_PROCESS_TEXT_READONLY, false)

        if (selectedText.isBlank()) {
            toast("Select some text to transform")
            finish(); return
        }

        setContentView(buildRoot())
        animateIn()
    }

    override fun onBackPressed() = dismiss()

    // ---------------- Layout ----------------

    private fun buildRoot(): View {
        val root = FrameLayout(this).apply {
            setBackgroundColor(if (night) 0x99000000.toInt() else 0x66000000)
            // Tapping the scrim (anywhere outside the sheet) cancels.
            setOnClickListener { dismiss() }
        }

        sheet = LinearLayout(this).apply {
            orientation = LinearLayout.VERTICAL
            background = GradientDrawable().apply {
                setColor(c(0xFFFFFFFF.toInt(), 0xFF191C22.toInt()))
                // Rounded top corners only — it's anchored to the bottom edge.
                cornerRadii = floatArrayOf(
                    dp(20f), dp(20f), dp(20f), dp(20f), 0f, 0f, 0f, 0f,
                )
            }
            setPadding(0, dp(10f).toInt(), 0, dp(14f).toInt())
            // Swallow touches so a tap on the sheet doesn't fall through to
            // the scrim's dismiss handler.
            isClickable = true
            layoutParams = FrameLayout.LayoutParams(
                ViewGroup.LayoutParams.MATCH_PARENT,
                ViewGroup.LayoutParams.WRAP_CONTENT,
                Gravity.BOTTOM,
            )
        }

        // Grabber handle.
        sheet.addView(View(this).apply {
            background = GradientDrawable().apply {
                setColor(c(0xFFCBD5E1.toInt(), 0xFF3A3F4B.toInt()))
                cornerRadius = dp(3f)
            }
            layoutParams = LinearLayout.LayoutParams(dp(36f).toInt(), dp(5f).toInt()).apply {
                gravity = Gravity.CENTER_HORIZONTAL
                bottomMargin = dp(10f).toInt()
            }
        })

        // Title.
        sheet.addView(TextView(this).apply {
            text = "Transform with Bulbul"
            textSize = 16f
            setTypeface(typeface, Typeface.BOLD)
            setTextColor(c(0xFF0F172A.toInt(), 0xFFF1F5F9.toInt()))
            setPadding(dp(20f).toInt(), 0, dp(20f).toInt(), dp(6f).toInt())
        })

        // Selected-text preview.
        sheet.addView(TextView(this).apply {
            val trimmed = selectedText.toString().trim()
            text = "“" + (if (trimmed.length > 120) trimmed.take(120) + "…" else trimmed) + "”"
            textSize = 13f
            maxLines = 2
            ellipsize = TextUtils.TruncateAt.END
            setTextColor(c(0xFF64748B.toInt(), 0xFF94A3B8.toInt()))
            setPadding(dp(20f).toInt(), 0, dp(20f).toInt(), dp(10f).toInt())
        })

        listContainer = LinearLayout(this).apply { orientation = LinearLayout.VERTICAL }
        Transforms.ALL.forEach { listContainer.addView(transformRow(it)) }

        sheet.addView(ScrollView(this).apply {
            addView(listContainer)
            isVerticalScrollBarEnabled = false
        })

        root.addView(sheet)
        return root
    }

    private fun transformRow(t: Transforms.Transform): View {
        val row = LinearLayout(this).apply {
            orientation = LinearLayout.HORIZONTAL
            gravity = Gravity.CENTER_VERTICAL
            setPadding(dp(20f).toInt(), dp(13f).toInt(), dp(20f).toInt(), dp(13f).toInt())
            background = rippleBackground()
            isClickable = true
            setOnClickListener { if (!busy) runTransform(t) }
        }

        // Round accent badge with the transform's initial.
        row.addView(TextView(this).apply {
            text = t.name.take(1).uppercase()
            gravity = Gravity.CENTER
            textSize = 15f
            setTypeface(typeface, Typeface.BOLD)
            setTextColor(accent())
            background = GradientDrawable().apply {
                shape = GradientDrawable.OVAL
                setColor(c(0x1412A594, 0x265EC8C0))
            }
            layoutParams = LinearLayout.LayoutParams(dp(38f).toInt(), dp(38f).toInt()).apply {
                rightMargin = dp(14f).toInt()
            }
        })

        val texts = LinearLayout(this).apply {
            orientation = LinearLayout.VERTICAL
            layoutParams = LinearLayout.LayoutParams(0, ViewGroup.LayoutParams.WRAP_CONTENT, 1f)
        }
        texts.addView(TextView(this).apply {
            text = t.name
            textSize = 15f
            setTypeface(typeface, Typeface.BOLD)
            setTextColor(c(0xFF0F172A.toInt(), 0xFFF1F5F9.toInt()))
        })
        texts.addView(TextView(this).apply {
            text = t.description
            textSize = 12f
            setTextColor(c(0xFF64748B.toInt(), 0xFF94A3B8.toInt()))
            setPadding(0, dp(2f).toInt(), 0, 0)
        })
        row.addView(texts)

        return row
    }

    // ---------------- Run + return ----------------

    private fun runTransform(t: Transforms.Transform) {
        busy = true
        showLoading(t)
        val input = selectedText.toString()
        thread(name = "BulbulTransform", isDaemon = true) {
            val apiKey = BulbulConfig.apiKey(this)
            if (apiKey.isBlank()) {
                runOnUiThread {
                    toast("Add your Groq API key in Bulbul settings")
                    finish()
                }
                return@thread
            }
            val out = GroqClient.chat(apiKey, t.prompt, input, BulbulConfig.chatModel(this))
            runOnUiThread {
                if (out.isNullOrBlank()) {
                    toast("Transform failed — check your connection")
                    finish()
                } else {
                    returnResult(out)
                }
            }
        }
    }

    private fun returnResult(text: String) {
        if (readOnly) {
            // The originating field is read-only, so the OS won't accept a
            // replacement — hand the result back on the clipboard instead so
            // it isn't lost.
            val cm = getSystemService(Context.CLIPBOARD_SERVICE) as ClipboardManager
            cm.setPrimaryClip(ClipData.newPlainText("Bulbul", text))
            toast("Transformed text copied")
            setResult(RESULT_CANCELED)
        } else {
            setResult(RESULT_OK, Intent().putExtra(Intent.EXTRA_PROCESS_TEXT, text as CharSequence))
        }
        finish()
    }

    private fun showLoading(t: Transforms.Transform) {
        listContainer.removeAllViews()
        listContainer.addView(LinearLayout(this).apply {
            orientation = LinearLayout.HORIZONTAL
            gravity = Gravity.CENTER_VERTICAL
            setPadding(dp(20f).toInt(), dp(18f).toInt(), dp(20f).toInt(), dp(18f).toInt())
            addView(ProgressBar(this@TransformActivity).apply {
                isIndeterminate = true
                indeterminateTintList = ColorStateList.valueOf(accent())
                layoutParams = LinearLayout.LayoutParams(dp(24f).toInt(), dp(24f).toInt()).apply {
                    rightMargin = dp(14f).toInt()
                }
            })
            addView(TextView(this@TransformActivity).apply {
                text = "${t.name}…"
                textSize = 15f
                setTextColor(c(0xFF0F172A.toInt(), 0xFFF1F5F9.toInt()))
            })
        })
    }

    private fun dismiss() {
        if (busy) return
        setResult(RESULT_CANCELED)
        finish()
    }

    // ---------------- Helpers ----------------

    private fun animateIn() {
        sheet.post {
            sheet.translationY = sheet.height.toFloat()
            ObjectAnimator.ofFloat(sheet, "translationY", 0f).apply {
                duration = 220
                start()
            }
        }
    }

    private fun toast(msg: String) = Toast.makeText(this, msg, Toast.LENGTH_SHORT).show()

    private fun c(light: Int, dark: Int): Int = if (night) dark else light

    private fun accent(): Int = c(0xFF12A594.toInt(), 0xFF5EC8C0.toInt())

    private fun dp(v: Float): Float =
        TypedValue.applyDimension(TypedValue.COMPLEX_UNIT_DIP, v, resources.displayMetrics)

    /// Native press ripple pulled from the platform theme so rows feel
    /// like standard list items. Fetched per-row because a Drawable
    /// instance can't be shared across views.
    private fun rippleBackground(): Drawable {
        val ta = obtainStyledAttributes(intArrayOf(android.R.attr.selectableItemBackground))
        val d = ta.getDrawable(0)
        ta.recycle()
        return d ?: ColorDrawable(0x00000000)
    }
}
