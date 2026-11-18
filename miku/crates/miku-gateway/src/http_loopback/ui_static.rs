use std::env;
use std::fs;
use std::path::{Path, PathBuf};

use super::security::sanitize_relative_ui_path;
use super::types::HttpResponse;

pub const LOOPBACK_UI_CSP: &str = "default-src 'self'; script-src 'self'; style-src 'self' 'unsafe-inline'; font-src 'self' data:; img-src 'self' data: blob:; connect-src 'self' ws://127.0.0.1:47400 ws://localhost:* wss: data: blob:";
const BUILTIN_FAVICON_SVG: &[u8] = br##"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 32 32"><defs><radialGradient id="g" cx="35%" cy="30%" r="70%"><stop offset="0%" stop-color="#ffffff"/><stop offset="65%" stop-color="#38bdf8"/><stop offset="100%" stop-color="#0e7490"/></radialGradient></defs><circle cx="16" cy="16" r="14" fill="url(#g)"/></svg>"##;

pub fn resolve_default_ui_dir() -> PathBuf {
    let manifest_rel = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("..")
        .join("..")
        .join("assets")
        .join("ui");
    if manifest_rel.is_dir() {
        return manifest_rel;
    }

    if let Ok(exe) = env::current_exe() {
        if let Some(dir) = exe.parent() {
            let sidecar = dir.join("assets").join("ui");
            if sidecar.is_dir() {
                return sidecar;
            }
        }
    }

    PathBuf::from("miku").join("assets").join("ui")
}

pub fn resolve_user_ui_override_dir() -> Option<PathBuf> {
    if let Some(local_app) = env::var_os("LOCALAPPDATA") {
        let p = PathBuf::from(local_app).join("Miku").join("ui");
        if p.is_dir() {
            return Some(p);
        }
    }
    if let Some(xdg) = env::var_os("XDG_CONFIG_HOME") {
        let p = PathBuf::from(xdg).join("miku").join("ui");
        if p.is_dir() {
            return Some(p);
        }
    }
    None
}

pub fn mime_type_for_path(rel_path: &str) -> &'static str {
    if rel_path.ends_with(".html") {
        "text/html; charset=utf-8"
    } else if rel_path.ends_with(".js") || rel_path.ends_with(".mjs") {
        "application/javascript; charset=utf-8"
    } else if rel_path.ends_with(".css") {
        "text/css; charset=utf-8"
    } else if rel_path.ends_with(".json") {
        "application/json; charset=utf-8"
    } else if rel_path.ends_with(".svg") {
        "image/svg+xml"
    } else if rel_path.ends_with(".png") {
        "image/png"
    } else if rel_path.ends_with(".wav") {
        "audio/wav"
    } else if rel_path.ends_with(".mp3") {
        "audio/mpeg"
    } else if rel_path.ends_with(".ogg") {
        "audio/ogg"
    } else if rel_path.ends_with(".woff2") {
        "font/woff2"
    } else if rel_path.ends_with(".woff") {
        "font/woff"
    } else if rel_path.ends_with(".ttf") {
        "font/ttf"
    } else {
        "application/octet-stream"
    }
}

fn read_jailed_asset(root: &Path, rel_path: &str) -> Result<Option<Vec<u8>>, &'static str> {
    if !root.is_dir() {
        return Ok(None);
    }
    let root_canon = root.canonicalize().map_err(|_| "invalid asset root")?;
    let mut candidate = root_canon.clone();
    for seg in rel_path.split('/') {
        candidate.push(seg);
    }
    if !candidate.exists() {
        return Ok(None);
    }
    let cand_canon = candidate.canonicalize().map_err(|_| "canonicalize failed")?;
    if !cand_canon.starts_with(&root_canon) || !cand_canon.is_file() {
        return Err("path escaped root jail");
    }
    let bytes = fs::read(&cand_canon).map_err(|_| "read failed")?;
    Ok(Some(bytes))
}

fn expand_ui_asset_candidates(raw_rel: &str) -> (String, Vec<String>) {
    let canonical = match raw_rel {
        "" | "index.html" | "island" | "island/index.html" => "index.html",
        "mascot" | "mascot.html" | "island/mascot" | "island/mascot.html" => "mascot.html",
        "settings" | "miku_settings.html" | "settings/miku_settings.html" => {
            "settings/miku_settings.html"
        }
        other => other,
    };

    let mut cands = Vec::with_capacity(3);
    if let Some(rest) = canonical.strip_prefix("shared/") {
        cands.push(canonical.to_string());
        cands.push(rest.to_string());
    } else if let Some(rest) = canonical.strip_prefix("island/") {
        cands.push(canonical.to_string());
        cands.push(rest.to_string());
    } else if let Some(rest) = canonical.strip_prefix("settings/") {
        cands.push(canonical.to_string());
        cands.push(rest.to_string());
    } else if canonical == "theme.css"
        || canonical.starts_with("mascot/")
        || canonical.starts_with("audio/")
        || canonical.starts_with("core/")
        || canonical.starts_with("locales/")
        || canonical.starts_with("fonts/")
    {
        cands.push(format!("shared/{}", canonical));
        cands.push(canonical.to_string());
    } else if matches!(
        canonical,
        "index.html" | "mascot.html" | "island.css" | "app.js"
    ) || canonical.starts_with("scenes/")
    {
        cands.push(format!("island/{}", canonical));
        cands.push(canonical.to_string());
    } else if matches!(
        canonical,
        "miku_settings.html" | "settings.css" | "particles_header.js" | "settings_app.js"
    ) {
        cands.push(format!("settings/{}", canonical));
        cands.push(canonical.to_string());
    } else {
        cands.push(canonical.to_string());
    }
    (canonical.to_string(), cands)
}

pub fn serve_ui_asset(
    raw_path: &str,
    user_override_dir: Option<&Path>,
    default_ui_dir: &Path,
    session_token: &str,
) -> HttpResponse {
    let rel_path = match sanitize_relative_ui_path(raw_path) {
        Ok(p) => p,
        Err(msg) => return HttpResponse::text(403, "Forbidden", msg),
    };
    let (canonical_rel, candidates) = expand_ui_asset_candidates(&rel_path);

    if let Some(user_dir) = user_override_dir {
        for cand in &candidates {
            match read_jailed_asset(user_dir, cand) {
                Ok(Some(bytes)) => {
                    return build_asset_response(
                        &canonical_rel,
                        bytes,
                        session_token,
                        "user-override",
                    );
                }
                Ok(None) => {}
                Err(msg) => return HttpResponse::text(403, "Forbidden", msg),
            }
        }
    }

    for cand in &candidates {
        match read_jailed_asset(default_ui_dir, cand) {
            Ok(Some(bytes)) => {
                return build_asset_response(&canonical_rel, bytes, session_token, "bundled");
            }
            Ok(None) => {}
            Err(msg) => return HttpResponse::text(403, "Forbidden", msg),
        }
    }

    if canonical_rel == "favicon.ico" || canonical_rel == "favicon.svg" {
        return build_asset_response(
            "favicon.svg",
            BUILTIN_FAVICON_SVG.to_vec(),
            session_token,
            "builtin",
        );
    }

    HttpResponse::text(404, "Not Found", "Asset not found")
}

fn build_asset_response(
    rel_path: &str,
    body: Vec<u8>,
    session_token: &str,
    source_tag: &str,
) -> HttpResponse {
    let content_type = mime_type_for_path(rel_path);
    let mut headers = vec![
        ("Content-Security-Policy".to_string(), LOOPBACK_UI_CSP.to_string()),
        ("X-Content-Type-Options".to_string(), "nosniff".to_string()),
        ("X-Frame-Options".to_string(), "DENY".to_string()),
        ("Cache-Control".to_string(), "no-store".to_string()),
        ("X-Miku-Ui-Source".to_string(), source_tag.to_string()),
    ];

    if rel_path.ends_with(".html") {
        headers.push((
            "Set-Cookie".to_string(),
            format!("miku_token={}; SameSite=Strict; Path=/", session_token),
        ));
    }

    HttpResponse {
        status: 200,
        reason: "OK",
        content_type,
        extra_headers: headers,
        body,
    }
}
