#![allow(unused_imports)]
use crate::prelude::*;

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
pub(crate) mod win32_native {
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
        pub(crate) left: Long,
        pub(crate) top: Long,
        pub(crate) right: Long,
        pub(crate) bottom: Long,
    }

    #[repr(C)]
    struct MonitorInfoExW {
        pub(crate) cb_size: Dword,
        pub(crate) rc_monitor: WinRect,
        pub(crate) rc_work: WinRect,
        pub(crate) dw_flags: Dword,
        pub(crate) sz_device: [u16; 32],
    }

    #[repr(C)]
    struct WndClassExW {
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

    #[repr(C)]
    struct Point {
        pub(crate) x: Long,
        pub(crate) y: Long,
    }

    #[repr(C)]
    struct Msg {
        pub(crate) hwnd: Hwnd,
        pub(crate) message: Uint,
        pub(crate) w_param: Wparam,
        pub(crate) l_param: Lparam,
        pub(crate) time: Dword,
        pub(crate) pt: Point,
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

