#![allow(unused_imports)]
use crate::prelude::*;

#[cfg(target_os = "windows")]
pub(crate) mod win32_demo_ffi {
    use super::*;
    use std::ffi::c_void;
    use std::mem::{size_of, zeroed};
    use std::ptr::{null, null_mut};
    use std::thread;
    use std::time::Duration;

    pub(crate) type Hwnd = *mut c_void;
    pub(crate) type Hinstance = *mut c_void;
    pub(crate) type Hdc = *mut c_void;
    pub(crate) type HgdIobj = *mut c_void;
    pub(crate) type Hbrush = *mut c_void;
    pub(crate) type Hrgn = *mut c_void;
    pub(crate) type Hicon = *mut c_void;
    pub(crate) type Hcursor = *mut c_void;
    pub(crate) type Hmenu = *mut c_void;
    pub(crate) type Atom = u16;
    pub(crate) type Bool = i32;
    pub(crate) type Uint = u32;
    pub(crate) type Dword = u32;
    pub(crate) type Long = i32;
    pub(crate) type Wparam = usize;
    pub(crate) type Lparam = isize;
    pub(crate) type Lresult = isize;

    pub(crate) const LWA_ALPHA: Dword = 0x0000_0002;
    pub(crate) const PM_REMOVE: Uint = 0x0001;
    pub(crate) const WM_HOTKEY: Uint = 0x0312;
    pub(crate) const WM_KEYDOWN: Uint = 0x0100;
    pub(crate) const WM_LBUTTONDOWN: Uint = 0x0201;
    pub(crate) const WM_CLOSE: Uint = 0x0010;
    pub(crate) const VK_LBUTTON: i32 = 0x01;
    pub(crate) const VK_R_KEY: i32 = 0x52;
    pub(crate) const TRANSPARENT_BK: i32 = 1;

    pub(crate) static DEMO_WINDOW_SEQ: AtomicU64 = AtomicU64::new(1);

    #[repr(C)]
    #[derive(Clone, Copy)]
    pub(crate) struct WinRect {
        pub(crate) left: Long,
        pub(crate) top: Long,
        pub(crate) right: Long,
        pub(crate) bottom: Long,
    }

    #[repr(C)]
    #[derive(Clone, Copy)]
    pub(crate) struct Point {
        pub(crate) x: Long,
        pub(crate) y: Long,
    }

    #[repr(C)]
    pub(crate) struct Msg {
        pub(crate) hwnd: Hwnd,
        pub(crate) message: Uint,
        pub(crate) w_param: Wparam,
        pub(crate) l_param: Lparam,
        pub(crate) time: Dword,
        pub(crate) pt: Point,
    }

    #[repr(C)]
    pub(crate) struct WndClassExW {
        pub(crate) cb_size: Uint,
        pub(crate) style: Uint,
        pub(crate) lpfn_wnd_proc: unsafe extern "system" fn(Hwnd, Uint, Wparam, Lparam) -> Lresult,
        pub(crate) cb_cls_extra: i32,
        pub(crate) cb_wnd_extra: i32,
        pub(crate) h_instance: Hinstance,
        pub(crate) h_icon: Hicon,
        pub(crate) h_cursor: Hcursor,
        pub(crate) hbr_background: Hbrush,
        pub(crate) lpsz_menu_name: *const u16,
        pub(crate) lpsz_class_name: *const u16,
        pub(crate) h_icon_sm: Hicon,
    }

    #[link(name = "kernel32")]
    extern "system" {
        pub(crate) fn GetModuleHandleW(lp_module_name: *const u16) -> Hinstance;
    }

    #[link(name = "user32")]
    extern "system" {
        pub(crate) fn RegisterClassExW(lpwcx: *const WndClassExW) -> Atom;
        pub(crate) fn CreateWindowExW(
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
        pub(crate) fn DefWindowProcW(hwnd: Hwnd, msg: Uint, wparam: Wparam, lparam: Lparam) -> Lresult;
        pub(crate) fn SetLayeredWindowAttributes(
            hwnd: Hwnd,
            cr_key: Dword,
            b_alpha: u8,
            dw_flags: Dword,
        ) -> Bool;
        pub(crate) fn ShowWindow(hwnd: Hwnd, n_cmd_show: i32) -> Bool;
        pub(crate) fn SetWindowPos(
            hwnd: Hwnd,
            h_wnd_insert_after: Hwnd,
            x: i32,
            y: i32,
            cx: i32,
            cy: i32,
            u_flags: Uint,
        ) -> Bool;
        pub(crate) fn SetWindowRgn(hwnd: Hwnd, hrgn: Hrgn, b_redraw: Bool) -> i32;
        pub(crate) fn GetForegroundWindow() -> Hwnd;
        pub(crate) fn SendMessageW(hwnd: Hwnd, msg: Uint, wparam: Wparam, lparam: Lparam) -> Lresult;
        pub(crate) fn PeekMessageW(
            lp_msg: *mut Msg,
            h_wnd: Hwnd,
            w_msg_filter_min: Uint,
            w_msg_filter_max: Uint,
            w_remove_msg: Uint,
        ) -> Bool;
        pub(crate) fn TranslateMessage(lp_msg: *const Msg) -> Bool;
        pub(crate) fn DispatchMessageW(lp_msg: *const Msg) -> Lresult;
        pub(crate) fn GetCursorPos(lp_point: *mut Point) -> Bool;
        pub(crate) fn GetAsyncKeyState(v_key: i32) -> i16;
        pub(crate) fn RegisterHotKey(hwnd: Hwnd, id: i32, fs_modifiers: Uint, vk: Uint) -> Bool;
        pub(crate) fn UnregisterHotKey(hwnd: Hwnd, id: i32) -> Bool;
        pub(crate) fn GetDC(hwnd: Hwnd) -> Hdc;
        pub(crate) fn ReleaseDC(hwnd: Hwnd, hdc: Hdc) -> i32;
        pub(crate) fn FillRect(hdc: Hdc, lprc: *const WinRect, hbr: Hbrush) -> i32;
        pub(crate) fn FrameRgn(hdc: Hdc, hrgn: Hrgn, hbr: Hbrush, n_width: i32, n_height: i32) -> Bool;
        pub(crate) fn DestroyWindow(hwnd: Hwnd) -> Bool;
    }

    #[link(name = "gdi32")]
    extern "system" {
        pub(crate) fn CreateSolidBrush(color: Dword) -> Hbrush;
        pub(crate) fn CreateRoundRectRgn(
            x1: i32,
            y1: i32,
            x2: i32,
            y2: i32,
            w: i32,
            h: i32,
        ) -> Hrgn;
        pub(crate) fn SelectObject(hdc: Hdc, h: HgdIobj) -> HgdIobj;
        pub(crate) fn DeleteObject(ho: HgdIobj) -> Bool;
        pub(crate) fn Ellipse(hdc: Hdc, left: i32, top: i32, right: i32, bottom: i32) -> Bool;
        pub(crate) fn SetBkMode(hdc: Hdc, mode: i32) -> i32;
        pub(crate) fn SetTextColor(hdc: Hdc, color: Dword) -> Dword;
        pub(crate) fn TextOutW(hdc: Hdc, x: i32, y: i32, lp_string: *const u16, c: i32) -> Bool;
    }

    pub(crate) const fn rgb_colorref(r: u8, g: u8, b: u8) -> Dword {
        (r as Dword) | ((g as Dword) << 8) | ((b as Dword) << 16)
    }

    pub(crate) unsafe extern "system" fn demo_pill_wndproc(
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

    pub(crate) unsafe fn paint_demo_pill_frame(
        hwnd: Hwnd,
        width: i32,
        height: i32,
        ctrl: &DemoInteractiveController,
    ) {
        let hdc = GetDC(hwnd);
        if hdc.is_null() {
            return;
        }

        let is_alert = matches!(ctrl.phase(), DemoInteractionPhase::KillSwitchTrippedAlert { .. });
        let is_ptt = matches!(ctrl.phase(), DemoInteractionPhase::PttVoiceListening { .. });
        let is_modal = matches!(ctrl.phase(), DemoInteractionPhase::PermissionModalActive { .. });

        // Surface #0B0F17 (RGB 11, 15, 23) or dark crimson tint on Kill Switch
        let bg_color = if is_alert {
            rgb_colorref(42, 10, 18)
        } else if is_modal {
            rgb_colorref(15, 23, 38)
        } else {
            rgb_colorref(11, 15, 23)
        };

        // Border accent: #F43F5E (244, 63, 94) on alert, #22D3EE (34, 211, 238) on active/idle
        let border_color = if is_alert {
            rgb_colorref(244, 63, 94)
        } else if is_ptt {
            rgb_colorref(52, 211, 153)
        } else {
            rgb_colorref(34, 211, 238)
        };

        let bg_brush = CreateSolidBrush(bg_color);
        let border_brush = CreateSolidBrush(border_color);
        let rc = WinRect {
            left: 0,
            top: 0,
            right: width,
            bottom: height,
        };
        let _ = FillRect(hdc, &rc, bg_brush);

        let rgn = CreateRoundRectRgn(0, 0, width, height, 24, 24);
        if !rgn.is_null() {
            let _ = FrameRgn(hdc, rgn, border_brush, 2, 2);
            let _ = DeleteObject(rgn);
        }

        // Draw procedural Miku avatar indicator driven by 6-spring offsets (jump + squash)
        let jump_offset = ctrl
            .avatar_runtime()
            .physics
            .springs
            .get("jump")
            .map(|s| s.position.clamp(-6.0, 6.0) as i32)
            .unwrap_or(0);
        let avatar_cx = 24;
        let avatar_cy = (height / 2 + jump_offset).clamp(14, height - 14);
        let head_brush = CreateSolidBrush(if is_alert {
            rgb_colorref(244, 63, 94)
        } else {
            rgb_colorref(34, 211, 238)
        });
        let old_brush = SelectObject(hdc, head_brush);
        let _ = Ellipse(
            hdc,
            avatar_cx - 11,
            avatar_cy - 11,
            avatar_cx + 11,
            avatar_cy + 11,
        );
        let _ = SelectObject(hdc, old_brush);
        let _ = DeleteObject(head_brush);

        // Draw 5-band soundwave indicator on the right side of the pill
        let bands = match ctrl.phase() {
            DemoInteractionPhase::PttVoiceListening { band_levels } => *band_levels,
            _ => [0.18, 0.28, 0.40, 0.28, 0.18],
        };
        let wave_brush = CreateSolidBrush(border_color);
        let wave_base_x = width - 42;
        for (idx, level) in bands.iter().enumerate() {
            let bar_h = ((level.clamp(0.12, 1.0)) * ((height - 16) as f64)).round() as i32;
            let bx = wave_base_x + (idx as i32) * 6;
            let top = (height - bar_h) / 2;
            let bar_rc = WinRect {
                left: bx,
                top,
                right: bx + 3,
                bottom: top + bar_h.max(4),
            };
            let _ = FillRect(hdc, &bar_rc, wave_brush);
        }
        let _ = DeleteObject(wave_brush);

        // Draw status text with mandatory [DEMO] badge
        let _ = SetBkMode(hdc, TRANSPARENT_BK);
        let _ = SetTextColor(
            hdc,
            if is_alert {
                rgb_colorref(254, 205, 211)
            } else {
                rgb_colorref(226, 232, 240)
            },
        );
        let display_text = ctrl.last_demo_banner();
        let text_utf16: Vec<u16> = display_text.encode_utf16().collect();
        let max_chars = text_utf16.len().min(62) as i32;
        let _ = TextOutW(
            hdc,
            44,
            (height / 2 - 8).max(6),
            text_utf16.as_ptr(),
            max_chars,
        );

        let _ = DeleteObject(bg_brush);
        let _ = DeleteObject(border_brush);
        let _ = ReleaseDC(hwnd, hdc);
    }
}
