package com.rxzsu.tg_ws_proxy

import android.Manifest
import android.content.pm.PackageManager
import android.os.Build
import android.os.Bundle
import androidx.activity.enableEdgeToEdge
import androidx.core.app.ActivityCompat
import androidx.core.content.ContextCompat

class MainActivity : TauriActivity() {
  override fun onCreate(savedInstanceState: Bundle?) {
    enableEdgeToEdge()
    super.onCreate(savedInstanceState)
    requestNotificationPermissionIfNeeded()
  }

  /**
   * Android 13+: the foreground-service notification ("Мост активен") is
   * hidden until POST_NOTIFICATIONS is granted — the proxy still works, but
   * the user cannot see that it does. The permission is declared in the
   * manifest; without a runtime request it is never granted.
   */
  private fun requestNotificationPermissionIfNeeded() {
    if (Build.VERSION.SDK_INT < Build.VERSION_CODES.TIRAMISU) return
    val granted = ContextCompat.checkSelfPermission(
      this,
      Manifest.permission.POST_NOTIFICATIONS,
    ) == PackageManager.PERMISSION_GRANTED
    if (!granted) {
      ActivityCompat.requestPermissions(
        this,
        arrayOf(Manifest.permission.POST_NOTIFICATIONS),
        NOTIFICATION_PERMISSION_REQUEST,
      )
    }
  }

  private companion object {
    const val NOTIFICATION_PERMISSION_REQUEST = 1443
  }
}
