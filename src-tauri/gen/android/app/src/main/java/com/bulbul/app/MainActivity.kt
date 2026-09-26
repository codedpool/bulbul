// SPDX-License-Identifier: GPL-3.0-only
// Copyright (c) 2026 Romanch Roshan Singh

package com.bulbul.app

import android.Manifest
import android.content.Intent
import android.content.pm.PackageManager
import android.content.res.Configuration
import android.os.Bundle
import android.os.FileObserver
import android.os.Handler
import android.os.Looper
import android.provider.Settings
import android.util.Log
import android.view.Gravity
import android.view.View
import android.view.ViewGroup
import android.webkit.WebView
import android.widget.Button
import android.widget.FrameLayout
import android.widget.LinearLayout
import android.widget.TextView
import androidx.activity.enableEdgeToEdge
import androidx.core.content.ContextCompat
import androidx.core.view.ViewCompat
import androidx.core.view.WindowCompat
import androidx.core.view.WindowInsetsCompat
import java.io.File

class MainActivity : TauriActivity() {
  private var configObserver: FileObserver? = null
  private var permissionBanner: View? = null
  private var webView: WebView? = null

  override fun onCreate(savedInstanceState: Bundle?) {
    enableEdgeToEdge()
    super.onCreate(savedInstanceState)
    // The app draws edge-to-edge behind the status/nav bars, so the
    // system icon colour must track the *in-app* theme (which the user
    // controls independently of the OS theme). Without this, light mode
    // over a dark OS theme leaves white status icons invisible on the
    // app's white top — the "notification bar hidden in the whites" bug.
    applyBarAppearance()
    startConfigWatch()
    // Opt-in engagement ping (no content). No-ops unless telemetry is on.
    Telemetry.track(this, "app_opened", org.json.JSONObject())
    // Launch the setup walker on top of the Tauri webview if any of
    // the three system permissions Bulbul depends on aren't granted.
    // Doing this in onCreate (not onResume) means the webview keeps
    // loading underneath — by the time the user finishes setup the
    // dashboard is ready, so they don't see a second loading spinner.
    //
    // First launch only: if permissions are missing, open the full setup
    // walker. Later returns — backing out of setup, or a permission revoked
    // while the app runs — are handled more gently in onResume via a
    // dismissible in-app banner (see showPermissionBanner) rather than
    // force-reopening SetupActivity, so the user is never trapped but also
    // never left on a silently non-functional app.
    if (!hasAllPermissions() || !SetupActivity.heroSeenStatic(this)) {
      startActivity(Intent(this, SetupActivity::class.java))
    }
  }

  /// WryActivity's hook for when the Tauri webview is created (see
  /// WryActivity.setWebView in the vendored wry source) — the only way to
  /// get a reference to it, since it's a private field on the base class.
  ///
  /// This exists to fix the bottom nav bar's spacing. CSS env(safe-area-
  /// inset-bottom) is unreliable in this WebView (see the .m-tabbar CSS
  /// comment) — RustWebView does no inset handling of its own at all — so
  /// the bottom padding was a flat guessed value. That guess can't match
  /// every device: gesture nav's real reserved strip is usually smaller
  /// than the guess (wasted gap), while some 3-button-nav devices reserve
  /// MORE than the guess (the system bar overlapping the app's own tab
  /// bar). Reading the real navigationBars() inset here and pushing it
  /// into the page as a CSS variable — which the OS re-delivers on its own
  /// whenever it actually changes (rotation, switching nav mode in
  /// Settings) — replaces the guess with the true per-device, per-mode
  /// value.
  override fun onWebViewCreate(webView: WebView) {
    super.onWebViewCreate(webView)
    this.webView = webView
    ViewCompat.setOnApplyWindowInsetsListener(webView) { v, insets ->
      applyNavInset(v, insets)
      insets
    }
    // This first request usually lands before the window has actually
    // settled (the webview is inserted into the hierarchy mid-setup) and
    // reports 0 for every inset — nothing then triggers a further one on
    // its own, since a fresh dispatch normally needs a real change (focus,
    // rotation, IME) to fire again. onWindowFocusChanged below forces the
    // real read once the window has genuinely settled.
    ViewCompat.requestApplyInsets(webView)
  }

  override fun onWindowFocusChanged(hasFocus: Boolean) {
    super.onWindowFocusChanged(hasFocus)
    if (!hasFocus) return
    // Querying the webview's own dispatched insets never produced a real
    // (non-zero) value — something in Wry's view hierarchy between the
    // decor view and the webview consumes them first. window.decorView is
    // the true root the system delivers insets to before any app view gets
    // a chance to touch them, so read from there directly instead of
    // relying on a dispatch reaching the webview.
    ViewCompat.getRootWindowInsets(window.decorView)?.let { applyNavInset(window.decorView, it) }
  }

  private fun applyNavInset(view: View, insets: WindowInsetsCompat) {
    val bottomPx = insets.getInsets(WindowInsetsCompat.Type.navigationBars()).bottom
    // WebView content renders in CSS px, which line up with dp (not raw
    // device pixels) once a standard "width=device-width" viewport is set
    // (see index.html) — same conversion applyBarAppearance's sibling code
    // already relies on elsewhere in this file.
    val bottomDp = bottomPx / view.resources.displayMetrics.density
    val js = "document.documentElement.style.setProperty('--android-nav-inset', '${bottomDp}px')"
    Log.d("BulbulNavInset", "navigationBars bottom=${bottomPx}px -> ${bottomDp}dp")
    webView?.evaluateJavascript(js, null)
  }

  override fun onResume() {
    super.onResume()
    // Catch a theme change made while we were backgrounded (or an OS
    // theme flip when the app is set to "system").
    applyBarAppearance()
    // Re-check permissions on every return to the foreground. If any of the
    // three Bulbul needs is missing — the user backed out of setup, or
    // revoked one in system settings while the app ran — surface a
    // dismissible banner offering a one-tap jump back to setup, rather than
    // auto-relaunching SetupActivity (which would trap a user just looking
    // around) or leaving the app silently non-functional.
    if (hasAllPermissions()) hidePermissionBanner() else showPermissionBanner()
  }

  override fun onDestroy() {
    configObserver?.stopWatching()
    configObserver = null
    super.onDestroy()
  }

  /// Resolves the app's effective theme to dark/light. "system" defers to
  /// the current OS night-mode; explicit prefs win.
  private fun isAppDark(): Boolean = when (BulbulConfig.theme(this)) {
    "dark" -> true
    "light" -> false
    else -> (resources.configuration.uiMode and Configuration.UI_MODE_NIGHT_MASK) ==
      Configuration.UI_MODE_NIGHT_YES
  }

  /// Light app background → dark system-bar icons, and vice-versa.
  private fun applyBarAppearance() {
    val dark = isAppDark()
    val controller = WindowCompat.getInsetsController(window, window.decorView)
    controller.isAppearanceLightStatusBars = !dark
    controller.isAppearanceLightNavigationBars = !dark
  }

  /// Watches config.json for writes so an in-app theme toggle (which the
  /// React side persists via save_config) updates the bar icons live —
  /// no wait for the next resume. Watches the directory rather than the
  /// file so it still fires if config.json is created after us.
  private fun startConfigWatch() {
    val dir = BulbulConfig.dataDir(this)
    val handler = Handler(Looper.getMainLooper())
    @Suppress("DEPRECATION")
    configObserver = object : FileObserver(dir.absolutePath, CLOSE_WRITE or MODIFY) {
      override fun onEvent(event: Int, path: String?) {
        if (path == "config.json") handler.post { applyBarAppearance() }
      }
    }.also { it.startWatching() }
  }

  /// Dismissible in-app banner shown at the top of the webview when a
  /// required permission is missing. Offers a one-tap "Fix" into the setup
  /// walker; "✕" dismisses it for now (it returns on the next resume if the
  /// permission is still missing). Built in code rather than XML so it
  /// doesn't depend on a layout resource in the generated Android project.
  private fun showPermissionBanner() {
    if (permissionBanner != null) return
    val dark = isAppDark()
    val density = resources.displayMetrics.density
    fun dp(v: Int): Int = (v * density).toInt()

    val row = LinearLayout(this).apply {
      orientation = LinearLayout.HORIZONTAL
      gravity = Gravity.CENTER_VERTICAL
      setBackgroundColor(if (dark) 0xFF3A2E1A.toInt() else 0xFFFDECC8.toInt())
      setPadding(dp(16), dp(12), dp(8), dp(12))
      // Under edge-to-edge the banner would slide under the status bar, so
      // pad its top by the status-bar inset to keep the text clear of it.
      ViewCompat.setOnApplyWindowInsetsListener(this) { v, insets ->
        val top = insets.getInsets(WindowInsetsCompat.Type.statusBars()).top
        v.setPadding(v.paddingLeft, dp(12) + top, v.paddingRight, v.paddingBottom)
        insets
      }
    }

    val label = TextView(this).apply {
      text = "Bulbul needs ${missingPermissionsLabel()} to work."
      setTextColor(if (dark) 0xFFF1E9D8.toInt() else 0xFF3A2E1A.toInt())
      textSize = 14f
      layoutParams = LinearLayout.LayoutParams(0, ViewGroup.LayoutParams.WRAP_CONTENT, 1f)
    }
    val fix = Button(this).apply {
      text = "Fix"
      isAllCaps = false
      setOnClickListener { startActivity(Intent(this@MainActivity, SetupActivity::class.java)) }
    }
    val dismiss = Button(this).apply {
      text = "✕"
      isAllCaps = false
      setOnClickListener { hidePermissionBanner() }
    }
    row.addView(label)
    row.addView(fix)
    row.addView(dismiss)

    findViewById<FrameLayout>(android.R.id.content).addView(
      row,
      FrameLayout.LayoutParams(
        ViewGroup.LayoutParams.MATCH_PARENT,
        ViewGroup.LayoutParams.WRAP_CONTENT,
        Gravity.TOP,
      ),
    )
    permissionBanner = row
    ViewCompat.requestApplyInsets(row)
  }

  private fun hidePermissionBanner() {
    val banner = permissionBanner ?: return
    (banner.parent as? ViewGroup)?.removeView(banner)
    permissionBanner = null
  }

  /// Human-readable list of the permissions still missing, for the banner
  /// copy (e.g. "microphone and accessibility").
  private fun missingPermissionsLabel(): String {
    val missing = mutableListOf<String>()
    if (ContextCompat.checkSelfPermission(this, Manifest.permission.RECORD_AUDIO) !=
      PackageManager.PERMISSION_GRANTED) {
      missing.add("microphone")
    }
    if (!Settings.canDrawOverlays(this)) missing.add("screen overlay")
    if (!SetupActivity.isAccessibilityServiceEnabled(
        this, BulbulAccessibilityService::class.java,
      )) {
      missing.add("accessibility")
    }
    return when (missing.size) {
      0 -> "permissions"
      1 -> missing[0]
      2 -> "${missing[0]} and ${missing[1]}"
      else -> "${missing.dropLast(1).joinToString(", ")}, and ${missing.last()}"
    }
  }

  private fun hasAllPermissions(): Boolean {
    val mic = ContextCompat.checkSelfPermission(this, Manifest.permission.RECORD_AUDIO) ==
      PackageManager.PERMISSION_GRANTED
    val overlay = Settings.canDrawOverlays(this)
    val a11y = SetupActivity.isAccessibilityServiceEnabled(
      this, BulbulAccessibilityService::class.java,
    )
    return mic && overlay && a11y
  }
}
