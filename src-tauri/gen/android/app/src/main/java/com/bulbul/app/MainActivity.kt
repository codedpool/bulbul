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
import androidx.activity.enableEdgeToEdge
import androidx.core.content.ContextCompat
import androidx.core.view.WindowCompat
import java.io.File

class MainActivity : TauriActivity() {
  private var configObserver: FileObserver? = null

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
    // TODO(v1.1.1): this only fires in onCreate, so it's one-shot. If the
    // user presses BACK out of SetupActivity without granting anything,
    // they land on the webview (onboarding wizard) with no permissions,
    // and the setup walker is never shown again — MainActivity is already
    // created, so onCreate won't re-run, and nothing re-checks on return.
    // Net: the app is unusable (no mic/overlay/a11y) and never re-prompts,
    // even after the user later disables a permission. Fix: re-evaluate in
    // onResume and re-launch SetupActivity when permissions are still
    // missing — guarded against a relaunch loop (e.g. don't relaunch while
    // SetupActivity is the thing that just returned, or gate on a "user
    // explicitly deferred" flag) so BACK can't trap them but a genuinely
    // unpermissioned app is always steered back to setup.
    if (!hasAllPermissions()) {
      startActivity(Intent(this, SetupActivity::class.java))
    }
  }

  override fun onResume() {
    super.onResume()
    // Catch a theme change made while we were backgrounded (or an OS
    // theme flip when the app is set to "system").
    applyBarAppearance()
    // TODO(v1.1.1): also re-check permissions here (see onCreate note) so
    // backing out of setup, or revoking a permission, re-surfaces the
    // setup walker instead of leaving the app silently non-functional.
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
