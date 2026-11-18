//! Global cursor telemetry (`GetCursorPos` + DPI normalization) and smart
//! click-through (`WS_EX_TRANSPARENT`) hit-testing for the floating Dynamic Island.
//! Suspends all Win32 polling and WebView2 IPC while the window is hidden or asleep (`0% CPU`).

use std::time::{Duration, Instant};

use super::live_host::LiveWebView2Window;

#[cfg(target_os = "windows")]
use super::win32_com_ffi::win_ffi;

#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct IslandHitRect {
    pub left: f64,
    pub top: f64,
    pub right: f64,
    pub bottom: f64,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct WindowRectPx {
    pub left: i32,
    pub top: i32,
    pub right: i32,
    pub bottom: i32,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct GlobalCursorSample {
    pub cursor_x: i32,
    pub cursor_y: i32,
    pub dpi: u32,
    pub inside_window: bool,
    pub inside_pill: bool,
    pub logical_x: f64,
    pub logical_y: f64,
    pub dx: f64,
    pub dy: f64,
}

/// Computes DPI-normalized cursor coordinates relative to the Island window and
/// determines whether the pointer sits inside the solid `#isl` pill hit-rect.
pub fn compute_global_cursor_sample(
    cursor_x: i32,
    cursor_y: i32,
    win_rect: WindowRectPx,
    dpi: u32,
    hit_rect: Option<IslandHitRect>,
    compact_mode: bool,
) -> GlobalCursorSample {
    let safe_dpi = if (48..=768).contains(&dpi) { dpi } else { 96 };
    let scale = (safe_dpi as f64) / 96.0;
    let inside_window = cursor_x >= win_rect.left
        && cursor_x < win_rect.right
        && cursor_y >= win_rect.top
        && cursor_y < win_rect.bottom;

    let logical_x = (cursor_x - win_rect.left) as f64 / scale;
    let logical_y = (cursor_y - win_rect.top) as f64 / scale;
    let win_w_css = ((win_rect.right - win_rect.left).max(1) as f64) / scale;
    let win_h_css = ((win_rect.bottom - win_rect.top).max(1) as f64) / scale;
    let cx_css = win_w_css * 0.5;
    let cy_css = win_h_css * 0.5;
    let dx = ((logical_x - cx_css) / 360.0).clamp(-1.0, 1.0);
    let dy = ((logical_y - cy_css) / 260.0).clamp(-1.0, 1.0);

    let inside_pill = if let Some(hr) = hit_rect.filter(|r| r.right > r.left && r.bottom > r.top) {
        logical_x >= hr.left && logical_x <= hr.right && logical_y >= hr.top && logical_y <= hr.bottom
    } else if compact_mode {
        let pad_x = if win_w_css > 48.0 { 12.0 } else { 0.0 };
        let pad_b = if win_h_css > 24.0 { 10.0 } else { 0.0 };
        logical_x >= pad_x
            && logical_x <= (win_w_css - pad_x)
            && logical_y >= 0.0
            && logical_y <= (win_h_css - pad_b)
    } else {
        inside_window
    };

    GlobalCursorSample {
        cursor_x,
        cursor_y,
        dpi: safe_dpi,
        inside_window,
        inside_pill,
        logical_x,
        logical_y,
        dx,
        dy,
    }
}

#[derive(Debug)]
pub struct GlobalCursorTracker {
    min_interval: Duration,
    last_emit: Option<Instant>,
    last_cursor: Option<(i32, i32)>,
    last_win_rect: Option<WindowRectPx>,
    last_inside_pill: Option<bool>,
    last_mouse_down: bool,
    last_outside_click: Option<Instant>,
    last_idle_emit: Option<Instant>,
    mascot_awake: bool,
    compact_mode: bool,
    hit_rect: Option<IslandHitRect>,
    latest_sample: Option<GlobalCursorSample>,
}

impl Default for GlobalCursorTracker {
    fn default() -> Self {
        Self::new()
    }
}

impl GlobalCursorTracker {
    pub fn new() -> Self {
        Self {
            min_interval: Duration::from_millis(32),
            last_emit: None,
            last_cursor: None,
            last_win_rect: None,
            last_inside_pill: None,
            last_mouse_down: false,
            last_outside_click: None,
            last_idle_emit: None,
            mascot_awake: true,
            compact_mode: true,
            hit_rect: None,
            latest_sample: None,
        }
    }

    pub fn update_region(&mut self, compact: Option<bool>, rect: Option<IslandHitRect>) {
        if let Some(c) = compact {
            self.compact_mode = c;
        }
        if rect.is_some() {
            self.hit_rect = rect;
        }
    }

    pub fn set_mascot_awake(&mut self, awake: bool) {
        self.mascot_awake = awake;
    }

    pub fn is_mascot_awake(&self) -> bool {
        self.mascot_awake
    }

    pub fn latest_sample(&self) -> Option<&GlobalCursorSample> {
        self.latest_sample.as_ref()
    }

    pub fn format_cursor_message(sample: &GlobalCursorSample) -> String {
        format!(
            "{{\"type\":\"cursor\",\"dx\":{:.4},\"dy\":{:.4},\"lx\":{:.2},\"ly\":{:.2},\"inside\":{},\"inside_window\":{}}}",
            sample.dx,
            sample.dy,
            sample.logical_x,
            sample.logical_y,
            sample.inside_pill,
            sample.inside_window
        )
    }

    /// Evaluates one probe sample against throttle/dedup/sleep gates, returning
    /// `(desired_pass_through, optional_json_ipc_message)`.
    pub fn step_with_probe(
        &mut self,
        win_visible: bool,
        cursor_x: i32,
        cursor_y: i32,
        win_rect: WindowRectPx,
        dpi: u32,
        now: Instant,
    ) -> (bool, Option<String>) {
        if !win_visible {
            return (false, None);
        }
        if !self.mascot_awake {
            let sample = compute_global_cursor_sample(
                cursor_x,
                cursor_y,
                win_rect,
                dpi,
                self.hit_rect,
                self.compact_mode,
            );
            let pass_through = self.compact_mode && !sample.inside_pill;
            let woke_into_pill =
                self.last_inside_pill == Some(false) && sample.inside_pill;
            self.last_inside_pill = Some(sample.inside_pill);
            if woke_into_pill {
                self.last_cursor = Some((cursor_x, cursor_y));
                self.last_win_rect = Some(win_rect);
                self.latest_sample = Some(sample);
                self.last_emit = Some(now);
                return (false, Some(Self::format_cursor_message(&sample)));
            }
            return (pass_through, None);
        }
        if self.last_cursor == Some((cursor_x, cursor_y)) && self.last_win_rect == Some(win_rect) {
            return (self.compact_mode && !self.last_inside_pill.unwrap_or(false), None);
        }

        let sample = compute_global_cursor_sample(
            cursor_x,
            cursor_y,
            win_rect,
            dpi,
            self.hit_rect,
            self.compact_mode,
        );
        self.last_cursor = Some((cursor_x, cursor_y));
        self.last_win_rect = Some(win_rect);
        self.latest_sample = Some(sample);

        let inside_changed = self.last_inside_pill != Some(sample.inside_pill);
        self.last_inside_pill = Some(sample.inside_pill);
        let pass_through = self.compact_mode && !sample.inside_pill;

        let due = inside_changed
            || self
                .last_emit
                .map(|t| now.duration_since(t) >= self.min_interval)
                .unwrap_or(true);
        if due {
            self.last_emit = Some(now);
            (pass_through, Some(Self::format_cursor_message(&sample)))
        } else {
            (pass_through, None)
        }
    }

    /// Detects rising mouse clicks outside the expanded pill (`outside_click`) and prolonged OS
    /// inactivity (`user_idle`) so the Island closes automatically even when `WS_EX_NOACTIVATE`
    /// prevents HTML `blur` events.
    pub fn step_input_events(
        &mut self,
        win_visible: bool,
        cursor_x: i32,
        cursor_y: i32,
        win_rect: WindowRectPx,
        dpi: u32,
        mouse_down: bool,
        user_idle_ms: u64,
        now: Instant,
    ) -> Option<String> {
        let rising_click = mouse_down && !self.last_mouse_down;
        self.last_mouse_down = mouse_down;
        if !win_visible || self.compact_mode {
            self.last_idle_emit = None;
            return None;
        }
        let sample = compute_global_cursor_sample(
            cursor_x,
            cursor_y,
            win_rect,
            dpi,
            self.hit_rect,
            self.compact_mode,
        );
        if rising_click && !sample.inside_pill {
            let allow = self
                .last_outside_click
                .map(|t| now.duration_since(t) >= Duration::from_millis(120))
                .unwrap_or(true);
            if allow {
                self.last_outside_click = Some(now);
                return Some(format!(
                    "{{\"type\":\"outside_click\",\"x\":{},\"y\":{}}}",
                    cursor_x, cursor_y
                ));
            }
        }
        if user_idle_ms >= 15_000 {
            let allow = self
                .last_idle_emit
                .map(|t| now.duration_since(t) >= Duration::from_secs(4))
                .unwrap_or(true);
            if allow {
                self.last_idle_emit = Some(now);
                return Some(format!(
                    "{{\"type\":\"user_idle\",\"idle_ms\":{}}}",
                    user_idle_ms
                ));
            }
        } else if user_idle_ms < 2_000 {
            self.last_idle_emit = None;
        }
        None
    }

    /// Queries Win32 `GetCursorPos`, `GetWindowRect`, `GetDpiForWindow`, and mouse/idle input state
    /// when the Island window is visible; updates pass-through and pushes WebMessage JSON.
    pub fn poll_and_dispatch(&mut self, win: &mut LiveWebView2Window) -> bool {
        if !win.is_visible() {
            return false;
        }
        let Some((cx, cy, rect, dpi, mouse_down, idle_ms)) =
            query_win32_cursor_rect_and_input(win.hwnd_raw())
        else {
            return false;
        };
        let now = Instant::now();
        let mut sent = false;
        if let Some(input_ev) =
            self.step_input_events(true, cx, cy, rect, dpi, mouse_down, idle_ms, now)
        {
            sent |= win.post_web_message_json(&input_ev).is_ok();
        }
        if !self.mascot_awake && !win.is_cursor_ignored() {
            return sent;
        }
        let (pass_through, json_opt) = self.step_with_probe(true, cx, cy, rect, dpi, now);
        win.set_cursor_pass_through(pass_through);
        if let Some(json) = json_opt {
            sent |= win.post_web_message_json(&json).is_ok();
        }
        sent
    }
}

#[cfg(target_os = "windows")]
fn query_win32_cursor_rect_and_input(
    hwnd_raw: usize,
) -> Option<(i32, i32, WindowRectPx, u32, bool, u64)> {
    if hwnd_raw == 0 {
        return None;
    }
    unsafe {
        let hwnd = hwnd_raw as win_ffi::Hwnd;
        let mut pt = win_ffi::Point::default();
        if win_ffi::GetCursorPos(&mut pt) == 0 {
            return None;
        }
        let mut rc = win_ffi::Rect::default();
        if win_ffi::GetWindowRect(hwnd, &mut rc) == 0 {
            return None;
        }
        let dpi = win_ffi::GetDpiForWindow(hwnd);
        let lbtn = win_ffi::GetAsyncKeyState(win_ffi::VK_LBUTTON) < 0;
        let rbtn = win_ffi::GetAsyncKeyState(win_ffi::VK_RBUTTON) < 0;
        let mut lii = win_ffi::LastInputInfo {
            cb_size: std::mem::size_of::<win_ffi::LastInputInfo>() as u32,
            dw_time: 0,
        };
        let idle_ms = if win_ffi::GetLastInputInfo(&mut lii) != 0 {
            win_ffi::GetTickCount().saturating_sub(lii.dw_time) as u64
        } else {
            0
        };
        Some((
            pt.x,
            pt.y,
            WindowRectPx {
                left: rc.left,
                top: rc.top,
                right: rc.right,
                bottom: rc.bottom,
            },
            if dpi > 0 { dpi } else { 96 },
            lbtn || rbtn,
            idle_ms,
        ))
    }
}

#[cfg(not(target_os = "windows"))]
fn query_win32_cursor_rect_and_input(
    _hwnd_raw: usize,
) -> Option<(i32, i32, WindowRectPx, u32, bool, u64)> {
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_8_4_global_cursor_dpi_150_and_outside_negative_bounds() {
        // 150% DPI (144) window occupying physical px [1200..1500, 0..90] -> 200x60 CSS px.
        let rect = WindowRectPx {
            left: 1200,
            top: 0,
            right: 1500,
            bottom: 90,
        };
        let hit = Some(IslandHitRect {
            left: 17.0,
            top: 0.0,
            right: 183.0,
            bottom: 44.0,
        });

        // Cursor outside window to the left on a secondary negative-X monitor (-240, 390 @ 144 DPI).
        let out_sample = compute_global_cursor_sample(-240, 390, rect, 144, hit, true);
        assert!(!out_sample.inside_window);
        assert!(!out_sample.inside_pill);
        assert_eq!(out_sample.dpi, 144);
        assert!((out_sample.dx - (-1.0)).abs() < 1e-6);
        assert!((out_sample.dy - (230.0 / 260.0)).abs() < 1e-3);

        // Cursor cleanly inside transparent shadow margin (logical x = 6, y = 20 -> physical 1209, 30).
        let margin_sample = compute_global_cursor_sample(1209, 30, rect, 144, hit, true);
        assert!(margin_sample.inside_window);
        assert!(!margin_sample.inside_pill);

        // Cursor cleanly inside pill (logical x = 100, y = 22 -> physical 1350, 33).
        let pill_sample = compute_global_cursor_sample(1350, 33, rect, 144, hit, true);
        assert!(pill_sample.inside_window);
        assert!(pill_sample.inside_pill);
        assert!((pill_sample.dx - 0.0).abs() < 1e-4);
    }

    #[test]
    fn test_8_4_cursor_tracker_suspends_when_hidden_or_asleep_and_dedups() {
        let mut tracker = GlobalCursorTracker::new();
        tracker.update_region(
            Some(true),
            Some(IslandHitRect {
                left: 17.0,
                top: 0.0,
                right: 285.0,
                bottom: 44.0,
            }),
        );
        let rect = WindowRectPx {
            left: 800,
            top: 0,
            right: 1102,
            bottom: 62,
        };
        let t0 = Instant::now();

        // Hidden window -> 0 telemetry.
        assert_eq!(tracker.step_with_probe(false, 400, 300, rect, 96, t0), (false, None));

        // Sleeping mascot outside pill -> pass_through=true, 0 IPC telemetry.
        tracker.set_mascot_awake(false);
        assert_eq!(tracker.step_with_probe(true, 400, 300, rect, 96, t0), (true, None));
        assert_eq!(tracker.step_with_probe(true, 410, 305, rect, 96, t0 + Duration::from_millis(50)), (true, None));

        // Awake + visible outside pill -> pass_through=true + emits IPC JSON.
        tracker.set_mascot_awake(true);
        let (pt1, msg1) = tracker.step_with_probe(true, 400, 300, rect, 96, t0);
        assert!(pt1);
        let json1 = msg1.expect("first movement must emit cursor JSON");
        assert!(json1.contains(r#""inside":false"#));

        // Unmoved cursor on next tick -> 0 duplicate IPC frames.
        let (pt_dup, msg_dup) =
            tracker.step_with_probe(true, 400, 300, rect, 96, t0 + Duration::from_millis(50));
        assert!(pt_dup);
        assert!(msg_dup.is_none());

        // Move inside pill even before 32ms elapsed -> inside_pill transition clears pass_through immediately.
        let (pt_in, msg_in) =
            tracker.step_with_probe(true, 920, 20, rect, 96, t0 + Duration::from_millis(12));
        assert!(!pt_in);
        let json_in = msg_in.expect("re-entering pill must notify immediately");
        assert!(json_in.contains(r#""inside":true"#));
    }

    #[test]
    fn test_outside_click_and_user_idle_collapse_signals() {
        let mut tracker = GlobalCursorTracker::new();
        tracker.update_region(
            Some(false),
            Some(IslandHitRect {
                left: 17.0,
                top: 4.0,
                right: 760.0,
                bottom: 240.0,
            }),
        );
        let rect = WindowRectPx {
            left: 500,
            top: 0,
            right: 1294,
            bottom: 262,
        };
        let t0 = Instant::now();

        // Click INSIDE expanded pill does not emit outside_click.
        assert_eq!(
            tracker.step_input_events(true, 700, 120, rect, 96, true, 100, t0),
            None
        );
        // Release mouse before clicking outside.
        assert_eq!(
            tracker.step_input_events(
                true,
                200,
                500,
                rect,
                96,
                false,
                100,
                t0 + Duration::from_millis(50)
            ),
            None
        );
        // Rising click OUTSIDE expanded pill emits outside_click.
        let click_msg = tracker
            .step_input_events(
                true,
                200,
                500,
                rect,
                96,
                true,
                50,
                t0 + Duration::from_millis(200),
            )
            .expect("outside click must emit");
        assert!(click_msg.contains(r#""type":"outside_click""#));

        // Idle >= 15s while expanded emits user_idle.
        let idle_msg = tracker
            .step_input_events(
                true,
                200,
                500,
                rect,
                96,
                false,
                16_000,
                t0 + Duration::from_secs(1),
            )
            .expect("idle >= 15s must emit");
        assert!(idle_msg.contains(r#""type":"user_idle""#));
    }
}
