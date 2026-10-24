use std::fs;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};
use tauri::{AppHandle, Emitter, Manager, State};
#[cfg(desktop)]
use tauri::{
    menu::{Menu, MenuItem, PredefinedMenuItem},
    tray::{MouseButton, MouseButtonState, TrayIconBuilder, TrayIconEvent},
};
#[cfg(desktop)]
use tauri_plugin_clipboard_manager::ClipboardExt;
use tauri_plugin_opener::OpenerExt;
use tg_ws_proxy_core::config::ProxyConfig;
use tg_ws_proxy_core::logging as core_logging;
use tg_ws_proxy_core::stats::Stats;
use tokio::sync::Mutex;

struct AppState {
    running: Arc<AtomicBool>,
    config: Mutex<ProxyConfig>,
    stats: Arc<Stats>,
    shutdown_tx: Mutex<Option<tokio::sync::broadcast::Sender<()>>>,
    /// Handle of the running bridge task. Stop waits for it instead of a
    /// fixed sleep, so a fast stop→start no longer races the old bridge for
    /// the listening port.
    bridge_task: Mutex<Option<tauri::async_runtime::JoinHandle<()>>>,
}

/// Global handle so native callbacks that have no Tauri context (the Android
/// Quick Settings tile JNI entry) can drive the bridge.
static APP_HANDLE: std::sync::OnceLock<AppHandle> = std::sync::OnceLock::new();

fn get_config_path(app: &AppHandle) -> PathBuf {
    let mut path = app
        .path()
        .app_config_dir()
        .unwrap_or_else(|_| PathBuf::from("."));
    let _ = fs::create_dir_all(&path);
    path.push("config.json");
    path
}

fn load_saved_config(app: &AppHandle) -> ProxyConfig {
    let path = get_config_path(app);
    if let Some(cfg) = read_config_file(&path) {
        return cfg;
    }
    // Check fallback location in APPDATA/TgWsProxy/config.json
    if let Ok(appdata) = std::env::var("APPDATA") {
        let legacy_path = PathBuf::from(appdata).join("TgWsProxy").join("config.json");
        if let Some(cfg) = read_config_file(&legacy_path) {
            return cfg;
        }
    }
    ProxyConfig::default()
}

/// Parse one config file. A corrupt file is preserved for recovery instead of
/// being silently overwritten with defaults (which used to rotate the secret
/// and break the user's Telegram link without any explanation).
fn read_config_file(path: &std::path::Path) -> Option<ProxyConfig> {
    let content = fs::read_to_string(path).ok()?;
    match serde_json::from_str::<ProxyConfig>(&content) {
        Ok(cfg) => Some(cfg),
        Err(e) => {
            log::error!("Config {} is invalid: {e}", path.display());
            let broken = path.with_extension("json.broken");
            if let Err(rename_err) = fs::rename(path, &broken) {
                log::error!("Could not preserve the broken config: {rename_err}");
            } else {
                log::error!("Broken config preserved as {}", broken.display());
            }
            None
        }
    }
}

/// Persist config atomically: write a temp file, keep a backup of the previous
/// version, then rename into place. A crash mid-write can no longer leave a
/// truncated `config.json` behind.
fn persist_config(app: &AppHandle, cfg: &ProxyConfig) -> Result<(), String> {
    let path = get_config_path(app);
    let serialized = serde_json::to_string_pretty(cfg)
        .map_err(|e| format!("Не удалось сериализовать настройки: {e}"))?;
    let tmp = path.with_extension("json.tmp");
    fs::write(&tmp, serialized)
        .map_err(|e| format!("Не удалось записать настройки: {e}"))?;
    if path.exists() {
        let _ = fs::copy(&path, path.with_extension("json.bak"));
    }
    if let Err(e) = fs::rename(&tmp, &path) {
        let _ = fs::remove_file(&tmp);
        return Err(format!("Не удалось сохранить настройки: {e}"));
    }
    Ok(())
}

pub fn get_log_file_path(app: &AppHandle) -> PathBuf {
    let mut path = app.path().app_log_dir().unwrap_or_else(|_| PathBuf::from("."));
    let _ = fs::create_dir_all(&path);
    path.push("proxy.log");
    path
}

pub fn generate_tg_link(cfg: &ProxyConfig) -> String {
    let host = tg_ws_proxy_core::config::get_link_host(&cfg.host);
    let mut clean_secret = cfg.secret.trim().to_string();
    if (clean_secret.starts_with("dd") || clean_secret.starts_with("ee")) && clean_secret.len() > 32 {
        clean_secret = clean_secret[2..].to_string();
    }
    let formatted_secret = if !cfg.fake_tls_domain.trim().is_empty() {
        let domain_hex = hex::encode(cfg.fake_tls_domain.trim().as_bytes());
        format!("ee{}{}", clean_secret, domain_hex)
    } else {
        format!("dd{}", clean_secret)
    };
    format!("tg://proxy?server={}&port={}&secret={}", host, cfg.port, formatted_secret)
}

fn sensitive_domains_of(cfg: &ProxyConfig) -> Vec<String> {
    let mut out = Vec::new();
    if cfg.cfproxy_user_domain_enabled {
        out.extend(cfg.cfproxy_user_domains.iter().cloned());
    }
    if cfg.cfproxy_worker_enabled {
        out.extend(cfg.cfproxy_worker_domains.iter().cloned());
    }
    out
}

#[cfg(desktop)]
async fn current_tg_link(state: &Arc<AppState>) -> String {
    let cfg = state.config.lock().await;
    generate_tg_link(&cfg)
}

/// Enable/disable the Android foreground service that keeps the proxy process
/// alive when the app is in the background. No-op on desktop.
#[cfg(target_os = "android")]
fn set_foreground_service(app: &AppHandle, enabled: bool) {
    use jni::objects::JValue;

    let Some(window) = app.get_webview_window("main") else {
        log::warn!("Cannot toggle foreground service: no main webview");
        return;
    };
    let result = window.with_webview(move |platform_webview| {
        platform_webview.jni_handle().exec(move |env, activity, _webview| {
            let Ok(class) = env.find_class("com/rxzsu/tg_ws_proxy/ProxyForegroundService") else {
                log::warn!("Foreground service class not found");
                return;
            };
            let method = if enabled { "start" } else { "stop" };
            if let Err(e) = env.call_static_method(
                class,
                method,
                "(Landroid/content/Context;)V",
                &[JValue::Object(activity)],
            ) {
                log::warn!("Foreground service {method} failed: {e}");
            }
        });
    });
    if let Err(e) = result {
        log::warn!("Cannot dispatch foreground service call: {e}");
    }
}

#[cfg(not(target_os = "android"))]
fn set_foreground_service(_app: &AppHandle, _enabled: bool) {}

/// Start/stop the bridge on behalf of the Android Quick Settings tile.
/// Mirrors `start_proxy`/`stop_proxy` including validation and diagnostics.
#[cfg(target_os = "android")]
async fn toggle_bridge_from_tile(app: AppHandle, on: bool) -> Result<(), String> {
    let state = app.state::<Arc<AppState>>().inner().clone();
    if on {
        if state.running.load(Ordering::SeqCst) {
            return Ok(());
        }
        let cfg = state.config.lock().await.clone();
        if let Err(msg) = tg_ws_proxy_core::config::validate(&cfg) {
            let _ = app.emit("proxy-error", msg.clone());
            return Err(msg);
        }
        let addr = format!("{}:{}", cfg.host, cfg.port);
        if let Err(e) = std::net::TcpListener::bind(&addr) {
            let diag = tg_ws_proxy_core::diagnose_bind_error(&e, &cfg.host, cfg.port);
            let _ = app.emit("proxy-error", diag.clone());
            return Err(diag);
        }
        start_bridge(app, state, cfg).await;
        Ok(())
    } else {
        stop_bridge(&state).await;
        set_foreground_service(&app, false);
        Ok(())
    }
}

/// JNI entry called by `ProxyTileService.RustBridge.nativeToggleProxy`.
/// Returns false when the Rust runtime is not up (cold process); the Kotlin
/// side then falls back to launching MainActivity, whose startup auto-starts
/// the bridge.
#[cfg(target_os = "android")]
#[no_mangle]
pub extern "system" fn Java_com_rxzsu_tg_ws_proxy_RustBridge_nativeToggleProxy(
    _env: jni::JNIEnv,
    _class: jni::objects::JClass,
    on: jni::sys::jboolean,
) -> jni::sys::jboolean {
    let Some(app) = APP_HANDLE.get() else {
        return 0;
    };
    let app = app.clone();
    tauri::async_runtime::spawn(async move {
        if let Err(e) = toggle_bridge_from_tile(app, on != 0).await {
            log::warn!("Tile toggle failed: {e}");
        }
    });
    1
}

/// Spawn the proxy bridge task. The shutdown channel is created and stored
/// *before* the task spawns, so a stop issued immediately after start always
/// reaches the bridge (previously the channel was installed inside the task,
/// leaving a window where stop silently did nothing).
async fn start_bridge(app: AppHandle, proxy_state: Arc<AppState>, cfg: ProxyConfig) {
    let (shutdown_tx, shutdown_rx) = tokio::sync::broadcast::channel(256);
    {
        let mut tx_guard = proxy_state.shutdown_tx.lock().await;
        *tx_guard = Some(shutdown_tx);
    }
    // Pre-flight bind already succeeded in the caller; from here on the
    // start is committed. Swap guards against a double start.
    if proxy_state.running.swap(true, Ordering::SeqCst) {
        return;
    }
    set_foreground_service(&app, true);

    let stats = proxy_state.stats.clone();
    let running = proxy_state.running.clone();
    let app_err = app.clone();
    let handle = tauri::async_runtime::spawn(async move {
        if let Err(e) =
            tg_ws_proxy_core::run_server_with_stats(cfg, stats, shutdown_rx).await
        {
            log::error!("Server error: {:?}", e);
            let _ = app_err.emit("proxy-error", e.to_string());
        }
        running.store(false, Ordering::SeqCst);
        set_foreground_service(&app_err, false);
    });
    *proxy_state.bridge_task.lock().await = Some(handle);
}

/// Send the shutdown signal, then wait for the bridge task to actually
/// finish (it drains live sessions for up to ~1s) before returning. Aborts
/// the task if it somehow outlives the grace window.
async fn stop_bridge(proxy_state: &Arc<AppState>) {
    {
        let mut tx_guard = proxy_state.shutdown_tx.lock().await;
        if let Some(tx) = tx_guard.take() {
            let _ = tx.send(());
        }
    }
    let handle = proxy_state.bridge_task.lock().await.take();
    proxy_state.running.store(false, Ordering::SeqCst);
    // Do not zero `connections_active` here: live sessions still decrement it
    // while draining, and a store(0) raced with those decrements (underflow).
    if let Some(mut handle) = handle {
        // Drain is bounded server-side (1s session grace); allow extra
        // headroom, then force-quit a wedged task.
        if tokio::time::timeout(Duration::from_secs(5), &mut handle)
            .await
            .is_err()
        {
            log::warn!("Bridge task did not finish within 5s of shutdown, aborting");
            handle.abort();
        }
    }
    proxy_state.stats.mark_stopped();
}

#[cfg(desktop)]
fn show_main_window(app: &AppHandle) {
    if let Some(window) = app.get_webview_window("main") {
        let _ = window.show();
        // Window state APIs are desktop-only.
        #[cfg(desktop)]
        let _ = window.unminimize();
        let _ = window.set_focus();
    }
}

/// Host platform: `android` / `ios` / `windows` / `macos` / `linux`.
/// The frontend uses it to hide desktop-only chrome (window buttons,
/// autostart, tray hints) instead of guessing from the user agent.
#[tauri::command]
fn get_platform() -> &'static str {
    std::env::consts::OS
}

#[tauri::command]
async fn get_config(state: State<'_, Arc<AppState>>) -> Result<ProxyConfig, String> {
    let cfg = state.config.lock().await;
    Ok(cfg.clone())
}

#[tauri::command]
async fn save_config(
    app: AppHandle,
    new_config: ProxyConfig,
    state: State<'_, Arc<AppState>>,
) -> Result<(), String> {
    if let Err(msg) = tg_ws_proxy_core::config::validate(&new_config) {
        log::error!("Config rejected: {}", msg);
        return Err(msg);
    }

    // Persist first: if the disk write fails the user must not be told the
    // settings were applied. The bridge keeps running with the old config.
    persist_config(&app, &new_config)?;
    {
        let mut cfg = state.config.lock().await;
        *cfg = new_config.clone();
    }
    core_logging::set_sensitive_domains(sensitive_domains_of(&new_config));

    // Only restart a bridge that is actually running. Saving settings while
    // the proxy is stopped used to silently start it.
    let was_running = state.running.load(Ordering::SeqCst);
    if !was_running {
        return Ok(());
    }

    // Seamlessly restart proxy bridge with updated settings.
    // Stop the old bridge first and *wait* for it (bounded), then verify the
    // port is actually free so the user gets a clear "port busy" diagnostic
    // instead of a silent failure inside the background task.
    stop_bridge(&state).await;

    let addr = format!("{}:{}", new_config.host, new_config.port);
    if let Err(e) = std::net::TcpListener::bind(&addr) {
        let diag = tg_ws_proxy_core::diagnose_bind_error(&e, &new_config.host, new_config.port);
        log::error!("Config apply failed: {}", diag);
        let _ = app.emit("proxy-error", diag.clone());
        return Err(diag);
    }

    let proxy_state = state.inner().clone();
    start_bridge(app, proxy_state, new_config).await;

    Ok(())
}

#[tauri::command]
async fn is_running(state: State<'_, Arc<AppState>>) -> Result<bool, String> {
    Ok(state.running.load(Ordering::SeqCst))
}

/// Pre-flight port check for the onboarding/settings UI. Binds and releases
/// the socket; the diagnostic is the same one `start_proxy` would show.
#[tauri::command]
async fn check_port(host: String, port: u16) -> Result<(), String> {
    let addr = format!("{host}:{port}");
    match std::net::TcpListener::bind(&addr) {
        Ok(_) => Ok(()),
        Err(e) => Err(tg_ws_proxy_core::diagnose_bind_error(&e, &host, port)),
    }
}

/// Dismiss the "last error" banner without restarting the bridge.
#[tauri::command]
async fn clear_last_error(state: State<'_, Arc<AppState>>) -> Result<(), String> {
    state.stats.clear_last_error();
    Ok(())
}

#[tauri::command]
async fn start_proxy(app: AppHandle, state: State<'_, Arc<AppState>>) -> Result<String, String> {
    if state.running.load(Ordering::SeqCst) {
        return Ok("Already running".into());
    }

    let cfg = {
        let guard = state.config.lock().await;
        guard.clone()
    };

    // Refuse to start with a config that cannot work, instead of binding a
    // random port or deriving an all-zero key from a mistyped secret.
    if let Err(msg) = tg_ws_proxy_core::config::validate(&cfg) {
        log::error!("Start refused: {}", msg);
        let _ = app.emit("proxy-error", msg.clone());
        return Err(msg);
    }

    // Pre-flight check port binding with diagnostic mapping
    let addr = format!("{}:{}", cfg.host, cfg.port);
    if let Err(e) = std::net::TcpListener::bind(&addr) {
        let diag = tg_ws_proxy_core::diagnose_bind_error(&e, &cfg.host, cfg.port);
        log::error!("Start failed: {}", diag);
        let _ = app.emit("proxy-error", diag.clone());
        return Err(diag);
    }

    let proxy_state = state.inner().clone();
    start_bridge(app, proxy_state, cfg).await;

    Ok("Started".into())
}

#[tauri::command]
async fn stop_proxy(app: AppHandle, state: State<'_, Arc<AppState>>) -> Result<(), String> {
    stop_bridge(state.inner()).await;
    set_foreground_service(&app, false);
    Ok(())
}

#[derive(serde::Serialize)]
pub struct LogTail {
    pub lines: Vec<String>,
    /// Byte offset the frontend passes back on the next poll.
    pub offset: u64,
    /// True when the view was rebuilt from scratch (rotation, clear, or the
    /// first call): the frontend must replace its buffer instead of appending.
    pub truncated: bool,
}

/// How many bytes of the file end to re-read on a full refresh. Enough for
/// ~1000 log lines; prevents re-reading a (potentially hundreds-of-MB) file.
const LOG_REFRESH_WINDOW: u64 = 256 * 1024;
/// Hard cap on the incremental payload so a pathological single-line flood
/// cannot balloon one IPC message.
const LOG_MAX_TAIL_BYTES: u64 = 512 * 1024;

/// Incremental log tail. The frontend polls with the offset from the previous
/// call; only the newly appended bytes cross the IPC boundary, instead of the
/// whole file every 2 seconds (which used to block a tokio worker on large
/// logs and re-render the whole view).
#[tauri::command]
async fn get_log_tail(app: AppHandle, offset: Option<u64>) -> Result<LogTail, String> {
    let path = get_log_file_path(&app);
    tokio::task::spawn_blocking(move || read_log_tail(&path, offset.unwrap_or(0)))
        .await
        .map_err(|e| format!("Не удалось прочитать логи: {e}"))?
}

fn read_log_tail(path: &std::path::Path, offset: u64) -> Result<LogTail, String> {
    use std::io::{Read, Seek, SeekFrom};
    if !path.exists() {
        return Ok(LogTail { lines: Vec::new(), offset: 0, truncated: true });
    }
    let mut file = fs::File::open(path)
        .map_err(|e| format!("Не удалось открыть файл логов: {e}"))?;
    let size = file
        .metadata()
        .map_err(|e| format!("Не удалось получить размер логов: {e}"))?
        .len();

    // Rotation or clear happened since the last poll: full refresh.
    let truncated = offset == 0 || size < offset;

    if truncated {
        let start = size.saturating_sub(LOG_REFRESH_WINDOW);
        file.seek(SeekFrom::Start(start))
            .map_err(|e| format!("Не удалось прочитать логи: {e}"))?;
        let mut bytes = Vec::new();
        file.read_to_end(&mut bytes)
            .map_err(|e| format!("Не удалось прочитать логи: {e}"))?;
        // Drop the possibly-torn first line when we seeked into the middle.
        let text = String::from_utf8_lossy(&bytes);
        let text = if start > 0 && !text.starts_with('\n') {
            text.split_once('\n').map(|(_, rest)| rest).unwrap_or("")
        } else {
            text.as_ref()
        };
        let lines: Vec<String> = text.lines().map(|s| s.to_string()).collect();
        Ok(LogTail { lines, offset: size, truncated: true })
    } else {
        let end = size.min(offset + LOG_MAX_TAIL_BYTES);
        let len = (end - offset) as usize;
        file.seek(SeekFrom::Start(offset))
            .map_err(|e| format!("Не удалось прочитать логи: {e}"))?;
        let mut bytes = vec![0u8; len];
        file.read_exact(&mut bytes)
            .map_err(|e| format!("Не удалось прочитать логи: {e}"))?;
        let text = String::from_utf8_lossy(&bytes);
        let mut lines: Vec<String> = text.lines().map(|s| s.to_string()).collect();
        // A torn last line (crash mid-write) becomes an empty fragment; drop
        // it so the view doesn't show a phantom blank entry.
        if !bytes.ends_with(b"\n") && !lines.is_empty() {
            lines.pop();
        }
        Ok(LogTail { lines, offset: end, truncated: false })
    }
}

#[tauri::command]
async fn open_log_file(app: AppHandle) -> Result<(), String> {
    let path = get_log_file_path(&app);
    if !path.exists() {
        let _ = fs::write(&path, "");
    }
    app.opener()
        .open_path(path.to_string_lossy(), None::<&str>)
        .map_err(|e| format!("Не удалось открыть лог: {e}"))
}

#[tauri::command]
async fn clear_logs(app: AppHandle) -> Result<(), String> {
    let path = get_log_file_path(&app);
    let _ = fs::write(path, "");
    Ok(())
}

/// Only schemes the app itself produces are allowed: the tg:// deep link and
/// GitHub release pages. A compromised webview must not be able to make the
/// OS open arbitrary handlers (file:, ms-settings:, executables, ...).
fn is_allowed_external_url(url: &str) -> bool {
    let lower = url.trim().to_ascii_lowercase();
    lower.starts_with("tg://") || lower.starts_with("https://")
}

#[tauri::command]
async fn open_url(app: AppHandle, url: String) -> Result<(), String> {
    if !is_allowed_external_url(&url) {
        return Err(format!("Схема ссылки не разрешена: {}", url));
    }
    app.opener()
        .open_url(url, None::<&str>)
        .map_err(|e| format!("Не удалось открыть ссылку: {e}"))
}

#[tauri::command]
async fn restart_proxy(app: AppHandle, state: State<'_, Arc<AppState>>) -> Result<String, String> {
    stop_bridge(state.inner()).await;

    let cfg = {
        let guard = state.config.lock().await;
        guard.clone()
    };
    let addr = format!("{}:{}", cfg.host, cfg.port);
    if let Err(e) = std::net::TcpListener::bind(&addr) {
        let diag = tg_ws_proxy_core::diagnose_bind_error(&e, &cfg.host, cfg.port);
        log::error!("Restart failed: {}", diag);
        let _ = app.emit("proxy-error", diag.clone());
        return Err(diag);
    }

    let proxy_state = state.inner().clone();
    start_bridge(app, proxy_state, cfg).await;
    Ok("Restarted".into())
}

#[tauri::command]
fn get_app_version() -> String {
    env!("CARGO_PKG_VERSION").to_string()
}

/// First-run detection: no persisted config file yet. After the onboarding
/// wizard saves settings once, this returns false forever.
#[tauri::command]
fn is_first_run(app: AppHandle) -> bool {
    !get_config_path(&app).exists()
}

#[tauri::command]
async fn minimize_window(app: AppHandle) -> Result<(), String> {
    // Window state APIs exist on desktop only; mobile keeps the command
    // registered so the shared frontend can call it unconditionally.
    #[cfg(desktop)]
    if let Some(w) = app.get_webview_window("main") {
        let _ = w.minimize();
    }
    #[cfg(mobile)]
    let _ = app;
    Ok(())
}

#[tauri::command]
async fn toggle_maximize_window(app: AppHandle) -> Result<(), String> {
    #[cfg(desktop)]
    if let Some(w) = app.get_webview_window("main") {
        if w.is_maximized().unwrap_or(false) {
            let _ = w.unmaximize();
        } else {
            let _ = w.maximize();
        }
    }
    #[cfg(mobile)]
    let _ = app;
    Ok(())
}

#[tauri::command]
async fn close_window(app: AppHandle) -> Result<(), String> {
    #[cfg(desktop)]
    if let Some(w) = app.get_webview_window("main") {
        let _ = w.hide();
    }
    #[cfg(mobile)]
    let _ = app;
    Ok(())
}

#[derive(serde::Serialize, Clone)]
pub struct DcTestResult {
    pub dc: String,
    pub host: String,
    pub ok: bool,
    /// DC has no `/apiws` front at all (DC1/3/5); not a connectivity failure.
    pub no_front: bool,
    pub latency_ms: u64,
    pub message: String,
}

#[tauri::command]
async fn test_connectivity(custom_domains: Vec<String>) -> Result<Vec<DcTestResult>, String> {
    use tg_ws_proxy_core::raw_websocket::RawWebSocket;
    // Telegram only serves `/apiws` for DC2/DC4. DC1/3/5 answer `302` there,
    // so probing them produced guaranteed red "timeout/error" rows that told
    // the user nothing about their network. Report them as "no WS front"
    // instead; DC203 is an alias of DC2.
    let mut targets = vec![
        ("DC1".to_string(), "kws1.web.telegram.org".to_string(), true),
        ("DC2".to_string(), "kws2.web.telegram.org".to_string(), false),
        ("DC3".to_string(), "kws3.web.telegram.org".to_string(), true),
        ("DC4".to_string(), "kws4.web.telegram.org".to_string(), false),
        ("DC5".to_string(), "kws5.web.telegram.org".to_string(), true),
        ("DC203".to_string(), "kws2.web.telegram.org".to_string(), false),
    ];

    for d in custom_domains {
        let clean = d.trim().to_string();
        if !clean.is_empty() {
            targets.push(("Custom".to_string(), clean, false));
        }
    }

    let mut handles = Vec::new();
    for (dc, host, no_front) in targets {
        handles.push(tokio::spawn(async move {
            if no_front {
                return DcTestResult {
                    dc,
                    host,
                    ok: false,
                    no_front: true,
                    latency_ms: 0,
                    message: "Нет WS-фронта (/apiws → 302)".to_string(),
                };
            }
            let start = std::time::Instant::now();
            let connect_fut = RawWebSocket::connect(&host, &host, "/apiws", true);
            match tokio::time::timeout(std::time::Duration::from_secs(6), connect_fut).await {
                Ok(Ok(mut ws)) => {
                    let ms = start.elapsed().as_millis() as u64;
                    let _ = ws.close().await;
                    DcTestResult {
                        dc,
                        host,
                        ok: true,
                        no_front: false,
                        latency_ms: ms,
                        message: format!("{} ms (WS OK)", ms),
                    }
                }
                Ok(Err(e)) => {
                    let ms = start.elapsed().as_millis() as u64;
                    DcTestResult {
                        dc,
                        host,
                        ok: false,
                        no_front: false,
                        latency_ms: ms,
                        message: format!("Ошибка: {}", e),
                    }
                }
                Err(_) => {
                    DcTestResult {
                        dc,
                        host,
                        ok: false,
                        no_front: false,
                        latency_ms: 6000,
                        message: "Таймаут (6с)".to_string(),
                    }
                }
            }
        }));
    }

    let mut results = Vec::new();
    for h in handles {
        if let Ok(res) = h.await {
            results.push(res);
        }
    }
    Ok(results)
}

#[derive(serde::Serialize, Clone)]
pub struct UpdateCheckResult {
    pub has_update: bool,
    pub current_version: String,
    pub latest_version: String,
    pub release_url: String,
}

#[tauri::command]
async fn check_updates() -> Result<UpdateCheckResult, String> {
    let current_version = env!("CARGO_PKG_VERSION").to_string();
    // Bounded: the button must never spin forever on a hung connection.
    let result = tokio::time::timeout(
        Duration::from_secs(25),
        tg_ws_proxy_core::balancer::check_latest_github_release(),
    )
    .await;
    match result {
        Ok(Ok((tag, url))) => {
            // Compare numerically: any tag mismatch used to count as an
            // update, so a rollback (or an older tag) was advertised too.
            let has_update = tg_ws_proxy_core::version::is_newer_version(&tag, &current_version);
            Ok(UpdateCheckResult {
                has_update,
                current_version,
                latest_version: tag,
                release_url: url,
            })
        }
        Ok(Err(e)) => {
            log::warn!("GitHub update check failed: {:?}", e);
            Ok(UpdateCheckResult {
                has_update: false,
                current_version: current_version.clone(),
                latest_version: current_version,
                release_url: "https://github.com/rxzsu/tg-ws-rust/releases".to_string(),
            })
        }
        Err(_) => {
            log::warn!("GitHub update check timed out");
            Ok(UpdateCheckResult {
                has_update: false,
                current_version: current_version.clone(),
                latest_version: current_version,
                release_url: "https://github.com/rxzsu/tg-ws-rust/releases".to_string(),
            })
        }
    }
}

#[tauri::command]
fn get_link_host(host: String) -> String {
    tg_ws_proxy_core::config::get_link_host(&host)
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    let stats = Arc::new(Stats::new());
    let state = Arc::new(AppState {
        running: Arc::new(AtomicBool::new(false)),
        config: Mutex::new(ProxyConfig::default()),
        stats: stats.clone(),
        shutdown_tx: Mutex::new(None),
        bridge_task: Mutex::new(None),
    });

    let builder = tauri::Builder::default()
        .plugin(tauri_plugin_clipboard_manager::init())
        .plugin(tauri_plugin_opener::init())
        .manage(state.clone());

    #[cfg(desktop)]
    let launched_from_autostart = std::env::args().any(|arg| arg == "--autostart");
    #[cfg(not(desktop))]
    let launched_from_autostart = false;

    // Autostart and single-instance are desktop-only (the crates themselves
    // are only pulled in for non-mobile targets in Cargo.toml).
    #[cfg(desktop)]
    let builder = builder
        .plugin(
            tauri_plugin_autostart::Builder::new()
                .arg("--autostart")
                .build(),
        )
        .plugin(tauri_plugin_single_instance::init(|app, _args, _cwd| {
            // Second launch: focus the already running window instead of
            // conflicting over the proxy port (Single-Instance Guard).
            show_main_window(app);
        }));

    builder
        .setup(move |app| {
            let _ = APP_HANDLE.set(app.handle().clone());
            if cfg!(debug_assertions) {
                app.handle().plugin(
                    tauri_plugin_log::Builder::default()
                        .level(log::LevelFilter::Info)
                        .build(),
                )?;
            }

            // Load persisted config first so logging knows rotation limits
            // and which domains must be censored.
            let saved_cfg = load_saved_config(app.handle());
            let app_state_clone = app.state::<Arc<AppState>>().inner().clone();
            let initial_cfg = saved_cfg.clone();
            tauri::async_runtime::block_on(async {
                let mut cfg = app_state_clone.config.lock().await;
                *cfg = saved_cfg;
            });

            // File logging with rotation + domain censor (always on,
            // not only in debug builds).
            {
                let handle = app.handle().clone();
                let log_path = get_log_file_path(&handle);
                core_logging::init_logging(
                    log_path,
                    initial_cfg.log_max_mb,
                    initial_cfg.verbose,
                    sensitive_domains_of(&initial_cfg),
                );
            }
            log::info!(
                "TG WS Proxy v{} starting on {}:{}",
                env!("CARGO_PKG_VERSION"),
                initial_cfg.host,
                initial_cfg.port
            );

            // Automatically launch proxy bridge on startup.
            // Failures (e.g. busy port) are emitted to the UI instead of
            // failing silently.
            let auto_state = app.state::<Arc<AppState>>().inner().clone();
            let auto_handle = app.handle().clone();
            tauri::async_runtime::spawn(async move {
                let cfg_snapshot = {
                    let guard = auto_state.config.lock().await;
                    guard.clone()
                };
                let addr = format!("{}:{}", cfg_snapshot.host, cfg_snapshot.port);
                if let Err(e) = std::net::TcpListener::bind(&addr) {
                    let diag = tg_ws_proxy_core::diagnose_bind_error(
                        &e,
                        &cfg_snapshot.host,
                        cfg_snapshot.port,
                    );
                    log::error!("Auto-start failed: {}", diag);
                    let _ = auto_handle.emit("proxy-error", diag);
                    return;
                }
                start_bridge(auto_handle, auto_state, cfg_snapshot).await;
            });

            // Telemetry broadcast loop. Emits every second only while the
            // bridge is running *and* the window is visible; otherwise it
            // falls back to a 5s heartbeat (and skips entirely while hidden)
            // so an idle tray-only app doesn't burn battery re-rendering
            // unchanged zeros — especially on Android.
            let app_handle = app.handle().clone();
            let stats_bg = stats.clone();
            let running_flag = app.state::<Arc<AppState>>().inner().running.clone();
            tauri::async_runtime::spawn(async move {
                let mut prev_up = 0u64;
                let mut prev_down = 0u64;
                let mut prev_time = Instant::now();

                loop {
                    let running = running_flag.load(Ordering::Relaxed);
                    let visible = app_handle
                        .get_webview_window("main")
                        .and_then(|w| w.is_visible().ok())
                        .unwrap_or(true);
                    let tick = if running && visible { 1 } else { 5 };
                    tokio::time::sleep(Duration::from_secs(tick)).await;
                    if !visible {
                        continue;
                    }
                    let now = Instant::now();
                    let elapsed = now.duration_since(prev_time).as_secs_f64();
                    if elapsed <= 0.0 {
                        continue;
                    }

                    let current_up = stats_bg.bytes_up.load(Ordering::Relaxed);
                    let current_down = stats_bg.bytes_down.load(Ordering::Relaxed);

                    let speed_up = ((current_up.saturating_sub(prev_up)) as f64 / 1024.0) / elapsed;
                    let speed_down = ((current_down.saturating_sub(prev_down)) as f64 / 1024.0) / elapsed;

                    prev_up = current_up;
                    prev_down = current_down;
                    prev_time = now;

                    let snapshot = stats_bg.snapshot(speed_up, speed_down);
                    let _ = app_handle.emit("telemetry-update", snapshot);
                }
            });

            // Tray is desktop-only: Android/iOS have no tray API at all.
            #[cfg(desktop)]
            {
            // Create Tray Menu (full quick actions, no need to open the window)
            let open_tg_i = MenuItem::with_id(app, "open_tg", "Открыть в Telegram", true, None::<&str>)?;
            let copy_link_i =
                MenuItem::with_id(app, "copy_link", "Скопировать ссылку", true, None::<&str>)?;
            let restart_i =
                MenuItem::with_id(app, "restart", "Перезапустить прокси", true, None::<&str>)?;
            let open_logs_i =
                MenuItem::with_id(app, "open_logs", "Открыть логи", true, None::<&str>)?;
            let settings_i = MenuItem::with_id(app, "settings", "Настройки", true, None::<&str>)?;
            let show_i = MenuItem::with_id(app, "show", "Показать окно", true, None::<&str>)?;
            let quit_i = MenuItem::with_id(app, "quit", "Выход", true, None::<&str>)?;
            let sep1 = PredefinedMenuItem::separator(app)?;
            let sep2 = PredefinedMenuItem::separator(app)?;
            let menu = Menu::with_items(
                app,
                &[
                    &open_tg_i,
                    &copy_link_i,
                    &restart_i,
                    &sep1,
                    &open_logs_i,
                    &settings_i,
                    &show_i,
                    &sep2,
                    &quit_i,
                ],
            )?;

            let mut tray_builder = TrayIconBuilder::with_id("main-tray")
                .tooltip("TG WS Proxy")
                .menu(&menu)
                .show_menu_on_left_click(false)
                .on_menu_event(|app, event| match event.id.as_ref() {
                    "quit" => {
                        app.exit(0);
                    }
                    "show" => {
                        show_main_window(app);
                    }
                    "settings" => {
                        show_main_window(app);
                        let _ = app.emit("open-settings", ());
                    }
                    "open_tg" => {
                        let link = tauri::async_runtime::block_on(async {
                            let st = app.state::<Arc<AppState>>();
                            current_tg_link(st.inner()).await
                        });
                        if let Err(e) = app.opener().open_url(link, None::<&str>) {
                            log::warn!("Tray open link failed: {:?}", e);
                        }
                    }
                    "copy_link" => {
                        let link = tauri::async_runtime::block_on(async {
                            let st = app.state::<Arc<AppState>>();
                            current_tg_link(st.inner()).await
                        });
                        if let Err(e) = app.clipboard().write_text(link) {
                            log::warn!("Tray copy link failed: {:?}", e);
                        }
                    }
                    "restart" => {
                        let handle = app.clone();
                        tauri::async_runtime::spawn(async move {
                            let st = handle.state::<Arc<AppState>>().inner().clone();
                            stop_bridge(&st).await;
                            let cfg = { st.config.lock().await.clone() };
                            let addr = format!("{}:{}", cfg.host, cfg.port);
                            if let Err(e) = std::net::TcpListener::bind(&addr) {
                                let diag = tg_ws_proxy_core::diagnose_bind_error(
                                    &e,
                                    &cfg.host,
                                    cfg.port,
                                );
                                log::error!("Tray restart failed: {}", diag);
                                let _ = handle.emit("proxy-error", diag);
                                return;
                            }
                            start_bridge(handle, st, cfg).await;
                        });
                    }
                    "open_logs" => {
                        let path = get_log_file_path(app);
                        if !path.exists() {
                            let _ = fs::write(&path, "");
                        }
                        let _ = app.opener().open_path(path.to_string_lossy(), None::<&str>);
                    }
                    _ => {}
                })
                .on_tray_icon_event(|tray, event| {
                    if let TrayIconEvent::Click {
                        button: MouseButton::Left,
                        button_state: MouseButtonState::Up,
                        ..
                    } = event
                    {
                        show_main_window(tray.app_handle());
                    }
                });
            // Use the bundled app icon for the tray so the logo is visible
            // instead of a blank/default glyph.
            if let Some(icon) = app.default_window_icon().cloned() {
                tray_builder = tray_builder.icon(icon);
            }
            let _tray = tray_builder.build(app)?;
            }

            // Re-write an existing autostart entry so builds upgraded from
            // <=1.0.0 also get the `--autostart` flag (the old entry would
            // still open the window on login once).
            #[cfg(desktop)]
            {
                use tauri_plugin_autostart::ManagerExt;
                if app.autolaunch().is_enabled().unwrap_or(false) {
                    let refreshed = app.autolaunch().enable();
                    if let Err(e) = refreshed {
                        log::warn!("Could not refresh autostart arguments: {e}");
                    }
                }
            }

            // The window starts hidden (avoids a flash on login). Normal
            // launches show it; autostart launches stay in the tray.
            if !launched_from_autostart {
                #[cfg(desktop)]
                show_main_window(app.handle());
                #[cfg(mobile)]
                if let Some(window) = app.get_webview_window("main") {
                    let _ = window.show();
                }
            } else {
                log::info!("Started from autostart: staying in the tray");
            }

            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            get_config,
            save_config,
            is_running,
            check_port,
            clear_last_error,
            start_proxy,
            stop_proxy,
            restart_proxy,
            get_app_version,
            get_platform,
            is_first_run,
            get_link_host,
            get_log_tail,
            open_log_file,
            clear_logs,
            open_url,
            minimize_window,
            toggle_maximize_window,
            close_window,
            test_connectivity,
            check_updates
        ])
        .run(tauri::generate_context!())
        .expect("error while building tauri application");
}
