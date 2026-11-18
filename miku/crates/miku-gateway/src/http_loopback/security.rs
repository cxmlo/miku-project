use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

use miku_protocol::{constant_time_token_eq, sha256_hex};

static TOKEN_SEQ: AtomicU64 = AtomicU64::new(1);

pub fn generate_ephemeral_session_token() -> String {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    let pid = std::process::id();
    let seq = TOKEN_SEQ.fetch_add(1, Ordering::SeqCst);
    let seed = format!("miku-gw:{}:{}:{}:loopback", pid, nanos, seq);
    sha256_hex(seed.as_bytes())
}

pub fn is_allowed_loopback_host(host_header: Option<&str>) -> bool {
    let Some(raw) = host_header.map(str::trim).filter(|s| !s.is_empty()) else {
        return false;
    };
    let host_only = if raw.starts_with('[') {
        match raw.find(']') {
            Some(end) => &raw[1..end],
            None => return false,
        }
    } else if let Some((h, port)) = raw.rsplit_once(':') {
        if port.chars().all(|c| c.is_ascii_digit()) {
            h
        } else {
            return false;
        }
    } else {
        raw
    };

    let lower = host_only.to_ascii_lowercase();
    matches!(
        lower.as_str(),
        "127.0.0.1" | "localhost" | "::1" | "miku.localhost"
    )
}

/// Validates private/link-local/Tailscale LAN IP `Host` headers when Phone Link LAN pairing is enabled.
/// Rejects arbitrary domain names and public IPs to prevent DNS rebinding / external probing.
pub fn is_allowed_lan_host(host_header: Option<&str>) -> bool {
    let Some(raw) = host_header.map(str::trim).filter(|s| !s.is_empty()) else {
        return false;
    };
    let host_only = if raw.starts_with('[') {
        match raw.find(']') {
            Some(end) => {
                let after = &raw[end + 1..];
                if !after.is_empty() {
                    let Some(port) = after.strip_prefix(':') else {
                        return false;
                    };
                    if port.is_empty() || !port.chars().all(|c| c.is_ascii_digit()) {
                        return false;
                    }
                }
                &raw[1..end]
            }
            None => return false,
        }
    } else if let Some((h, port)) = raw.rsplit_once(':') {
        if !port.is_empty() && port.chars().all(|c| c.is_ascii_digit()) {
            h
        } else {
            return false;
        }
    } else {
        raw
    };

    let Ok(ip) = host_only.parse::<std::net::IpAddr>() else {
        return false;
    };
    match ip {
        std::net::IpAddr::V4(v4) => {
            let oct = v4.octets();
            let is_cgnat = oct[0] == 100 && (64..=127).contains(&oct[1]);
            v4.is_private() || v4.is_link_local() || is_cgnat
        }
        std::net::IpAddr::V6(v6) => {
            let seg0 = v6.segments()[0];
            (seg0 & 0xffc0) == 0xfe80 || (seg0 & 0xfe00) == 0xfc00
        }
    }
}

pub fn is_allowed_origin(origin_header: Option<&str>) -> bool {
    let Some(raw) = origin_header.map(str::trim).filter(|s| !s.is_empty()) else {
        return true;
    };
    let rest = if let Some(r) = raw.strip_prefix("http://") {
        r
    } else if let Some(r) = raw.strip_prefix("https://") {
        r
    } else {
        return false;
    };

    let authority = rest.split('/').next().unwrap_or("");
    is_allowed_loopback_host(Some(authority))
}

pub fn extract_session_token(
    headers: &[(String, String)],
    query: Option<&str>,
) -> Option<String> {
    for (k, v) in headers {
        if k == "x-miku-token" {
            let trimmed = v.trim();
            if !trimmed.is_empty() {
                return Some(trimmed.to_string());
            }
        }
        if k == "authorization" {
            if let Some(bearer) = v.trim().strip_prefix("Bearer ") {
                let b = bearer.trim();
                if !b.is_empty() {
                    return Some(b.to_string());
                }
            }
        }
    }

    if let Some(q) = query {
        for pair in q.split('&') {
            if let Some((k, v)) = pair.split_once('=') {
                if k == "token" && !v.trim().is_empty() {
                    return Some(v.trim().to_string());
                }
            }
        }
    }

    None
}

pub fn has_valid_session_token(
    headers: &[(String, String)],
    query: Option<&str>,
    expected: &str,
) -> bool {
    let Some(provided) = extract_session_token(headers, query) else {
        return false;
    };
    constant_time_token_eq(expected, &provided)
}

pub fn sanitize_relative_ui_path(raw_path: &str) -> Result<String, &'static str> {
    let trimmed = raw_path.trim();
    if trimmed.contains('\0')
        || trimmed.contains('\\')
        || trimmed.contains("://")
        || trimmed.contains('%')
    {
        return Err("forbidden characters in UI asset path");
    }

    let rel = trimmed.trim_matches('/');
    if rel.is_empty() {
        return Ok("index.html".to_string());
    }

    for seg in rel.split('/') {
        if seg.is_empty() || seg == "." || seg == ".." {
            return Err("path traversal segment rejected");
        }
        if !seg
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '_' | '-' | '.'))
        {
            return Err("invalid characters in UI path segment");
        }
    }

    Ok(rel.to_string())
}
