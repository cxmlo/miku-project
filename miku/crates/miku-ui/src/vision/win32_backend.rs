use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

use crate::vision::types_and_codec::{zeroize_slice, RawRgbaFrame, VisionCaptureTarget};
use crate::win32::UiError;

pub trait ScreenCaptureBackend {
    fn capture_rgba(&self, target: &VisionCaptureTarget, now_ms: u64) -> Result<RawRgbaFrame, UiError>;

    /// Must always remain `false` for Miku's strict on-demand vision contract.
    fn is_continuous_stream_active(&self) -> bool {
        false
    }
}

pub struct DeterministicFixtureVisionBackend {
    pub frame_width: u32,
    pub frame_height: u32,
    pub window_title: String,
    pub virtual_bounds: (i32, i32, u32, u32),
    pub capture_calls: AtomicUsize,
    pub force_continuous_stream_flag: AtomicBool,
    pub injected_error: Option<String>,
}

impl DeterministicFixtureVisionBackend {
    pub fn new(width: u32, height: u32, window_title: impl Into<String>) -> Self {
        Self {
            frame_width: width,
            frame_height: height,
            window_title: window_title.into(),
            virtual_bounds: (0, 0, width.max(1920), height.max(1080)),
            capture_calls: AtomicUsize::new(0),
            force_continuous_stream_flag: AtomicBool::new(false),
            injected_error: None,
        }
    }

    pub fn capture_count(&self) -> usize {
        self.capture_calls.load(Ordering::SeqCst)
    }

    fn build_pattern_rgba(&self, w: u32, h: u32) -> Result<Vec<u8>, UiError> {
        let total = (w as usize)
            .checked_mul(h as usize)
            .and_then(|px| px.checked_mul(4))
            .ok_or_else(|| {
                UiError::VisionCaptureError("fixture RGBA buffer size overflow".to_string())
            })?;
        let mut rgba = vec![255u8; total];
        for y in 0..h {
            let row_off = (y as usize) * (w as usize) * 4;
            for x in 0..w {
                let off = row_off + (x as usize) * 4;
                rgba[off] = ((x.wrapping_mul(3) ^ y) & 0xFF) as u8;
                rgba[off + 1] = ((y.wrapping_mul(5) + x) & 0xFF) as u8;
                rgba[off + 2] = (((x ^ y).wrapping_mul(7)) & 0xFF) as u8;
                rgba[off + 3] = 255;
            }
        }
        Ok(rgba)
    }
}

impl ScreenCaptureBackend for DeterministicFixtureVisionBackend {
    fn capture_rgba(&self, target: &VisionCaptureTarget, now_ms: u64) -> Result<RawRgbaFrame, UiError> {
        target.validate()?;
        if let Some(err) = &self.injected_error {
            return Err(UiError::VisionCaptureError(err.clone()));
        }

        let (w, h, title) = match target {
            VisionCaptureTarget::ActiveWindow => (
                self.frame_width,
                self.frame_height,
                self.window_title.clone(),
            ),
            VisionCaptureTarget::SelectedRegion {
                x,
                y,
                width,
                height,
            } => {
                let (vx, vy, vw, vh) = self.virtual_bounds;
                let rx_end = (*x as i64) + (*width as i64);
                let ry_end = (*y as i64) + (*height as i64);
                let vx_end = (vx as i64) + (vw as i64);
                let vy_end = (vy as i64) + (vh as i64);
                if *x < vx || *y < vy || rx_end > vx_end || ry_end > vy_end {
                    return Err(UiError::VisionCaptureError(format!(
                        "selected region ({},{},{}x{}) outside desktop bounds ({},{},{}x{})",
                        x, y, width, height, vx, vy, vw, vh
                    )));
                }
                (*width, *height, self.window_title.clone())
            }
        };

        self.capture_calls.fetch_add(1, Ordering::SeqCst);
        let rgba = self.build_pattern_rgba(w, h)?;
        let (vx, vy) = match target {
            VisionCaptureTarget::ActiveWindow => (self.virtual_bounds.0, self.virtual_bounds.1),
            VisionCaptureTarget::SelectedRegion { x, y, .. } => (*x, *y),
        };
        let transform = miku_protocol::CaptureTransform {
            origin_virtual_x: vx,
            origin_virtual_y: vy,
            width_px: w,
            height_px: h,
            dpi_scale: 1.0,
        };
        Ok(RawRgbaFrame {
            width: w,
            height: h,
            rgba,
            window_title: title,
            captured_at_ms: now_ms,
            transform,
        })
    }

    fn is_continuous_stream_active(&self) -> bool {
        self.force_continuous_stream_flag.load(Ordering::SeqCst)
    }
}

#[derive(Debug, Default)]
pub struct Win32GdiScreenVisionBackend;

impl Win32GdiScreenVisionBackend {
    pub fn new() -> Self {
        Self
    }

    /// Verifies Win32 GDI handle lifecycle (`CreateCompatibleDC`, `CreateCompatibleBitmap`,
    /// `SelectObject`, `BitBlt`, `GetDIBits`, and RAII cleanup) without needing an interactive
    /// foreground desktop session (useful in headless CI / non-interactive runners).
    pub fn probe_offscreen_gdi_pipeline(&self, width: u32, height: u32) -> Result<RawRgbaFrame, UiError> {
        #[cfg(windows)]
        {
            win32_gdi_ffi::probe_offscreen_gdi_win32(width, height)
        }
        #[cfg(not(windows))]
        {
            let _ = (width, height);
            Err(UiError::VisionCaptureError(
                "Win32GdiScreenVisionBackend requires Windows".to_string(),
            ))
        }
    }
}

#[cfg(windows)]
mod win32_gdi_ffi {
    use super::*;
    use std::ffi::c_void;

    pub type WinHwnd = *mut c_void;
    pub type WinHdc = *mut c_void;
    pub type WinHbitmap = *mut c_void;
    pub type WinHgdiobj = *mut c_void;
    pub type WinBool = i32;
    pub type WinInt = i32;
    pub type WinUint = u32;
    pub type WinDword = u32;
    pub type WinLong = i32;

    const SRCCOPY: WinDword = 0x00CC_0020;
    const CAPTUREBLT: WinDword = 0x4000_0000;
    const DIB_RGB_COLORS: WinUint = 0;
    const BI_RGB: WinDword = 0;
    const SM_XVIRTUALSCREEN: WinInt = 76;
    const SM_YVIRTUALSCREEN: WinInt = 77;
    const SM_CXVIRTUALSCREEN: WinInt = 78;
    const SM_CYVIRTUALSCREEN: WinInt = 79;

    #[repr(C)]
    #[derive(Default)]
    pub struct WinRect {
        pub left: WinLong,
        pub top: WinLong,
        pub right: WinLong,
        pub bottom: WinLong,
    }

    #[repr(C)]
    pub struct WinBitmapInfoHeader {
        pub bi_size: WinDword,
        pub bi_width: WinLong,
        pub bi_height: WinLong,
        pub bi_planes: u16,
        pub bi_bit_count: u16,
        pub bi_compression: WinDword,
        pub bi_size_image: WinDword,
        pub bi_x_pels_per_meter: WinLong,
        pub bi_y_pels_per_meter: WinLong,
        pub bi_clr_used: WinDword,
        pub bi_clr_important: WinDword,
    }

    #[repr(C)]
    pub struct WinBitmapInfo {
        pub bmi_header: WinBitmapInfoHeader,
        pub bmi_colors: [WinDword; 1],
    }

    #[link(name = "kernel32")]
    unsafe extern "system" {
        fn GetLastError() -> WinDword;
        fn GetCurrentProcessId() -> WinDword;
    }

    #[link(name = "user32")]
    unsafe extern "system" {
        fn GetForegroundWindow() -> WinHwnd;
        fn GetWindow(hwnd: WinHwnd, u_cmd: WinUint) -> WinHwnd;
        fn GetWindowThreadProcessId(hwnd: WinHwnd, lpdw_process_id: *mut WinDword) -> WinDword;
        fn IsWindowVisible(hwnd: WinHwnd) -> WinBool;
        fn IsIconic(hwnd: WinHwnd) -> WinBool;
        fn GetWindowRect(hwnd: WinHwnd, rect: *mut WinRect) -> WinBool;
        fn GetWindowTextW(hwnd: WinHwnd, lp_string: *mut u16, max_count: WinInt) -> WinInt;
        fn GetDC(hwnd: WinHwnd) -> WinHdc;
        fn ReleaseDC(hwnd: WinHwnd, hdc: WinHdc) -> WinInt;
        fn GetSystemMetrics(index: WinInt) -> WinInt;
    }

    #[link(name = "gdi32")]
    unsafe extern "system" {
        fn CreateCompatibleDC(hdc: WinHdc) -> WinHdc;
        fn CreateCompatibleBitmap(hdc: WinHdc, cx: WinInt, cy: WinInt) -> WinHbitmap;
        fn SelectObject(hdc: WinHdc, h: WinHgdiobj) -> WinHgdiobj;
        fn BitBlt(hdc: WinHdc, x: WinInt, y: WinInt, cx: WinInt, cy: WinInt, hdc_src: WinHdc, x1: WinInt, y1: WinInt, rop: WinDword) -> WinBool;
        fn GetDIBits(hdc: WinHdc, hbm: WinHbitmap, start: WinUint, lines: WinUint, bits: *mut c_void, lpbmi: *mut WinBitmapInfo, usage: WinUint) -> WinInt;
        fn DeleteObject(ho: WinHgdiobj) -> WinBool;
        fn DeleteDC(hdc: WinHdc) -> WinBool;
    }

    struct ScreenDcGuard(WinHdc);
    impl Drop for ScreenDcGuard {
        fn drop(&mut self) {
            if !self.0.is_null() { unsafe { let _ = ReleaseDC(std::ptr::null_mut(), self.0); } }
        }
    }

    struct MemDcGuard(WinHdc);
    impl Drop for MemDcGuard {
        fn drop(&mut self) {
            if !self.0.is_null() { unsafe { let _ = DeleteDC(self.0); } }
        }
    }

    struct BitmapGuard { mem_dc: WinHdc, prev_obj: WinHgdiobj, bmp: WinHbitmap }
    impl Drop for BitmapGuard {
        fn drop(&mut self) {
            unsafe {
                if !self.mem_dc.is_null() && !self.prev_obj.is_null() { let _ = SelectObject(self.mem_dc, self.prev_obj); }
                if !self.bmp.is_null() { let _ = DeleteObject(self.bmp as WinHgdiobj); }
            }
        }
    }

    pub fn capture_screen_win32(
        target: &VisionCaptureTarget,
        now_ms: u64,
    ) -> Result<RawRgbaFrame, UiError> {
        target.validate()?;
        let _dpi_guard = crate::vision::win32_dpi_guard::DpiAwarenessThreadGuard::enter_per_monitor_v2();

        // SAFETY: reads virtual screen coordinates and foreground window metadata cleanly.
        let (cap_x, cap_y, cap_w, cap_h, title, fg_hwnd_cap, dpi_scale) = unsafe {
            let my_pid = GetCurrentProcessId();
            let read_title = |h: WinHwnd| -> String {
                if h.is_null() {
                    return String::new();
                }
                let mut wide = [0u16; 256];
                let len = GetWindowTextW(h, wide.as_mut_ptr(), wide.len() as WinInt);
                if len > 0 {
                    String::from_utf16_lossy(&wide[..(len as usize)])
                } else {
                    String::new()
                }
            };
            let mut fg_hwnd = GetForegroundWindow();
            let mut fg_pid: WinDword = 0;
            if !fg_hwnd.is_null() {
                let _ = GetWindowThreadProcessId(fg_hwnd, &mut fg_pid);
            }
            // If Miku itself holds focus (e.g., user just clicked/typed in Island),
            // walk Z-order to the underlying non-Miku visible window.
            if !fg_hwnd.is_null() && fg_pid == my_pid {
                let mut cursor = GetWindow(fg_hwnd, 2 /* GW_HWNDNEXT */);
                let mut steps = 0usize;
                let mut found: WinHwnd = std::ptr::null_mut();
                while !cursor.is_null() && steps < 64 {
                    steps += 1;
                    let mut c_pid: WinDword = 0;
                    let _ = GetWindowThreadProcessId(cursor, &mut c_pid);
                    if c_pid != my_pid && IsWindowVisible(cursor) != 0 && IsIconic(cursor) == 0 {
                        let mut r = WinRect::default();
                        if GetWindowRect(cursor, &mut r) != 0
                            && (r.right - r.left) >= 64
                            && (r.bottom - r.top) >= 64
                            && !read_title(cursor).trim().is_empty()
                        {
                            found = cursor;
                            break;
                        }
                    }
                    cursor = GetWindow(cursor, 2 /* GW_HWNDNEXT */);
                }
                if !found.is_null() {
                    fg_hwnd = found;
                }
            }

            let fg_title = read_title(fg_hwnd);
            crate::vision::win32_dpi_guard::verify_capture_target_safe(fg_hwnd, &fg_title)?;
            let scale = crate::vision::win32_dpi_guard::query_window_dpi_scale(fg_hwnd);

            match target {
                VisionCaptureTarget::ActiveWindow => {
                    let mut use_virtual_fallback = fg_hwnd.is_null();
                    if !fg_hwnd.is_null() {
                        let mut c_pid: WinDword = 0;
                        let _ = GetWindowThreadProcessId(fg_hwnd, &mut c_pid);
                        if c_pid == my_pid {
                            use_virtual_fallback = true;
                        }
                    }
                    if use_virtual_fallback {
                        let vx = GetSystemMetrics(SM_XVIRTUALSCREEN);
                        let vy = GetSystemMetrics(SM_YVIRTUALSCREEN);
                        let vw = GetSystemMetrics(SM_CXVIRTUALSCREEN).max(1) as u32;
                        let vh = GetSystemMetrics(SM_CYVIRTUALSCREEN).max(1) as u32;
                        (vx, vy, vw, vh, "Desktop Screen".to_string(), std::ptr::null_mut(), scale)
                    } else {
                        let mut rect = WinRect::default();
                        if GetWindowRect(fg_hwnd, &mut rect) == 0 {
                            return Err(UiError::VisionCaptureError(
                                "GetWindowRect failed for active foreground window".to_string(),
                            ));
                        }
                        let w = (rect.right - rect.left).max(0) as u32;
                        let h = (rect.bottom - rect.top).max(0) as u32;
                        if w == 0 || h == 0 {
                            return Err(UiError::VisionCaptureError(
                                "active foreground window has zero capture area".to_string(),
                            ));
                        }
                        let safe_title = if fg_title.is_empty() {
                            "Active Window".to_string()
                        } else {
                            fg_title
                        };
                        (rect.left, rect.top, w, h, safe_title, fg_hwnd, scale)
                    }
                }
                VisionCaptureTarget::SelectedRegion {
                    x,
                    y,
                    width,
                    height,
                } => {
                    let vx = GetSystemMetrics(SM_XVIRTUALSCREEN);
                    let vy = GetSystemMetrics(SM_YVIRTUALSCREEN);
                    let vw = GetSystemMetrics(SM_CXVIRTUALSCREEN);
                    let vh = GetSystemMetrics(SM_CYVIRTUALSCREEN);
                    if vw > 0 && vh > 0 {
                        let rx_end = (*x as i64) + (*width as i64);
                        let ry_end = (*y as i64) + (*height as i64);
                        let vx_end = (vx as i64) + (vw as i64);
                        let vy_end = (vy as i64) + (vh as i64);
                        if *x < vx || *y < vy || rx_end > vx_end || ry_end > vy_end {
                            return Err(UiError::VisionCaptureError(format!(
                                "selected region ({},{},{}x{}) exceeds virtual desktop ({},{},{}x{})",
                                x, y, width, height, vx, vy, vw, vh
                            )));
                        }
                    }
                    let label = format!("Region {}x{} @ ({},{})", width, height, x, y);
                    (*x, *y, *width, *height, label, std::ptr::null_mut(), scale)
                }
            }
        };

        // SAFETY: RAII guards guarantee ReleaseDC / DeleteDC / DeleteObject on every path.
        unsafe {
            let screen_dc = GetDC(std::ptr::null_mut());
            if screen_dc.is_null() {
                return Err(UiError::VisionCaptureError(
                    "GetDC(NULL) returned null handle".to_string(),
                ));
            }
            let _screen_guard = ScreenDcGuard(screen_dc);

            let mem_dc = CreateCompatibleDC(screen_dc);
            if mem_dc.is_null() {
                return Err(UiError::VisionCaptureError(
                    "CreateCompatibleDC failed".to_string(),
                ));
            }
            let _mem_guard = MemDcGuard(mem_dc);

            let bmp = CreateCompatibleBitmap(screen_dc, cap_w as WinInt, cap_h as WinInt);
            if bmp.is_null() {
                return Err(UiError::VisionCaptureError(
                    "CreateCompatibleBitmap failed".to_string(),
                ));
            }
            let prev_obj = SelectObject(mem_dc, bmp as WinHgdiobj);
            let _bmp_guard = BitmapGuard { mem_dc, prev_obj, bmp };

            let rendered_pw = !fg_hwnd_cap.is_null()
                && crate::vision::win32_dpi_guard::try_print_window(fg_hwnd_cap, mem_dc);
            let bitblt_ok = rendered_pw
                || BitBlt(
                    mem_dc,
                    0,
                    0,
                    cap_w as WinInt,
                    cap_h as WinInt,
                    screen_dc,
                    cap_x,
                    cap_y,
                    SRCCOPY | CAPTUREBLT,
                ) != 0
                || BitBlt(
                    mem_dc,
                    0,
                    0,
                    cap_w as WinInt,
                    cap_h as WinInt,
                    screen_dc,
                    cap_x,
                    cap_y,
                    SRCCOPY,
                ) != 0;

            if !bitblt_ok {
                let err_code = GetLastError();
                return Err(UiError::VisionCaptureError(format!(
                    "BitBlt/PrintWindow capture failed (win32_err={})",
                    err_code
                )));
            }

            read_gdi_bitmap_rgba(mem_dc, bmp, cap_x, cap_y, cap_w, cap_h, dpi_scale, title, now_ms)
        }
    }

    unsafe fn read_gdi_bitmap_rgba(
        mem_dc: WinHdc,
        bmp: WinHbitmap,
        origin_x: i32,
        origin_y: i32,
        width: u32,
        height: u32,
        dpi_scale: f32,
        window_title: String,
        captured_at_ms: u64,
    ) -> Result<RawRgbaFrame, UiError> {
        let byte_len = (width as usize)
            .checked_mul(height as usize)
            .and_then(|px| px.checked_mul(4))
            .ok_or_else(|| {
                UiError::VisionCaptureError("Win32 DIB byte size overflow".to_string())
            })?;
        let mut bgra = vec![0u8; byte_len];
        let mut bmi = WinBitmapInfo {
            bmi_header: WinBitmapInfoHeader {
                bi_size: std::mem::size_of::<WinBitmapInfoHeader>() as WinDword,
                bi_width: width as WinLong,
                bi_height: -(height as WinLong),
                bi_planes: 1,
                bi_bit_count: 32,
                bi_compression: BI_RGB,
                bi_size_image: 0,
                bi_x_pels_per_meter: 0,
                bi_y_pels_per_meter: 0,
                bi_clr_used: 0,
                bi_clr_important: 0,
            },
            bmi_colors: [0],
        };
        let copied = unsafe {
            GetDIBits(
                mem_dc,
                bmp,
                0,
                height as WinUint,
                bgra.as_mut_ptr() as *mut c_void,
                &mut bmi,
                DIB_RGB_COLORS,
            )
        };
        if copied == 0 {
            zeroize_slice(&mut bgra);
            return Err(UiError::VisionCaptureError(
                "GetDIBits failed to read GDI bitmap".to_string(),
            ));
        }
        for chunk in bgra.chunks_exact_mut(4) {
            chunk.swap(0, 2); // BGRA -> RGBA
            chunk[3] = 255;
        }
        let transform = miku_protocol::CaptureTransform {
            origin_virtual_x: origin_x,
            origin_virtual_y: origin_y,
            width_px: width,
            height_px: height,
            dpi_scale: if dpi_scale <= 0.0 { 1.0 } else { dpi_scale },
        };
        Ok(RawRgbaFrame {
            width,
            height,
            rgba: bgra,
            window_title,
            captured_at_ms,
            transform,
        })
    }

    pub fn probe_offscreen_gdi_win32(width: u32, height: u32) -> Result<RawRgbaFrame, UiError> {
        if width == 0 || height == 0 {
            return Err(UiError::VisionCaptureError("offscreen GDI probe dimensions must be non-zero".to_string()));
        }
        // SAFETY: exercises real GDI32 DC, Bitmap, BitBlt, GetDIBits and RAII teardown.
        unsafe {
            let mem_src = CreateCompatibleDC(std::ptr::null_mut());
            if mem_src.is_null() {
                return Err(UiError::VisionCaptureError("CreateCompatibleDC(NULL) failed in offscreen GDI probe".to_string()));
            }
            let _src_guard = MemDcGuard(mem_src);
            let mem_dst = CreateCompatibleDC(std::ptr::null_mut());
            if mem_dst.is_null() {
                return Err(UiError::VisionCaptureError("CreateCompatibleDC(dst) failed in offscreen GDI probe".to_string()));
            }
            let _dst_guard = MemDcGuard(mem_dst);
            let bmp_src = CreateCompatibleBitmap(mem_src, width as WinInt, height as WinInt);
            let bmp_dst = CreateCompatibleBitmap(mem_dst, width as WinInt, height as WinInt);
            if bmp_src.is_null() || bmp_dst.is_null() {
                return Err(UiError::VisionCaptureError("CreateCompatibleBitmap failed in offscreen GDI probe".to_string()));
            }
            let prev_src = SelectObject(mem_src, bmp_src as WinHgdiobj);
            let _bmp_src_guard = BitmapGuard { mem_dc: mem_src, prev_obj: prev_src, bmp: bmp_src };
            let prev_dst = SelectObject(mem_dst, bmp_dst as WinHgdiobj);
            let _bmp_dst_guard = BitmapGuard { mem_dc: mem_dst, prev_obj: prev_dst, bmp: bmp_dst };
            if BitBlt(mem_dst, 0, 0, width as WinInt, height as WinInt, mem_src, 0, 0, SRCCOPY) == 0 {
                return Err(UiError::VisionCaptureError(format!("offscreen GDI BitBlt failed (win32_err={})", GetLastError())));
            }
            read_gdi_bitmap_rgba(mem_dst, bmp_dst, 0, 0, width, height, 1.0, "Win32 Offscreen GDI Probe".to_string(), 0)
        }
    }
}

impl ScreenCaptureBackend for Win32GdiScreenVisionBackend {
    fn capture_rgba(&self, target: &VisionCaptureTarget, now_ms: u64) -> Result<RawRgbaFrame, UiError> {
        #[cfg(windows)]
        {
            win32_gdi_ffi::capture_screen_win32(target, now_ms)
        }
        #[cfg(not(windows))]
        {
            let _ = (target, now_ms);
            Err(UiError::VisionCaptureError(
                "Win32GdiScreenVisionBackend requires Windows".to_string(),
            ))
        }
    }
}
