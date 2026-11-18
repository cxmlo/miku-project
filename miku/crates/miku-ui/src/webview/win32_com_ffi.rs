#![allow(unused_imports)]
use crate::prelude::*;

use std::ffi::c_void;

#[cfg(target_os = "windows")]
pub(crate) mod win_ffi {
    use super::*;

    pub type Bool = i32;
    pub type Dword = u32;
    pub type Long = i32;
    pub type Hresult = i32;
    pub type Hwnd = *mut c_void;
    pub type Hmodule = *mut c_void;
    pub type Hinstance = *mut c_void;
    pub type Hmenu = *mut c_void;
    pub type Hicon = *mut c_void;
    pub type Hcursor = *mut c_void;
    pub type Hbrush = *mut c_void;
    pub type Uint = u32;
    pub type Atom = u16;
    pub type Wparam = usize;
    pub type Lparam = isize;
    pub type Lresult = isize;
    pub type Handle = *mut c_void;
    pub type SizeT = usize;

    pub const COINIT_APARTMENTTHREADED: Dword = 0x2;
    pub const S_OK: Hresult = 0;
    pub const S_FALSE: Hresult = 1;
    pub const RPC_E_CHANGED_MODE: Hresult = 0x8001_0106u32 as i32;
    pub const GWL_EXSTYLE: i32 = -20;
    pub const LWA_ALPHA: Dword = 0x0000_0002;
    pub const SW_HIDE: i32 = 0;
    pub const SW_SHOW: i32 = 5;
    pub const SW_MINIMIZE: i32 = 6;
    pub const SW_RESTORE: i32 = 9;
    pub const WM_CLOSE: Uint = 0x0010;
    pub const WM_SYSCOMMAND: Uint = 0x0112;
    pub const SC_MOVE_HTCAPTION: Wparam = 0xF012;
    pub const PM_REMOVE: Uint = 0x0001;
    pub const TH32CS_SNAPPROCESS: Dword = 0x0000_0002;
    pub const INVALID_HANDLE_VALUE: Handle = -1isize as Handle;
    pub const LOAD_WITH_ALTERED_SEARCH_PATH: Dword = 0x0000_0008;
    pub const VK_LBUTTON: i32 = 0x01;
    pub const VK_RBUTTON: i32 = 0x02;
    pub const WDA_EXCLUDEFROMCAPTURE: u32 = 0x0000_0011;

    #[repr(C)]
    #[derive(Debug, Clone, Copy, Default)]
    pub struct LastInputInfo {
        pub cb_size: Uint,
        pub dw_time: Dword,
    }

    #[repr(C)]
    #[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
    pub struct Rect {
        pub left: Long,
        pub top: Long,
        pub right: Long,
        pub bottom: Long,
    }

    #[repr(C)]
    #[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
    pub struct Point {
        pub x: Long,
        pub y: Long,
    }

    #[repr(C)]
    pub struct Msg {
        pub hwnd: Hwnd,
        pub message: Uint,
        pub w_param: Wparam,
        pub l_param: Lparam,
        pub time: Dword,
        pub pt: Point,
    }

    #[repr(C)]
    pub struct WndClassExW {
        pub cb_size: Uint,
        pub style: Uint,
        pub lpfn_wnd_proc: unsafe extern "system" fn(Hwnd, Uint, Wparam, Lparam) -> Lresult,
        pub cb_cls_extra: i32,
        pub cb_wnd_extra: i32,
        pub h_instance: Hinstance,
        pub h_icon: Hicon,
        pub h_cursor: Hcursor,
        pub hbr_background: Hbrush,
        pub lpsz_menu_name: *const u16,
        pub lpsz_class_name: *const u16,
        pub h_icon_sm: Hicon,
    }

    #[repr(C)]
    pub struct ProcessEntry32W {
        pub dw_size: Dword,
        pub cnt_usage: Dword,
        pub th32_process_id: Dword,
        pub th32_default_heap_id: usize,
        pub th32_module_id: Dword,
        pub cnt_threads: Dword,
        pub th32_parent_process_id: Dword,
        pub pc_pri_class_base: Long,
        pub dw_flags: Dword,
        pub sz_exe_file: [u16; 260],
    }

    #[repr(C)]
    pub struct ProcessMemoryCountersEx {
        pub cb: Dword,
        pub page_fault_count: Dword,
        pub peak_working_set_size: SizeT,
        pub working_set_size: SizeT,
        pub quota_peak_paged_pool_usage: SizeT,
        pub quota_paged_pool_usage: SizeT,
        pub quota_peak_non_paged_pool_usage: SizeT,
        pub quota_non_paged_pool_usage: SizeT,
        pub pagefile_usage: SizeT,
        pub peak_pagefile_usage: SizeT,
        pub private_usage: SizeT,
    }

    #[link(name = "ole32")]
    extern "system" {
        pub fn CoInitializeEx(pv_reserved: *mut c_void, dw_co_init: Dword) -> Hresult;
        pub fn CoUninitialize();
        pub fn CoTaskMemFree(pv: *mut c_void);
    }

    #[link(name = "kernel32")]
    extern "system" {
        pub fn GetModuleHandleW(lp_module_name: *const u16) -> Hinstance;
        pub fn LoadLibraryW(lp_lib_file_name: *const u16) -> Hmodule;
        pub fn LoadLibraryExW(
            lp_lib_file_name: *const u16,
            h_file: Handle,
            dw_flags: Dword,
        ) -> Hmodule;
        pub fn FreeLibrary(h_lib_module: Hmodule) -> Bool;
        pub fn GetProcAddress(h_module: Hmodule, lp_proc_name: *const i8) -> *mut c_void;
        pub fn GetCurrentProcessId() -> Dword;
        pub fn GetCurrentProcess() -> Handle;
        pub fn CreateToolhelp32Snapshot(dw_flags: Dword, th32_process_id: Dword) -> Handle;
        pub fn Process32FirstW(h_snapshot: Handle, lppe: *mut ProcessEntry32W) -> Bool;
        pub fn Process32NextW(h_snapshot: Handle, lppe: *mut ProcessEntry32W) -> Bool;
        pub fn CloseHandle(h_object: Handle) -> Bool;
        pub fn K32GetProcessMemoryInfo(
            process: Handle,
            ppsmem_counters: *mut ProcessMemoryCountersEx,
            cb: Dword,
        ) -> Bool;
    }

    #[link(name = "gdi32")]
    extern "system" {
        pub fn CreateSolidBrush(color: Dword) -> Hbrush;
    }

    #[link(name = "user32")]
    extern "system" {
        pub fn RegisterClassExW(lpwcx: *const WndClassExW) -> Atom;
        pub fn CreateWindowExW(
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
        pub fn DefWindowProcW(hwnd: Hwnd, msg: Uint, wparam: Wparam, lparam: Lparam) -> Lresult;
        pub fn SetLayeredWindowAttributes(
            hwnd: Hwnd,
            cr_key: Dword,
            b_alpha: u8,
            dw_flags: Dword,
        ) -> Bool;
        pub fn ShowWindow(hwnd: Hwnd, n_cmd_show: i32) -> Bool;
        pub fn IsWindowVisible(hwnd: Hwnd) -> Bool;
        pub fn ReleaseCapture() -> Bool;
        pub fn SetForegroundWindow(hwnd: Hwnd) -> Bool;
        pub fn SetWindowPos(
            hwnd: Hwnd,
            h_wnd_insert_after: Hwnd,
            x: i32,
            y: i32,
            cx: i32,
            cy: i32,
            u_flags: Uint,
        ) -> Bool;
        pub fn GetWindowLongW(hwnd: Hwnd, n_index: i32) -> Long;
        pub fn GetForegroundWindow() -> Hwnd;
        pub fn GetCursorPos(lp_point: *mut Point) -> Bool;
        pub fn GetWindowRect(hwnd: Hwnd, lp_rect: *mut Rect) -> Bool;
        pub fn GetDpiForWindow(hwnd: Hwnd) -> Uint;
        pub fn GetAsyncKeyState(v_key: i32) -> i16;
        pub fn GetLastInputInfo(plii: *mut LastInputInfo) -> Bool;
        pub fn GetTickCount() -> Dword;
        pub fn SendMessageW(hwnd: Hwnd, msg: Uint, wparam: Wparam, lparam: Lparam) -> Lresult;
        pub fn PeekMessageW(
            lp_msg: *mut Msg,
            h_wnd: Hwnd,
            w_msg_filter_min: Uint,
            w_msg_filter_max: Uint,
            w_remove_msg: Uint,
        ) -> Bool;
        pub fn TranslateMessage(lp_msg: *const Msg) -> Bool;
        pub fn DispatchMessageW(lp_msg: *const Msg) -> Lresult;
        pub fn DestroyWindow(hwnd: Hwnd) -> Bool;
        pub fn SetWindowDisplayAffinity(hwnd: Hwnd, dw_affinity: Dword) -> Bool;
    }

    pub unsafe extern "system" fn com_host_wndproc(
        hwnd: Hwnd,
        msg: Uint,
        wparam: Wparam,
        lparam: Lparam,
    ) -> Lresult {
        if msg == 0x0021
            && crate::webview::transparent_frame::is_hwnd_interactive_focus(hwnd as usize)
        {
            return 1 as Lresult;
        }
        if let Some(ma) = handle_wm_mouseactivate(msg) {
            return ma;
        }
        DefWindowProcW(hwnd, msg, wparam, lparam)
    }

    /// Window procedure for persistent frameless dialog windows (e.g. `Configuración · Miku`):
    /// preserves normal keyboard focus (`MA_ACTIVATE`) and converts `WM_CLOSE` to `SW_HIDE` so the
    /// WebView2 controller stays warm without blank-window bugs on reopen.
    pub unsafe extern "system" fn settings_host_wndproc(
        hwnd: Hwnd,
        msg: Uint,
        wparam: Wparam,
        lparam: Lparam,
    ) -> Lresult {
        if msg == WM_CLOSE {
            let _ = ShowWindow(hwnd, SW_HIDE);
            return 0;
        }
        DefWindowProcW(hwnd, msg, wparam, lparam)
    }

    pub fn wide_z(s: &str) -> Vec<u16> {
        let mut v: Vec<u16> = s.encode_utf16().collect();
        v.push(0);
        v
    }

    pub fn probe_loader_dll_and_symbol() -> bool {
        let dll_w = wide_z("WebView2Loader.dll");
        unsafe {
            let hmod = LoadLibraryW(dll_w.as_ptr());
            if hmod.is_null() {
                return false;
            }
            let sym = b"CreateCoreWebView2EnvironmentWithOptions\0";
            let proc_addr = GetProcAddress(hmod, sym.as_ptr() as *const i8);
            let _ = FreeLibrary(hmod);
            !proc_addr.is_null()
        }
    }
}
