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
        }
    }
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

pub fn ws_domains(dc: i32, is_media: bool) -> Vec<String> {
    let resolved_dc = if dc == 203 { 2 } else { dc };
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
    if host == "0.0.0.0" {
        if let Ok(socket) = std::net::UdpSocket::bind("0.0.0.0:0") {
            if socket.connect("8.8.8.8:80").is_ok() {
                if let Ok(addr) = socket.local_addr() {
                    let ip = addr.ip().to_string();
                    if ip != "0.0.0.0" {
                        return ip;
                    }
                }
            }
        }
        "127.0.0.1".to_string()
    } else {
        host.to_string()
    }
}

