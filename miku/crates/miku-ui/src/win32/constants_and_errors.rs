#![allow(unused_imports)]
use crate::prelude::*;

// ============================================================================
// 1. Win32 Style Constants & Focus Invariants (ADR-001 / ADR-011 / F-01)
// ============================================================================

pub const STANDARD_DPI: u32 = 96;

pub const IDLE_COMPACT_WIDTH_PX: u32 = 248;
pub const IDLE_COMPACT_HEIGHT_PX: u32 = 44;
pub const IDLE_CORNER_RADIUS_PX: u32 = 22;
pub const IDLE_AVATAR_SCALE_K: f64 = 0.34;
pub const IDLE_AVATAR_BODY_PX: f64 = 33.3;

pub const HOVER_PEEK_WIDTH_PX: u32 = 284;
pub const HOVER_PEEK_HEIGHT_PX: u32 = 52;
pub const HOVER_CORNER_RADIUS_PX: u32 = 26;
pub const HOVER_AVATAR_SCALE_K: f64 = 0.571;
pub const HOVER_AVATAR_BODY_PX: f64 = 56.0;

pub const EXPANDED_DEFAULT_WIDTH_PX: u32 = 640;
pub const EXPANDED_CORNER_RADIUS_PX: u32 = 32;
pub const TOP_MARGIN_DEFAULT_PX: u32 = 0;
pub const HOVER_HYSTERESIS_PX: i32 = 6;
pub const MAX_HOVER_RESPONSE_MS: f64 = 16.0;

pub const WS_POPUP_STYLE: u32 = 0x8000_0000;
pub const WS_VISIBLE_STYLE: u32 = 0x1000_0000;
pub const REQUIRED_BASE_STYLE: u32 = WS_POPUP_STYLE | WS_VISIBLE_STYLE;

pub const WS_EX_TOPMOST_STYLE: u32 = 0x0000_0008;
pub const WS_EX_TOOLWINDOW_STYLE: u32 = 0x0000_0080;
pub const WS_EX_APPWINDOW_FORBIDDEN: u32 = 0x0004_0000;
pub const WS_EX_LAYERED_STYLE: u32 = 0x0008_0000;
pub const WS_EX_NOACTIVATE_STYLE: u32 = 0x0800_0000;

/// Exact required extended style mask:
/// `WS_EX_LAYERED | WS_EX_TOPMOST | WS_EX_TOOLWINDOW | WS_EX_NOACTIVATE` = `0x0808_0088`.
pub const REQUIRED_EX_STYLE: u32 =
    WS_EX_LAYERED_STYLE | WS_EX_TOPMOST_STYLE | WS_EX_TOOLWINDOW_STYLE | WS_EX_NOACTIVATE_STYLE;

pub const WM_MOUSEACTIVATE_MSG: u32 = 0x0021;
pub const WM_DISPLAYCHANGE_MSG: u32 = 0x007E;
pub const WM_DPICHANGED_MSG: u32 = 0x02E0;
pub const MA_NOACTIVATE_RESULT: isize = 3;
pub const SW_SHOWNOACTIVATE_CMD: i32 = 4;

pub const SWP_NOSIZE_FLAG: u32 = 0x0001;
pub const SWP_NOMOVE_FLAG: u32 = 0x0002;
pub const SWP_NOZORDER_FLAG: u32 = 0x0004;
pub const SWP_NOACTIVATE_FLAG: u32 = 0x0010;
pub const SWP_SHOWWINDOW_FLAG: u32 = 0x0040;

pub const EXPECTED_AVATAR_SVG_COUNT: usize = 22;

pub const EXPECTED_AVATAR_SVG_REL_PATHS: &[&str] = &[
    "body.svg",
    "ears.svg",
    "expressions/eyes_open.svg",
    "expressions/eyes_blink.svg",
    "expressions/mouth_idle.svg",
    "expressions/mouth_talk.svg",
    "accessories/beanie.svg",
    "accessories/bow.svg",
    "accessories/bunny.svg",
    "accessories/crown.svg",
    "accessories/crown_back.svg",
    "accessories/halo.svg",
    "accessories/party.svg",
    "accessories/phones.svg",
    "accessories/propeller.svg",
    "accessories/pumpkin.svg",
    "accessories/round.svg",
    "accessories/santa.svg",
    "accessories/scarf.svg",
    "accessories/shades.svg",
    "accessories/witch.svg",
    "accessories/witch_back.svg",
];

pub const EXPANDED_DEFAULT_HEIGHT_PX: u32 = 380;
pub const WEBVIEW2_LOCAL_VIRTUAL_ORIGIN: &str = "https://miku.local/";
pub const WEBVIEW2_ISOLATED_SUBDIR_WINDOWS: &str = r"miku\webview2";
pub const DEFAULT_WEBVIEW2_WARM_TTL_MS: u64 = 15_000;
pub const MAX_EXPAND_VISIBLE_LATENCY_MS: f64 = 150.0;
pub const MAX_IDLE_POST_TTL_RAM_MB: f64 = 30.0;
pub const MAX_EXPANDED_WEBVIEW2_RAM_MB: f64 = 120.0;
pub const FIVE_CYCLE_STRESS_REPS: usize = 5;

pub const MIKU_JSONRPC_V2: &str = "2.0";
pub const MIKU_WEBVIEW_CSP_HEADER: &str = "default-src 'self' https://miku.local; script-src 'self' https://miku.local; style-src 'self' 'unsafe-inline'; img-src 'self' data: https://miku.local; connect-src 'none'; object-src 'none'; frame-ancestors 'none'; base-uri 'none'; form-action 'none'";
pub const MAX_IPC_MESSAGE_BYTES: usize = 65_536;
pub const MAX_IPC_QUEUE_DEPTH: usize = 256;
pub const MAX_IPC_RTT_BUDGET_MS: f64 = 2.0;

pub const OVERVIEW_VIRTUAL_INDEX_URL: &str = "https://miku.local/index.html";
pub const MAX_OVERVIEW_BUNDLE_BYTES: usize = 153_600;
pub const OVERVIEW_EXPANDED_WIDTH_PX: u32 = 640;
pub const OVERVIEW_EXPANDED_HEIGHT_PX: u32 = 380;
pub const OVERVIEW_CORNER_RADIUS_PX: u32 = 32;
pub const OVERVIEW_AVATAR_SCALE_K: f64 = 0.72;
pub const OVERVIEW_AVATAR_BODY_PX: f64 = 70.6;
pub const MAX_OVERVIEW_EVENT_FEED: usize = 32;
pub const AVATAR_VIEWBOX_SIZE: [u32; 2] = [128, 128];
pub const AVATAR_HEAD_TOP_ANCHOR: [f64; 2] = [64.0, 22.0];
pub const AVATAR_EYES_CENTER_ANCHOR: [f64; 2] = [64.0, 73.79];
pub const AVATAR_NECK_ANCHOR: [f64; 2] = [64.0, 99.77];

pub const MAX_AVATAR_HOT_RELOAD_LATENCY_MS: f64 = 50.0;
pub const MAX_SINGLE_SVG_ASSET_BYTES: usize = 65_536;
pub const EXPECTED_RIG_SPRING_COUNT: usize = 6;
pub const RIG_SLEEP_EPSILON_DEFAULT: f64 = 0.001;
pub const RIG_MAX_DT_SEC: f64 = 0.04;
pub const EXPECTED_RIG_SPRING_NAMES: [&str; EXPECTED_RIG_SPRING_COUNT] = [
    "accessory_follow",
    "chase",
    "jump",
    "squash",
    "tail",
    "ui_default",
];

pub const MAX_GATEKEEPER_INTERCEPT_LATENCY_MS: f64 = 1.0;
pub const DEFAULT_CONFIRMATION_MODAL_TTL_MS: u64 = 30_000;

pub const AUDIT_ISOLATED_SUBDIR_WINDOWS: &str = r"miku\audit";
pub const ACTIVE_AUDIT_JSONL_FILENAME: &str = "audit.jsonl";
pub const DEFAULT_MAX_AUDIT_FILE_BYTES: usize = 5 * 1024 * 1024;
pub const DEFAULT_MAX_ROTATED_AUDIT_FILES: usize = 5;
pub const MAX_AUDIT_APPEND_LATENCY_MS: f64 = 1.0;
pub const MAX_10K_VERIFY_LATENCY_MS: f64 = 50.0;
pub const MAX_AUDIT_VIEWER_PAGE_SIZE: usize = 50;

pub const VOICE_SAMPLE_RATE_HZ: u32 = 16_000;
pub const VOICE_CHANNELS: u8 = 1;
pub const DEFAULT_PTT_HOTKEY_COMBO: &str = "Alt+Space";
pub const DEFAULT_WAKE_WORD_PHRASE: &str = "Hey Miku";
pub const DEFAULT_VOICE_SILENCE_TIMEOUT_MS: u64 = 3_500;
pub const MIN_PTT_HOLD_DURATION_MS: u64 = 120;
pub const MAX_VOICE_CANCEL_LATENCY_MS: f64 = 5.0;
pub const MAX_SOUNDWAVE_UPDATE_LATENCY_MS: f64 = 16.0;
pub const VOICE_PCM_RING_CAPACITY_SAMPLES: usize = 240_000;
pub const SOUNDWAVE_BAND_COUNT: usize = 5;

pub const KILL_SWITCH_HOTKEY_ID: i32 = 0x4D4B;
pub const DEFAULT_KILL_SWITCH_HOTKEY_COMBO: &str = "Ctrl+Alt+Shift+K";
pub const MAX_KILL_SWITCH_LATENCY_MS: f64 = 20.0;
pub const REARM_CHALLENGE_TTL_MS: u64 = 30_000;
pub const MAX_REARM_ATTEMPTS: u8 = 3;
pub const WIN32_MOD_ALT: u32 = 0x0001;
pub const WIN32_MOD_CONTROL: u32 = 0x0002;
pub const WIN32_MOD_SHIFT: u32 = 0x0004;
pub const WIN32_MOD_NOREPEAT: u32 = 0x4000;
pub const KILL_SWITCH_WIN32_MODIFIERS: u32 =
    WIN32_MOD_CONTROL | WIN32_MOD_ALT | WIN32_MOD_SHIFT | WIN32_MOD_NOREPEAT;
pub const KILL_SWITCH_WIN32_VK_K: u32 = 0x4B;
pub const KILL_SWITCH_ALERT_COLOR_HEX: &str = "#F43F5E";

pub const DEFAULT_IDLE_SUSPENSION_TTL_MS: u64 = 3_000;
pub const MAX_WARM_WAKE_LATENCY_MS: f64 = 16.0;
pub const MAX_GAMING_CPU_BUDGET_PCT: f64 = 0.5;
pub const TARGET_IDLE_CPU_BUDGET_PCT: f64 = 0.0;
pub const MAX_DEFERRED_GAMING_EVENTS: usize = 32;
pub const TEN_CYCLE_STABILITY_REPS: usize = 10;

pub const DEMO_BADGE_PREFIX: &str = "[DEMO]";
pub const DEMO_PTT_HOTKEY_ID: i32 = 0x4D50;
pub const DEMO_DEFAULT_DURATION_MS: u64 = 15_000;
pub const DEMO_MAX_STARTUP_LATENCY_MS: f64 = 50.0;
pub const WIN32_VK_ESCAPE: i32 = 0x1B;
pub const WIN32_VK_SPACE: u32 = 0x20;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct JsonlTamperDiagnostic {
    pub file_name: String,
    pub corrupted_line_index: usize,
    pub expected_seq: u64,
    pub found_seq: Option<u64>,
    pub reason: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum UiError {
    InvalidMonitorBounds(String),
    InvalidDpi(u32),
    StyleFlagViolation(String),
    FocusStealDetected {
        fg_before: usize,
        fg_after: usize,
        pill_hwnd: usize,
    },
    MouseActivateViolation {
        expected: isize,
        received: isize,
    },
    AssetCatalogError(String),
    Win32Error(String),
    InvalidUserDataFolder(String),
    UnsafeNavigationBlocked(String),
    WebView2LifecycleError(String),
    UnauthorizedIpcOrigin(String),
    IpcFrameError {
        code: String,
        detail: String,
    },
    CspViolation(String),
    AvatarPackReloadRejected(String),
    GatekeeperInterceptionError(String),
    AuditLogIoError(String),
    AuditLogTamperDetected(JsonlTamperDiagnostic),
    VoiceInteractionError(String),
    KillSwitchError(String),
    IdleSuspensionError(String),
    DemoModeError(String),
}

impl std::fmt::Display for UiError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            UiError::InvalidMonitorBounds(msg) => write!(f, "invalid monitor bounds: {}", msg),
            UiError::InvalidDpi(dpi) => write!(f, "invalid monitor DPI: {}", dpi),
            UiError::StyleFlagViolation(msg) => write!(f, "Win32 style flag violation: {}", msg),
            UiError::FocusStealDetected {
                fg_before,
                fg_after,
                pill_hwnd,
            } => write!(
                f,
                "focus steal detected: fg_before=0x{:X}, fg_after=0x{:X}, pill_hwnd=0x{:X}",
                fg_before, fg_after, pill_hwnd
            ),
            UiError::MouseActivateViolation { expected, received } => write!(
                f,
                "WM_MOUSEACTIVATE violation: expected MA_NOACTIVATE ({}), got {}",
                expected, received
            ),
            UiError::AssetCatalogError(msg) => write!(f, "avatar asset catalog error: {}", msg),
            UiError::Win32Error(msg) => write!(f, "Win32 error: {}", msg),
            UiError::InvalidUserDataFolder(msg) => {
                write!(f, "invalid WebView2 user data folder: {}", msg)
            }
            UiError::UnsafeNavigationBlocked(msg) => {
                write!(f, "unsafe WebView2 navigation blocked: {}", msg)
            }
            UiError::WebView2LifecycleError(msg) => {
                write!(f, "WebView2 lifecycle error: {}", msg)
            }
            UiError::UnauthorizedIpcOrigin(msg) => {
                write!(f, "unauthorized WebView2 IPC origin: {}", msg)
            }
            UiError::IpcFrameError { code, detail } => {
                write!(f, "IPC frame error [{}]: {}", code, detail)
            }
            UiError::CspViolation(msg) => {
                write!(f, "WebView2 CSP policy violation: {}", msg)
            }
            UiError::AvatarPackReloadRejected(msg) => {
                write!(f, "avatar pack reload rejected (rolled back to active baseline): {}", msg)
            }
            UiError::GatekeeperInterceptionError(msg) => {
                write!(f, "Gatekeeper UI/Core interception error: {}", msg)
            }
            UiError::AuditLogIoError(msg) => {
                write!(f, "persistent audit log I/O error: {}", msg)
            }
            UiError::AuditLogTamperDetected(diag) => {
                write!(
                    f,
                    "audit log tamper detected in {} at line {} (expected seq {}, found {:?}): {}",
                    diag.file_name,
                    diag.corrupted_line_index,
                    diag.expected_seq,
                    diag.found_seq,
                    diag.reason
                )
            }
            UiError::VoiceInteractionError(msg) => {
                write!(f, "voice interaction error: {}", msg)
            }
            UiError::KillSwitchError(msg) => {
                write!(f, "kill switch error: {}", msg)
            }
            UiError::IdleSuspensionError(msg) => {
                write!(f, "idle suspension error: {}", msg)
            }
            UiError::DemoModeError(msg) => {
                write!(f, "demo mode error: {}", msg)
            }
        }
    }
}

impl std::error::Error for UiError {}

