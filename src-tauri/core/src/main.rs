use tg_ws_proxy_core::config::ProxyConfig;
use tg_ws_proxy_core::run_server;
use tracing_subscriber::EnvFilter;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    // rustls >= 0.23.46 requires an explicit process-wide crypto provider
    // when both `ring` and `aws-lc-rs` are present in the tree.
    let _ = rustls::crypto::ring::default_provider().install_default();

    tracing_subscriber::fmt()
        .with_env_filter(
            EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| EnvFilter::new("info,tg_ws_proxy_core=debug")),
        )
        .init();

    let mut config = ProxyConfig::default();

    // Optional overrides for local runs (the GUI passes config directly).
    if let Ok(host) = std::env::var("TG_WS_HOST") {
        config.host = host;
    }
    if let Ok(port) = std::env::var("TG_WS_PORT") {
        config.port = port.parse().unwrap_or(config.port);
    }
    if let Ok(secret) = std::env::var("TG_WS_SECRET") {
        config.secret = secret;
    }
    if let Ok(pool) = std::env::var("TG_WS_POOL") {
        config.pool_size = pool.parse().unwrap_or(config.pool_size);
    }

    println!("Starting tg-ws-proxy-core on {}:{}", config.host, config.port);
    println!("Secret: {}", config.secret);

    run_server(config).await
}
