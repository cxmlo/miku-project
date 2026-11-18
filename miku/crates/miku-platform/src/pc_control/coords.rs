//! Multi-DPI screen and virtual desktop coordinate conversions using `CaptureTransform`.

pub use miku_protocol::CaptureTransform;

/// Converts normalized coordinates [0.0, 1.0] to virtual desktop coordinates `(vx, vy)`.
/// Virtual coordinates can be negative on multi-monitor configurations.
pub fn screen_coords_to_virtual(
    x_norm: f32,
    y_norm: f32,
    transform: &CaptureTransform,
) -> (i32, i32) {
    transform.screen_coords_to_virtual(x_norm, y_norm)
}

/// Alias for `screen_coords_to_virtual`.
pub fn normalized_coords_to_virtual(
    x_norm: f32,
    y_norm: f32,
    transform: &CaptureTransform,
) -> (i32, i32) {
    transform.screen_coords_to_virtual(x_norm, y_norm)
}

/// Converts pixel offsets `(px, py)` in the captured frame to virtual desktop coordinates `(vx, vy)`
/// applying DPI scaling and virtual origin translation.
pub fn pixel_to_virtual(
    px: i32,
    py: i32,
    transform: &CaptureTransform,
) -> (i32, i32) {
    transform.pixel_to_virtual(px, py)
}

/// Converts virtual desktop coordinates `(vx, vy)` back into captured frame pixel offsets `(px, py)`.
pub fn virtual_to_screen_pixel(
    vx: i32,
    vy: i32,
    transform: &CaptureTransform,
) -> (i32, i32) {
    let scale = if transform.dpi_scale <= 0.0 {
        1.0
    } else {
        transform.dpi_scale
    };
    let px = ((vx - transform.origin_virtual_x) as f32 / scale).round() as i32;
    let py = ((vy - transform.origin_virtual_y) as f32 / scale).round() as i32;
    (px, py)
}

/// Converts virtual desktop coordinates `(vx, vy)` into normalized [0.0, 1.0] image coordinates.
pub fn virtual_to_normalized_coords(
    vx: i32,
    vy: i32,
    transform: &CaptureTransform,
) -> (f32, f32) {
    let width = transform.width_px.max(1) as f32;
    let height = transform.height_px.max(1) as f32;
    let norm_x = (vx - transform.origin_virtual_x) as f32 / width;
    let norm_y = (vy - transform.origin_virtual_y) as f32 / height;
    (norm_x.clamp(0.0, 1.0), norm_y.clamp(0.0, 1.0))
}

/// Scales raw mouse click coordinates to virtual coordinates based on a `CaptureTransform`.
pub fn scale_mouse_click_coords(
    x: i32,
    y: i32,
    transform: &CaptureTransform,
) -> (i32, i32) {
    pixel_to_virtual(x, y, transform)
}

/// Converts LLM normalized coordinates [0.0, 1000.0] into desktop virtual coordinates `(vx, vy)`.
pub fn normalized_1000_to_virtual(
    x_1000: f32,
    y_1000: f32,
    transform: &CaptureTransform,
) -> (i32, i32) {
    transform.normalized_1000_to_virtual(x_1000, y_1000)
}

/// Converts virtual desktop pixel coordinates `(vx, vy)` into `SendInput` normalized virtual
/// coordinates `(0..=65535, 0..=65535)` relative to explicit virtual desktop bounds.
pub fn desktop_virtual_to_sendinput_abs(
    vx: i32,
    vy: i32,
    desk_x: i32,
    desk_y: i32,
    desk_w: i32,
    desk_h: i32,
) -> (i32, i32) {
    let w = desk_w.max(1) as i64;
    let h = desk_h.max(1) as i64;
    let rx = ((vx as i64 - desk_x as i64) * 65535 + (w / 2)) / w;
    let ry = ((vy as i64 - desk_y as i64) * 65535 + (h / 2)) / h;
    (rx.clamp(0, 65535) as i32, ry.clamp(0, 65535) as i32)
}

/// Converts virtual desktop coordinates `(vx, vy)` into Win32 `SendInput` absolute coordinates `(0..=65535)`.
pub fn virtual_to_sendinput_abs(vx: i32, vy: i32) -> (i32, i32) {
    #[cfg(windows)]
    {
        extern "system" {
            fn GetSystemMetrics(n_index: i32) -> i32;
        }
        unsafe {
            let dx = GetSystemMetrics(76); // SM_XVIRTUALSCREEN
            let dy = GetSystemMetrics(77); // SM_YVIRTUALSCREEN
            let dw = GetSystemMetrics(78); // SM_CXVIRTUALSCREEN
            let dh = GetSystemMetrics(79); // SM_CYVIRTUALSCREEN
            if dw > 0 && dh > 0 {
                return desktop_virtual_to_sendinput_abs(vx, vy, dx, dy, dw, dh);
            }
        }
    }
    desktop_virtual_to_sendinput_abs(vx, vy, 0, 0, 1920, 1080)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_multi_dpi_coordinate_conversion() {
        // Primary monitor 1920x1080 at 100% scale
        let t1 = CaptureTransform::new(0, 0, 1920, 1080, 1.0);
        assert_eq!(screen_coords_to_virtual(0.5, 0.5, &t1), (960, 540));
        assert_eq!(pixel_to_virtual(100, 200, &t1), (100, 200));
        assert_eq!(virtual_to_screen_pixel(100, 200, &t1), (100, 200));

        // High-DPI monitor at 150% (1.5x) scale with negative offset (secondary screen left)
        let t2 = CaptureTransform::new(-1920, 0, 1920, 1080, 1.5);
        let (vx, vy) = pixel_to_virtual(100, 100, &t2);
        assert_eq!(vx, -1920 + 150); // -1770
        assert_eq!(vy, 150);
        let (px, py) = virtual_to_screen_pixel(vx, vy, &t2);
        assert_eq!((px, py), (100, 100));

        // Multi-monitor right at 125% scale
        let t3 = CaptureTransform::new(1920, 100, 2560, 1440, 1.25);
        let (vx3, vy3) = pixel_to_virtual(200, 400, &t3);
        assert_eq!(vx3, 1920 + 250);
        assert_eq!(vy3, 100 + 500);

        // 0..1000 LLM coords -> virtual -> SendInput 0..65535 across negative virtual desktop
        let (vx_mid, vy_mid) = normalized_1000_to_virtual(500.0, 500.0, &t2);
        assert_eq!((vx_mid, vy_mid), (-960, 540));
        let (ax, ay) = desktop_virtual_to_sendinput_abs(-960, 540, -1920, 0, 3840, 1080);
        assert!((ax - 16384).abs() <= 1);
        assert!((ay - 32768).abs() <= 1);
    }
}
