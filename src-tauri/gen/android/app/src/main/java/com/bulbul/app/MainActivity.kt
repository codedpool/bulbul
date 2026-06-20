package com.bulbul.app

import android.Manifest
import android.content.Intent
import android.content.pm.PackageManager
import android.os.Bundle
import android.provider.Settings
import androidx.activity.enableEdgeToEdge
import androidx.core.content.ContextCompat

class MainActivity : TauriActivity() {
  override fun onCreate(savedInstanceState: Bundle?) {
    enableEdgeToEdge()
    super.onCreate(savedInstanceState)
    // Launch the setup walker on top of the Tauri webview if any of
    // the three system permissions Bulbul depends on aren't granted.
    // Doing this in onCreate (not onResume) means the webview keeps
    // loading underneath — by the time the user finishes setup the
    // dashboard is ready, so they don't see a second loading spinner.
    if (!hasAllPermissions()) {
      startActivity(Intent(this, SetupActivity::class.java))
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
