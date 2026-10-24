use serde::{Deserialize, Serialize};
use std::collections::HashMap;

pub const PROTO_ABRIDGED_INT: u32 = 0xEFEFEFEF;
pub const PROTO_INTERMEDIATE_INT: u32 = 0xEEEEEEEE;
pub const PROTO_PADDED_INTERMEDIATE_INT: u32 = 0xDDDDDDDD;

pub const PROTO_TAG_ABRIDGED: [u8; 4] = [0xef, 0xef, 0xef, 0xef];
pub const PROTO_TAG_INTERMEDIATE: [u8; 4] = [0xee, 0xee, 0xee, 0xee];
pub const PROTO_TAG_SECURE: [u8; 4] = [0xdd, 0xdd, 0xdd, 0xdd];

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProxyConfig {
    pub host: String,
    pub port: u16,
    pub secret: String,
    pub dc_redirects: HashMap<i32, String>,
    pub buffer_size: usize,
    pub pool_size: usize,
    pub fallback_cfproxy: bool,
    pub cfproxy_user_domain_enabled: bool,
    pub cfproxy_user_domains: Vec<String>,
    pub cfproxy_worker_enabled: bool,
    pub cfproxy_worker_domains: Vec<String>,
    pub cfproxy_h2_media: bool,
    pub disable_secure: bool,
    pub fake_tls_domain: String,
    pub proxy_protocol: bool,
    pub force_test_dc: bool,
    pub verbose: bool,
    pub log_max_mb: u32,
    #[serde(default = "default_pool_max_age_secs")]
    pub pool_max_age_secs: u64,
    /// Keep-alive ping interval for pooled sockets in seconds.
    /// `0` disables pings entirely (some relays do not answer WS pings).
    #[serde(default = "default_pool_keepalive_secs")]
    pub pool_keepalive_secs: u64,
    #[serde(default = "default_true")]
    pub splitter_enabled: bool,
    #[serde(default = "default_connect_timeout_secs")]
    pub connect_timeout_secs: u64,
    /// Use the built-in third-party Cloudflare domain pool (and its hourly
    /// GitHub refresh). Traffic through it leaves the user's control, so it
    /// can be turned off; with no user domains/workers the fallback chain
    /// then goes straight to direct TCP.
    #[serde(default = "default_true")]
    pub use_builtin_cf_domains: bool,
    /// Upstream transport: "auto" (WS, then QUIC/h3 relays if set, then
    /// fallbacks), "ws" (WebSocket only), "quic" (custom-ALPN QUIC relays
    /// first), "h3" (HTTP/3 relays first).
    #[serde(default = "default_transport")]
    pub transport: String,
    /// User-run QUIC relays as `host:port` (experimental, v1.0 phase 3).
    #[serde(default)]
    pub quic_relays: Vec<String>,
    /// Encrypted Client Hello (ECH) on upstream TLS connections: hides the
    /// real SNI (`kws*.web.telegram.org`, user CF domains) from DPI by
    /// encrypting the ClientHello with the ECH config published in the
    /// domain's DNS HTTPS record. Falls back to plain TLS automatically when
    /// a domain offers no ECH config or the handshake is rejected.
    #[serde(default = "default_true")]
    pub ech_enabled: bool,
}

fn default_pool_max_age_secs() -> u64 {
    120
}

fn default_pool_keepalive_secs() -> u64 {
    15
}

fn default_true() -> bool {
    true
}

fn default_connect_timeout_secs() -> u64 {
    10
}

fn default_transport() -> String {
    "auto".to_string()
}

impl Default for ProxyConfig {
    fn default() -> Self {
        let mut dc_redirects = HashMap::new();
        dc_redirects.insert(2, "149.154.167.220".to_string());
        dc_redirects.insert(4, "149.154.167.220".to_string());

        let mut rng_bytes = [0u8; 16];
        rand::RngExt::fill(&mut rand::rng(), &mut rng_bytes);
        let secret = hex::encode(rng_bytes);

        Self {
            host: "127.0.0.1".to_string(),
            port: 1443,
            secret,
            dc_redirects,
            buffer_size: 256 * 1024,
            pool_size: 4,
            fallback_cfproxy: true,
            cfproxy_user_domain_enabled: false,
            cfproxy_user_domains: Vec::new(),
            cfproxy_worker_enabled: false,
            cfproxy_worker_domains: Vec::new(),
            cfproxy_h2_media: true,
            disable_secure: false,
            fake_tls_domain: String::new(),
            proxy_protocol: false,
            force_test_dc: false,
            verbose: false,
            log_max_mb: 20,
            pool_max_age_secs: default_pool_max_age_secs(),
            pool_keepalive_secs: default_pool_keepalive_secs(),
            splitter_enabled: default_true(),
            connect_timeout_secs: default_connect_timeout_secs(),
            use_builtin_cf_domains: true,
            transport: default_transport(),
            quic_relays: Vec::new(),
            ech_enabled: true,
        }
    }
}

/// Reject configurations that would silently misbehave.
///
/// Everything here has burned a real user at least once: a mistyped secret
/// used to fall back to an all-zero key (the proxy "worked" but nothing
/// connected), a 0 port bound a random one, and relay typos were only
/// discovered at connect time deep inside the fallback chain.
pub fn validate(cfg: &ProxyConfig) -> Result<(), String> {
    if cfg.host.trim().is_empty() {
        return Err("Хост не может быть пустым".into());
    }
    if cfg.host.parse::<std::net::IpAddr>().is_err() && cfg.host != "localhost" {
        return Err(format!(
            "Хост «{}» — нужен IP-адрес (например 127.0.0.1) или localhost",
            cfg.host
        ));
    }

    if cfg.port == 0 {
        return Err("Порт не может быть 0".into());
    }

    let secret = cfg.secret.trim();
    if secret.len() != 32 || hex::decode(secret).is_err() {
        return Err("Секрет должен быть 32 hex-символа (16 байт)".into());
    }

    if !(4096..=1024 * 1024).contains(&cfg.buffer_size) {
        return Err(format!(
            "Размер буфера {} вне диапазона 4 КБ – 1 МБ",
            cfg.buffer_size
        ));
    }

    if !(2..=120).contains(&cfg.connect_timeout_secs) {
        return Err("Таймаут подключения должен быть от 2 до 120 секунд".into());
    }

    if cfg.pool_max_age_secs < 10 {
        return Err("Время жизни пула должно быть не меньше 10 секунд".into());
    }

    if cfg.pool_keepalive_secs > 3600 {
        return Err("Интервал keep-alive пула должен быть не больше 3600 секунд (0 — выключено)".into());
    }

    for relay in &cfg.quic_relays {
        let relay = relay.trim();
        if relay.is_empty() {
            continue;
        }
        match relay.rsplit_once(':') {
            Some((host, port)) if !host.is_empty() && port.parse::<u16>().is_ok() => {}
            _ => {
                return Err(format!(
                    "Релей «{}» — нужен формат host:port (например relay.example.com:443)",
                    relay
                ))
            }
        }
    }

    for (dc, ip) in &cfg.dc_redirects {
        if ip.parse::<std::net::IpAddr>().is_err() {
            return Err(format!("DC{}: «{}» — не IP-адрес", dc, ip));
        }
    }

    Ok(())
}

pub fn get_default_dc_ip(dc: i32, is_test: bool) -> Option<&'static str> {
    if is_test {
        match dc {
            1 => Some("149.154.175.10"),
            2 => Some("149.154.167.40"),
            3 => Some("149.154.175.117"),
            _ => None,
        }
    } else {
        match dc {
            1 => Some("149.154.175.50"),
            2 | 203 => Some("149.154.167.51"),
            3 => Some("149.154.175.100"),
            4 => Some("149.154.167.91"),
            5 => Some("149.154.171.5"),
            _ => None,
        }
    }
}

/// DCs whose `/apiws` WebSocket front actually exists.
///
/// Measured against the shared front (149.154.167.220): DC2 and DC4 answer
/// `101 Switching Protocols`, while DC1/DC3/DC5 answer `302 → core.telegram.org`
/// — that front simply does not proxy `/apiws` for them, and their own
/// per-DC IPs (149.154.17x.100) are DPI-blackholed on most regional networks.
/// Trying them costs a full `connect_timeout` of dead air per attempt and
/// leaves the client in a reconnect loop.
/// Normalize protocol DC ids to the canonical production DC number:
/// test DCs are encoded as `10000 + dc` and DC203 is an alias of DC2.
pub fn normalize_dc(dc: i32) -> i32 {
    let stripped = if dc >= 10000 { dc - 10000 } else { dc };
    if stripped == 203 { 2 } else { stripped }
}

pub fn has_ws_front(dc: i32) -> bool {
    matches!(normalize_dc(dc), 2 | 4)
}

pub fn ws_domains(dc: i32, is_media: bool) -> Vec<String> {
    // Test DCs (`10002`, …) and DC203 resolve to production DC2; without
    // this the whole WS route was skipped for them even though `kws2`
    // answers there (see `only_dc2_and_dc4_have_a_ws_front`).
    let resolved_dc = normalize_dc(dc);
    if !has_ws_front(resolved_dc) {
        return Vec::new();
    }
    if !is_media {
        vec![format!("kws{}.web.telegram.org", resolved_dc)]
    } else {
        vec![
            format!("kws{}-1.web.telegram.org", resolved_dc),
            format!("kws{}.web.telegram.org", resolved_dc),
        ]
    }
}

pub fn get_link_host(host: &str) -> String {
    if host != "0.0.0.0" {
        return host.to_string();
    }
    if let Ok(socket) = std::net::UdpSocket::bind("0.0.0.0:0")
        && socket.connect("8.8.8.8:80").is_ok()
        && let Ok(addr) = socket.local_addr()
    {
        let ip = addr.ip().to_string();
        if ip != "0.0.0.0" {
            return ip;
        }
    }
    "127.0.0.1".to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Configs saved by v0.2.0 lack `pool_max_age_secs` — they must still
    /// parse (with the default) instead of resetting user settings.
    #[test]
    fn legacy_config_without_pool_max_age_parses() {
        let legacy = r#"{
            "host": "127.0.0.1",
            "port": 1443,
            "secret": "abcd",
            "dc_redirects": {},
            "buffer_size": 262144,
            "pool_size": 4,
            "fallback_cfproxy": true,
            "cfproxy_user_domain_enabled": false,
            "cfproxy_user_domains": [],
            "cfproxy_worker_enabled": false,
            "cfproxy_worker_domains": [],
            "cfproxy_h2_media": true,
            "disable_secure": false,
            "fake_tls_domain": "",
            "proxy_protocol": false,
            "force_test_dc": false,
            "verbose": false,
            "log_max_mb": 20
        }"#;
        let cfg: ProxyConfig = serde_json::from_str(legacy).expect("legacy config must parse");
        assert_eq!(cfg.pool_max_age_secs, default_pool_max_age_secs());
        assert_eq!(cfg.pool_keepalive_secs, default_pool_keepalive_secs());
        assert!(cfg.splitter_enabled);
        assert_eq!(cfg.connect_timeout_secs, default_connect_timeout_secs());
        assert_eq!(cfg.transport, default_transport());
        assert!(cfg.quic_relays.is_empty());
        assert_eq!(cfg.port, 1443);
    }

    #[test]
    fn default_config_is_valid() {
        validate(&ProxyConfig::default()).expect("default config must pass validation");
    }

    #[test]
    fn only_dc2_and_dc4_have_a_ws_front() {
        assert!(has_ws_front(2));
        assert!(has_ws_front(4));
        // DC203 and test DCs are aliases of DC2 and therefore reachable.
        assert!(has_ws_front(203));
        assert!(has_ws_front(10002));
        for dc in [1, 3, 5] {
            assert!(!has_ws_front(dc), "DC{} must have no WS front", dc);
        }
        assert_eq!(ws_domains(1, false), Vec::<String>::new());
        assert_eq!(ws_domains(3, true), Vec::<String>::new());
        assert_eq!(ws_domains(2, false), vec!["kws2.web.telegram.org"]);
        assert_eq!(
            ws_domains(4, true),
            vec!["kws4-1.web.telegram.org", "kws4.web.telegram.org"]
        );
        // Test DCs collapse onto DC2 and therefore stay reachable.
        assert_eq!(ws_domains(10002, false), vec!["kws2.web.telegram.org"]);
        assert_eq!(ws_domains(203, false), vec!["kws2.web.telegram.org"]);
    }

    #[test]
    fn rejects_bad_secret() {
        let mut cfg = ProxyConfig {
            secret: "not-hex".into(),
            ..Default::default()
        };
        assert!(validate(&cfg).unwrap_err().contains("Секрет"));

        // 32 chars but not hex.
        cfg.secret = "zz".repeat(16);
        assert!(validate(&cfg).is_err());

        // Correct length, wrong case is still hex.
        cfg.secret = "AABBCCDDEEFF00112233445566778899".into();
        assert!(validate(&cfg).is_ok());
    }

    #[test]
    fn rejects_bad_host_and_port() {
        let mut cfg = ProxyConfig {
            host: "127.0.0.1:1443".into(),
            ..Default::default()
        };
        assert!(validate(&cfg).is_err());

        cfg.host = "localhost".into();
        assert!(validate(&cfg).is_ok());

        cfg.port = 0;
        assert!(validate(&cfg).is_err());
    }

    #[test]
    fn rejects_bad_relay_and_dc_ip() {
        let mut cfg = ProxyConfig {
            quic_relays: vec!["relay.example.com".into()],
            ..Default::default()
        };
        assert!(validate(&cfg).unwrap_err().contains("host:port"));

        cfg.quic_relays = vec!["relay.example.com:443".into()];
        assert!(validate(&cfg).is_ok());

        cfg.dc_redirects.insert(2, "not-an-ip".into());
        assert!(validate(&cfg).unwrap_err().contains("IP"));
    }
}

