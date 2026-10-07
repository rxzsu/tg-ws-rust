//! File logging with size-based rotation and domain censoring.
//!
//! Logs are written to `proxy.log` in the Tauri log dir. Before writing,
//! user-configured Cloudflare / Worker domains are masked so the log file
//! can be safely attached to a GitHub issue.

use std::fs::{self, OpenOptions};
use std::io;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, RwLock};

use tracing_subscriber::EnvFilter;

/// In-memory list of sensitive domain substrings to mask in logs.
static SENSITIVE_DOMAINS: RwLock<Vec<String>> = RwLock::new(Vec::new());

/// Replace the sensitive domain list (called on startup and on config save).
pub fn set_sensitive_domains(domains: Vec<String>) {
    let mut cleaned: Vec<String> = domains
        .into_iter()
        .map(|d| d.trim().to_string())
        .filter(|d| !d.is_empty())
        .collect();
    cleaned.sort();
    cleaned.dedup();
    if let Ok(mut guard) = SENSITIVE_DOMAINS.write() {
        *guard = cleaned;
    }
}

/// Mask every known sensitive domain inside `text`.
pub fn censor_text(text: &str) -> String {
    let guard = match SENSITIVE_DOMAINS.read() {
        Ok(g) => g,
        Err(_) => return text.to_string(),
    };
    if guard.is_empty() {
        return text.to_string();
    }
    let mut out = text.to_string();
    for domain in guard.iter() {
        if !domain.is_empty() && out.contains(domain) {
            out = out.replace(domain, "***");
        }
    }
    out
}

fn rotate_if_needed(path: &Path, max_bytes: u64) {
    let Ok(meta) = fs::metadata(path) else {
        return;
    };
    if meta.len() < max_bytes {
        return;
    }
    let backup = path.with_extension("log.1");
    let _ = fs::remove_file(&backup);
    let _ = fs::rename(path, &backup);
}

struct RotatingFile {
    file: fs::File,
    path: PathBuf,
    max_bytes: u64,
}

impl RotatingFile {
    fn open(path: &Path, max_bytes: u64) -> io::Result<Self> {
        if let Some(parent) = path.parent() {
            let _ = fs::create_dir_all(parent);
        }
        rotate_if_needed(path, max_bytes);
        let file = OpenOptions::new().create(true).append(true).open(path)?;
        Ok(Self {
            file,
            path: path.to_path_buf(),
            max_bytes,
        })
    }

    fn write_censored(&mut self, buf: &[u8]) -> io::Result<usize> {
        // Rotate *before* writing when the file already exceeds the limit.
        if let Ok(meta) = self.file.metadata()
            && meta.len() >= self.max_bytes
        {
            drop(fs::rename(&self.path, self.path.with_extension("log.1")));
            self.file = OpenOptions::new()
                .create(true)
                .write(true)
                .truncate(true)
                .open(&self.path)?;
        }
        let text = String::from_utf8_lossy(buf);
        let censored = censor_text(&text);
        use std::io::Write;
        self.file.write_all(censored.as_bytes())?;
        self.file.flush()?;
        Ok(buf.len())
    }
}

/// Shared, thread-safe writer for the tracing fmt layer.
#[derive(Clone)]
pub struct LogFileWriter {
    inner: Arc<Mutex<RotatingFile>>,
}

impl LogFileWriter {
    pub fn new(path: PathBuf, max_mb: u32) -> io::Result<Self> {
        let max_bytes = (u64::from(max_mb.clamp(1, 500))) * 1024 * 1024;
        Ok(Self {
            inner: Arc::new(Mutex::new(RotatingFile::open(&path, max_bytes)?)),
        })
    }
}

impl io::Write for LogFileWriter {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        match self.inner.lock() {
            Ok(mut guard) => guard.write_censored(buf),
            Err(_) => Err(io::Error::other("log writer lock poisoned")),
        }
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

impl<'a> tracing_subscriber::fmt::MakeWriter<'a> for LogFileWriter {
    type Writer = LogFileWriter;
    fn make_writer(&'a self) -> Self::Writer {
        self.clone()
    }
}

/// Initialise global tracing output: file (censored, rotated) + stderr.
///
/// Safe to call once at startup; subsequent calls are ignored (`try_init`).
pub fn init_logging(log_path: PathBuf, max_mb: u32, verbose: bool, sensitive: Vec<String>) {
    set_sensitive_domains(sensitive);
    let writer = match LogFileWriter::new(log_path, max_mb) {
        Ok(w) => w,
        Err(_) => return,
    };
    let default_filter = if verbose {
        "info,tg_ws_proxy_core=debug"
    } else {
        "info,tg_ws_proxy_core=info"
    };
    let filter = EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new(default_filter));
    let _ = tracing_subscriber::fmt()
        .with_env_filter(filter)
        .with_writer(writer)
        .with_ansi(false)
        .try_init();
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn censor_masks_domains() {
        set_sensitive_domains(vec!["proxy.example.com".to_string()]);
        let out = censor_text("connect to proxy.example.com via cf");
        assert!(!out.contains("proxy.example.com"));
        assert!(out.contains("***"));
        set_sensitive_domains(Vec::new());
    }
}
