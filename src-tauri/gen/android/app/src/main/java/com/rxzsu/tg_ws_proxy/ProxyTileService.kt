package com.rxzsu.tg_ws_proxy

import android.app.PendingIntent
import android.content.Intent
import android.os.Build
import android.os.Handler
import android.os.Looper
import android.service.quicksettings.Tile
import android.service.quicksettings.TileService

/**
 * Shared bridge state. The authoritative source is the Rust runtime: the
 * tile seeds it optimistically on click and reconciles it against
 * [RustBridge.isRunning] whenever the tile becomes visible (and shortly
 * after every toggle, because the Rust-side toggle is asynchronous).
 */
object ProxyState {
    @Volatile
    var running = false
}

/**
 * Thin JNI shim into the Rust runtime (cdylib `tg_ws_proxy_gui_lib`).
 * Loading is a no-op if the Tauri webview already loaded it.
 */
object RustBridge {
    @Volatile
    var loaded = false

    init {
        loaded = runCatching { System.loadLibrary("tg_ws_proxy_gui_lib") }.isSuccess
    }

    /** Returns false when the Rust runtime is not up (cold process). */
    @JvmStatic
    external fun nativeToggleProxy(on: Boolean): Boolean

    /** Authoritative bridge state; false when the Rust runtime is not up. */
    @JvmStatic
    external fun nativeIsRunning(): Boolean

    fun isRunning(): Boolean =
        loaded && runCatching { nativeIsRunning() }.getOrDefault(false)
}

/**
 * Quick Settings tile: start/stop the proxy bridge straight from the
 * notification shade, without opening the app.
 *
 * The foreground service is NOT managed here: Rust turns it on when the
 * bridge actually starts and off when it stops, so a failed start (bad
 * config, busy port) never leaves a "bridge running" notification behind.
 * If the Rust runtime is not up (the OS killed the process earlier), the
 * tile launches MainActivity via `startActivityAndCollapse` — Tauri
 * auto-starts the bridge on boot.
 */
class ProxyTileService : TileService() {

    private val mainHandler = Handler(Looper.getMainLooper())

    override fun onStartListening() {
        reconcileFromRust()
    }

    override fun onClick() {
        val turnOn = !ProxyState.running

        if (turnOn) {
            val dispatched = RustBridge.loaded &&
                runCatching { RustBridge.nativeToggleProxy(true) }.getOrDefault(false)
            if (!dispatched) {
                // Cold process: the Rust runtime only exists behind the
                // activity, and its startup auto-starts the bridge. Plain
                // startActivity is a background-activity-start on Android 10+
                // and may be silently dropped; startActivityAndCollapse is
                // the supported path from a tile.
                if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.UPSIDE_DOWN_CAKE) {
                    startActivityAndCollapse(launchMainActivityPendingIntent())
                } else {
                    @Suppress("DEPRECATION")
                    startActivityAndCollapse(
                        Intent(this, MainActivity::class.java)
                            .addFlags(Intent.FLAG_ACTIVITY_NEW_TASK),
                    )
                }
            }
        } else {
            if (RustBridge.loaded) {
                runCatching { RustBridge.nativeToggleProxy(false) }
            }
        }

        ProxyState.running = turnOn
        updateTileState()
        // The Rust-side toggle is async (validation, bind, spawn): a failed
        // start must not leave the tile ACTIVE. Re-check the real state.
        if (RustBridge.loaded) {
            mainHandler.postDelayed({ reconcileFromRust() }, RECONCILE_DELAY_MS)
        }
    }

    private fun reconcileFromRust() {
        ProxyState.running = RustBridge.isRunning()
        updateTileState()
    }

    private fun launchMainActivityPendingIntent(): PendingIntent =
        PendingIntent.getActivity(
            this,
            0,
            Intent(this, MainActivity::class.java).addFlags(Intent.FLAG_ACTIVITY_NEW_TASK),
            PendingIntent.FLAG_IMMUTABLE,
        )

    private fun updateTileState() {
        qsTile ?: return
        qsTile.state = if (ProxyState.running) Tile.STATE_ACTIVE else Tile.STATE_INACTIVE
        qsTile.updateTile()
    }

    private companion object {
        /** Enough time for validate + bind + start_bridge on a warm process. */
        const val RECONCILE_DELAY_MS = 1500L
    }
}
