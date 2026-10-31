package com.rxzsu.tg_ws_proxy

import android.app.NotificationChannel
import android.app.NotificationManager
import android.app.PendingIntent
import android.app.Service
import android.content.Context
import android.content.Intent
import android.content.pm.ServiceInfo
import android.os.Build
import android.os.IBinder
import androidx.core.app.NotificationCompat
import androidx.core.content.ContextCompat

/**
 * Keeps the Rust proxy process alive while the app is in the background.
 *
 * Android may freeze or kill a cached process even if the WebView is still
 * holding a socket. A foreground service raises the process priority and shows
 * an ongoing notification with the bridge status, which is the supported way
 * to keep long-lived networking work alive.
 *
 * The service deliberately does not own the proxy itself: the Rust server runs
 * inside the Tauri process, this only pins that process. Rust is also the
 * only one who starts/stops it (`set_foreground_service` on every bridge
 * start/stop); Kotlin used to start it directly on tile clicks, which left
 * an active notification behind whenever the start was rejected.
 */
class ProxyForegroundService : Service() {
  override fun onBind(intent: Intent?): IBinder? = null

  override fun onStartCommand(intent: Intent?, flags: Int, startId: Int): Int {
    // Only show the "bridge running" notification when the bridge is
    // actually running. A system restart of this service after process death
    // (START_STICKY) or a stale start() call used to display an active
    // notification inside a cold process where no bridge exists. When Rust
    // itself starts the service, `running` is committed before the JNI call,
    // so a legitimate start always passes this check.
    if (!RustBridge.isRunning()) {
      stopSelf()
      return START_NOT_STICKY
    }

    createChannel()

    val openApp = PendingIntent.getActivity(
      this,
      0,
      Intent(this, MainActivity::class.java).addFlags(Intent.FLAG_ACTIVITY_SINGLE_TOP),
      PendingIntent.FLAG_IMMUTABLE or PendingIntent.FLAG_UPDATE_CURRENT,
    )

    val notification = NotificationCompat.Builder(this, CHANNEL_ID)
      .setSmallIcon(R.drawable.ic_stat_proxy)
      .setContentTitle(getString(R.string.fgs_title))
      .setContentText(getString(R.string.fgs_text))
      .setContentIntent(openApp)
      .setOngoing(true)
      .setSilent(true)
      .setPriority(NotificationCompat.PRIORITY_LOW)
      .setCategory(NotificationCompat.CATEGORY_SERVICE)
      .build()

    if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.UPSIDE_DOWN_CAKE) {
      startForeground(
        NOTIFICATION_ID,
        notification,
        ServiceInfo.FOREGROUND_SERVICE_TYPE_SPECIAL_USE,
      )
    } else {
      startForeground(NOTIFICATION_ID, notification)
    }

    // If the system kills the process, recreate the service when possible.
    return START_STICKY
  }

  private fun createChannel() {
    if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.O) {
      val manager = getSystemService(Context.NOTIFICATION_SERVICE) as NotificationManager
      if (manager.getNotificationChannel(CHANNEL_ID) == null) {
        val channel = NotificationChannel(
          CHANNEL_ID,
          getString(R.string.fgs_channel),
          NotificationManager.IMPORTANCE_LOW,
        ).apply {
          description = getString(R.string.fgs_channel_desc)
          setShowBadge(false)
        }
        manager.createNotificationChannel(channel)
      }
    }
  }

  companion object {
    private const val CHANNEL_ID = "tg_ws_proxy_bridge"
    private const val NOTIFICATION_ID = 1443

    @JvmStatic
    fun start(context: Context) {
      val intent = Intent(context, ProxyForegroundService::class.java)
      ContextCompat.startForegroundService(context, intent)
    }

    @JvmStatic
    fun stop(context: Context) {
      context.stopService(Intent(context, ProxyForegroundService::class.java))
    }
  }
}
