package com.rxzsu.tg_ws_proxy

import android.content.Intent
import android.service.quicksettings.Tile
import android.service.quicksettings.TileService

/**
 * Shared bridge state, mirrored from the Rust side: `set_foreground_service`
 * calls [ProxyForegroundService.start]/[.stop] on every bridge start/stop,
 * and the tile updates it optimistically on click. Best effort — the tile
 * re-reads it every time it becomes visible.
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
}

/**
 * Quick Settings tile: start/stop the proxy bridge straight from the
 * notification shade, without opening the app.
 *
 * Toggling ON also pins the process with the foreground service. If the Rust
 * runtime is not up (the OS killed the process earlier), the tile launches
 * MainActivity instead — Tauri auto-starts the bridge on boot.
 */
class ProxyTileService : TileService() {

    override fun onStartListening() {
        updateTileState()
    }

    override fun onClick() {
        val turnOn = !ProxyState.running

        if (turnOn) {
            ProxyForegroundService.start(applicationContext)
            val dispatched = RustBridge.loaded &&
                runCatching { RustBridge.nativeToggleProxy(true) }.getOrDefault(false)
            if (!dispatched) {
                // Cold process: the Rust runtime only exists behind the
                // activity, and its startup auto-starts the bridge.
                startActivity(
                    Intent(this, MainActivity::class.java)
                        .addFlags(Intent.FLAG_ACTIVITY_NEW_TASK),
                )
            }
        } else {
            if (RustBridge.loaded) {
                runCatching { RustBridge.nativeToggleProxy(false) }
            }
            ProxyForegroundService.stop(applicationContext)
        }

        ProxyState.running = turnOn
        updateTileState()
    }

    private fun updateTileState() {
        qsTile ?: return
        qsTile.state = if (ProxyState.running) Tile.STATE_ACTIVE else Tile.STATE_INACTIVE
        qsTile.updateTile()
    }
}
