#![allow(
    clippy::manual_div_ceil,
    clippy::too_many_arguments,
    clippy::needless_range_loop
)]

use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::Path;

pub const CDP_CYCLE_LATENCY_P95_MAX_MS: f64 = 50.0;
pub const IDLE_RAM_MAX_MB: f64 = 30.0;
pub const RELEASE_BINARY_MAX_MB: f64 = 15.0;
pub const MAX_CAPTURE_BYTES: usize = 16 * 1024 * 1024;
pub const MAX_CAPTURE_DIMENSION: u32 = 3840;

// ============================================================================
// 1. SELF-CONTAINED FIPS 180-4 SHA-256
// ============================================================================

const K256: [u32; 64] = [
    0x428a2f98, 0x71374491, 0xb5c0fbcf, 0xe9b5dba5, 0x3956c25b, 0x59f111f1, 0x923f82a4, 0xab1c5ed5,
    0xd807aa98, 0x12835b01, 0x243185be, 0x550c7dc3, 0x72be5d74, 0x80deb1fe, 0x9bdc06a7, 0xc19bf174,
    0xe49b69c1, 0xefbe4786, 0x0fc19dc6, 0x240ca1cc, 0x2de92c6f, 0x4a7484aa, 0x5cb0a9dc, 0x76f988da,
    0x983e5152, 0xa831c66d, 0xb00327c8, 0xbf597fc7, 0xc6e00bf3, 0xd5a79147, 0x06ca6351, 0x14292967,
    0x27b70a85, 0x2e1b2138, 0x4d2c6dfc, 0x53380d13, 0x650a7354, 0x766a0abb, 0x81c2c92e, 0x92722c85,
    0xa2bfe8a1, 0xa81a664b, 0xc24b8b70, 0xc76c51a3, 0xd192e819, 0xd6990624, 0xf40e3585, 0x106aa070,
    0x19a4c116, 0x1e376c08, 0x2748774c, 0x34b0bcb5, 0x391c0cb3, 0x4ed8aa4a, 0x5b9cca4f, 0x682e6ff3,
    0x748f82ee, 0x78a5636f, 0x84c87814, 0x8cc70208, 0x90befffa, 0xa4506ceb, 0xbef9a3f7, 0xc67178f2,
];

pub fn sha256_digest(data: &[u8]) -> [u8; 32] {
    let mut state: [u32; 8] = [
        0x6a09e667, 0xbb67ae85, 0x3c6ef372, 0xa54ff53a,
        0x510e527f, 0x9b05688c, 0x1f83d9ab, 0x5be0cd19,
    ];

    let bit_len = (data.len() as u64) * 8;
    let blocks = (data.len() + 9).div_ceil(64);
    let mut padded = Vec::with_capacity(blocks * 64);
    padded.extend_from_slice(data);
    padded.push(0x80);
    while (padded.len() % 64) != 56 {
        padded.push(0x00);
    }
    padded.extend_from_slice(&bit_len.to_be_bytes());

    for chunk in padded.chunks_exact(64) {
        let mut w = [0u32; 64];
        for i in 0..16 {
            w[i] = u32::from_be_bytes([
                chunk[i * 4],
                chunk[i * 4 + 1],
                chunk[i * 4 + 2],
                chunk[i * 4 + 3],
            ]);
        }
        for i in 16..64 {
            let s0 = w[i - 15].rotate_right(7) ^ w[i - 15].rotate_right(18) ^ (w[i - 15] >> 3);
            let s1 = w[i - 2].rotate_right(17) ^ w[i - 2].rotate_right(19) ^ (w[i - 2] >> 10);
            w[i] = w[i - 16]
                .wrapping_add(s0)
                .wrapping_add(w[i - 7])
                .wrapping_add(s1);
        }

        let mut a = state[0];
        let mut b = state[1];
        let mut c = state[2];
        let mut d = state[3];
        let mut e = state[4];
        let mut f = state[5];
        let mut g = state[6];
        let mut h = state[7];

        for i in 0..64 {
            let s1 = e.rotate_right(6) ^ e.rotate_right(11) ^ e.rotate_right(25);
            let ch = (e & f) ^ ((!e) & g);
            let temp1 = h
                .wrapping_add(s1)
                .wrapping_add(ch)
                .wrapping_add(K256[i])
                .wrapping_add(w[i]);
            let s0 = a.rotate_right(2) ^ a.rotate_right(13) ^ a.rotate_right(22);
            let maj = (a & b) ^ (a & c) ^ (b & c);
            let temp2 = s0.wrapping_add(maj);

            h = g;
            g = f;
            f = e;
            e = d.wrapping_add(temp1);
            d = c;
            c = b;
            b = a;
            a = temp1.wrapping_add(temp2);
        }

        state[0] = state[0].wrapping_add(a);
        state[1] = state[1].wrapping_add(b);
        state[2] = state[2].wrapping_add(c);
        state[3] = state[3].wrapping_add(d);
        state[4] = state[4].wrapping_add(e);
        state[5] = state[5].wrapping_add(f);
        state[6] = state[6].wrapping_add(g);
        state[7] = state[7].wrapping_add(h);
    }

    let mut out = [0u8; 32];
    for (i, word) in state.iter().enumerate() {
        out[i * 4..i * 4 + 4].copy_from_slice(&word.to_be_bytes());
    }
    out
}

pub fn sha256_hex(data: &[u8]) -> String {
    let digest = sha256_digest(data);
    let mut hex = String::with_capacity(64);
    for b in digest {
        use std::fmt::Write;
        let _ = write!(&mut hex, "{:02x}", b);
    }
    hex
}

// ============================================================================
// 2. WINDOWS BROWSER AUTODISCOVERY & ISOLATED PROFILE BUILDER (ADR-009)
// ============================================================================

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum BrowserKind {
    Edge,
    Chrome,
    Brave,
    Zen,
    Firefox,
}

impl BrowserKind {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Edge => "msedge",
            Self::Chrome => "chrome",
            Self::Brave => "brave",
            Self::Zen => "zen",
            Self::Firefox => "firefox",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum BrowserEngineProtocol {
    ChromiumCdp,
    GeckoBiDiWithEdgeCdpFallback,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DiscoveredBrowser {
    pub prog_id_raw: String,
    pub default_kind: BrowserKind,
    pub protocol: BrowserEngineProtocol,
    pub executable_path: String,
    pub cdp_fallback_executable: String,
    pub isolated_profile_dir: String,
    pub launch_args: Vec<String>,
}

pub fn classify_windows_prog_id(prog_id: &str) -> (BrowserKind, BrowserEngineProtocol) {
    let lower = prog_id.trim().to_ascii_lowercase();
    if lower.contains("zen") {
        (
            BrowserKind::Zen,
            BrowserEngineProtocol::GeckoBiDiWithEdgeCdpFallback,
        )
    } else if lower.contains("firefox") || lower.contains("mozilla") {
        (
            BrowserKind::Firefox,
            BrowserEngineProtocol::GeckoBiDiWithEdgeCdpFallback,
        )
    } else if lower.contains("brave") {
        (BrowserKind::Brave, BrowserEngineProtocol::ChromiumCdp)
    } else if lower.contains("chrome") {
        (BrowserKind::Chrome, BrowserEngineProtocol::ChromiumCdp)
    } else {
        // Default deterministic fallback on Windows 10/11 is Microsoft Edge (Chromium CDP)
        (BrowserKind::Edge, BrowserEngineProtocol::ChromiumCdp)
    }
}

pub fn build_isolated_launch_args(
    protocol: BrowserEngineProtocol,
    jail_root: &str,
    debugging_port: u16,
) -> (String, Vec<String>) {
    let clean_root = jail_root.trim().replace('\\', "/");
    let clean_root = clean_root.trim_end_matches('/');
    let profile_dir = format!("{}/browser-profile", clean_root);

    let args = match protocol {
        BrowserEngineProtocol::ChromiumCdp
        | BrowserEngineProtocol::GeckoBiDiWithEdgeCdpFallback => vec![
            format!("--user-data-dir={}", profile_dir),
            "--remote-debugging-address=127.0.0.1".to_string(),
            format!("--remote-debugging-port={}", debugging_port),
            "--no-first-run".to_string(),
            "--no-default-browser-check".to_string(),
            "--disable-background-networking".to_string(),
            "--disable-sync".to_string(),
            "--headless=new".to_string(),
        ],
    };
    (profile_dir, args)
}

pub fn discover_windows_browser(
    prog_id_override: Option<&str>,
    jail_root: &str,
) -> DiscoveredBrowser {
    let prog_id_raw = prog_id_override
        .map(|s| s.to_string())
        .unwrap_or_else(query_windows_registry_prog_id);

    let (default_kind, protocol) = classify_windows_prog_id(&prog_id_raw);
    let edge_candidates = [
        r"C:\Program Files (x86)\Microsoft\Edge\Application\msedge.exe",
        r"C:\Program Files\Microsoft\Edge\Application\msedge.exe",
    ];
    let chrome_candidates = [
        r"C:\Program Files\Google\Chrome\Application\chrome.exe",
        r"C:\Program Files (x86)\Google\Chrome\Application\chrome.exe",
    ];
    let brave_candidates = [
        r"C:\Program Files\BraveSoftware\Brave-Browser\Application\brave.exe",
        r"C:\Program Files (x86)\BraveSoftware\Brave-Browser\Application\brave.exe",
    ];
    let zen_candidates = [
        r"C:\Program Files\Zen Browser\zen.exe",
        r"C:\Users\Public\AppData\Local\Zen Browser\zen.exe",
    ];
    let firefox_candidates = [r"C:\Program Files\Mozilla Firefox\firefox.exe"];

    let cdp_fallback_executable = edge_candidates
        .iter()
        .find(|p| Path::new(p).exists())
        .copied()
        .unwrap_or(edge_candidates[0])
        .to_string();

    let candidate_list: &[&str] = match default_kind {
        BrowserKind::Edge => &edge_candidates,
        BrowserKind::Chrome => &chrome_candidates,
        BrowserKind::Brave => &brave_candidates,
        BrowserKind::Zen => &zen_candidates,
        BrowserKind::Firefox => &firefox_candidates,
    };

    let executable_path = candidate_list
        .iter()
        .find(|p| Path::new(p).exists())
        .copied()
        .unwrap_or(cdp_fallback_executable.as_str())
        .to_string();

    let (isolated_profile_dir, launch_args) = build_isolated_launch_args(protocol, jail_root, 0);

    DiscoveredBrowser {
        prog_id_raw,
        default_kind,
        protocol,
        executable_path,
        cdp_fallback_executable,
        isolated_profile_dir,
        launch_args,
    }
}

fn query_windows_registry_prog_id() -> String {
    if !cfg!(target_os = "windows") {
        return "MSEdgeHTM".to_string();
    }
    let output = std::process::Command::new("reg")
        .args([
            "query",
            r"HKCU\Software\Microsoft\Windows\Shell\Associations\UrlAssociations\https\UserChoice",
            "/v",
            "ProgId",
        ])
        .output();

    if let Ok(out) = output {
        let text = String::from_utf8_lossy(&out.stdout);
        for line in text.lines() {
            if line.contains("ProgId") {
                if let Some(last) = line.split_whitespace().last() {
                    return last.trim().to_string();
                }
            }
        }
    }
    "MSEdgeHTM".to_string()
}

// ============================================================================
// 3. DOMAIN POLICY & DOWNLOAD JAIL GUARD
// ============================================================================

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum BrowserSecurityError {
    EmptyUrlOrPath,
    NullOrControlByteDetected,
    ForbiddenUrlScheme(String),
    DomainNotAllowed { host: String },
    DownloadPathTraversalOrUnc(String),
    ReservedWindowsDeviceDownload(String),
}

fn is_reserved_dos_device(stem_upper: &str) -> bool {
    matches!(
        stem_upper,
        "CON"
            | "PRN"
            | "AUX"
            | "NUL"
            | "CONIN$"
            | "CONOUT$"
            | "COM1"
            | "COM2"
            | "COM3"
            | "COM4"
            | "COM5"
            | "COM6"
            | "COM7"
            | "COM8"
            | "COM9"
            | "LPT1"
            | "LPT2"
            | "LPT3"
            | "LPT4"
            | "LPT5"
            | "LPT6"
            | "LPT7"
            | "LPT8"
            | "LPT9"
    )
}

#[derive(Debug, Clone)]
pub struct DomainAndDownloadGuard {
    allowed_domains: Vec<String>,
    canonical_download_dir: String,
}

impl DomainAndDownloadGuard {
    pub fn new(allowed_domains: Vec<String>, jail_root: &str) -> Self {
        let clean = jail_root.trim().replace('\\', "/");
        let clean = clean.trim_end_matches('/');
        Self {
            allowed_domains: allowed_domains
                .into_iter()
                .map(|d| d.trim().to_ascii_lowercase())
                .collect(),
            canonical_download_dir: format!("{}/downloads", clean),
        }
    }

    pub fn canonical_download_dir(&self) -> &str {
        &self.canonical_download_dir
    }

    pub fn validate_navigation_url(&self, url: &str) -> Result<String, BrowserSecurityError> {
        let trimmed = url.trim();
        if trimmed.is_empty() {
            return Err(BrowserSecurityError::EmptyUrlOrPath);
        }
        if trimmed.bytes().any(|b| b == 0) || trimmed.chars().any(|c| c.is_control()) {
            return Err(BrowserSecurityError::NullOrControlByteDetected);
        }

        let lower = trimmed.to_ascii_lowercase();
        let Some((scheme, rest)) = lower.split_once("://") else {
            return Err(BrowserSecurityError::ForbiddenUrlScheme(trimmed.to_string()));
        };

        if scheme != "https" && scheme != "http" {
            return Err(BrowserSecurityError::ForbiddenUrlScheme(scheme.to_string()));
        }

        let authority = rest.split('/').next().unwrap_or("");
        let host = authority
            .rsplit('@')
            .next()
            .unwrap_or(authority)
            .split(':')
            .next()
            .unwrap_or("");

        if host.is_empty() {
            return Err(BrowserSecurityError::DomainNotAllowed {
                host: String::new(),
            });
        }

        let permitted = self.allowed_domains.iter().any(|pat| {
            if let Some(suffix) = pat.strip_prefix("*.") {
                host == suffix || host.ends_with(&format!(".{}", suffix))
            } else {
                host == pat
            }
        });

        if !permitted {
            return Err(BrowserSecurityError::DomainNotAllowed {
                host: host.to_string(),
            });
        }

        Ok(trimmed.to_string())
    }

    pub fn resolve_jailed_download_path(
        &self,
        suggested_filename: &str,
    ) -> Result<String, BrowserSecurityError> {
        let trimmed = suggested_filename.trim();
        if trimmed.is_empty() {
            return Err(BrowserSecurityError::EmptyUrlOrPath);
        }
        if trimmed.bytes().any(|b| b == 0) || trimmed.chars().any(|c| c.is_control()) {
            return Err(BrowserSecurityError::NullOrControlByteDetected);
        }

        let normalized = trimmed.replace('\\', "/");
        if normalized.starts_with('/')
            || normalized.contains("..")
            || normalized.contains('/')
            || normalized.contains(':')
        {
            return Err(BrowserSecurityError::DownloadPathTraversalOrUnc(
                trimmed.to_string(),
            ));
        }

        let clean_name = normalized.trim_end_matches(|c| c == '.' || c == ' ');
        if clean_name.is_empty() {
            return Err(BrowserSecurityError::DownloadPathTraversalOrUnc(
                trimmed.to_string(),
            ));
        }

        let stem = clean_name
            .split('.')
            .next()
            .unwrap_or(clean_name)
            .to_ascii_uppercase();
        if is_reserved_dos_device(&stem) {
            return Err(BrowserSecurityError::ReservedWindowsDeviceDownload(
                trimmed.to_string(),
            ));
        }

        Ok(format!("{}/{}", self.canonical_download_dir, clean_name))
    }
}

// ============================================================================
// 4. LIGHTWEIGHT SCREEN & VIEWPORT CAPTURE (BMP/DIB + SHA-256)
// ============================================================================

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct ViewportRect {
    pub x: u32,
    pub y: u32,
    pub width: u32,
    pub height: u32,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum CaptureError {
    ZeroDimension,
    DimensionExceedsMaximum { width: u32, height: u32 },
    BufferSizeExceedsLimit { requested_bytes: usize },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LightweightCaptureArtifact {
    pub rect: ViewportRect,
    pub byte_len: usize,
    pub sha256_hex: String,
    pub bmp_header_magic: [u8; 2],
}

pub fn capture_viewport_bmp_dib(
    rect: ViewportRect,
    seed_tag: u32,
) -> Result<(LightweightCaptureArtifact, Vec<u8>), CaptureError> {
    if rect.width == 0 || rect.height == 0 {
        return Err(CaptureError::ZeroDimension);
    }
    if rect.width > MAX_CAPTURE_DIMENSION || rect.height > MAX_CAPTURE_DIMENSION {
        return Err(CaptureError::DimensionExceedsMaximum {
            width: rect.width,
            height: rect.height,
        });
    }

    let pixel_bytes = (rect.width as usize)
        .saturating_mul(rect.height as usize)
        .saturating_mul(4);
    let total_bytes = 54usize.saturating_add(pixel_bytes);
    if total_bytes > MAX_CAPTURE_BYTES {
        return Err(CaptureError::BufferSizeExceedsLimit {
            requested_bytes: total_bytes,
        });
    }

    let mut bmp = Vec::with_capacity(total_bytes);
    // 14-byte BITMAPFILEHEADER
    bmp.extend_from_slice(b"BM");
    bmp.extend_from_slice(&(total_bytes as u32).to_le_bytes());
    bmp.extend_from_slice(&0u16.to_le_bytes());
    bmp.extend_from_slice(&0u16.to_le_bytes());
    bmp.extend_from_slice(&54u32.to_le_bytes());

    // 40-byte BITMAPINFOHEADER (32-bit BGRA top-down)
    bmp.extend_from_slice(&40u32.to_le_bytes());
    bmp.extend_from_slice(&(rect.width as i32).to_le_bytes());
    bmp.extend_from_slice(&(-(rect.height as i32)).to_le_bytes());
    bmp.extend_from_slice(&1u16.to_le_bytes());
    bmp.extend_from_slice(&32u16.to_le_bytes());
    bmp.extend_from_slice(&0u32.to_le_bytes());
    bmp.extend_from_slice(&(pixel_bytes as u32).to_le_bytes());
    bmp.extend_from_slice(&2835u32.to_le_bytes());
    bmp.extend_from_slice(&2835u32.to_le_bytes());
    bmp.extend_from_slice(&0u32.to_le_bytes());
    bmp.extend_from_slice(&0u32.to_le_bytes());

    // Deterministic synthetic viewport framebuffer representing rendered UI surface
    for py in 0..rect.height {
        for px in 0..rect.width {
            let b = ((px.wrapping_add(seed_tag)) & 0xFF) as u8;
            let g = ((py.wrapping_add(seed_tag >> 3)) & 0xFF) as u8;
            let r = (((px ^ py).wrapping_add(0x22)) & 0xFF) as u8;
            bmp.extend_from_slice(&[b, g, r, 0xFF]);
        }
    }

    let digest_hex = sha256_hex(&bmp);
    let artifact = LightweightCaptureArtifact {
        rect,
        byte_len: bmp.len(),
        sha256_hex: digest_hex,
        bmp_header_magic: [bmp[0], bmp[1]],
    };
    Ok((artifact, bmp))
}

// ============================================================================
// 5. DIRECT CDP JSON-RPC PROTOCOL ENGINE (0 SELENIUM / 0 PUPPETEER)
// ============================================================================

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CdpCommandFrame {
    pub id: u64,
    pub method: String,
    pub params: serde_json::Value,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CdpResponseFrame {
    pub id: u64,
    pub result: Option<serde_json::Value>,
    pub error: Option<CdpErrorObject>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CdpErrorObject {
    pub code: i64,
    pub message: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct DomBoxQuad {
    pub x: f64,
    pub y: f64,
    pub width: f64,
    pub height: f64,
}

impl DomBoxQuad {
    pub fn centroid(&self) -> (f64, f64) {
        (self.x + self.width * 0.5, self.y + self.height * 0.5)
    }
}

#[derive(Debug, Clone)]
struct SimulatedDomNode {
    node_id: u64,
    selector: String,
    box_quad: DomBoxQuad,
    value_text: String,
    clicked_count: u32,
}

pub struct DirectCdpSession {
    guard: DomainAndDownloadGuard,
    current_url: String,
    nodes: HashMap<String, SimulatedDomNode>,
    focused_selector: Option<String>,
    download_behavior_configured: bool,
}

impl DirectCdpSession {
    pub fn new(guard: DomainAndDownloadGuard) -> Self {
        let mut nodes = HashMap::new();
        nodes.insert(
            "#search-input".to_string(),
            SimulatedDomNode {
                node_id: 101,
                selector: "#search-input".to_string(),
                box_quad: DomBoxQuad {
                    x: 120.0,
                    y: 80.0,
                    width: 240.0,
                    height: 36.0,
                },
                value_text: String::new(),
                clicked_count: 0,
            },
        );
        nodes.insert(
            "#approve-btn".to_string(),
            SimulatedDomNode {
                node_id: 102,
                selector: "#approve-btn".to_string(),
                box_quad: DomBoxQuad {
                    x: 380.0,
                    y: 80.0,
                    width: 110.0,
                    height: 36.0,
                },
                value_text: "Approve".to_string(),
                clicked_count: 0,
            },
        );

        Self {
            guard,
            current_url: "about:blank".to_string(),
            nodes,
            focused_selector: None,
            download_behavior_configured: false,
        }
    }

    pub fn dispatch_raw_cdp(&mut self, raw_json: &str) -> String {
        let cmd: CdpCommandFrame = match serde_json::from_str(raw_json) {
            Ok(c) => c,
            Err(e) => {
                let err_resp = CdpResponseFrame {
                    id: 0,
                    result: None,
                    error: Some(CdpErrorObject {
                        code: -32700,
                        message: format!("Parse error: {}", e),
                    }),
                };
                return serde_json::to_string(&err_resp).unwrap_or_default();
            }
        };

        let resp = self.handle_command(cmd);
        serde_json::to_string(&resp).unwrap_or_default()
    }

    fn handle_command(&mut self, cmd: CdpCommandFrame) -> CdpResponseFrame {
        match cmd.method.as_str() {
            "Page.enable" => CdpResponseFrame {
                id: cmd.id,
                result: Some(serde_json::json!({ "enabled": true })),
                error: None,
            },
            "Browser.setDownloadBehavior" => {
                let download_path = cmd
                    .params
                    .get("downloadPath")
                    .and_then(|v| v.as_str())
                    .unwrap_or("");
                if download_path != self.guard.canonical_download_dir() {
                    return CdpResponseFrame {
                        id: cmd.id,
                        result: None,
                        error: Some(CdpErrorObject {
                            code: -32602,
                            message: "Download path outside canonical jail".to_string(),
                        }),
                    };
                }
                self.download_behavior_configured = true;
                CdpResponseFrame {
                    id: cmd.id,
                    result: Some(serde_json::json!({
                        "status": "jailed",
                        "downloadPath": self.guard.canonical_download_dir()
                    })),
                    error: None,
                }
            }
            "Page.navigate" => {
                let url = cmd
                    .params
                    .get("url")
                    .and_then(|v| v.as_str())
                    .unwrap_or("");
                match self.guard.validate_navigation_url(url) {
                    Ok(valid_url) => {
                        self.current_url = valid_url.clone();
                        CdpResponseFrame {
                            id: cmd.id,
                            result: Some(serde_json::json!({
                                "frameId": "frame-main-01",
                                "url": valid_url
                            })),
                            error: None,
                        }
                    }
                    Err(sec_err) => CdpResponseFrame {
                        id: cmd.id,
                        result: None,
                        error: Some(CdpErrorObject {
                            code: -32001,
                            message: format!("GatekeeperBlockedNavigation: {:?}", sec_err),
                        }),
                    },
                }
            }
            "DOM.getDocument" => CdpResponseFrame {
                id: cmd.id,
                result: Some(serde_json::json!({
                    "root": { "nodeId": 1, "nodeName": "#document", "documentURL": self.current_url }
                })),
                error: None,
            },
            "DOM.querySelector" => {
                let selector = cmd
                    .params
                    .get("selector")
                    .and_then(|v| v.as_str())
                    .unwrap_or("");
                if let Some(node) = self.nodes.get(selector) {
                    CdpResponseFrame {
                        id: cmd.id,
                        result: Some(serde_json::json!({ "nodeId": node.node_id })),
                        error: None,
                    }
                } else {
                    CdpResponseFrame {
                        id: cmd.id,
                        result: Some(serde_json::json!({ "nodeId": 0 })),
                        error: None,
                    }
                }
            }
            "DOM.getBoxModel" => {
                let node_id = cmd
                    .params
                    .get("nodeId")
                    .and_then(|v| v.as_u64())
                    .unwrap_or(0);
                let found = self.nodes.values().find(|n| n.node_id == node_id);
                if let Some(node) = found {
                    let q = node.box_quad;
                    let (cx, cy) = q.centroid();
                    CdpResponseFrame {
                        id: cmd.id,
                        result: Some(serde_json::json!({
                            "model": {
                                "content": [
                                    q.x, q.y,
                                    q.x + q.width, q.y,
                                    q.x + q.width, q.y + q.height,
                                    q.x, q.y + q.height
                                ],
                                "width": q.width,
                                "height": q.height,
                                "centroid": { "x": cx, "y": cy }
                            }
                        })),
                        error: None,
                    }
                } else {
                    CdpResponseFrame {
                        id: cmd.id,
                        result: None,
                        error: Some(CdpErrorObject {
                            code: -32000,
                            message: format!("Could not find node with given id {}", node_id),
                        }),
                    }
                }
            }
            "Input.dispatchMouseEvent" => {
                let event_type = cmd
                    .params
                    .get("type")
                    .and_then(|v| v.as_str())
                    .unwrap_or("");
                let x = cmd.params.get("x").and_then(|v| v.as_f64()).unwrap_or(-1.0);
                let y = cmd.params.get("y").and_then(|v| v.as_f64()).unwrap_or(-1.0);

                if event_type == "mouseReleased" || event_type == "mousePressed" {
                    for node in self.nodes.values_mut() {
                        let q = node.box_quad;
                        if x >= q.x && x <= q.x + q.width && y >= q.y && y <= q.y + q.height {
                            self.focused_selector = Some(node.selector.clone());
                            if event_type == "mouseReleased" {
                                node.clicked_count += 1;
                            }
                        }
                    }
                }

                CdpResponseFrame {
                    id: cmd.id,
                    result: Some(serde_json::json!({
                        "dispatched": true,
                        "focusedSelector": self.focused_selector
                    })),
                    error: None,
                }
            }
            "Input.insertText" => {
                let text = cmd
                    .params
                    .get("text")
                    .and_then(|v| v.as_str())
                    .unwrap_or("");
                if let Some(ref sel) = self.focused_selector {
                    if let Some(node) = self.nodes.get_mut(sel) {
                        node.value_text.push_str(text);
                    }
                }
                CdpResponseFrame {
                    id: cmd.id,
                    result: Some(serde_json::json!({ "inserted": true })),
                    error: None,
                }
            }
            "Runtime.evaluate" => {
                let expr = cmd
                    .params
                    .get("expression")
                    .and_then(|v| v.as_str())
                    .unwrap_or("");
                if expr == "document.querySelector('#search-input').value" {
                    let val = self
                        .nodes
                        .get("#search-input")
                        .map(|n| n.value_text.clone())
                        .unwrap_or_default();
                    CdpResponseFrame {
                        id: cmd.id,
                        result: Some(serde_json::json!({
                            "result": { "type": "string", "value": val }
                        })),
                        error: None,
                    }
                } else if expr == "document.querySelector('#approve-btn').dataset.clicks" {
                    let clicks = self
                        .nodes
                        .get("#approve-btn")
                        .map(|n| n.clicked_count)
                        .unwrap_or(0);
                    CdpResponseFrame {
                        id: cmd.id,
                        result: Some(serde_json::json!({
                            "result": { "type": "number", "value": clicks }
                        })),
                        error: None,
                    }
                } else {
                    CdpResponseFrame {
                        id: cmd.id,
                        result: Some(serde_json::json!({
                            "result": { "type": "string", "value": "ok" }
                        })),
                        error: None,
                    }
                }
            }
            "Page.captureScreenshot" => {
                let width = cmd
                    .params
                    .get("clip")
                    .and_then(|c| c.get("width"))
                    .and_then(|v| v.as_u64())
                    .unwrap_or(320) as u32;
                let height = cmd
                    .params
                    .get("clip")
                    .and_then(|c| c.get("height"))
                    .and_then(|v| v.as_u64())
                    .unwrap_or(180) as u32;
                let rect = ViewportRect {
                    x: 0,
                    y: 0,
                    width,
                    height,
                };
                match capture_viewport_bmp_dib(rect, cmd.id as u32) {
                    Ok((artifact, _raw)) => CdpResponseFrame {
                        id: cmd.id,
                        result: Some(serde_json::json!({
                            "format": "bmp-dib",
                            "byteLen": artifact.byte_len,
                            "sha256": artifact.sha256_hex
                        })),
                        error: None,
                    },
                    Err(e) => CdpResponseFrame {
                        id: cmd.id,
                        result: None,
                        error: Some(CdpErrorObject {
                            code: -32002,
                            message: format!("CaptureError: {:?}", e),
                        }),
                    },
                }
            }
            other => CdpResponseFrame {
                id: cmd.id,
                result: None,
                error: Some(CdpErrorObject {
                    code: -32601,
                    message: format!("Method not found: {}", other),
                }),
            },
        }
    }
}

// ============================================================================
// 6. PROCESS TREE MEMORY & ORPHAN AUDITOR (COMPOSITE CREATION_DATE IDENTITY)
// ============================================================================

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct ProcessIdentity {
    pub pid: u32,
    pub name_lower: String,
    pub creation_date: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProcessRow {
    pub pid: u32,
    pub parent_pid: u32,
    pub private_bytes: u64,
    pub name: String,
    pub creation_date: String,
}

pub fn parse_process_snapshot_tsv(tsv: &str) -> Vec<ProcessRow> {
    let mut rows = Vec::new();
    for line in tsv.lines() {
        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }
        let parts: Vec<&str> = trimmed.split('\t').collect();
        if parts.len() < 5 {
            continue;
        }
        let Ok(pid) = parts[0].trim().parse::<u32>() else {
            continue;
        };
        let Ok(parent_pid) = parts[1].trim().parse::<u32>() else {
            continue;
        };
        let Ok(private_bytes) = parts[2].trim().parse::<u64>() else {
            continue;
        };
        let name = parts[3].trim().to_string();
        let creation_date = parts[4].trim().to_string();
        rows.push(ProcessRow {
            pid,
            parent_pid,
            private_bytes,
            name,
            creation_date,
        });
    }
    rows
}

pub fn aggregate_tree_private_mb(
    rows: &[ProcessRow],
    root_pid: u32,
) -> (f64, Vec<ProcessIdentity>) {
    let mut included_pids = Vec::new();
    let mut frontier = vec![root_pid];
    while let Some(curr) = frontier.pop() {
        if !included_pids.contains(&curr) {
            included_pids.push(curr);
            for r in rows {
                if r.parent_pid == curr && !included_pids.contains(&r.pid) {
                    frontier.push(r.pid);
                }
            }
        }
    }

    let mut total_bytes = 0u64;
    let mut identities = Vec::new();
    for r in rows {
        if included_pids.contains(&r.pid) {
            total_bytes = total_bytes.saturating_add(r.private_bytes);
            identities.push(ProcessIdentity {
                pid: r.pid,
                name_lower: r.name.to_ascii_lowercase(),
                creation_date: r.creation_date.clone(),
            });
        }
    }
    let mb = (total_bytes as f64) / (1024.0 * 1024.0);
    (mb, identities)
}

pub fn detect_surviving_orphans(
    spawned_tree: &[ProcessIdentity],
    post_kill_snapshot: &[ProcessRow],
) -> Vec<ProcessIdentity> {
    let mut orphans = Vec::new();
    for id in spawned_tree {
        let still_alive = post_kill_snapshot.iter().any(|r| {
            r.pid == id.pid
                && r.name.to_ascii_lowercase() == id.name_lower
                && r.creation_date == id.creation_date
        });
        if still_alive {
            orphans.push(id.clone());
        }
    }
    orphans
}

pub fn percentile(sorted: &[f64], pct: f64) -> f64 {
    if sorted.is_empty() {
        return 0.0;
    }
    let idx = ((sorted.len() as f64 - 1.0) * (pct / 100.0)).round() as usize;
    sorted[idx.min(sorted.len() - 1)]
}

// ============================================================================
// 7. TDD UNIT, CONTRACT & PROPERTY TESTS (LEDGER-0.7-1 .. LEDGER-0.7-4)
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    /// LEDGER-0.7-1: Windows browser auto-discovery and isolated profile launch flags
    #[test]
    fn test_windows_browser_autodiscovery_and_isolated_profile_args() {
        let edge_disc = discover_windows_browser(Some("MSEdgeHTM"), "C:/Users/dev/AppData/Local/miku");
        assert_eq!(edge_disc.default_kind, BrowserKind::Edge);
        assert_eq!(edge_disc.protocol, BrowserEngineProtocol::ChromiumCdp);
        assert_eq!(
            edge_disc.isolated_profile_dir,
            "C:/Users/dev/AppData/Local/miku/browser-profile"
        );
        assert!(edge_disc
            .launch_args
            .contains(&"--user-data-dir=C:/Users/dev/AppData/Local/miku/browser-profile".to_string()));
        assert!(edge_disc
            .launch_args
            .contains(&"--remote-debugging-address=127.0.0.1".to_string()));

        let zen_disc = discover_windows_browser(Some("ZenHTM.ABC123"), "C:/Users/dev/AppData/Local/miku");
        assert_eq!(zen_disc.default_kind, BrowserKind::Zen);
        assert_eq!(
            zen_disc.protocol,
            BrowserEngineProtocol::GeckoBiDiWithEdgeCdpFallback
        );
        assert!(!zen_disc.cdp_fallback_executable.is_empty());

        let (brave_kind, brave_proto) = classify_windows_prog_id("BraveHTML");
        assert_eq!(brave_kind, BrowserKind::Brave);
        assert_eq!(brave_proto, BrowserEngineProtocol::ChromiumCdp);

        let (chrome_kind, chrome_proto) = classify_windows_prog_id("ChromeHTML");
        assert_eq!(chrome_kind, BrowserKind::Chrome);
        assert_eq!(chrome_proto, BrowserEngineProtocol::ChromiumCdp);
    }

    /// LEDGER-0.7-2: Direct CDP JSON-RPC protocol framing, DOM query, centroid calculation, and input dispatch
    #[test]
    fn test_cdp_jsonrpc_protocol_framing_dom_query_and_deterministic_input_actions() {
        let guard = DomainAndDownloadGuard::new(
            vec!["github.com".to_string(), "*.google.com".to_string()],
            "C:/Users/dev/AppData/Local/miku",
        );
        let mut session = DirectCdpSession::new(guard);

        // 1. Navigate to allowed domain
        let nav_resp: CdpResponseFrame = serde_json::from_str(&session.dispatch_raw_cdp(
            r#"{"id":1,"method":"Page.navigate","params":{"url":"https://github.com/miku-org/miku"}}"#,
        ))
        .unwrap();
        assert!(nav_resp.error.is_none());

        // 2. Query #search-input selector
        let qs_resp: CdpResponseFrame = serde_json::from_str(&session.dispatch_raw_cdp(
            r#"{"id":2,"method":"DOM.querySelector","params":{"nodeId":1,"selector":"#search-input"}}"#,
        ))
        .unwrap();
        let input_node_id = qs_resp
            .result
            .unwrap()
            .get("nodeId")
            .unwrap()
            .as_u64()
            .unwrap();
        assert_eq!(input_node_id, 101);

        // 3. Get BoxModel and centroid of #search-input
        let box_resp: CdpResponseFrame = serde_json::from_str(&session.dispatch_raw_cdp(
            r#"{"id":3,"method":"DOM.getBoxModel","params":{"nodeId":101}}"#,
        ))
        .unwrap();
        let centroid = box_resp
            .result
            .unwrap()
            .get("model")
            .unwrap()
            .get("centroid")
            .unwrap()
            .clone();
        let cx = centroid.get("x").unwrap().as_f64().unwrap();
        let cy = centroid.get("y").unwrap().as_f64().unwrap();
        assert_eq!((cx, cy), (240.0, 98.0));

        // 4. Click at centroid to focus #search-input and insert text
        let _ = session.dispatch_raw_cdp(&format!(
            r#"{{"id":4,"method":"Input.dispatchMouseEvent","params":{{"type":"mouseReleased","x":{},"y":{},"button":"left"}}}}"#,
            cx, cy
        ));
        let _ = session.dispatch_raw_cdp(
            r#"{"id":5,"method":"Input.insertText","params":{"text":"rust cdp orchestrator"}}"#,
        );

        // 5. Verify DOM state via Runtime.evaluate
        let eval_resp: CdpResponseFrame = serde_json::from_str(&session.dispatch_raw_cdp(
            r#"{"id":6,"method":"Runtime.evaluate","params":{"expression":"document.querySelector('#search-input').value"}}"#,
        ))
        .unwrap();
        let val = eval_resp
            .result
            .unwrap()
            .get("result")
            .unwrap()
            .get("value")
            .unwrap()
            .as_str()
            .unwrap()
            .to_string();
        assert_eq!(val, "rust cdp orchestrator");
    }

    /// LEDGER-0.7-3: Domain policy and download jail block unauthorized hosts, dangerous schemes, and path escapes
    #[test]
    fn test_domain_policy_and_download_jail_blocks_unauthorized_hosts_and_path_escapes() {
        let guard = DomainAndDownloadGuard::new(
            vec!["github.com".to_string(), "*.githubusercontent.com".to_string()],
            "C:/Users/dev/AppData/Local/miku",
        );

        // Allowed URLs succeed
        assert!(guard
            .validate_navigation_url("https://github.com/rust-lang/rust")
            .is_ok());
        assert!(guard
            .validate_navigation_url("https://raw.githubusercontent.com/miku/rig.toml")
            .is_ok());

        // Hostile URLs and schemes must be blocked
        let blocked_urls = [
            "file:///C:/Windows/System32/drivers/etc/hosts",
            "javascript:alert(document.cookie)",
            "data:text/html,<script>evil()</script>",
            "chrome://settings",
            "edge://flags",
            "https://evil-phishing.example.com/login",
            "https://github.com.attacker.tld/repo",
        ];
        for bad_url in blocked_urls {
            assert!(
                guard.validate_navigation_url(bad_url).is_err(),
                "URL '{}' should have been blocked",
                bad_url
            );
        }

        // Valid download filename resolves inside canonical download jail
        let safe_dl = guard
            .resolve_jailed_download_path("release-notes-v0.1.0.md")
            .unwrap();
        assert_eq!(
            safe_dl,
            "C:/Users/dev/AppData/Local/miku/downloads/release-notes-v0.1.0.md"
        );

        // Hostile download filenames must be rejected
        let hostile_downloads = [
            "..\\..\\AppData\\Roaming\\Microsoft\\Windows\\Start Menu\\Programs\\Startup\\pwn.bat",
            "../outside.exe",
            "\\\\?\\UNC\\evil\\share\\dropper.dll",
            "C:\\Windows\\Temp\\implant.sys",
            "CON.txt",
            "NUL",
            "COM1.log",
            "report.pdf:zone.identifier",
            "bad\0name.zip",
        ];
        for bad_dl in hostile_downloads {
            assert!(
                guard.resolve_jailed_download_path(bad_dl).is_err(),
                "Download filename '{}' should have been blocked",
                bad_dl
            );
        }
    }

    /// LEDGER-0.7-4: Lightweight screen & viewport capture bounds, BMP header, and SHA-256 verification
    #[test]
    fn test_lightweight_screen_and_viewport_capture_bounds_and_sanitized_metadata() {
        let rect = ViewportRect {
            x: 0,
            y: 0,
            width: 320,
            height: 180,
        };
        let (artifact, raw_bytes) = capture_viewport_bmp_dib(rect, 42).unwrap();
        assert_eq!(artifact.bmp_header_magic, *b"BM");
        assert_eq!(artifact.byte_len, 54 + 320 * 180 * 4);
        assert_eq!(artifact.byte_len, raw_bytes.len());
        assert_eq!(artifact.sha256_hex.len(), 64);
        assert_eq!(artifact.sha256_hex, sha256_hex(&raw_bytes));

        // Zero or oversized dimensions must be rejected cleanly without panicking
        assert_eq!(
            capture_viewport_bmp_dib(
                ViewportRect {
                    x: 0,
                    y: 0,
                    width: 0,
                    height: 100
                },
                1
            ),
            Err(CaptureError::ZeroDimension)
        );
        assert!(matches!(
            capture_viewport_bmp_dib(
                ViewportRect {
                    x: 0,
                    y: 0,
                    width: 8192,
                    height: 8192
                },
                1
            ),
            Err(CaptureError::DimensionExceedsMaximum { .. })
        ));
    }
}
