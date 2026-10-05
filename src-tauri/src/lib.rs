use std::fs;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};
use tauri::{
    menu::{Menu, MenuItem},
    tray::{MouseButton, MouseButtonState, TrayIconBuilder, TrayIconEvent},
    AppHandle, Emitter, Manager, State,
};
use tg_ws_proxy_core::config::ProxyConfig;
use tg_ws_proxy_core::stats::{Stats, TelemetrySnapshot};
use tokio::sync::Mutex;

struct AppState {
    running: AtomicBool,
    config: Mutex<ProxyConfig>,
    stats: Arc<Stats>,
    shutdown_tx: Mutex<Option<tokio::sync::broadcast::Sender<()>>>,
}

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
    if let Ok(content) = fs::read_to_string(&path) {
        if let Ok(cfg) = serde_json::from_str::<ProxyConfig>(&content) {
            return cfg;
        }
    }
    // Check fallback location in APPDATA/TgWsProxy/config.json
    if let Ok(appdata) = std::env::var("APPDATA") {
        let legacy_path = PathBuf::from(appdata).join("TgWsProxy").join("config.json");
        if let Ok(content) = fs::read_to_string(&legacy_path) {
            if let Ok(cfg) = serde_json::from_str::<ProxyConfig>(&content) {
                return cfg;
            }
        }
    }
    ProxyConfig::default()
}

fn persist_config(app: &AppHandle, cfg: &ProxyConfig) {
    let path = get_config_path(app);
    if let Ok(serialized) = serde_json::to_string_pretty(cfg) {
        let _ = fs::write(path, serialized);
    }
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
    {
        let mut cfg = state.config.lock().await;
        *cfg = new_config.clone();
    }
    persist_config(&app, &new_config);

    // Seamlessly restart proxy bridge with updated settings
    {
        let mut tx_guard = state.shutdown_tx.lock().await;
        if let Some(tx) = tx_guard.take() {
            let _ = tx.send(());
        }
    }
    tokio::time::sleep(std::time::Duration::from_millis(150)).await;

    let (shutdown_tx, shutdown_rx) = tokio::sync::broadcast::channel(256);
    {
        let mut tx_guard = state.shutdown_tx.lock().await;
        *tx_guard = Some(shutdown_tx);
    }
    state.running.store(true, Ordering::SeqCst);

    let proxy_state = state.inner().clone();
    let stats = proxy_state.stats.clone();
    let cfg = new_config.clone();
    tauri::async_runtime::spawn(async move {
        if let Err(e) = tg_ws_proxy_core::run_server_with_stats(cfg, stats, shutdown_rx).await {
            log::error!("Server reload error: {:?}", e);
        }
        proxy_state.running.store(false, Ordering::SeqCst);
    });

    Ok(())
}

#[tauri::command]
async fn is_running(state: State<'_, Arc<AppState>>) -> Result<bool, String> {
    Ok(state.running.load(Ordering::SeqCst))
}

#[tauri::command]
async fn start_proxy(_app: AppHandle, state: State<'_, Arc<AppState>>) -> Result<String, String> {
    if state.running.load(Ordering::SeqCst) {
        return Ok("Already running".into());
    }

    let cfg = {
        let guard = state.config.lock().await;
        guard.clone()
    };

    let proxy_state = state.inner().clone();
    proxy_state.running.store(true, Ordering::SeqCst);
    let stats = proxy_state.stats.clone();

    let (shutdown_tx, shutdown_rx) = tokio::sync::broadcast::channel(256);
    {
        let mut tx_guard = proxy_state.shutdown_tx.lock().await;
        *tx_guard = Some(shutdown_tx);
    }

    tauri::async_runtime::spawn(async move {
        if let Err(e) = tg_ws_proxy_core::run_server_with_stats(cfg, stats, shutdown_rx).await {
            log::error!("Server error: {:?}", e);
        }
        proxy_state.running.store(false, Ordering::SeqCst);
    });

    Ok("Started".into())
}

#[tauri::command]
async fn stop_proxy(state: State<'_, Arc<AppState>>) -> Result<(), String> {
    let mut tx_guard = state.shutdown_tx.lock().await;
    if let Some(tx) = tx_guard.take() {
        let _ = tx.send(());
    }
    state.running.store(false, Ordering::SeqCst);
    state.stats.connections_active.store(0, Ordering::SeqCst);
    Ok(())
}

#[tauri::command]
async fn open_url(url: String) -> Result<(), String> {
    #[cfg(target_os = "windows")]
    {
        use std::ffi::OsStr;
        use std::os::windows::ffi::OsStrExt;

        #[link(name = "shell32")]
        unsafe extern "system" {
            fn ShellExecuteW(
                hwnd: *mut std::ffi::c_void,
                lpOperation: *const u16,
                lpFile: *const u16,
                lpParameters: *const u16,
                lpDirectory: *const u16,
                nShowCmd: i32,
            ) -> isize;
        }

        let url_wide: Vec<u16> = OsStr::new(&url).encode_wide().chain(std::iter::once(0)).collect();
        let op_wide: Vec<u16> = OsStr::new("open").encode_wide().chain(std::iter::once(0)).collect();

        let res = unsafe {
            ShellExecuteW(
                std::ptr::null_mut(),
                op_wide.as_ptr(),
                url_wide.as_ptr(),
                std::ptr::null(),
                std::ptr::null(),
                1, // SW_SHOWNORMAL
            )
        };

        if res <= 32 {
            // If direct ShellExecuteW failed, try t.me web redirect fallback
            let alt_url = if url.starts_with("tg://proxy?") {
                url.replace("tg://proxy?", "https://t.me/proxy?")
            } else {
                url.clone()
            };
            let alt_wide: Vec<u16> = OsStr::new(&alt_url).encode_wide().chain(std::iter::once(0)).collect();
            let alt_res = unsafe {
                ShellExecuteW(
                    std::ptr::null_mut(),
                    op_wide.as_ptr(),
                    alt_wide.as_ptr(),
                    std::ptr::null(),
                    std::ptr::null(),
                    1,
                )
            };
            if alt_res <= 32 {
                let ps_cmd = format!("Start-Process '{}'", alt_url.replace("'", "''"));
                let _ = std::process::Command::new("powershell")
                    .args(["-NoProfile", "-NonInteractive", "-Command", &ps_cmd])
                    .spawn();
            }
        }
    }
    #[cfg(not(target_os = "windows"))]
    {
        let _ = std::process::Command::new("xdg-open").arg(&url).spawn();
    }
    Ok(())
}

#[tauri::command]
async fn minimize_window(app: AppHandle) -> Result<(), String> {
    if let Some(w) = app.get_webview_window("main") {
        let _ = w.minimize();
    }
    Ok(())
}

#[tauri::command]
async fn toggle_maximize_window(app: AppHandle) -> Result<(), String> {
    if let Some(w) = app.get_webview_window("main") {
        if w.is_maximized().unwrap_or(false) {
            let _ = w.unmaximize();
        } else {
            let _ = w.maximize();
        }
    }
    Ok(())
}

#[tauri::command]
async fn close_window(app: AppHandle) -> Result<(), String> {
    if let Some(w) = app.get_webview_window("main") {
        let _ = w.hide();
    }
    Ok(())
}

#[derive(serde::Serialize, Clone)]
pub struct DcTestResult {
    pub dc: String,
    pub host: String,
    pub ok: bool,
    pub latency_ms: u64,
    pub message: String,
}

#[tauri::command]
async fn test_connectivity(custom_domains: Vec<String>) -> Result<Vec<DcTestResult>, String> {
    use tg_ws_proxy_core::raw_websocket::RawWebSocket;
    let mut targets = Vec::new();

    targets.push(("DC1".to_string(), "kws1.web.telegram.org".to_string()));
    targets.push(("DC2".to_string(), "kws2.web.telegram.org".to_string()));
    targets.push(("DC3".to_string(), "kws3.web.telegram.org".to_string()));
    targets.push(("DC4".to_string(), "kws4.web.telegram.org".to_string()));
    targets.push(("DC5".to_string(), "kws5.web.telegram.org".to_string()));
    targets.push(("DC203".to_string(), "kws2.web.telegram.org".to_string()));

    for d in custom_domains {
        let clean = d.trim().to_string();
        if !clean.is_empty() {
            targets.push(("Custom".to_string(), clean));
        }
    }

    let mut handles = Vec::new();
    for (dc, host) in targets {
        handles.push(tokio::spawn(async move {
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
                        latency_ms: ms,
                        message: format!("{} ms (101 OK)", ms),
                    }
                }
                Ok(Err(e)) => {
                    let ms = start.elapsed().as_millis() as u64;
                    DcTestResult {
                        dc,
                        host,
                        ok: false,
                        latency_ms: ms,
                        message: format!("Ошибка: {}", e),
                    }
                }
                Err(_) => {
                    DcTestResult {
                        dc,
                        host,
                        ok: false,
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
    match tg_ws_proxy_core::balancer::check_latest_github_release().await {
        Ok((tag, url)) => {
            let tag_clean = tag.trim_start_matches('v').trim();
            let cur_clean = current_version.trim_start_matches('v').trim();

            let has_update = !tag_clean.is_empty() && tag_clean != cur_clean;
            Ok(UpdateCheckResult {
                has_update,
                current_version,
                latest_version: tag,
                release_url: url,
            })
        }
        Err(e) => {
            log::warn!("GitHub update check failed: {:?}", e);
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

#[tauri::command]
async fn get_telemetry(state: State<'_, Arc<AppState>>) -> Result<TelemetrySnapshot, String> {
    Ok(state.stats.snapshot(0.0, 0.0))
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    let stats = Arc::new(Stats::new());
    let state = Arc::new(AppState {
        running: AtomicBool::new(false),
        config: Mutex::new(ProxyConfig::default()),
        stats: stats.clone(),
        shutdown_tx: Mutex::new(None),
    });

    tauri::Builder::default()
        .plugin(tauri_plugin_autostart::Builder::new().build())
        .manage(state.clone())
        .setup(move |app| {
            if cfg!(debug_assertions) {
                app.handle().plugin(
                    tauri_plugin_log::Builder::default()
                        .level(log::LevelFilter::Info)
                        .build(),
                )?;
            }

            // Load persisted config
            let saved_cfg = load_saved_config(app.handle());
            let app_state_clone = app.state::<Arc<AppState>>().inner().clone();
            let initial_cfg = saved_cfg.clone();
            tauri::async_runtime::block_on(async {
                let mut cfg = app_state_clone.config.lock().await;
                *cfg = saved_cfg;
            });

            // Automatically launch proxy bridge on startup
            let auto_state = app.state::<Arc<AppState>>().inner().clone();
            tauri::async_runtime::spawn(async move {
                auto_state.running.store(true, Ordering::SeqCst);
                let stats = auto_state.stats.clone();
                let (shutdown_tx, shutdown_rx) = tokio::sync::broadcast::channel(256);
                {
                    let mut tx_guard = auto_state.shutdown_tx.lock().await;
                    *tx_guard = Some(shutdown_tx);
                }
                if let Err(e) = tg_ws_proxy_core::run_server_with_stats(initial_cfg, stats, shutdown_rx).await {
                    log::error!("Auto-start server error: {:?}", e);
                }
                auto_state.running.store(false, Ordering::SeqCst);
            });

            // Telemetry broadcast loop (every 1 second)
            let app_handle = app.handle().clone();
            let stats_bg = stats.clone();
            tauri::async_runtime::spawn(async move {
                let mut prev_up = 0u64;
                let mut prev_down = 0u64;
                let mut prev_time = Instant::now();

                loop {
                    tokio::time::sleep(Duration::from_secs(1)).await;
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

            // Create Tray Menu
            let quit_i = MenuItem::with_id(app, "quit", "Выход", true, None::<&str>)?;
            let show_i = MenuItem::with_id(app, "show", "Показать окно", true, None::<&str>)?;
            let menu = Menu::with_items(app, &[&show_i, &quit_i])?;

            let _tray = TrayIconBuilder::new()
                .menu(&menu)
                .show_menu_on_left_click(false)
                .on_menu_event(|app, event| match event.id.as_ref() {
                    "quit" => {
                        app.exit(0);
                    }
                    "show" => {
                        if let Some(window) = app.get_webview_window("main") {
                            let _ = window.show();
                            let _ = window.set_focus();
                        }
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
                        let app = tray.app_handle();
                        if let Some(window) = app.get_webview_window("main") {
                            let _ = window.show();
                            let _ = window.set_focus();
                        }
                    }
                })
                .build(app)?;

            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            get_config,
            save_config,
            is_running,
            start_proxy,
            stop_proxy,
            get_telemetry,
            get_link_host,
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
