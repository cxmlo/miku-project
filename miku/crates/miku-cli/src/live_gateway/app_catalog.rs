//! Lookup tables and Windows Start Menu / PATH app discovery for `app_launcher`.

use std::collections::HashSet;
use std::env;
use std::fs;
use std::path::{Path, PathBuf};

use super::app_launcher::{strip_es_accents, ResolvedTarget};

pub(super) fn known_website_url(key: &str) -> Option<(&'static str, &'static str)> {
    match key {
        "pinterest" => Some(("Pinterest", "https://www.pinterest.com/")),
        "youtube" | "yt" => Some(("YouTube", "https://www.youtube.com/")),
        "gmail" | "correo gmail" => Some(("Gmail", "https://mail.google.com/")),
        "google" | "buscador" => Some(("Google", "https://www.google.com/")),
        "github" | "gh" => Some(("GitHub", "https://github.com/")),
        "chatgpt" => Some(("ChatGPT", "https://chatgpt.com/")),
        "claude" => Some(("Claude", "https://claude.ai/")),
        "gemini" => Some(("Gemini", "https://gemini.google.com/")),
        "reddit" => Some(("Reddit", "https://www.reddit.com/")),
        "twitch" => Some(("Twitch", "https://www.twitch.tv/")),
        "netflix" => Some(("Netflix", "https://www.netflix.com/")),
        "instagram" | "ig" => Some(("Instagram", "https://www.instagram.com/")),
        "facebook" | "fb" => Some(("Facebook", "https://www.facebook.com/")),
        "twitter" | "x" => Some(("X", "https://x.com/")),
        "tiktok" => Some(("TikTok", "https://www.tiktok.com/")),
        "linkedin" => Some(("LinkedIn", "https://www.linkedin.com/")),
        "google drive" | "drive" => Some(("Google Drive", "https://drive.google.com/")),
        "google maps" | "maps" => Some(("Google Maps", "https://maps.google.com/")),
        "traductor" | "translate" | "google translate" => {
            Some(("Google Translate", "https://translate.google.com/"))
        }
        "canva" => Some(("Canva", "https://www.canva.com/")),
        "perplexity" => Some(("Perplexity", "https://www.perplexity.ai/")),
        _ => None,
    }
}

pub(super) fn known_app_web_fallback(key: &str) -> Option<(&'static str, &'static str)> {
    match key {
        "discord" => Some(("Discord", "https://discord.com/app")),
        "spotify" => Some(("Spotify", "https://open.spotify.com/")),
        "whatsapp" => Some(("WhatsApp", "https://web.whatsapp.com/")),
        "telegram" => Some(("Telegram", "https://web.telegram.org/")),
        "notion" => Some(("Notion", "https://www.notion.so/")),
        "figma" => Some(("Figma", "https://www.figma.com/")),
        "slack" => Some(("Slack", "https://app.slack.com/")),
        _ => None,
    }
}

fn collect_start_menu_shortcuts(dir: &Path, depth: usize, out: &mut Vec<(String, PathBuf)>) {
    if depth > 3 || !dir.is_dir() {
        return;
    }
    let Ok(entries) = fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            collect_start_menu_shortcuts(&path, depth + 1, out);
        } else if path
            .extension()
            .and_then(|e| e.to_str())
            .map(|e| e.eq_ignore_ascii_case("lnk"))
            .unwrap_or(false)
        {
            if let Some(stem) = path.file_stem().and_then(|s| s.to_str()) {
                out.push((strip_es_accents(stem), path));
            }
        }
    }
}

pub(super) fn find_windows_start_menu_app(query_lower: &str) -> Option<(String, PathBuf)> {
    let mut dirs = Vec::new();
    if let Ok(appdata) = env::var("APPDATA") {
        dirs.push(PathBuf::from(appdata).join(r"Microsoft\Windows\Start Menu\Programs"));
    }
    if let Ok(pd) = env::var("ProgramData") {
        dirs.push(PathBuf::from(pd).join(r"Microsoft\Windows\Start Menu\Programs"));
    }
    let mut shortcuts = Vec::new();
    for d in dirs {
        collect_start_menu_shortcuts(&d, 0, &mut shortcuts);
    }
    for (stem_low, path) in &shortcuts {
        if stem_low == query_lower {
            let label = path
                .file_stem()
                .and_then(|s| s.to_str())
                .unwrap_or(query_lower)
                .to_string();
            return Some((label, path.clone()));
        }
    }
    let mut best: Option<(usize, String, PathBuf)> = None;
    for (stem_low, path) in shortcuts {
        if stem_low.contains("uninstall") || stem_low.contains("desinstalar") {
            continue;
        }
        if stem_low.contains(query_lower) {
            let score = stem_low.len();
            if best.as_ref().map(|b| score < b.0).unwrap_or(true) {
                let label = path
                    .file_stem()
                    .and_then(|s| s.to_str())
                    .unwrap_or(query_lower)
                    .to_string();
                best = Some((score, label, path));
            }
        }
    }
    best.map(|(_, label, path)| (label, path))
}

pub(super) fn find_known_windows_desktop_app(query_lower: &str) -> Option<ResolvedTarget> {
    if query_lower == "discord" {
        if let Ok(local) = env::var("LOCALAPPDATA") {
            let updater = PathBuf::from(&local).join("Discord").join("Update.exe");
            if updater.is_file() {
                return Some(ResolvedTarget::AppPath {
                    label: "Discord".to_string(),
                    path: updater.to_string_lossy().to_string(),
                    args: Some("--processStart Discord.exe".to_string()),
                });
            }
        }
    }

    if matches!(
        query_lower,
        "configuracion" | "ajustes windows" | "windows settings"
    ) && cfg!(target_os = "windows")
    {
        return Some(ResolvedTarget::Protocol {
            label: "Configuración de Windows".to_string(),
            uri: "ms-settings:".to_string(),
        });
    }

    let (label, exe): (&str, &str) = match query_lower {
        "bloc de notas" | "notepad" => ("Bloc de notas", "notepad.exe"),
        "calculadora" | "calculator" | "calc" => ("Calculadora", "calc.exe"),
        "explorador" | "explorador de archivos" | "explorer" | "archivos" => {
            ("Explorador", "explorer.exe")
        }
        "paint" => ("Paint", "mspaint.exe"),
        "terminal" | "windows terminal" => ("Terminal", "wt.exe"),
        _ => return None,
    };

    if cfg!(target_os = "windows") && find_exe_on_path(exe).is_some() {
        return Some(ResolvedTarget::AppPath {
            label: label.to_string(),
            path: exe.to_string(),
            args: None,
        });
    }
    None
}

pub(super) fn find_exe_on_path(exe: &str) -> Option<PathBuf> {
    let mut dirs: Vec<PathBuf> = Vec::new();
    if let Ok(sys) = env::var("SystemRoot") {
        dirs.push(PathBuf::from(&sys).join("System32"));
        dirs.push(PathBuf::from(sys));
    }
    if let Some(paths) = env::var_os("PATH") {
        dirs.extend(env::split_paths(&paths));
    }
    let mut seen = HashSet::new();
    for dir in dirs {
        if !seen.insert(dir.clone()) {
            continue;
        }
        let cand = dir.join(exe);
        if cand.is_file() {
            return Some(cand);
        }
    }
    None
}
