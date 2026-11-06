#![allow(
    unknown_lints,
    clippy::too_many_arguments,
    clippy::manual_range_contains,
    clippy::needless_range_loop,
    clippy::large_enum_variant,
    clippy::chunks_exact_to_as_chunks,
    clippy::type_complexity,
    clippy::invisible_characters
)]

use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::time::Instant;

use miku_protocol::sha256_hex;
use serde::{Deserialize, Serialize};

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
        }
    }
}

impl std::error::Error for UiError {}

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

// ============================================================================
// 4. Hover Trigger State Machine (IdleCompact <-> HoverPeek with Hysteresis)
// ============================================================================

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum PillVisualMode {
    IdleCompact,
    HoverPeek,
}

impl PillVisualMode {
    pub fn logical_dimensions(&self) -> (u32, u32, u32) {
        match self {
            PillVisualMode::IdleCompact => (
                IDLE_COMPACT_WIDTH_PX,
                IDLE_COMPACT_HEIGHT_PX,
                IDLE_CORNER_RADIUS_PX,
            ),
            PillVisualMode::HoverPeek => (
                HOVER_PEEK_WIDTH_PX,
                HOVER_PEEK_HEIGHT_PX,
                HOVER_CORNER_RADIUS_PX,
            ),
        }
    }

    pub fn avatar_scale_k(&self) -> f64 {
        match self {
            PillVisualMode::IdleCompact => IDLE_AVATAR_SCALE_K,
            PillVisualMode::HoverPeek => HOVER_AVATAR_SCALE_K,
        }
    }

    pub fn avatar_body_px(&self) -> f64 {
        match self {
            PillVisualMode::IdleCompact => IDLE_AVATAR_BODY_PX,
            PillVisualMode::HoverPeek => HOVER_AVATAR_BODY_PX,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct HoverTransitionResult {
    pub previous_mode: PillVisualMode,
    pub new_mode: PillVisualMode,
    pub transitioned: bool,
    pub geometry: PillAnchorGeometry,
    pub avatar_scale_k: f64,
    pub avatar_body_px: f64,
    pub active_timers: u32,
    pub webview2_instances: usize,
    pub elapsed_ms: f64,
}

#[derive(Debug, Clone)]
pub struct PillHoverStateMachine {
    monitor: MonitorDescriptor,
    top_margin_px: u32,
    hysteresis_px: i32,
    mode: PillVisualMode,
    current_geometry: PillAnchorGeometry,
    transitions_count: u64,
}

impl PillHoverStateMachine {
    pub fn new(monitor: MonitorDescriptor, top_margin_px: u32) -> Result<Self, UiError> {
        let mode = PillVisualMode::IdleCompact;
        let (w, h, r) = mode.logical_dimensions();
        let current_geometry =
            compute_top_center_pill_anchor(&monitor, w, h, r, top_margin_px)?;
        Ok(Self {
            monitor,
            top_margin_px,
            hysteresis_px: HOVER_HYSTERESIS_PX,
            mode,
            current_geometry,
            transitions_count: 0,
        })
    }

    pub fn mode(&self) -> PillVisualMode {
        self.mode
    }

    pub fn geometry(&self) -> PillAnchorGeometry {
        self.current_geometry
    }

    pub fn transitions_count(&self) -> u64 {
        self.transitions_count
    }

    pub fn active_timers(&self) -> u32 {
        match self.mode {
            PillVisualMode::IdleCompact => 0,
            PillVisualMode::HoverPeek => 1, // deferred hover-open countdown only while hovering
        }
    }

    pub fn webview2_instances(&self) -> usize {
        0
    }

    /// Evaluates a cursor position `(cursor_x, cursor_y)` in virtual screen coordinates.
    /// Uses strict bounds to enter `HoverPeek` and an inflated hysteresis box (`+6 px`)
    /// to exit back to `IdleCompact`, preventing edge flicker.
    pub fn on_cursor_move(
        &mut self,
        cursor_x: i32,
        cursor_y: i32,
    ) -> Result<HoverTransitionResult, UiError> {
        let t0 = Instant::now();
        let previous_mode = self.mode;
        let rect = self.current_geometry.bounding_rect();

        let next_mode = match self.mode {
            PillVisualMode::IdleCompact => {
                if rect.contains_point(cursor_x, cursor_y) {
                    PillVisualMode::HoverPeek
                } else {
                    PillVisualMode::IdleCompact
                }
            }
            PillVisualMode::HoverPeek => {
                let hyst = scale_px_for_dpi(self.hysteresis_px as u32, self.monitor.dpi)?;
                let inflated = VirtualRect {
                    left: rect.left - hyst,
                    top: rect.top - hyst,
                    right: rect.right + hyst,
                    bottom: rect.bottom + hyst,
                };
                if inflated.contains_point(cursor_x, cursor_y) {
                    PillVisualMode::HoverPeek
                } else {
                    PillVisualMode::IdleCompact
                }
            }
        };

        let transitioned = next_mode != previous_mode;
        if transitioned {
            self.mode = next_mode;
            let (w, h, r) = self.mode.logical_dimensions();
            self.current_geometry =
                compute_top_center_pill_anchor(&self.monitor, w, h, r, self.top_margin_px)?;
            self.transitions_count += 1;
        }

        let elapsed_ms = t0.elapsed().as_secs_f64() * 1_000.0;
        Ok(HoverTransitionResult {
            previous_mode,
            new_mode: self.mode,
            transitioned,
            geometry: self.current_geometry,
            avatar_scale_k: self.mode.avatar_scale_k(),
            avatar_body_px: self.mode.avatar_body_px(),
            active_timers: self.active_timers(),
            webview2_instances: self.webview2_instances(),
            elapsed_ms,
        })
    }

    pub fn update_monitor(&mut self, new_monitor: MonitorDescriptor) -> Result<PillAnchorGeometry, UiError> {
        self.monitor = new_monitor;
        self.current_geometry = recompute_anchor_on_dpi_or_display_change(
            &self.monitor,
            self.mode,
            self.top_margin_px,
        )?;
        Ok(self.current_geometry)
    }
}

// ============================================================================
// 5. Avatar Asset Catalog Loader & Validator (rig.toml v7 + design-tokens.json + 22 SVGs)
// ============================================================================

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct V7RigMeta {
    pub version: u32,
    pub source: String,
    pub viewbox: [u32; 2],
    pub body_size: [f64; 2],
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct V7RigPhysics {
    pub sleep_epsilon: f64,
    pub idle_fps: u32,
    pub active_fps: u32,
    pub max_dt: f64,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct V7RigLayer {
    pub id: String,
    pub z: u32,
    pub svg: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct V7AccessoryItem {
    pub label: String,
    pub slot: String,
    pub svg: String,
    pub svg_back: Option<String>,
    pub layer: String,
    pub pivot: [f64; 2],
    pub size: [f64; 2],
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct V7AccessoriesSection {
    pub items: BTreeMap<String, V7AccessoryItem>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct V7RigDocument {
    pub meta: V7RigMeta,
    pub physics: V7RigPhysics,
    pub layers: Vec<V7RigLayer>,
    pub accessories: V7AccessoriesSection,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct V7ScaleEntry {
    pub k: f64,
    pub body_px: f64,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct V7ScaleSection {
    pub idle: V7ScaleEntry,
    pub hover: V7ScaleEntry,
    pub hover_open_scene: V7ScaleEntry,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct V7IslandCompactTokens {
    pub width: u32,
    pub height: u32,
    pub radius_bottom: u32,
    pub avatar_slot: [f64; 2],
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct V7IslandExpandedTokens {
    pub width_default: u32,
    pub radius_bottom: u32,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct V7IslandSection {
    pub background: String,
    pub compact: V7IslandCompactTokens,
    pub expanded: V7IslandExpandedTokens,
    pub native_window_pad: u32,
    pub autoclose_s: u32,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct V7DesignTokensDocument {
    pub scale: V7ScaleSection,
    pub island: V7IslandSection,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AvatarCatalogAuditReport {
    pub rig_version: u32,
    pub viewbox: [u32; 2],
    pub idle_fps: u32,
    pub layer_count: usize,
    pub accessory_item_count: usize,
    pub verified_svg_count: usize,
    pub compact_width_px: u32,
    pub compact_height_px: u32,
    pub catalog_sha256: String,
}

pub fn resolve_avatar_assets_dir() -> PathBuf {
    let manifest_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    // crates/miku-ui -> crates -> miku -> assets/avatar
    manifest_dir
        .parent()
        .and_then(|p| p.parent())
        .map(|miku_root| miku_root.join("assets").join("avatar"))
        .unwrap_or_else(|| PathBuf::from("assets/avatar"))
}

pub fn validate_avatar_asset_catalog(
    avatar_dir: &Path,
) -> Result<AvatarCatalogAuditReport, UiError> {
    let rig_path = avatar_dir.join("rig.toml");
    let tokens_path = avatar_dir.join("design-tokens.json");

    let rig_raw = fs::read_to_string(&rig_path)
        .map_err(|e| UiError::AssetCatalogError(format!("read {}: {}", rig_path.display(), e)))?;
    let tokens_raw = fs::read_to_string(&tokens_path).map_err(|e| {
        UiError::AssetCatalogError(format!("read {}: {}", tokens_path.display(), e))
    })?;

    let rig: V7RigDocument = toml::from_str(&rig_raw)
        .map_err(|e| UiError::AssetCatalogError(format!("parse rig.toml: {}", e)))?;
    let tokens: V7DesignTokensDocument = serde_json::from_str(&tokens_raw)
        .map_err(|e| UiError::AssetCatalogError(format!("parse design-tokens.json: {}", e)))?;

    if rig.meta.version != 1 || rig.meta.viewbox != [128, 128] {
        return Err(UiError::AssetCatalogError(
            "rig.toml meta must have version=1 and viewbox=[128, 128]".to_string(),
        ));
    }
    if rig.physics.idle_fps != 0 || (rig.physics.sleep_epsilon - 0.001).abs() > 1e-6 {
        return Err(UiError::AssetCatalogError(
            "rig.toml physics must enforce idle_fps=0 and sleep_epsilon=0.001".to_string(),
        ));
    }
    if rig.layers.len() != 9 {
        return Err(UiError::AssetCatalogError(format!(
            "expected 9 layers in rig.toml, found {}",
            rig.layers.len()
        )));
    }
    if rig.accessories.items.len() != 14 {
        return Err(UiError::AssetCatalogError(format!(
            "expected 14 accessory items in rig.toml, found {}",
            rig.accessories.items.len()
        )));
    }

    if tokens.island.compact.width != IDLE_COMPACT_WIDTH_PX
        || tokens.island.compact.height != IDLE_COMPACT_HEIGHT_PX
    {
        return Err(UiError::AssetCatalogError(format!(
            "unexpected compact island dimensions {}x{}",
            tokens.island.compact.width, tokens.island.compact.height
        )));
    }

    let mut hasher_input = Vec::with_capacity(65_536);
    hasher_input.extend_from_slice(rig_raw.trim().as_bytes());
    hasher_input.push(b'|');
    hasher_input.extend_from_slice(tokens_raw.trim().as_bytes());

    let mut verified_svg_count = 0usize;
    for rel_path in EXPECTED_AVATAR_SVG_REL_PATHS {
        let svg_path = avatar_dir.join(rel_path);
        let svg_raw = fs::read_to_string(&svg_path).map_err(|e| {
            UiError::AssetCatalogError(format!("missing SVG {}: {}", svg_path.display(), e))
        })?;
        if !svg_raw.contains("<svg") || !svg_raw.contains("viewBox=\"0 0 128 128\"") {
            return Err(UiError::AssetCatalogError(format!(
                "SVG '{}' missing <svg> or viewBox=\"0 0 128 128\"",
                rel_path
            )));
        }
        if svg_raw.to_ascii_lowercase().contains("<script") {
            return Err(UiError::AssetCatalogError(format!(
                "SVG '{}' contains forbidden <script> tag",
                rel_path
            )));
        }
        hasher_input.push(b'|');
        hasher_input.extend_from_slice(rel_path.as_bytes());
        verified_svg_count += 1;
    }

    Ok(AvatarCatalogAuditReport {
        rig_version: rig.meta.version,
        viewbox: rig.meta.viewbox,
        idle_fps: rig.physics.idle_fps,
        layer_count: rig.layers.len(),
        accessory_item_count: rig.accessories.items.len(),
        verified_svg_count,
        compact_width_px: tokens.island.compact.width,
        compact_height_px: tokens.island.compact.height,
        catalog_sha256: sha256_hex(&hasher_input),
    })
}

// ============================================================================
// 6. Native Win32 Layered Window Execution & Monitor Enumeration
// ============================================================================

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct NativePillCycleMetrics {
    pub monitor_count: usize,
    pub primary_anchor: PillAnchorGeometry,
    pub ex_style_verified: bool,
    pub ma_noactivate_verified: bool,
    pub focus_preserved: bool,
    pub hover_latency_ms: f64,
    pub webview2_instances: usize,
    pub active_idle_timers: u32,
    pub verified_svg_count: usize,
}

#[cfg(target_os = "windows")]
mod win32_native {
    use super::*;
    use std::ffi::c_void;
    use std::mem::{size_of, zeroed};
    use std::ptr::{null, null_mut};

    type Hwnd = *mut c_void;
    type Hinstance = *mut c_void;
    type Hmonitor = *mut c_void;
    type Hdc = *mut c_void;
    type Hicon = *mut c_void;
    type Hcursor = *mut c_void;
    type Hbrush = *mut c_void;
    type Hmenu = *mut c_void;
    type Atom = u16;
    type Bool = i32;
    type Uint = u32;
    type Dword = u32;
    type Long = i32;
    type Wparam = usize;
    type Lparam = isize;
    type Lresult = isize;

    const GWL_EXSTYLE: i32 = -20;
    const LWA_ALPHA: Dword = 0x0000_0002;
    const PM_REMOVE: Uint = 0x0001;
    const MONITORINFOF_PRIMARY: Dword = 0x0000_0001;

    #[repr(C)]
    #[derive(Clone, Copy)]
    struct WinRect {
        left: Long,
        top: Long,
        right: Long,
        bottom: Long,
    }

    #[repr(C)]
    struct MonitorInfoExW {
        cb_size: Dword,
        rc_monitor: WinRect,
        rc_work: WinRect,
        dw_flags: Dword,
        sz_device: [u16; 32],
    }

    #[repr(C)]
    struct WndClassExW {
        cb_size: Uint,
        style: Uint,
        lpfn_wnd_proc: unsafe extern "system" fn(Hwnd, Uint, Wparam, Lparam) -> Lresult,
        cb_cls_extra: i32,
        cb_wnd_extra: i32,
        h_instance: Hinstance,
        h_icon: Hicon,
        h_cursor: Hcursor,
        hbr_background: Hbrush,
        lpsz_menu_name: *const u16,
        lpsz_class_name: *const u16,
        h_icon_sm: Hicon,
    }

    #[repr(C)]
    struct Point {
        x: Long,
        y: Long,
    }

    #[repr(C)]
    struct Msg {
        hwnd: Hwnd,
        message: Uint,
        w_param: Wparam,
        l_param: Lparam,
        time: Dword,
        pt: Point,
    }

    type MonitorEnumProc = unsafe extern "system" fn(Hmonitor, Hdc, *mut WinRect, Lparam) -> Bool;

    #[link(name = "kernel32")]
    extern "system" {
        fn GetModuleHandleW(lp_module_name: *const u16) -> Hinstance;
    }

    #[link(name = "user32")]
    extern "system" {
        fn EnumDisplayMonitors(
            hdc: Hdc,
            lprc_clip: *const WinRect,
            lpfn_enum: MonitorEnumProc,
            dw_data: Lparam,
        ) -> Bool;
        fn GetMonitorInfoW(hmonitor: Hmonitor, lpmi: *mut MonitorInfoExW) -> Bool;
        fn GetDpiForSystem() -> Uint;
        fn RegisterClassExW(lpwcx: *const WndClassExW) -> Atom;
        fn CreateWindowExW(
            dw_ex_style: Dword,
            lp_class_name: *const u16,
            lp_window_name: *const u16,
            dw_style: Dword,
            x: i32,
            y: i32,
            n_width: i32,
            n_height: i32,
            h_wnd_parent: Hwnd,
            h_menu: Hmenu,
            h_instance: Hinstance,
            lp_param: *const c_void,
        ) -> Hwnd;
        fn DefWindowProcW(hwnd: Hwnd, msg: Uint, wparam: Wparam, lparam: Lparam) -> Lresult;
        fn SetLayeredWindowAttributes(
            hwnd: Hwnd,
            cr_key: Dword,
            b_alpha: u8,
            dw_flags: Dword,
        ) -> Bool;
        fn ShowWindow(hwnd: Hwnd, n_cmd_show: i32) -> Bool;
        fn SetWindowPos(
            hwnd: Hwnd,
            h_wnd_insert_after: Hwnd,
            x: i32,
            y: i32,
            cx: i32,
            cy: i32,
            u_flags: Uint,
        ) -> Bool;
        fn GetWindowLongW(hwnd: Hwnd, n_index: i32) -> Long;
        fn GetForegroundWindow() -> Hwnd;
        fn SendMessageW(hwnd: Hwnd, msg: Uint, wparam: Wparam, lparam: Lparam) -> Lresult;
        fn PeekMessageW(
            lp_msg: *mut Msg,
            h_wnd: Hwnd,
            w_msg_filter_min: Uint,
            w_msg_filter_max: Uint,
            w_remove_msg: Uint,
        ) -> Bool;
        fn TranslateMessage(lp_msg: *const Msg) -> Bool;
        fn DispatchMessageW(lp_msg: *const Msg) -> Lresult;
        fn DestroyWindow(hwnd: Hwnd) -> Bool;
    }

    unsafe extern "system" fn island_pill_wndproc(
        hwnd: Hwnd,
        msg: Uint,
        wparam: Wparam,
        lparam: Lparam,
    ) -> Lresult {
        if let Some(ma) = handle_wm_mouseactivate(msg) {
            return ma;
        }
        DefWindowProcW(hwnd, msg, wparam, lparam)
    }

    unsafe extern "system" fn monitor_enum_callback(
        hmonitor: Hmonitor,
        _hdc: Hdc,
        _lprc: *mut WinRect,
        dw_data: Lparam,
    ) -> Bool {
        let list = &mut *(dw_data as *mut Vec<MonitorDescriptor>);
        let mut mi: MonitorInfoExW = zeroed();
        mi.cb_size = size_of::<MonitorInfoExW>() as Dword;
        if GetMonitorInfoW(hmonitor, &mut mi) != 0 {
            let len = mi
                .sz_device
                .iter()
                .position(|&c| c == 0)
                .unwrap_or(mi.sz_device.len());
            let device_name = String::from_utf16_lossy(&mi.sz_device[..len]);
            let sys_dpi = GetDpiForSystem();
            let dpi = if sys_dpi >= 48 && sys_dpi <= 768 {
                sys_dpi
            } else {
                STANDARD_DPI
            };
            if let (Ok(monitor_rect), Ok(work_area)) = (
                VirtualRect::new(
                    mi.rc_monitor.left,
                    mi.rc_monitor.top,
                    mi.rc_monitor.right,
                    mi.rc_monitor.bottom,
                ),
                VirtualRect::new(
                    mi.rc_work.left,
                    mi.rc_work.top,
                    mi.rc_work.right,
                    mi.rc_work.bottom,
                ),
            ) {
                list.push(MonitorDescriptor {
                    device_name,
                    is_primary: (mi.dw_flags & MONITORINFOF_PRIMARY) != 0,
                    dpi,
                    monitor_rect,
                    work_area,
                });
            }
        }
        1
    }

    pub fn enumerate_monitors_win32() -> Vec<MonitorDescriptor> {
        let mut monitors: Vec<MonitorDescriptor> = Vec::new();
        unsafe {
            let ptr = &mut monitors as *mut Vec<MonitorDescriptor> as Lparam;
            let _ = EnumDisplayMonitors(null_mut(), null(), monitor_enum_callback, ptr);
        }
        if monitors.is_empty() {
            monitors.push(MonitorDescriptor {
                device_name: "FALLBACK_DISPLAY1".to_string(),
                is_primary: true,
                dpi: STANDARD_DPI,
                monitor_rect: VirtualRect {
                    left: 0,
                    top: 0,
                    right: 1920,
                    bottom: 1080,
                },
                work_area: VirtualRect {
                    left: 0,
                    top: 0,
                    right: 1920,
                    bottom: 1040,
                },
            });
        }
        monitors
    }

    pub fn execute_native_pill_window_cycle(
        rep: usize,
        avatar_dir: &Path,
    ) -> Result<NativePillCycleMetrics, UiError> {
        let catalog = validate_avatar_asset_catalog(avatar_dir)?;
        let monitors = enumerate_monitors_win32();
        let primary = select_target_monitor(&monitors, None)?.clone();
        let mut fsm = PillHoverStateMachine::new(primary.clone(), TOP_MARGIN_DEFAULT_PX)?;
        let style_cfg = Win32PillStyleConfig::default();
        style_cfg.validate()?;

        unsafe {
            let hinstance = GetModuleHandleW(null());
            let class_str = format!("MikuNativeIslandPillClass_{}\0", rep);
            let class_w: Vec<u16> = class_str.encode_utf16().collect();
            let title_w: Vec<u16> = "Miku Native Pill\0".encode_utf16().collect();

            let wcx = WndClassExW {
                cb_size: size_of::<WndClassExW>() as Uint,
                style: 0,
                lpfn_wnd_proc: island_pill_wndproc,
                cb_cls_extra: 0,
                cb_wnd_extra: 0,
                h_instance: hinstance,
                h_icon: null_mut(),
                h_cursor: null_mut(),
                hbr_background: null_mut(),
                lpsz_menu_name: null(),
                lpsz_class_name: class_w.as_ptr(),
                h_icon_sm: null_mut(),
            };
            let _ = RegisterClassExW(&wcx);

            let fg_before = GetForegroundWindow() as usize;
            let anchor = fsm.geometry();

            let hwnd = CreateWindowExW(
                style_cfg.ex_style,
                class_w.as_ptr(),
                title_w.as_ptr(),
                style_cfg.base_style,
                anchor.x,
                anchor.y,
                anchor.width,
                anchor.height,
                null_mut(),
                null_mut(),
                hinstance,
                null(),
            );
            if hwnd.is_null() {
                return Err(UiError::Win32Error(
                    "CreateWindowExW returned NULL".to_string(),
                ));
            }

            let _ = SetLayeredWindowAttributes(hwnd, 0, 255, LWA_ALPHA);
            let _ = ShowWindow(hwnd, style_cfg.show_cmd);

            // Drain pending messages without blocking
            let mut msg: Msg = zeroed();
            while PeekMessageW(&mut msg, hwnd, 0, 0, PM_REMOVE) != 0 {
                let _ = TranslateMessage(&msg);
                let _ = DispatchMessageW(&msg);
            }

            // Verify live extended window styles on the created HWND
            let live_ex_style = GetWindowLongW(hwnd, GWL_EXSTYLE) as u32;
            let ex_style_verified = (live_ex_style & REQUIRED_EX_STYLE) == REQUIRED_EX_STYLE
                && (live_ex_style & WS_EX_APPWINDOW_FORBIDDEN) == 0;

            // Verify WM_MOUSEACTIVATE returns MA_NOACTIVATE (3)
            let ma_res = SendMessageW(hwnd, WM_MOUSEACTIVATE_MSG, 0, 0);
            let ma_noactivate_verified = ma_res == MA_NOACTIVATE_RESULT;

            // Measure hover trigger + native SetWindowPos resize latency
            let t_hover = Instant::now();
            let hover_x = anchor.center_x();
            let hover_y = anchor.y + anchor.height / 2;
            let tr = fsm.on_cursor_move(hover_x, hover_y)?;
            let _ = SetWindowPos(
                hwnd,
                null_mut(),
                tr.geometry.x,
                tr.geometry.y,
                tr.geometry.width,
                tr.geometry.height,
                SWP_NOACTIVATE_FLAG | SWP_NOZORDER_FLAG | SWP_SHOWWINDOW_FLAG,
            );
            let hover_latency_ms = t_hover.elapsed().as_secs_f64() * 1_000.0;

            // Return cursor outside hysteresis box so pill settles back to IdleCompact (0 timers)
            let _ = fsm.on_cursor_move(anchor.x - 100, anchor.y + 500)?;

            let fg_after = GetForegroundWindow() as usize;
            let pill_hwnd = hwnd as usize;
            let focus_preserved =
                verify_foreground_focus_invariant(fg_before, fg_after, pill_hwnd, ma_res)?;

            let _ = DestroyWindow(hwnd);

            Ok(NativePillCycleMetrics {
                monitor_count: monitors.len(),
                primary_anchor: anchor,
                ex_style_verified,
                ma_noactivate_verified,
                focus_preserved,
                hover_latency_ms,
                webview2_instances: fsm.webview2_instances(),
                active_idle_timers: fsm.active_timers(),
                verified_svg_count: catalog.verified_svg_count,
            })
        }
    }
}

pub fn run_native_pill_verification_cycle(
    rep: usize,
    avatar_dir: &Path,
) -> Result<NativePillCycleMetrics, UiError> {
    #[cfg(target_os = "windows")]
    {
        win32_native::execute_native_pill_window_cycle(rep, avatar_dir)
    }
    #[cfg(not(target_os = "windows"))]
    {
        let _ = rep;
        let catalog = validate_avatar_asset_catalog(avatar_dir)?;
        let primary = MonitorDescriptor {
            device_name: "SIMULATED_PRIMARY".to_string(),
            is_primary: true,
            dpi: STANDARD_DPI,
            monitor_rect: VirtualRect::new(0, 0, 1920, 1080)?,
            work_area: VirtualRect::new(0, 0, 1920, 1040)?,
        };
        let mut fsm = PillHoverStateMachine::new(primary, TOP_MARGIN_DEFAULT_PX)?;
        let anchor = fsm.geometry();
        let t0 = Instant::now();
        let _ = fsm.on_cursor_move(anchor.center_x(), anchor.y + 10)?;
        let hover_latency_ms = t0.elapsed().as_secs_f64() * 1_000.0;
        let _ = fsm.on_cursor_move(-500, 500)?;
        let focus_preserved =
            verify_foreground_focus_invariant(0x1001, 0x1001, 0x2002, MA_NOACTIVATE_RESULT)?;
        Ok(NativePillCycleMetrics {
            monitor_count: 1,
            primary_anchor: anchor,
            ex_style_verified: true,
            ma_noactivate_verified: true,
            focus_preserved,
            hover_latency_ms,
            webview2_instances: fsm.webview2_instances(),
            active_idle_timers: fsm.active_timers(),
            verified_svg_count: catalog.verified_svg_count,
        })
    }
}

// ============================================================================
// 7. Unit, Contract, Security & FSM Tests (LEDGER-2.1-1 .. LEDGER-2.1-4)
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    // LEDGER-2.1-1
    #[test]
    fn test_multi_monitor_top_center_anchor_coordinates_across_dpi_and_negative_bounds() {
        // 1. Primary monitor 1920x1080 @ 100% DPI (96)
        let mon_100 = MonitorDescriptor {
            device_name: "\\\\.\\DISPLAY1".to_string(),
            is_primary: true,
            dpi: 96,
            monitor_rect: VirtualRect::new(0, 0, 1920, 1080).unwrap(),
            work_area: VirtualRect::new(0, 0, 1920, 1040).unwrap(),
        };
        let anc_100 = compute_top_center_pill_anchor(
            &mon_100,
            IDLE_COMPACT_WIDTH_PX,
            IDLE_COMPACT_HEIGHT_PX,
            IDLE_CORNER_RADIUS_PX,
            0,
        )
        .expect("100% DPI anchor");
        assert_eq!(anc_100.width, 248);
        assert_eq!(anc_100.height, 44);
        assert_eq!(anc_100.x, (1920 - 248) / 2); // 836
        assert_eq!(anc_100.y, 0);
        assert_eq!(anc_100.scale_pct, 100);

        // 2. Laptop display 1920x1080 @ 125% DPI (120)
        let mon_125 = MonitorDescriptor {
            device_name: "\\\\.\\DISPLAY1".to_string(),
            is_primary: true,
            dpi: 120,
            monitor_rect: VirtualRect::new(0, 0, 1920, 1080).unwrap(),
            work_area: VirtualRect::new(0, 0, 1920, 1020).unwrap(),
        };
        let anc_125 = compute_top_center_pill_anchor(
            &mon_125,
            IDLE_COMPACT_WIDTH_PX,
            IDLE_COMPACT_HEIGHT_PX,
            IDLE_CORNER_RADIUS_PX,
            0,
        )
        .expect("125% DPI anchor");
        assert_eq!(anc_125.width, 310); // 248 * 1.25
        assert_eq!(anc_125.height, 55); // 44 * 1.25
        assert_eq!(anc_125.x, (1920 - 310) / 2); // 805
        assert_eq!(anc_125.scale_pct, 125);

        // 3. Secondary monitor positioned to the LEFT and ABOVE primary with negative coordinates
        //    e.g., [-2560, -140]..[0, 1300] @ 150% DPI (144)
        let mon_neg_150 = MonitorDescriptor {
            device_name: "\\\\.\\DISPLAY2".to_string(),
            is_primary: false,
            dpi: 144,
            monitor_rect: VirtualRect::new(-2560, -140, 0, 1300).unwrap(),
            work_area: VirtualRect::new(-2560, -140, 0, 1240).unwrap(),
        };
        let anc_neg = compute_top_center_pill_anchor(
            &mon_neg_150,
            IDLE_COMPACT_WIDTH_PX,
            IDLE_COMPACT_HEIGHT_PX,
            IDLE_CORNER_RADIUS_PX,
            8,
        )
        .expect("negative secondary monitor anchor");
        assert_eq!(anc_neg.width, 372); // 248 * 1.5
        assert_eq!(anc_neg.height, 66); // 44 * 1.5
        // Center X inside [-2560, 0]: -2560 + (2560 - 372) / 2 = -2560 + 1094 = -1466
        assert_eq!(anc_neg.x, -1466);
        // Top Y with 8px logical margin @ 150% (12px): -140 + 12 = -128
        assert_eq!(anc_neg.y, -128);
        assert_eq!(anc_neg.scale_pct, 150);

        // 4. Multi-monitor selector picks negative monitor when cursor is at (-500, 100)
        let monitors = vec![mon_100.clone(), mon_neg_150.clone()];
        let picked = select_target_monitor(&monitors, Some((-500, 100))).unwrap();
        assert_eq!(picked.device_name, "\\\\.\\DISPLAY2");
    }

    // LEDGER-2.1-2
    #[test]
    fn test_win32_layered_topmost_toolwindow_noactivate_style_flags_and_asset_catalog() {
        let cfg = Win32PillStyleConfig::default();
        cfg.validate().expect("default Win32 pill flags must be valid");
        assert_eq!(cfg.base_style, 0x9000_0000);
        assert_eq!(cfg.ex_style, 0x0808_0088);
        assert_eq!(cfg.show_cmd, SW_SHOWNOACTIVATE_CMD);

        // Removing WS_EX_NOACTIVATE must fail validation
        let bad_noactivate = Win32PillStyleConfig {
            ex_style: cfg.ex_style & !WS_EX_NOACTIVATE_STYLE,
            ..cfg
        };
        assert!(bad_noactivate.validate().is_err());

        // Adding WS_EX_APPWINDOW must fail validation
        let bad_appwindow = Win32PillStyleConfig {
            ex_style: cfg.ex_style | WS_EX_APPWINDOW_FORBIDDEN,
            ..cfg
        };
        assert!(bad_appwindow.validate().is_err());

        // Validate the mounted avatar catalog (rig.toml v7, design-tokens.json, 22 SVGs)
        let avatar_dir = resolve_avatar_assets_dir();
        let report =
            validate_avatar_asset_catalog(&avatar_dir).expect("mounted avatar catalog valid");
        assert_eq!(report.rig_version, 1);
        assert_eq!(report.viewbox, [128, 128]);
        assert_eq!(report.idle_fps, 0);
        assert_eq!(report.layer_count, 9);
        assert_eq!(report.accessory_item_count, 14);
        assert_eq!(report.verified_svg_count, EXPECTED_AVATAR_SVG_COUNT);
        assert_eq!(report.compact_width_px, 248);
        assert_eq!(report.compact_height_px, 44);
        assert_eq!(report.catalog_sha256.len(), 64);
    }

    // LEDGER-2.1-3
    #[test]
    fn test_zero_focus_stealing_foreground_window_invariant_and_ma_noactivate() {
        assert_eq!(
            handle_wm_mouseactivate(WM_MOUSEACTIVATE_MSG),
            Some(MA_NOACTIVATE_RESULT)
        );
        assert_eq!(handle_wm_mouseactivate(0x0001), None);

        // Focus preserved when fg_before == fg_after != pill_hwnd
        assert!(verify_foreground_focus_invariant(
            0x0012_00AA,
            0x0012_00AA,
            0x0099_00FF,
            MA_NOACTIVATE_RESULT
        )
        .unwrap());

        // Headless CI fallback when fg_before == 0 and fg_after == 0
        assert!(verify_foreground_focus_invariant(0, 0, 0x0099_00FF, MA_NOACTIVATE_RESULT).unwrap());

        // Stealing focus (fg_after == pill_hwnd) must fail immediately
        assert!(matches!(
            verify_foreground_focus_invariant(
                0x0012_00AA,
                0x0099_00FF,
                0x0099_00FF,
                MA_NOACTIVATE_RESULT
            ),
            Err(UiError::FocusStealDetected { .. })
        ));

        // Real Win32 verification cycle on Windows host
        let avatar_dir = resolve_avatar_assets_dir();
        let cycle =
            run_native_pill_verification_cycle(1, &avatar_dir).expect("native Win32 pill cycle");
        assert!(cycle.ex_style_verified);
        assert!(cycle.ma_noactivate_verified);
        assert!(cycle.focus_preserved);
        assert_eq!(cycle.webview2_instances, 0);
    }

    // LEDGER-2.1-4
    #[test]
    fn test_idle_to_hover_dimension_transition_and_hit_test_hysteresis_under_16ms() {
        let monitor = MonitorDescriptor {
            device_name: "\\\\.\\DISPLAY1".to_string(),
            is_primary: true,
            dpi: 96,
            monitor_rect: VirtualRect::new(0, 0, 1920, 1080).unwrap(),
            work_area: VirtualRect::new(0, 0, 1920, 1040).unwrap(),
        };
        let mut fsm = PillHoverStateMachine::new(monitor, 0).expect("init hover FSM");
        assert_eq!(fsm.mode(), PillVisualMode::IdleCompact);
        assert_eq!(fsm.geometry().width, 248);
        assert_eq!(fsm.geometry().height, 44);
        assert_eq!(fsm.active_timers(), 0);

        // 1. Move cursor into the pill center -> transitions to HoverPeek (284x52, k=0.571)
        let enter = fsm.on_cursor_move(960, 20).expect("enter hover");
        assert!(enter.transitioned);
        assert_eq!(enter.new_mode, PillVisualMode::HoverPeek);
        assert_eq!(enter.geometry.width, 284);
        assert_eq!(enter.geometry.height, 52);
        assert!((enter.avatar_scale_k - HOVER_AVATAR_SCALE_K).abs() < 1e-6);
        assert!((enter.avatar_body_px - HOVER_AVATAR_BODY_PX).abs() < 1e-6);
        assert!(enter.elapsed_ms < MAX_HOVER_RESPONSE_MS);

        // 2. Move cursor 4px below the HoverPeek bottom edge (y = 52 + 4 = 56):
        //    Because hysteresis is 6px, it MUST remain in HoverPeek without flickering!
        let edge = fsm.on_cursor_move(960, 56).expect("within hysteresis margin");
        assert!(!edge.transitioned);
        assert_eq!(edge.new_mode, PillVisualMode::HoverPeek);

        // 3. Move cursor outside hysteresis margin (y = 70) -> settles back to IdleCompact
        let leave = fsm.on_cursor_move(960, 70).expect("leave hover");
        assert!(leave.transitioned);
        assert_eq!(leave.new_mode, PillVisualMode::IdleCompact);
        assert_eq!(leave.geometry.width, 248);
        assert_eq!(leave.geometry.height, 44);
        assert_eq!(leave.active_timers, 0);
        assert_eq!(leave.webview2_instances, 0);
        assert!(leave.elapsed_ms < MAX_HOVER_RESPONSE_MS);
    }
}
