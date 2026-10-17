// Prevents additional console window on Windows in release, DO NOT REMOVE!!
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

fn main() {
    // rustls >= 0.23.46 no longer auto-selects a process-wide crypto
    // provider when both `ring` (quinn/tokio-rustls) and `aws-lc-rs` (rcgen)
    // are present in the dependency tree. Pick `ring` explicitly, matching
    // the QUIC transport module.
    let _ = rustls::crypto::ring::default_provider().install_default();
    tg_ws_proxy_gui_lib::run();
}
