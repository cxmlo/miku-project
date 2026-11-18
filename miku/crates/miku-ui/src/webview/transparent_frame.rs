//! Windows 11 DWM frameless per-pixel transparency & cursor pass-through helpers for WebView2.

#[cfg(target_os = "windows")]
use std::ffi::c_void;

pub(crate) const WS_EX_NOREDIRECTIONBITMAP: u32 = 0x0020_0000;
pub(crate) const WS_EX_TRANSPARENT_STYLE: u32 = 0x0000_0020;

#[cfg(target_os = "windows")]
const GWL_EXSTYLE: i32 = -20;
#[cfg(target_os = "windows")]
const DWMWA_USE_IMMERSIVE_DARK_MODE: u32 = 20;
#[cfg(target_os = "windows")]
const DWMWA_WINDOW_CORNER_PREFERENCE: u32 = 33;
#[cfg(target_os = "windows")]
const DWMWCP_DONOTROUND: u32 = 1;
#[cfg(target_os = "windows")]
const DWMWCP_ROUND: u32 = 2;
#[cfg(target_os = "windows")]
const DWMWA_BORDER_COLOR: u32 = 34;
#[cfg(target_os = "windows")]
const DWMWA_COLOR_NONE: u32 = 0xFFFF_FFFE;
#[cfg(target_os = "windows")]
const DWMWA_DARK_SUBTLE_BORDER: u32 = 0x0024_2426;

#[cfg(target_os = "windows")]
#[repr(C)]
struct DwmMargins {
    cx_left_width: i32,
    cx_right_width: i32,
    cy_top_height: i32,
    cy_bottom_height: i32,
}

#[cfg(target_os = "windows")]
#[link(name = "dwmapi")]
extern "system" {
    fn DwmExtendFrameIntoClientArea(hwnd: *mut c_void, p_mar_inset: *const DwmMargins) -> i32;
    fn DwmSetWindowAttribute(
        hwnd: *mut c_void,
        dw_attribute: u32,
        pv_attribute: *const c_void,
        cb_attribute: u32,
    ) -> i32;
}

#[cfg(target_os = "windows")]
#[link(name = "user32")]
extern "system" {
    fn GetWindowLongW(hwnd: *mut c_void, n_index: i32) -> i32;
    fn SetWindowLongW(hwnd: *mut c_void, n_index: i32, dw_new_long: i32) -> i32;
}

/// Applies Windows 11 DirectComposition transparency without outer DWM borders or rounding artifacts.
#[cfg(target_os = "windows")]
pub(crate) unsafe fn apply_transparent_compositor_frame(hwnd_raw: usize, transparent_bg: bool) {
    if hwnd_raw == 0 {
        return;
    }
    let hwnd = hwnd_raw as *mut c_void;
    let corner = if transparent_bg {
        DWMWCP_DONOTROUND
    } else {
        DWMWCP_ROUND
    };
    let _ = DwmSetWindowAttribute(
        hwnd,
        DWMWA_WINDOW_CORNER_PREFERENCE,
        &corner as *const u32 as *const c_void,
        std::mem::size_of::<u32>() as u32,
    );
    if transparent_bg {
        let no_border = DWMWA_COLOR_NONE;
        let _ = DwmSetWindowAttribute(
            hwnd,
            DWMWA_BORDER_COLOR,
            &no_border as *const u32 as *const c_void,
            std::mem::size_of::<u32>() as u32,
        );
        let margins = DwmMargins {
            cx_left_width: -1,
            cx_right_width: -1,
            cy_top_height: -1,
            cy_bottom_height: -1,
        };
        let _ = DwmExtendFrameIntoClientArea(hwnd, &margins);
    } else {
        let dark_mode: u32 = 1;
        let _ = DwmSetWindowAttribute(
            hwnd,
            DWMWA_USE_IMMERSIVE_DARK_MODE,
            &dark_mode as *const u32 as *const c_void,
            std::mem::size_of::<u32>() as u32,
        );
        let dark_border = DWMWA_DARK_SUBTLE_BORDER;
        let _ = DwmSetWindowAttribute(
            hwnd,
            DWMWA_BORDER_COLOR,
            &dark_border as *const u32 as *const c_void,
            std::mem::size_of::<u32>() as u32,
        );
    }
}

#[cfg(not(target_os = "windows"))]
pub(crate) unsafe fn apply_transparent_compositor_frame(_hwnd_raw: usize, _transparent_bg: bool) {}

/// Toggles `WS_EX_TRANSPARENT` so outer invisible padding around the Dynamic Island passes clicks through.
#[cfg(target_os = "windows")]
pub(crate) unsafe fn set_click_through(hwnd_raw: usize, ignore_cursor: bool) {
    if hwnd_raw == 0 {
        return;
    }
    let hwnd = hwnd_raw as *mut c_void;
    let cur = GetWindowLongW(hwnd, GWL_EXSTYLE) as u32;
    let next = if ignore_cursor {
        cur | WS_EX_TRANSPARENT_STYLE
    } else {
        cur & !WS_EX_TRANSPARENT_STYLE
    };
    if next != cur {
        let _ = SetWindowLongW(hwnd, GWL_EXSTYLE, next as i32);
    }
}

#[cfg(target_os = "windows")]
const WS_EX_NOACTIVATE_STYLE_RAW: u32 = 0x0800_0000;

#[cfg(target_os = "windows")]
const INTERACTIVE_FOCUS_PROP_UTF16: &[u16] = &[
    b'M' as u16, b'i' as u16, b'k' as u16, b'u' as u16,
    b'I' as u16, b'n' as u16, b't' as u16, b'e' as u16,
    b'r' as u16, b'a' as u16, b'c' as u16, b't' as u16,
    b'i' as u16, b'v' as u16, b'e' as u16,
    b'F' as u16, b'o' as u16, b'c' as u16, b'u' as u16, b's' as u16, 0,
];

#[cfg(target_os = "windows")]
mod focus_ffi {
    use std::ffi::c_void;
    #[link(name = "user32")]
    extern "system" {
        pub(super) fn SetPropW(hwnd: *mut c_void, lp_string: *const u16, h_data: *mut c_void) -> i32;
        pub(super) fn GetPropW(hwnd: *mut c_void, lp_string: *const u16) -> *mut c_void;
        pub(super) fn RemovePropW(hwnd: *mut c_void, lp_string: *const u16) -> *mut c_void;
        pub(super) fn SetForegroundWindow(hwnd: *mut c_void) -> i32;
    }
}

/// Toggles interactive keyboard activation on the floating Dynamic Island HWND.
/// When `interactive` is true, removes `WS_EX_NOACTIVATE` and marks the HWND
/// so `WM_MOUSEACTIVATE` returns `MA_ACTIVATE`, enabling typing in `<textarea>`.
/// When `activate_now` is also true, brings the Island to the foreground.
#[cfg(target_os = "windows")]
pub(crate) fn set_hwnd_interactive_focus(hwnd_raw: usize, interactive: bool, activate_now: bool) {
    if hwnd_raw == 0 {
        return;
    }
    unsafe {
        let hwnd = hwnd_raw as *mut c_void;
        let cur = GetWindowLongW(hwnd, GWL_EXSTYLE) as u32;
        let next = if interactive {
            let _ = focus_ffi::SetPropW(
                hwnd,
                INTERACTIVE_FOCUS_PROP_UTF16.as_ptr(),
                std::ptr::dangling_mut::<c_void>(),
            );
            cur & !WS_EX_NOACTIVATE_STYLE_RAW
        } else {
            let _ = focus_ffi::RemovePropW(hwnd, INTERACTIVE_FOCUS_PROP_UTF16.as_ptr());
            cur | WS_EX_NOACTIVATE_STYLE_RAW
        };
        if next != cur {
            let _ = SetWindowLongW(hwnd, GWL_EXSTYLE, next as i32);
        }
        if interactive && activate_now {
            let _ = focus_ffi::SetForegroundWindow(hwnd);
        }
    }
}

/// Returns `true` when the HWND is temporarily opted into `MA_ACTIVATE` for chat/settings typing.
#[cfg(target_os = "windows")]
pub(crate) fn is_hwnd_interactive_focus(hwnd_raw: usize) -> bool {
    if hwnd_raw == 0 {
        return false;
    }
    unsafe {
        let hwnd = hwnd_raw as *mut c_void;
        !focus_ffi::GetPropW(hwnd, INTERACTIVE_FOCUS_PROP_UTF16.as_ptr()).is_null()
    }
}

#[cfg(not(target_os = "windows"))]
pub(crate) fn set_hwnd_interactive_focus(_hwnd_raw: usize, _interactive: bool, _activate_now: bool) {}

#[cfg(not(target_os = "windows"))]
pub(crate) fn is_hwnd_interactive_focus(_hwnd_raw: usize) -> bool {
    false
}

#[cfg(not(target_os = "windows"))]
pub(crate) unsafe fn set_click_through(_hwnd_raw: usize, _ignore_cursor: bool) {}

#[cfg(target_os = "windows")]
#[link(name = "gdi32")]
extern "system" {
    fn CreateRoundRectRgn(x1: i32, y1: i32, x2: i32, y2: i32, w: i32, h: i32) -> *mut c_void;
    fn DeleteObject(ho: *mut c_void) -> i32;
}

#[cfg(target_os = "windows")]
#[link(name = "user32")]
extern "system" {
    fn SetWindowRgn(hwnd: *mut c_void, h_rgn: *mut c_void, b_redraw: i32) -> i32;
}

#[cfg(target_os = "windows")]
pub(crate) fn create_live_pill_hwnd(
    bounds: crate::prelude::PillAnchorGeometry,
    transparent_bg: bool,
) -> Result<super::win32_com_ffi::win_ffi::Hwnd, crate::prelude::UiError> {
    use super::win32_com_ffi::win_ffi;
    use crate::prelude::{UiError, Win32PillStyleConfig, WS_EX_LAYERED_STYLE, WS_VISIBLE_STYLE};

    let style_cfg = Win32PillStyleConfig::default();
    style_cfg.validate()?;
    unsafe {
        let hinst = win_ffi::GetModuleHandleW(std::ptr::null());
        let class_w = win_ffi::wide_z("MikuLiveIslandWebView2");
        let title_w = win_ffi::wide_z("Miku");
        let wcx = win_ffi::WndClassExW {
            cb_size: std::mem::size_of::<win_ffi::WndClassExW>() as u32,
            style: 0,
            lpfn_wnd_proc: win_ffi::com_host_wndproc,
            cb_cls_extra: 0,
            cb_wnd_extra: 0,
            h_instance: hinst,
            h_icon: std::ptr::null_mut(),
            h_cursor: std::ptr::null_mut(),
            hbr_background: std::ptr::null_mut(),
            lpsz_menu_name: std::ptr::null(),
            lpsz_class_name: class_w.as_ptr(),
            h_icon_sm: std::ptr::null_mut(),
        };
        let _ = win_ffi::RegisterClassExW(&wcx);

        let create_style = style_cfg.base_style & !WS_VISIBLE_STYLE;
        // Transparent WebView2 uses DirectComposition (`WS_EX_NOREDIRECTIONBITMAP`) + DWM frame
        // extension without `WS_EX_LAYERED` (an un-updated layered bitmap renders an opaque black cutout).
        let ex_style = if transparent_bg {
            (style_cfg.ex_style & !WS_EX_LAYERED_STYLE) | WS_EX_NOREDIRECTIONBITMAP
        } else {
            style_cfg.ex_style
        };
        let hwnd = win_ffi::CreateWindowExW(
            ex_style,
            class_w.as_ptr(),
            title_w.as_ptr(),
            create_style,
            bounds.x,
            bounds.y,
            bounds.width,
            bounds.height,
            std::ptr::null_mut(),
            std::ptr::null_mut(),
            hinst,
            std::ptr::null(),
        );
        if hwnd.is_null() {
            return Err(UiError::Win32Error(
                "CreateWindowExW returned NULL for LiveWebView2Window".to_string(),
            ));
        }

        // Screen vision affinity: exclude transparent Island window from desktop BitBlt captures
        let _ = win_ffi::SetWindowDisplayAffinity(hwnd, win_ffi::WDA_EXCLUDEFROMCAPTURE);

        apply_transparent_compositor_frame(hwnd as usize, transparent_bg);
        if !transparent_bg {
            let _ = win_ffi::SetLayeredWindowAttributes(hwnd, 0, 255, win_ffi::LWA_ALPHA);
            apply_opaque_rounded_rgn(hwnd as usize, bounds.width, bounds.height, bounds.corner_radius_px);
        }
        let _ = win_ffi::ShowWindow(hwnd, style_cfg.show_cmd);
        Ok(hwnd)
    }
}

/// Creates an independent frameless dark desktop window (`Configuración · Miku`, e.g. 1080×780)
/// with rounded Windows 11 DWM corners, full keyboard focus, taskbar presence, and hide-on-close
/// `WM_CLOSE` handling so its WebView2 controller never blanks on reopen.
#[cfg(target_os = "windows")]
pub(crate) fn create_settings_dialog_hwnd(
    bounds: crate::prelude::PillAnchorGeometry,
    title: &str,
) -> Result<super::win32_com_ffi::win_ffi::Hwnd, crate::prelude::UiError> {
    use super::win32_com_ffi::win_ffi;
    use crate::prelude::UiError;

    const WS_POPUP: u32 = 0x8000_0000;
    const WS_MINIMIZEBOX: u32 = 0x0002_0000;
    const WS_SYSMENU: u32 = 0x0008_0000;
    const WS_CLIPCHILDREN: u32 = 0x0200_0000;
    const WS_EX_APPWINDOW: u32 = 0x0004_0000;

    unsafe {
        let hinst = win_ffi::GetModuleHandleW(std::ptr::null());
        let class_w = win_ffi::wide_z("MikuSettingsWebView2");
        let title_w = win_ffi::wide_z(title);
        let black_brush = win_ffi::CreateSolidBrush(0x0000_0000);
        let wcx = win_ffi::WndClassExW {
            cb_size: std::mem::size_of::<win_ffi::WndClassExW>() as u32,
            style: 0,
            lpfn_wnd_proc: win_ffi::settings_host_wndproc,
            cb_cls_extra: 0,
            cb_wnd_extra: 0,
            h_instance: hinst,
            h_icon: std::ptr::null_mut(),
            h_cursor: std::ptr::null_mut(),
            hbr_background: black_brush,
            lpsz_menu_name: std::ptr::null(),
            lpsz_class_name: class_w.as_ptr(),
            h_icon_sm: std::ptr::null_mut(),
        };
        let _ = win_ffi::RegisterClassExW(&wcx);

        let hwnd = win_ffi::CreateWindowExW(
            WS_EX_APPWINDOW,
            class_w.as_ptr(),
            title_w.as_ptr(),
            WS_POPUP | WS_MINIMIZEBOX | WS_SYSMENU | WS_CLIPCHILDREN,
            bounds.x,
            bounds.y,
            bounds.width,
            bounds.height,
            std::ptr::null_mut(),
            std::ptr::null_mut(),
            hinst,
            std::ptr::null(),
        );
        if hwnd.is_null() {
            return Err(UiError::Win32Error(
                "CreateWindowExW returned NULL for MikuSettingsWebView2".to_string(),
            ));
        }

        apply_transparent_compositor_frame(hwnd as usize, false);
        let _ = win_ffi::ShowWindow(hwnd, win_ffi::SW_SHOW);
        let _ = win_ffi::SetForegroundWindow(hwnd);
        Ok(hwnd)
    }
}

/// Applies GDI rounded region only on legacy opaque host windows (never on transparent WebView2).
#[cfg(target_os = "windows")]
pub(crate) unsafe fn apply_opaque_rounded_rgn(hwnd_raw: usize, w: i32, h: i32, radius: i32) {
    if hwnd_raw == 0 {
        return;
    }
    let hwnd = hwnd_raw as *mut c_void;
    let diam = (radius.max(8) * 2).min(w).min(h);
    let rgn = CreateRoundRectRgn(0, 0, w + 1, h + 1, diam, diam);
    if !rgn.is_null() && SetWindowRgn(hwnd, rgn, 1) == 0 {
        let _ = DeleteObject(rgn);
    }
}
