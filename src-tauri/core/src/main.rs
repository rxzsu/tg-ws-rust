use tg_ws_proxy_core::config::ProxyConfig;
use tg_ws_proxy_core::run_server;
use tracing_subscriber::EnvFilter;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    tracing_subscriber::fmt()
        .with_env_filter(
            EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| EnvFilter::new("info,tg_ws_proxy_core=debug")),
        )
        .init();

    let config = ProxyConfig::default();
    
    // Command-line or env overrides can be applied here
    println!("Starting tg-ws-proxy-core on {}:{}", config.host, config.port);
    println!("Secret: {}", config.secret);

    run_server(config).await
}
