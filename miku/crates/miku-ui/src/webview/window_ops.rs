//! Window positioning, click-through hit testing, visibility/memory trimming, drag,
//! and WebView2 IPC message helpers for [`LiveWebView2Window`].

use crate::prelude::*;

use super::live_host::{LiveWebView2Window, LiveWindowKind};
#[cfg(target_os = "windows")]
use super::com_abi::win_com;
#[cfg(target_os = "windows")]
use super::win32_com_ffi::win_ffi;

impl LiveWebView2Window {
    pub fn resize_bounds(&mut self, next: PillAnchorGeometry) -> Result<(), UiError> {
        if next.width <= 0 || next.height <= 0 {
            return Err(UiError::InvalidMonitorBounds(
                "bounds width and height must be positive".to_string(),
            ));
        }
        self.bounds = next;
        #[cfg(target_os = "windows")]
        unsafe {
            if self.hwnd_raw != 0 {
                let hwnd = self.hwnd_raw as win_ffi::Hwnd;
                let flags = if self.visible {
                    SWP_NOACTIVATE_FLAG | SWP_NOZORDER_FLAG | SWP_SHOWWINDOW_FLAG
                } else {
                    SWP_NOACTIVATE_FLAG | SWP_NOZORDER_FLAG
                };
                let _ = win_ffi::SetWindowPos(
                    hwnd,
                    std::ptr::null_mut(),
                    next.x,
                    next.y,
                    next.width,
                    next.height,
                    flags,
                );
                if self.window_kind == LiveWindowKind::FloatingIsland && !self.transparent_bg {
                    super::transparent_frame::apply_opaque_rounded_rgn(
                        hwnd as usize,
                        next.width,
                        next.height,
                        next.corner_radius_px,
                    );
                }
            }
            if self.ctrl_ptr != 0 {
                let rect = win_ffi::Rect {
                    left: 0,
                    top: 0,
                    right: next.width,
                    bottom: next.height,
                };
                let _ = win_com::ctrl_put_bounds(self.ctrl_ptr as *mut std::ffi::c_void, &rect);
            }
        }
        Ok(())
    }

    /// Enables or disables `WS_EX_TRANSPARENT` pass-through on transparent outer padding pixels.
    pub fn set_cursor_pass_through(&mut self, ignore_cursor: bool) {
        if self.cursor_ignored == ignore_cursor {
            return;
        }
        self.cursor_ignored = ignore_cursor;
        #[cfg(target_os = "windows")]
        unsafe {
            super::transparent_frame::set_click_through(self.hwnd_raw, ignore_cursor);
        }
    }

    pub fn is_cursor_ignored(&self) -> bool {
        self.cursor_ignored
    }

    /// Toggles interactive keyboard activation (`MA_ACTIVATE` + clearing `WS_EX_NOACTIVATE`)
    /// so Dynamic Island inputs can accept typing without stealing focus during passive hover.
    pub fn set_interactive_focus(&self, interactive: bool, activate_now: bool) {
        super::transparent_frame::set_hwnd_interactive_focus(
            self.hwnd_raw,
            interactive,
            activate_now,
        );
    }

    pub fn set_memory_usage_low(&self, low: bool) {
        #[cfg(target_os = "windows")]
        unsafe {
            if self.web_ptr != 0 {
                let _ = win_com::web19_set_memory_usage_low(self.web_ptr as *mut std::ffi::c_void, low);
            }
        }
        #[cfg(not(target_os = "windows"))]
        let _ = low;
    }

    pub fn is_visible(&self) -> bool {
        self.visible
    }

    /// Synchronizes visibility after Win32 `WM_CLOSE` hides a persistent dialog window,
    /// triggering WebView2 memory trimming when the window leaves the screen.
    pub fn sync_visibility_from_hwnd(&mut self) -> bool {
        #[cfg(target_os = "windows")]
        unsafe {
            if self.hwnd_raw != 0 {
                let actually_visible = win_ffi::IsWindowVisible(self.hwnd_raw as win_ffi::Hwnd) != 0;
                if self.visible != actually_visible {
                    self.visible = actually_visible;
                    if self.ctrl_ptr != 0 {
                        let _ = win_com::ctrl_put_visible(
                            self.ctrl_ptr as *mut std::ffi::c_void,
                            actually_visible,
                        );
                    }
                    self.set_memory_usage_low(!actually_visible);
                }
            }
        }
        self.visible
    }

    pub fn set_visible(&mut self, visible: bool) {
        self.visible = visible;
        #[cfg(target_os = "windows")]
        unsafe {
            if self.ctrl_ptr != 0 {
                let _ = win_com::ctrl_put_visible(self.ctrl_ptr as *mut std::ffi::c_void, visible);
            }
            if self.hwnd_raw != 0 {
                let cmd = match (self.window_kind, visible) {
                    (_, false) => win_ffi::SW_HIDE,
                    (LiveWindowKind::FloatingIsland, true) => SW_SHOWNOACTIVATE_CMD,
                    (LiveWindowKind::FramelessDialog, true) => win_ffi::SW_RESTORE,
                };
                let _ = win_ffi::ShowWindow(self.hwnd_raw as win_ffi::Hwnd, cmd);
                if visible && self.window_kind == LiveWindowKind::FramelessDialog {
                    let _ = win_ffi::SetForegroundWindow(self.hwnd_raw as win_ffi::Hwnd);
                }
            }
        }
        self.set_memory_usage_low(!visible);
    }

    pub fn minimize(&self) {
        #[cfg(target_os = "windows")]
        unsafe {
            if self.hwnd_raw != 0 {
                let _ = win_ffi::ShowWindow(self.hwnd_raw as win_ffi::Hwnd, win_ffi::SW_MINIMIZE);
            }
        }
    }

    pub fn restore_and_focus(&mut self) {
        self.set_visible(true);
    }

    pub fn hide_and_trim(&mut self) {
        self.set_visible(false);
    }

    pub fn start_drag(&self) {
        #[cfg(target_os = "windows")]
        unsafe {
            if self.hwnd_raw != 0 {
                let hwnd = self.hwnd_raw as win_ffi::Hwnd;
                let _ = win_ffi::ReleaseCapture();
                let _ = win_ffi::SendMessageW(
                    hwnd,
                    win_ffi::WM_SYSCOMMAND,
                    win_ffi::SC_MOVE_HTCAPTION,
                    0,
                );
            }
        }
    }

    pub fn post_web_message_json(&self, json_str: &str) -> Result<(), UiError> {
        #[cfg(target_os = "windows")]
        unsafe {
            if self.web_ptr != 0 {
                let hr =
                    win_com::web_post_message_json(self.web_ptr as *mut std::ffi::c_void, json_str);
                if hr < 0 {
                    return Err(UiError::WebView2LifecycleError(format!(
                        "PostWebMessageAsJson failed HRESULT=0x{:08x}",
                        hr as u32
                    )));
                }
            }
        }
        Ok(())
    }
}
