//! Dual-window coordinator for the independent `Configuración · Miku` (1080×780) WebView2 window.

use std::path::PathBuf;

use miku_ui::{
    LiveWebView2Config, LiveWebView2Event, LiveWebView2Window, PillAnchorGeometry,
    WebView2SecurityPolicy, STANDARD_DPI,
};
use serde_json::json;

use crate::live_gateway::LiveGatewayRunner;
use crate::live_ui_host::{
    is_safe_external_url, open_path_or_url_in_shell, parse_live_ui_bridge_command,
    LiveUiBridgeCommand,
};

pub const SETTINGS_DIALOG_WIDTH_PX: i32 = 1080;
pub const SETTINGS_DIALOG_HEIGHT_PX: i32 = 780;

#[cfg(target_os = "windows")]
#[link(name = "user32")]
extern "system" {
    fn GetSystemMetrics(n_index: i32) -> i32;
}

/// Computes centered desktop coordinates for a standalone frameless dialog window.
pub fn compute_centered_dialog_bounds(width: i32, height: i32) -> PillAnchorGeometry {
    let w = width.clamp(760, 1440);
    let h = height.clamp(540, 1000);
    #[cfg(target_os = "windows")]
    let (screen_w, screen_h) = unsafe {
        let sx = GetSystemMetrics(0);
        let sy = GetSystemMetrics(1);
        (if sx >= 640 { sx } else { 1920 }, if sy >= 480 { sy } else { 1080 })
    };
    #[cfg(not(target_os = "windows"))]
    let (screen_w, screen_h) = (1920, 1080);

    PillAnchorGeometry {
        x: ((screen_w - w) / 2).max(16),
        y: ((screen_h - h) / 2).max(16),
        width: w,
        height: h,
        corner_radius_px: 14,
        dpi: STANDARD_DPI,
        scale_pct: 100,
    }
}

/// Sanitizes an optional settings category slug and builds the standalone settings URL.
pub fn build_settings_ui_url(gateway: &LiveGatewayRunner, section: Option<&str>) -> String {
    let mut url = format!(
        "http://{}/settings/miku_settings.html?core=1&token={}",
        gateway.local_addr, gateway.session_token
    );
    if let Some(sec) = section {
        let clean: String = sec
            .chars()
            .filter(|c| c.is_ascii_alphanumeric() || matches!(c, '_' | '-'))
            .collect();
        if !clean.is_empty() {
            url.push_str("&cat=");
            url.push_str(&clean);
        }
    }
    url
}

/// Coordinates lazy creation, hide-on-close RAM trimming (`Low`), category navigation,
/// window chrome drag/minimize/close IPC, and clean teardown of `Configuración · Miku`.
pub struct SettingsWindowCoordinator {
    gateway_addr: String,
    session_token: String,
    user_data_dir_override: Option<PathBuf>,
    window: Option<LiveWebView2Window>,
}

impl SettingsWindowCoordinator {
    pub fn new(gateway: &LiveGatewayRunner, user_data_dir_override: Option<PathBuf>) -> Self {
        Self {
            gateway_addr: gateway.local_addr.to_string(),
            session_token: gateway.session_token.clone(),
            user_data_dir_override,
            window: None,
        }
    }

    pub fn is_open_in_memory(&self) -> bool {
        self.window.is_some()
    }

    pub fn is_visible(&self) -> bool {
        self.window.as_ref().map(|w| w.is_visible()).unwrap_or(false)
    }

    pub fn is_controller_ready(&self) -> bool {
        self.window
            .as_ref()
            .map(|w| w.is_controller_ready())
            .unwrap_or(false)
    }

    pub fn bounds(&self) -> Option<PillAnchorGeometry> {
        self.window.as_ref().map(|w| w.bounds())
    }

    pub fn open_or_focus(&mut self, section: Option<&str>) -> Result<(), String> {
        let clean_cat: Option<String> = section.and_then(|s| {
            let f: String = s
                .chars()
                .filter(|c| c.is_ascii_alphanumeric() || matches!(c, '_' | '-'))
                .collect();
            if f.is_empty() {
                None
            } else {
                Some(f)
            }
        });

        if let Some(win) = self.window.as_mut() {
            win.restore_and_focus();
            if let Some(cat) = clean_cat {
                let msg = json!({ "type": "go_cat", "cat": cat }).to_string();
                let _ = win.post_web_message_json(&msg);
            }
            return Ok(());
        }

        let mut url = format!(
            "http://{}/settings/miku_settings.html?core=1&token={}",
            self.gateway_addr, self.session_token
        );
        if let Some(cat) = clean_cat {
            url.push_str("&cat=");
            url.push_str(&cat);
        }

        let bounds = compute_centered_dialog_bounds(
            SETTINGS_DIALOG_WIDTH_PX,
            SETTINGS_DIALOG_HEIGHT_PX,
        );
        let win = LiveWebView2Window::open_dialog(
            LiveWebView2Config {
                url,
                auth_token: Some(self.session_token.clone()),
                bounds,
                security_policy: WebView2SecurityPolicy::release_hardened(),
                user_data_dir_override: self.user_data_dir_override.clone(),
                transparent_bg: false,
            },
            "Configuración · Miku",
        )
        .map_err(|e| format!("open settings WebView2 window: {}", e))?;
        self.window = Some(win);
        Ok(())
    }

    pub fn pump(&mut self, gateway: &LiveGatewayRunner) -> Result<(), String> {
        if let Some(sec) = gateway.api_handler().take_pending_open_settings() {
            self.open_or_focus(sec.as_deref())?;
        }

        let Some(win) = self.window.as_mut() else {
            return Ok(());
        };
        let _ = win.sync_visibility_from_hwnd();
        let events = win.pump_events();
        for ev in events {
            match ev {
                LiveWebView2Event::ControllerError(err) => {
                    return Err(format!("Settings WebView2 controller error: {}", err));
                }
                LiveWebView2Event::WebMessageJson(raw) => {
                    if let Some(cmd) = parse_live_ui_bridge_command(&raw) {
                        match cmd {
                            LiveUiBridgeCommand::StartDrag => win.start_drag(),
                            LiveUiBridgeCommand::WindowControl { action } => match action.as_str() {
                                "minimize" => win.minimize(),
                                "hide" | "close" => win.hide_and_trim(),
                                "focus" | "show" => win.restore_and_focus(),
                                _ => {}
                            },
                            LiveUiBridgeCommand::OpenExternal { url } => {
                                if is_safe_external_url(&url) {
                                    open_path_or_url_in_shell(&url);
                                }
                            }
                            LiveUiBridgeCommand::ToggleKillSwitch => {
                                let _ = gateway.api_handler().toggle_kill_switch();
                            }
                            _ => {}
                        }
                    }
                }
                LiveWebView2Event::ControllerReady => {}
                LiveWebView2Event::PermissionRequested { .. } => {}
            }
        }
        Ok(())
    }

    pub fn release_window(&mut self) {
        if let Some(win) = self.window.take() {
            win.close_without_drain();
        }
    }

    pub fn close_and_drain(&mut self) -> Vec<u32> {
        match self.window.take() {
            Some(win) => win.close_and_drain(),
            None => Vec::new(),
        }
    }
}
