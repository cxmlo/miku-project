#![allow(unused_imports)]
use crate::prelude::*;

// ============================================================================
// 2. Multi-Monitor & DPI Geometry Engine
// ============================================================================

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct VirtualRect {
    pub left: i32,
    pub top: i32,
    pub right: i32,
    pub bottom: i32,
}

impl VirtualRect {
    pub fn new(left: i32, top: i32, right: i32, bottom: i32) -> Result<Self, UiError> {
        if right <= left || bottom <= top {
            return Err(UiError::InvalidMonitorBounds(format!(
                "rect ({}, {})..({}, {}) has non-positive width or height",
                left, top, right, bottom
            )));
        }
        Ok(Self {
            left,
            top,
            right,
            bottom,
        })
    }

    pub fn width(&self) -> i32 {
        self.right - self.left
    }

    pub fn height(&self) -> i32 {
        self.bottom - self.top
    }

    pub fn contains_point(&self, x: i32, y: i32) -> bool {
        x >= self.left && x < self.right && y >= self.top && y < self.bottom
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MonitorDescriptor {
    pub device_name: String,
    pub is_primary: bool,
    pub dpi: u32,
    pub monitor_rect: VirtualRect,
    pub work_area: VirtualRect,
}

impl MonitorDescriptor {
    pub fn scale_factor(&self) -> f64 {
        (self.dpi as f64) / (STANDARD_DPI as f64)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct PillAnchorGeometry {
    pub x: i32,
    pub y: i32,
    pub width: i32,
    pub height: i32,
    pub corner_radius_px: i32,
    pub dpi: u32,
    pub scale_pct: u32,
}

impl PillAnchorGeometry {
    pub fn bounding_rect(&self) -> VirtualRect {
        VirtualRect {
            left: self.x,
            top: self.y,
            right: self.x + self.width,
            bottom: self.y + self.height,
        }
    }

    pub fn center_x(&self) -> i32 {
        self.x + self.width / 2
    }
}

pub fn scale_px_for_dpi(logical_px: u32, dpi: u32) -> Result<i32, UiError> {
    if dpi < 48 || dpi > 768 {
        return Err(UiError::InvalidDpi(dpi));
    }
    let scaled = ((logical_px as f64) * (dpi as f64) / (STANDARD_DPI as f64)).round() as i32;
    Ok(scaled)
}

/// Computes the exact top-center anchor coordinates `(x, y, width, height)` for a monitor's
/// `work_area`, accurately preserving negative coordinates on secondary monitors to the
/// left or above the primary monitor.
pub fn compute_top_center_pill_anchor(
    monitor: &MonitorDescriptor,
    logical_width: u32,
    logical_height: u32,
    logical_corner_radius: u32,
    logical_top_margin: u32,
) -> Result<PillAnchorGeometry, UiError> {
    if monitor.work_area.width() <= 0 || monitor.work_area.height() <= 0 {
        return Err(UiError::InvalidMonitorBounds(
            "work_area width/height must be positive".to_string(),
        ));
    }
    let width = scale_px_for_dpi(logical_width, monitor.dpi)?;
    let height = scale_px_for_dpi(logical_height, monitor.dpi)?;
    let corner_radius_px = scale_px_for_dpi(logical_corner_radius, monitor.dpi)?;
    let top_margin = scale_px_for_dpi(logical_top_margin, monitor.dpi)?;

    if width <= 0 || height <= 0 || width > monitor.work_area.width() {
        return Err(UiError::InvalidMonitorBounds(format!(
            "scaled pill width {} exceeds monitor work_area width {}",
            width,
            monitor.work_area.width()
        )));
    }

    let x = monitor.work_area.left + (monitor.work_area.width() - width) / 2;
    let y = monitor.work_area.top + top_margin;
    let scale_pct = ((monitor.dpi as f64 / STANDARD_DPI as f64) * 100.0).round() as u32;

    Ok(PillAnchorGeometry {
        x,
        y,
        width,
        height,
        corner_radius_px,
        dpi: monitor.dpi,
        scale_pct,
    })
}

/// Selects the target monitor for the Island pill:
/// - If `cursor_pos` is provided and falls inside a monitor's `monitor_rect`, selects that monitor.
/// - Otherwise selects the primary monitor (`is_primary == true`), or the first monitor.
pub fn select_target_monitor(
    monitors: &[MonitorDescriptor],
    cursor_pos: Option<(i32, i32)>,
) -> Result<&MonitorDescriptor, UiError> {
    if monitors.is_empty() {
        return Err(UiError::InvalidMonitorBounds(
            "monitor list cannot be empty".to_string(),
        ));
    }
    if let Some((cx, cy)) = cursor_pos {
        if let Some(found) = monitors.iter().find(|m| m.monitor_rect.contains_point(cx, cy)) {
            return Ok(found);
        }
    }
    if let Some(primary) = monitors.iter().find(|m| m.is_primary) {
        return Ok(primary);
    }
    Ok(&monitors[0])
}

/// Handles `WM_DPICHANGED` or `WM_DISPLAYCHANGE` by updating the monitor's DPI/bounds
/// and recomputing the top-center anchor in O(1).
pub fn recompute_anchor_on_dpi_or_display_change(
    updated_monitor: &MonitorDescriptor,
    mode: PillVisualMode,
    logical_top_margin: u32,
) -> Result<PillAnchorGeometry, UiError> {
    let (w, h, r) = mode.logical_dimensions();
    compute_top_center_pill_anchor(updated_monitor, w, h, r, logical_top_margin)
}

// ============================================================================
// 3. Win32 Style Flag Validator & Focus Guard
// ============================================================================

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct Win32PillStyleConfig {
    pub base_style: u32,
    pub ex_style: u32,
    pub show_cmd: i32,
}

impl Default for Win32PillStyleConfig {
    fn default() -> Self {
        Self {
            base_style: REQUIRED_BASE_STYLE,
            ex_style: REQUIRED_EX_STYLE,
            show_cmd: SW_SHOWNOACTIVATE_CMD,
        }
    }
}

impl Win32PillStyleConfig {
    pub fn validate(&self) -> Result<(), UiError> {
        if (self.base_style & WS_POPUP_STYLE) == 0 {
            return Err(UiError::StyleFlagViolation(
                "missing WS_POPUP (0x80000000) in base_style".to_string(),
            ));
        }
        if (self.ex_style & WS_EX_LAYERED_STYLE) == 0 {
            return Err(UiError::StyleFlagViolation(
                "missing WS_EX_LAYERED (0x00080000) in ex_style".to_string(),
            ));
        }
        if (self.ex_style & WS_EX_TOPMOST_STYLE) == 0 {
            return Err(UiError::StyleFlagViolation(
                "missing WS_EX_TOPMOST (0x00000008) in ex_style".to_string(),
            ));
        }
        if (self.ex_style & WS_EX_TOOLWINDOW_STYLE) == 0 {
            return Err(UiError::StyleFlagViolation(
                "missing WS_EX_TOOLWINDOW (0x00000080) in ex_style".to_string(),
            ));
        }
        if (self.ex_style & WS_EX_NOACTIVATE_STYLE) == 0 {
            return Err(UiError::StyleFlagViolation(
                "missing WS_EX_NOACTIVATE (0x08000000) in ex_style".to_string(),
            ));
        }
        if (self.ex_style & WS_EX_APPWINDOW_FORBIDDEN) != 0 {
            return Err(UiError::StyleFlagViolation(
                "WS_EX_APPWINDOW (0x00040000) is forbidden on the Island pill".to_string(),
            ));
        }
        if self.show_cmd != SW_SHOWNOACTIVATE_CMD {
            return Err(UiError::StyleFlagViolation(format!(
                "ShowWindow command must be SW_SHOWNOACTIVATE (4), got {}",
                self.show_cmd
            )));
        }
        Ok(())
    }
}

/// Evaluates how the Island window procedure responds to `WM_MOUSEACTIVATE`.
pub fn handle_wm_mouseactivate(msg: u32) -> Option<isize> {
    if msg == WM_MOUSEACTIVATE_MSG {
        Some(MA_NOACTIVATE_RESULT)
    } else {
        None
    }
}

/// Verifies that showing, hovering, or clicking the native pill window did not steal focus
/// from `fg_before`. Supports headless CI environments where `fg_before == 0`.
pub fn verify_foreground_focus_invariant(
    fg_before: usize,
    fg_after: usize,
    pill_hwnd: usize,
    ma_result: isize,
) -> Result<bool, UiError> {
    if ma_result != MA_NOACTIVATE_RESULT {
        return Err(UiError::MouseActivateViolation {
            expected: MA_NOACTIVATE_RESULT,
            received: ma_result,
        });
    }
    if pill_hwnd != 0 && fg_after == pill_hwnd {
        return Err(UiError::FocusStealDetected {
            fg_before,
            fg_after,
            pill_hwnd,
        });
    }
    // If an interactive foreground window existed prior to showing the pill,
    // it must remain the exact foreground window afterward.
    if fg_before != 0 && fg_after != 0 && fg_before != fg_after {
        return Err(UiError::FocusStealDetected {
            fg_before,
            fg_after,
            pill_hwnd,
        });
    }
    Ok(true)
}

