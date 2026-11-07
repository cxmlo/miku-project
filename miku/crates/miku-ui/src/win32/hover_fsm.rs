#![allow(unused_imports)]
use crate::prelude::*;

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
    pub(crate) monitor: MonitorDescriptor,
    pub(crate) top_margin_px: u32,
    pub(crate) hysteresis_px: i32,
    pub(crate) mode: PillVisualMode,
    pub(crate) current_geometry: PillAnchorGeometry,
    pub(crate) transitions_count: u64,
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

