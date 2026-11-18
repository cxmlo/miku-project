//! WebView2 domain: Mode C on-demand lifecycle host with warm TTL (`30_000 ms`), isolated
//! `%LOCALAPPDATA%\miku\webview2` data folder, JSON-RPC v1 IPC bridge with mandatory `textContent`
//! XSS neutralization, and Overview scene (`https://miku.local/index.html`) (`ADR-001` / `ADR-011`).

pub mod host_fsm;
pub mod ipc_bridge;
pub mod ipc_envelope;
pub mod overview_bundle;
pub mod overview_controller;
mod overview_svg_assets;
mod overview_ui_panels;
mod overview_ui_script;
mod overview_ui_styles;
mod com_abi;
pub mod cursor_tracker;
pub mod live_host;
pub mod security_and_driver;
mod transparent_frame;
pub mod win32_com_driver;
mod win32_com_ffi;
mod window_ops;

pub use self::cursor_tracker::*;
pub use self::host_fsm::*;
pub use self::ipc_bridge::*;
pub use self::ipc_envelope::*;
pub use self::live_host::{
    LiveWebView2Config, LiveWebView2Event, LiveWebView2Window, LiveWindowKind,
};
pub use self::overview_bundle::*;
pub use self::overview_controller::*;
pub use self::security_and_driver::*;
pub use self::win32_com_driver::*;

/// Controls whether WebView2 `PermissionRequested` approves microphone access (`voice_stt_enabled`).
pub fn set_webview_mic_permission_allowed(allowed: bool) {
    #[cfg(target_os = "windows")]
    com_abi::win_com::set_mic_permission_allowed(allowed);
    #[cfg(not(target_os = "windows"))]
    let _ = allowed;
}

/// Returns whether WebView2 microphone permission auto-approval is currently enabled.
pub fn is_webview_mic_permission_allowed() -> bool {
    #[cfg(target_os = "windows")]
    {
        com_abi::win_com::is_mic_permission_allowed()
    }
    #[cfg(not(target_os = "windows"))]
    {
        false
    }
}

#[cfg(test)]
pub(crate) static WEBVIEW2_CHILD_PROC_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

#[cfg(test)]
mod tests_com_and_facade;
#[cfg(test)]
mod tests_host_and_ipc;
#[cfg(test)]
mod tests_live_webview2;
#[cfg(test)]
mod tests_overview;
#[cfg(test)]
mod tests_overview_ui_v7;
