use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::types_and_guards::{BrowserError, BrowserProtocolKind};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BrowserEngineKind {
    Zen,
    Firefox,
    Edge,
    Chrome,
    Brave,
}

impl BrowserEngineKind {
    pub fn protocol(&self) -> BrowserProtocolKind {
        match self {
            BrowserEngineKind::Zen | BrowserEngineKind::Firefox => BrowserProtocolKind::GeckoBiDi,
            BrowserEngineKind::Edge | BrowserEngineKind::Chrome | BrowserEngineKind::Brave => {
                BrowserProtocolKind::ChromiumCdp
            }
        }
    }

    pub fn slug(&self) -> &'static str {
        match self {
            BrowserEngineKind::Zen => "zen",
            BrowserEngineKind::Firefox => "firefox",
            BrowserEngineKind::Edge => "edge",
            BrowserEngineKind::Chrome => "chrome",
            BrowserEngineKind::Brave => "brave",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct BrowserCandidate {
    pub engine: BrowserEngineKind,
    pub executable_path: String,
    pub protocol: BrowserProtocolKind,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct BrowserDiscoveryReport {
    pub windows_prog_id: Option<String>,
    pub selected: Option<BrowserCandidate>,
    pub installed_candidates: Vec<BrowserCandidate>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct BrowserLaunchSpec {
    pub engine: BrowserEngineKind,
    pub protocol: BrowserProtocolKind,
    pub executable_path: String,
    pub isolated_profile_dir: String,
    pub headless: bool,
    pub args: Vec<String>,
}

#[cfg(windows)]
fn read_windows_https_prog_id() -> Option<String> {
    use core::ffi::c_void;

    const HKEY_CURRENT_USER: isize = -2147483647_isize; // 0x80000001u32 as i32 as isize
    const KEY_READ: u32 = 0x20019;
    const ERROR_SUCCESS: i32 = 0;
    const REG_SZ: u32 = 1;

    #[link(name = "advapi32")]
    extern "system" {
        fn RegOpenKeyExW(
            hkey: isize,
            lp_sub_key: *const u16,
            ul_options: u32,
            sam_desired: u32,
            phk_result: *mut isize,
        ) -> i32;
        fn RegQueryValueExW(
            hkey: isize,
            lp_value_name: *const u16,
            lp_reserved: *mut u32,
            lp_type: *mut u32,
            lp_data: *mut u8,
            lpcb_data: *mut u32,
        ) -> i32;
        fn RegCloseKey(hkey: isize) -> i32;
    }

    let subkey: Vec<u16> =
        "Software\\Microsoft\\Windows\\Shell\\Associations\\UrlAssociations\\https\\UserChoice\0"
            .encode_utf16()
            .collect();
    let value_name: Vec<u16> = "ProgId\0".encode_utf16().collect();

    let mut hkey: isize = 0;
    let rc = unsafe { RegOpenKeyExW(HKEY_CURRENT_USER, subkey.as_ptr(), 0, KEY_READ, &mut hkey) };
    if rc != ERROR_SUCCESS || hkey == 0 {
        return None;
    }

    let mut data_type: u32 = 0;
    let mut byte_len: u32 = 512;
    let mut buf = vec![0u8; byte_len as usize];
    let qrc = unsafe {
        RegQueryValueExW(
            hkey,
            value_name.as_ptr(),
            std::ptr::null_mut::<c_void>().cast::<u32>(),
            &mut data_type,
            buf.as_mut_ptr(),
            &mut byte_len,
        )
    };
    unsafe {
        RegCloseKey(hkey);
    }
    if qrc != ERROR_SUCCESS || data_type != REG_SZ || byte_len < 2 {
        return None;
    }
    let u16_len = (byte_len as usize) / 2;
    let units: Vec<u16> = buf[..u16_len * 2]
        .as_chunks::<2>()
        .0
        .iter()
        .map(|c| u16::from_le_bytes(*c))
        .collect();
    let s = String::from_utf16_lossy(&units);
    let trimmed = s.trim_end_matches('\0').trim().to_string();
    if trimmed.is_empty() {
        None
    } else {
        Some(trimmed)
    }
}

#[cfg(not(windows))]
fn read_windows_https_prog_id() -> Option<String> {
    None
}

fn known_browser_locations() -> Vec<(BrowserEngineKind, PathBuf)> {
    let mut paths = Vec::new();
    let pf = std::env::var_os("ProgramFiles").map(PathBuf::from);
    let pf86 = std::env::var_os("ProgramFiles(x86)").map(PathBuf::from);
    let local = std::env::var_os("LOCALAPPDATA").map(PathBuf::from);

    if let Some(ref p) = pf {
        paths.push((BrowserEngineKind::Zen, p.join("Zen Browser\\zen.exe")));
        paths.push((
            BrowserEngineKind::Firefox,
            p.join("Mozilla Firefox\\firefox.exe"),
        ));
        paths.push((
            BrowserEngineKind::Chrome,
            p.join("Google\\Chrome\\Application\\chrome.exe"),
        ));
        paths.push((
            BrowserEngineKind::Brave,
            p.join("BraveSoftware\\Brave-Browser\\Application\\brave.exe"),
        ));
    }
    if let Some(ref p86) = pf86 {
        paths.push((
            BrowserEngineKind::Edge,
            p86.join("Microsoft\\Edge\\Application\\msedge.exe"),
        ));
    }
    if let Some(ref l) = local {
        paths.push((
            BrowserEngineKind::Zen,
            l.join("Programs\\Zen Browser\\zen.exe"),
        ));
        paths.push((
            BrowserEngineKind::Chrome,
            l.join("Google\\Chrome\\Application\\chrome.exe"),
        ));
    }
    paths
}

pub fn detect_installed_browsers() -> Vec<BrowserCandidate> {
    let mut found = Vec::new();
    for (engine, path) in known_browser_locations() {
        if path.exists() {
            let exe = path.to_string_lossy().to_string();
            if !found
                .iter()
                .any(|c: &BrowserCandidate| c.executable_path == exe)
            {
                found.push(BrowserCandidate {
                    engine,
                    executable_path: exe,
                    protocol: engine.protocol(),
                });
            }
        }
    }
    found
}

fn preferred_engine_from_prog_id(prog_id: &str) -> Option<BrowserEngineKind> {
    let upper = prog_id.to_ascii_uppercase();
    if upper.contains("ZEN") {
        Some(BrowserEngineKind::Zen)
    } else if upper.contains("FIREFOX") {
        // Note: Zen Browser on Windows often registers a FirefoxURL-* ProgId (see ADR-009)
        Some(BrowserEngineKind::Zen)
    } else if upper.contains("MSEDGE") {
        Some(BrowserEngineKind::Edge)
    } else if upper.contains("CHROME") {
        Some(BrowserEngineKind::Chrome)
    } else if upper.contains("BRAVE") {
        Some(BrowserEngineKind::Brave)
    } else {
        None
    }
}

pub fn discover_default_windows_browser() -> BrowserDiscoveryReport {
    let installed = detect_installed_browsers();
    let prog_id = read_windows_https_prog_id();
    let preferred = prog_id.as_deref().and_then(preferred_engine_from_prog_id);

    let selected = preferred
        .and_then(|eng| installed.iter().find(|c| c.engine == eng).cloned())
        .or_else(|| {
            if preferred == Some(BrowserEngineKind::Zen) {
                installed
                    .iter()
                    .find(|c| c.engine == BrowserEngineKind::Firefox)
                    .cloned()
            } else {
                None
            }
        })
        .or_else(|| installed.first().cloned());

    BrowserDiscoveryReport {
        windows_prog_id: prog_id,
        selected,
        installed_candidates: installed,
    }
}

pub fn forbidden_personal_profile_roots() -> Vec<PathBuf> {
    let mut roots = Vec::new();
    if let Some(roaming) = std::env::var_os("APPDATA").map(PathBuf::from) {
        roots.push(roaming.join("zen"));
        roots.push(roaming.join("Mozilla\\Firefox"));
    }
    if let Some(local) = std::env::var_os("LOCALAPPDATA").map(PathBuf::from) {
        roots.push(local.join("Microsoft\\Edge\\User Data"));
        roots.push(local.join("Google\\Chrome\\User Data"));
        roots.push(local.join("BraveSoftware\\Brave-Browser\\User Data"));
    }
    roots
}

pub fn is_personal_profile_protected(candidate_profile: &Path) -> bool {
    let candidate_str = candidate_profile
        .to_string_lossy()
        .replace('/', "\\")
        .to_ascii_lowercase();
    for forbidden in forbidden_personal_profile_roots() {
        let f_str = forbidden
            .to_string_lossy()
            .replace('/', "\\")
            .to_ascii_lowercase();
        if candidate_str == f_str || candidate_str.starts_with(&format!("{f_str}\\")) {
            return false;
        }
    }
    true
}

pub fn resolve_isolated_profile_dir(
    base_dir: Option<&Path>,
    engine: BrowserEngineKind,
) -> Result<PathBuf, BrowserError> {
    let root = match base_dir {
        Some(p) => p.to_path_buf(),
        None => std::env::var_os("LOCALAPPDATA")
            .map(PathBuf::from)
            .unwrap_or_else(std::env::temp_dir)
            .join("miku")
            .join("browser-profile"),
    };
    let dir = root.join(engine.slug());
    if !is_personal_profile_protected(&dir) {
        return Err(BrowserError::PersonalProfileViolation(format!(
            "profile '{}' overlaps user personal browser profile",
            dir.display()
        )));
    }
    std::fs::create_dir_all(&dir)
        .map_err(|e| BrowserError::Io(format!("create isolated browser profile failed: {e}")))?;
    Ok(dir)
}

pub fn build_isolated_launch_spec(
    candidate: &BrowserCandidate,
    profile_base_dir: Option<&Path>,
    remote_port: u16,
    headless: bool,
) -> Result<BrowserLaunchSpec, BrowserError> {
    let isolated_dir = resolve_isolated_profile_dir(profile_base_dir, candidate.engine)?;
    let profile_str = isolated_dir.to_string_lossy().to_string();
    let mut args = Vec::new();
    match candidate.protocol {
        BrowserProtocolKind::GeckoBiDi => {
            args.push("-profile".to_string());
            args.push(profile_str.clone());
            args.push("-no-remote".to_string());
            if headless {
                args.push("--headless".to_string());
            }
            args.push(format!("--remote-debugging-port={remote_port}"));
        }
        BrowserProtocolKind::ChromiumCdp => {
            args.push(format!("--user-data-dir={profile_str}"));
            args.push("--no-first-run".to_string());
            args.push("--no-default-browser-check".to_string());
            args.push(format!("--remote-debugging-port={remote_port}"));
            if headless {
                args.push("--headless=new".to_string());
            }
        }
    }
    Ok(BrowserLaunchSpec {
        engine: candidate.engine,
        protocol: candidate.protocol,
        executable_path: candidate.executable_path.clone(),
        isolated_profile_dir: profile_str,
        headless,
        args,
    })
}
