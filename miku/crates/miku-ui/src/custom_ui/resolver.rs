use crate::prelude::*;

pub const REQUIRED_CUSTOM_UI_FILES: &[&str] = &[
    "README.md",
    "shared/theme.css",
    "shared/mascot/mascot.config.json",
    "shared/mascot/mascot.js",
    "shared/mascot/playground.js",
    "shared/audio/sound-fx.js",
    "shared/audio/sounds/manifest.json",
    "shared/locales/es.json",
    "shared/locales/en.json",
    "shared/core/qr.js",
    "shared/core/state.js",
    "shared/core/components.js",
    "shared/core/bridge.js",
    "island/index.html",
    "island/mascot.html",
    "island/island.css",
    "island/app.js",
    "island/scenes/renderers.js",
    "island/scenes/chat.js",
    "island/scenes/settings.js",
    "settings/miku_settings.html",
    "settings/settings.css",
    "settings/particles_header.js",
    "settings/settings_app.js",
];

fn expand_modular_asset_candidates(clean_rel: &str) -> Vec<String> {
    let mut cands = Vec::with_capacity(3);
    if let Some(rest) = clean_rel.strip_prefix("shared/") {
        cands.push(clean_rel.to_string());
        cands.push(rest.to_string());
    } else if let Some(rest) = clean_rel.strip_prefix("island/") {
        cands.push(clean_rel.to_string());
        cands.push(rest.to_string());
    } else if let Some(rest) = clean_rel.strip_prefix("settings/") {
        cands.push(clean_rel.to_string());
        cands.push(rest.to_string());
    } else if clean_rel == "theme.css"
        || clean_rel.starts_with("mascot/")
        || clean_rel.starts_with("audio/")
        || clean_rel.starts_with("core/")
        || clean_rel.starts_with("locales/")
        || clean_rel.starts_with("fonts/")
    {
        cands.push(format!("shared/{}", clean_rel));
        cands.push(clean_rel.to_string());
    } else if matches!(clean_rel, "index.html" | "mascot.html" | "island.css" | "app.js")
        || clean_rel.starts_with("scenes/")
    {
        cands.push(format!("island/{}", clean_rel));
        cands.push(clean_rel.to_string());
    } else if matches!(
        clean_rel,
        "miku_settings.html" | "settings.css" | "particles_header.js" | "settings_app.js"
    ) {
        cands.push(format!("settings/{}", clean_rel));
        cands.push(clean_rel.to_string());
    } else {
        cands.push(clean_rel.to_string());
    }
    cands
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ResolvedUiAsset {
    pub relative_path: String,
    pub resolved_disk_path: PathBuf,
    pub content_type: String,
    pub from_user_override: bool,
    pub bytes: Vec<u8>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CustomUiTreeReport {
    pub base_dir: PathBuf,
    pub file_count: usize,
    pub total_bytes: usize,
    pub json_files_validated: usize,
    pub uses_strict_self_csp: bool,
    pub has_external_cdn_links: bool,
    pub mascot_is_decoupled: bool,
}

#[derive(Debug, Clone)]
pub struct CustomUiResolver {
    base_dir: PathBuf,
    user_override_dir: Option<PathBuf>,
}

impl CustomUiResolver {
    pub fn new(base_dir: PathBuf, user_override_dir: Option<PathBuf>) -> Self {
        Self {
            base_dir,
            user_override_dir,
        }
    }

    pub fn from_defaults() -> Self {
        Self::new(
            resolve_custom_ui_base_dir(),
            resolve_user_ui_override_dir(),
        )
    }

    pub fn base_dir(&self) -> &Path {
        &self.base_dir
    }

    pub fn user_override_dir(&self) -> Option<&Path> {
        self.user_override_dir.as_deref()
    }

    pub fn resolve_route(&self, raw_route: &str) -> Result<ResolvedUiAsset, UiError> {
        let trimmed = raw_route.split(['?', '#']).next().unwrap_or("").trim();
        let normalized = trimmed.trim_matches('/');
        let rel = match normalized {
            "" | "index.html" | "island" | "island/index.html" => "index.html",
            "mascot" | "mascot.html" | "island/mascot" | "island/mascot.html" => "mascot.html",
            "settings" | "miku_settings.html" | "settings/miku_settings.html" => {
                "settings/miku_settings.html"
            }
            other => other,
        };
        self.resolve_asset(rel)
    }

    pub fn resolve_asset(&self, rel_path: &str) -> Result<ResolvedUiAsset, UiError> {
        let clean_rel = validate_relative_ui_path(rel_path)?;
        let content_type = mime_for_ui_extension(&clean_rel)?.to_string();
        let candidates = expand_modular_asset_candidates(&clean_rel);

        if let Some(override_root) = &self.user_override_dir {
            for cand in &candidates {
                if let Some(hit) = try_read_jailed_asset(override_root, cand)? {
                    return Ok(ResolvedUiAsset {
                        relative_path: clean_rel,
                        resolved_disk_path: hit.0,
                        content_type,
                        from_user_override: true,
                        bytes: hit.1,
                    });
                }
            }
        }

        for cand in &candidates {
            if let Some(hit) = try_read_jailed_asset(&self.base_dir, cand)? {
                return Ok(ResolvedUiAsset {
                    relative_path: clean_rel,
                    resolved_disk_path: hit.0,
                    content_type,
                    from_user_override: false,
                    bytes: hit.1,
                });
            }
        }

        Err(UiError::AssetCatalogError(format!(
            "custom UI asset '{}' not found in base dir '{}'",
            clean_rel,
            self.base_dir.display()
        )))
    }
}

pub fn resolve_custom_ui_base_dir() -> PathBuf {
    let manifest = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let workspace_ui = manifest
        .parent()
        .and_then(|p| p.parent())
        .map(|p| p.join("assets").join("ui"));
    if let Some(dir) = workspace_ui {
        if dir.is_dir() {
            return dir;
        }
    }
    if let Ok(exe) = std::env::current_exe() {
        let mut cur = exe.parent();
        while let Some(dir) = cur {
            let cand = dir.join("assets").join("ui");
            if cand.is_dir() {
                return cand;
            }
            let cand2 = dir.join("miku").join("assets").join("ui");
            if cand2.is_dir() {
                return cand2;
            }
            cur = dir.parent();
        }
    }
    PathBuf::from("miku/assets/ui")
}

pub fn resolve_user_ui_override_dir() -> Option<PathBuf> {
    std::env::var_os("LOCALAPPDATA")
        .or_else(|| std::env::var_os("XDG_CONFIG_HOME"))
        .map(|root| PathBuf::from(root).join("Miku").join("ui"))
}

fn validate_relative_ui_path(raw: &str) -> Result<String, UiError> {
    let s = raw.trim();
    if s.is_empty()
        || s.contains('\0')
        || s.contains(':')
        || s.contains('\\')
        || s.starts_with('/')
    {
        return Err(UiError::UnsafeNavigationBlocked(format!(
            "unsafe custom UI asset path '{}'",
            raw
        )));
    }
    let lower = s.to_ascii_lowercase();
    if lower.contains("..") || lower.contains("%2e") || lower.contains("%2f") || lower.contains("%5c")
    {
        return Err(UiError::UnsafeNavigationBlocked(format!(
            "path traversal blocked for custom UI asset '{}'",
            raw
        )));
    }
    let mut normalized_parts = Vec::new();
    for seg in s.split('/') {
        if seg.is_empty() || seg == "." || seg == ".." {
            return Err(UiError::UnsafeNavigationBlocked(format!(
                "invalid path segment in custom UI asset '{}'",
                raw
            )));
        }
        normalized_parts.push(seg);
    }
    Ok(normalized_parts.join("/"))
}

fn mime_for_ui_extension(rel_path: &str) -> Result<&'static str, UiError> {
    let ext = Path::new(rel_path)
        .extension()
        .and_then(|e| e.to_str())
        .map(|s| s.to_ascii_lowercase())
        .unwrap_or_default();
    match ext.as_str() {
        "html" => Ok("text/html; charset=utf-8"),
        "css" => Ok("text/css; charset=utf-8"),
        "js" => Ok("application/javascript; charset=utf-8"),
        "json" => Ok("application/json; charset=utf-8"),
        "svg" => Ok("image/svg+xml"),
        "png" => Ok("image/png"),
        "wav" => Ok("audio/wav"),
        "mp3" => Ok("audio/mpeg"),
        "ogg" => Ok("audio/ogg"),
        "woff2" => Ok("font/woff2"),
        "woff" => Ok("font/woff"),
        "ttf" => Ok("font/ttf"),
        "md" | "txt" => Ok("text/plain; charset=utf-8"),
        other => Err(UiError::UnsafeNavigationBlocked(format!(
            "unsupported custom UI extension '.{}'",
            other
        ))),
    }
}

fn try_read_jailed_asset(root: &Path, rel: &str) -> Result<Option<(PathBuf, Vec<u8>)>, UiError> {
    if !root.is_dir() {
        return Ok(None);
    }
    let canon_root = fs::canonicalize(root).map_err(|e| {
        UiError::AssetCatalogError(format!("canonicalize root {}: {}", root.display(), e))
    })?;
    let candidate = root.join(rel);
    if !candidate.is_file() {
        return Ok(None);
    }
    let canon_target = fs::canonicalize(&candidate).map_err(|e| {
        UiError::UnsafeNavigationBlocked(format!("canonicalize {}: {}", candidate.display(), e))
    })?;
    if !canon_target.starts_with(&canon_root) {
        return Err(UiError::UnsafeNavigationBlocked(format!(
            "custom UI escape blocked: '{}'",
            canon_target.display()
        )));
    }
    let bytes = fs::read(&canon_target).map_err(|e| {
        UiError::AssetCatalogError(format!("read {}: {}", canon_target.display(), e))
    })?;
    Ok(Some((canon_target, bytes)))
}

pub fn validate_custom_ui_tree(base_dir: &Path) -> Result<CustomUiTreeReport, UiError> {
    if !base_dir.is_dir() {
        return Err(UiError::AssetCatalogError(format!(
            "custom UI dir missing: {}",
            base_dir.display()
        )));
    }
    let resolver = CustomUiResolver::new(base_dir.to_path_buf(), None);
    let mut total_bytes = 0usize;
    let mut json_files_validated = 0usize;

    for rel in REQUIRED_CUSTOM_UI_FILES {
        let asset = resolver.resolve_asset(rel)?;
        total_bytes += asset.bytes.len();
        let text = String::from_utf8(asset.bytes).map_err(|e| {
            UiError::AssetCatalogError(format!("non-UTF-8 custom UI text '{}': {}", rel, e))
        })?;

        if rel.ends_with(".json") {
            serde_json::from_str::<serde_json::Value>(&text).map_err(|e| {
                UiError::AssetCatalogError(format!("invalid JSON in '{}': {}", rel, e))
            })?;
            json_files_validated += 1;
        }

        let lower = text.to_ascii_lowercase();
        for forbidden_net in [
            "fonts.googleapis.com",
            "fonts.gstatic.com",
            "cdn.jsdelivr.net",
            "unpkg.com",
            "http://",
        ] {
            if lower.contains(forbidden_net) {
                return Err(UiError::CspViolation(format!(
                    "custom UI '{}' references forbidden external URL '{}'",
                    rel, forbidden_net
                )));
            }
        }

        if rel.ends_with(".html") {
            if !lower.contains("content-security-policy") || !lower.contains("script-src 'self'") {
                return Err(UiError::CspViolation(format!(
                    "HTML file '{}' missing strict script-src 'self' CSP meta",
                    rel
                )));
            }
            for forbidden_attr in [
                "onclick=",
                "onload=",
                "onerror=",
                "oninput=",
                "onchange=",
                "<script>",
            ] {
                if lower.contains(forbidden_attr) {
                    return Err(UiError::CspViolation(format!(
                        "HTML file '{}' contains CSP-forbidden '{}'",
                        rel, forbidden_attr
                    )));
                }
            }
        }

        if rel.ends_with(".js") {
            for forbidden_js in [
                "eval(",
                "new function(",
                "document.write(",
                "miku_secure_key_2026",
                "server.py",
                "pywebview",
                "localhost:8000",
                "\"8000\"",
                "(demo) recib",
            ] {
                if lower.contains(forbidden_js) {
                    return Err(UiError::CspViolation(format!(
                        "JS module '{}' contains forbidden legacy/unsafe pattern '{}'",
                        rel, forbidden_js
                    )));
                }
            }
        }
    }

    let mascot_js = String::from_utf8(resolver.resolve_asset("mascot/mascot.js")?.bytes)
        .map_err(|e| UiError::AssetCatalogError(e.to_string()))?;
    let mascot_is_decoupled = mascot_js.contains("MikuMascot")
        && mascot_js.contains("window.buddy")
        && !mascot_js.contains("window.Bridge")
        && !mascot_js.contains("fetch(")
        && !mascot_js.contains("miku_agents");

    if !mascot_is_decoupled {
        return Err(UiError::AssetCatalogError(
            "mascot/mascot.js must expose MikuMascot + buddy and stay decoupled from Bridge/panels"
                .to_string(),
        ));
    }

    Ok(CustomUiTreeReport {
        base_dir: base_dir.to_path_buf(),
        file_count: REQUIRED_CUSTOM_UI_FILES.len(),
        total_bytes,
        json_files_validated,
        uses_strict_self_csp: true,
        has_external_cdn_links: false,
        mascot_is_decoupled,
    })
}
