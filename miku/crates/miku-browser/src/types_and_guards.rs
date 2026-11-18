use std::fmt;
use std::path::{Component, Path, PathBuf};

use serde::{Deserialize, Serialize};

pub const MAX_DOM_TEXT_EXCERPT_BYTES: usize = 32 * 1024;
pub const DEFAULT_MAX_DOWNLOAD_BYTES: usize = 32 * 1024 * 1024;

pub const DANGEROUS_DOWNLOAD_EXTENSIONS: &[&str] = &[
    "exe", "msi", "msp", "bat", "cmd", "ps1", "psm1", "vbs", "vbe", "js", "jse", "wsf", "wsh",
    "scr", "com", "dll", "sys", "drv", "hta", "lnk", "reg", "pif", "cpl", "jar", "appx", "msix",
];

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BrowserProtocolKind {
    GeckoBiDi,
    ChromiumCdp,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct BrowserPageSnapshot {
    pub url: String,
    pub host: String,
    pub title: String,
    pub dom_text: String,
    pub protocol_used: BrowserProtocolKind,
    pub isolated_profile_used: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DownloadReceipt {
    pub file_name: String,
    pub canonical_path: String,
    pub bytes_written: usize,
    pub source_url: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BrowserError {
    InvalidUrl(String),
    BlockedSsrfOrScheme(String),
    DangerousDownloadBlocked(String),
    DownloadOutsideJail(String),
    PersonalProfileViolation(String),
    ProtocolError(String),
    Io(String),
    KillSwitchAborted,
}

impl fmt::Display for BrowserError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            BrowserError::InvalidUrl(msg) => write!(f, "invalid URL: {msg}"),
            BrowserError::BlockedSsrfOrScheme(msg) => {
                write!(f, "blocked URL scheme or SSRF host: {msg}")
            }
            BrowserError::DangerousDownloadBlocked(msg) => {
                write!(f, "dangerous download blocked: {msg}")
            }
            BrowserError::DownloadOutsideJail(msg) => {
                write!(f, "download path escaped jail: {msg}")
            }
            BrowserError::PersonalProfileViolation(msg) => {
                write!(f, "personal browser profile protection triggered: {msg}")
            }
            BrowserError::ProtocolError(msg) => write!(f, "browser protocol error: {msg}"),
            BrowserError::Io(msg) => write!(f, "browser I/O error: {msg}"),
            BrowserError::KillSwitchAborted => write!(f, "browser task aborted by kill switch"),
        }
    }
}

impl std::error::Error for BrowserError {}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DownloadJailPolicy {
    pub canonical_jail_dir: PathBuf,
    pub max_download_bytes: usize,
}

impl DownloadJailPolicy {
    pub fn new(jail_dir: impl AsRef<Path>) -> Result<Self, BrowserError> {
        let dir = jail_dir.as_ref();
        std::fs::create_dir_all(dir)
            .map_err(|e| BrowserError::Io(format!("create download jail failed: {e}")))?;
        let canonical = std::fs::canonicalize(dir)
            .map_err(|e| BrowserError::Io(format!("canonicalize download jail failed: {e}")))?;
        Ok(Self {
            canonical_jail_dir: canonical,
            max_download_bytes: DEFAULT_MAX_DOWNLOAD_BYTES,
        })
    }
}

pub fn extract_url_host(raw_url: &str) -> Result<(String, String, u16), BrowserError> {
    let trimmed = raw_url.trim();
    let Some((scheme, rest)) = trimmed.split_once("://") else {
        return Err(BrowserError::InvalidUrl(
            "URL missing scheme (expected https://)".to_string(),
        ));
    };
    let scheme_lower = scheme.to_ascii_lowercase();
    if scheme_lower != "https" && scheme_lower != "http" {
        return Err(BrowserError::BlockedSsrfOrScheme(format!(
            "scheme '{scheme_lower}' is forbidden"
        )));
    }
    let authority = rest.split(['/', '?', '#']).next().unwrap_or("");
    if authority.is_empty() || authority.contains('@') {
        return Err(BrowserError::InvalidUrl(
            "empty host or unsupported userinfo in URL".to_string(),
        ));
    }
    let (host, port) = if let Some((h, p)) = authority.rsplit_once(':') {
        let port = p
            .parse::<u16>()
            .map_err(|_| BrowserError::InvalidUrl("invalid port".to_string()))?;
        (h.to_ascii_lowercase(), port)
    } else {
        (
            authority.to_ascii_lowercase(),
            if scheme_lower == "https" { 443 } else { 80 },
        )
    };
    Ok((scheme_lower, host, port))
}

pub fn validate_browser_url(
    raw_url: &str,
    allow_loopback_http: bool,
) -> Result<(String, String, u16), BrowserError> {
    let (scheme, host, port) = extract_url_host(raw_url)?;
    if host.is_empty() {
        return Err(BrowserError::InvalidUrl("empty host".to_string()));
    }
    let is_loopback = host == "127.0.0.1" || host == "localhost" || host == "[::1]";
    if scheme == "http" && !(allow_loopback_http && is_loopback) {
        return Err(BrowserError::BlockedSsrfOrScheme(
            "cleartext http:// is allowed only for local loopback tests".to_string(),
        ));
    }
    if !allow_loopback_http && is_loopback {
        return Err(BrowserError::BlockedSsrfOrScheme(
            "loopback host blocked unless explicitly enabled".to_string(),
        ));
    }
    // Block cloud metadata & link-local SSRF targets always
    if host == "169.254.169.254" || host.starts_with("169.254.") || host.ends_with(".internal") {
        return Err(BrowserError::BlockedSsrfOrScheme(format!(
            "metadata/link-local host '{host}' blocked"
        )));
    }
    Ok((scheme, host, port))
}

pub fn validate_download_target(
    policy: &DownloadJailPolicy,
    requested_filename: &str,
    payload_len: usize,
) -> Result<PathBuf, BrowserError> {
    let trimmed = requested_filename.trim();
    if trimmed.is_empty() || trimmed.contains('\0') {
        return Err(BrowserError::DownloadOutsideJail(
            "empty or null-byte filename".to_string(),
        ));
    }
    if payload_len > policy.max_download_bytes {
        return Err(BrowserError::DangerousDownloadBlocked(format!(
            "download size {payload_len} exceeds limit {}",
            policy.max_download_bytes
        )));
    }
    let rel = Path::new(trimmed);
    if rel.is_absolute() {
        return Err(BrowserError::DownloadOutsideJail(
            "absolute download path rejected".to_string(),
        ));
    }
    let mut clean = PathBuf::new();
    for comp in rel.components() {
        match comp {
            Component::Normal(seg) => clean.push(seg),
            Component::ParentDir | Component::RootDir | Component::Prefix(_) => {
                return Err(BrowserError::DownloadOutsideJail(format!(
                    "path traversal or root component in '{trimmed}'"
                )));
            }
            Component::CurDir => {}
        }
    }
    let file_name = clean
        .file_name()
        .and_then(|n| n.to_str())
        .ok_or_else(|| BrowserError::DownloadOutsideJail("missing filename".to_string()))?;

    // Check extension and double extensions (e.g. report.pdf.exe)
    let lower_name = file_name.to_ascii_lowercase();
    for ext in DANGEROUS_DOWNLOAD_EXTENSIONS {
        let dot_ext = format!(".{ext}");
        if lower_name.ends_with(&dot_ext) {
            return Err(BrowserError::DangerousDownloadBlocked(format!(
                "blocked dangerous executable/script extension '{dot_ext}' for '{file_name}'"
            )));
        }
    }

    let joined = policy.canonical_jail_dir.join(&clean);
    if let Some(parent) = joined.parent() {
        std::fs::create_dir_all(parent)
            .map_err(|e| BrowserError::Io(format!("create download subdir failed: {e}")))?;
        let canonical_parent = std::fs::canonicalize(parent)
            .map_err(|e| BrowserError::Io(format!("canonicalize parent failed: {e}")))?;
        if !canonical_parent.starts_with(&policy.canonical_jail_dir) {
            return Err(BrowserError::DownloadOutsideJail(
                "parent symlink escapes download jail".to_string(),
            ));
        }
    }
    Ok(joined)
}

pub fn sanitize_browser_text(raw: &str, max_bytes: usize) -> String {
    let mut out = String::with_capacity(raw.len().min(max_bytes));
    for ch in raw.chars() {
        if ch.is_control() && ch != '\n' && ch != '\t' {
            continue;
        }
        let next_len = out.len() + ch.len_utf8();
        if next_len > max_bytes {
            break;
        }
        out.push(ch);
    }
    out
}
