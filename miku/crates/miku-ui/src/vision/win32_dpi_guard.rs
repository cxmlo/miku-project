//! Per-Monitor DPI v2 helper, PrintWindow fallback, and sensitive/UAC window guard for screen vision.

#[cfg(windows)]
#[allow(clippy::missing_transmute_annotations, clippy::not_unsafe_ptr_arg_deref)]
mod ffi {
    use std::ffi::c_void;
    use std::path::Path;
    use std::ptr::null_mut;

    use crate::win32::UiError;

    const PROCESS_QUERY_LIMITED_INFORMATION: u32 = 0x1000;
    const PW_RENDERFULLCONTENT: u32 = 0x0000_0002;
    const DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2: isize = -4;

    #[link(name = "kernel32")]
    unsafe extern "system" {
        fn LoadLibraryA(lp_lib_file_name: *const i8) -> *mut c_void;
        fn GetModuleHandleA(lp_module_name: *const i8) -> *mut c_void;
        fn GetProcAddress(h_module: *mut c_void, lp_proc_name: *const i8) -> *mut c_void;
        fn OpenProcess(
            dw_desired_access: u32,
            b_inherit_handle: i32,
            dw_process_id: u32,
        ) -> *mut c_void;
        fn CloseHandle(h_object: *mut c_void) -> i32;
        fn QueryFullProcessImageNameW(
            h_process: *mut c_void,
            dw_flags: u32,
            lp_exe_name: *mut u16,
            lpdw_size: *mut u32,
        ) -> i32;
    }

    #[link(name = "user32")]
    unsafe extern "system" {
        fn GetClassNameW(hwnd: *mut c_void, lp_class_name: *mut u16, n_max_count: i32) -> i32;
        fn GetWindowThreadProcessId(hwnd: *mut c_void, lpdw_process_id: *mut u32) -> u32;
        fn PrintWindow(hwnd: *mut c_void, hdc_blt: *mut c_void, n_flags: u32) -> i32;
    }

    pub struct DpiAwarenessThreadGuard {
        prev_context: *mut c_void,
        restore_fn: Option<unsafe extern "system" fn(*mut c_void) -> *mut c_void>,
    }

    impl DpiAwarenessThreadGuard {
        pub fn enter_per_monitor_v2() -> Self {
            unsafe {
                let mut hmod = GetModuleHandleA(c"user32.dll".as_ptr());
                if hmod.is_null() {
                    hmod = LoadLibraryA(c"user32.dll".as_ptr());
                }
                if hmod.is_null() {
                    return Self {
                        prev_context: null_mut(),
                        restore_fn: None,
                    };
                }
                let sym = GetProcAddress(hmod, c"SetThreadDpiAwarenessContext".as_ptr());
                if sym.is_null() {
                    return Self {
                        prev_context: null_mut(),
                        restore_fn: None,
                    };
                }
                let set_ctx: unsafe extern "system" fn(*mut c_void) -> *mut c_void =
                    std::mem::transmute(sym);
                let prev = set_ctx(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2 as *mut c_void);
                Self {
                    prev_context: prev,
                    restore_fn: Some(set_ctx),
                }
            }
        }
    }

    impl Drop for DpiAwarenessThreadGuard {
        fn drop(&mut self) {
            if let Some(restore) = self.restore_fn {
                if !self.prev_context.is_null() {
                    unsafe {
                        let _ = restore(self.prev_context);
                    }
                }
            }
        }
    }

    pub fn query_window_dpi_scale(hwnd: *mut c_void) -> f32 {
        unsafe {
            let hmod = GetModuleHandleA(c"user32.dll".as_ptr());
            if !hmod.is_null() && !hwnd.is_null() {
                let sym = GetProcAddress(hmod, c"GetDpiForWindow".as_ptr());
                if !sym.is_null() {
                    let get_dpi: unsafe extern "system" fn(*mut c_void) -> u32 =
                        std::mem::transmute(sym);
                    let dpi = get_dpi(hwnd);
                    if dpi >= 48 {
                        return (dpi as f32) / 96.0;
                    }
                }
                let sym_sys = GetProcAddress(hmod, c"GetDpiForSystem".as_ptr());
                if !sym_sys.is_null() {
                    let get_sys_dpi: unsafe extern "system" fn() -> u32 =
                        std::mem::transmute(sym_sys);
                    let dpi = get_sys_dpi();
                    if dpi >= 48 {
                        return (dpi as f32) / 96.0;
                    }
                }
            }
            1.0
        }
    }

    pub fn try_print_window(hwnd: *mut c_void, mem_dc: *mut c_void) -> bool {
        if hwnd.is_null() || mem_dc.is_null() {
            return false;
        }
        unsafe { PrintWindow(hwnd, mem_dc, PW_RENDERFULLCONTENT) != 0 }
    }

    pub fn foreground_window_class_and_exe(hwnd: *mut c_void) -> (String, String) {
        if hwnd.is_null() {
            return (String::new(), String::new());
        }
        unsafe {
            let mut cls_buf = [0u16; 128];
            let cls_len = GetClassNameW(hwnd, cls_buf.as_mut_ptr(), cls_buf.len() as i32);
            let cls_name = if cls_len > 0 {
                String::from_utf16_lossy(&cls_buf[..(cls_len as usize)])
            } else {
                String::new()
            };

            let mut pid: u32 = 0;
            let _ = GetWindowThreadProcessId(hwnd, &mut pid);
            let exe_name = if pid > 0 {
                let proc_h = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, pid);
                if !proc_h.is_null() {
                    let mut exe_buf = [0u16; 260];
                    let mut size = exe_buf.len() as u32;
                    let ok =
                        QueryFullProcessImageNameW(proc_h, 0, exe_buf.as_mut_ptr(), &mut size);
                    let _ = CloseHandle(proc_h);
                    if ok != 0 && size > 0 {
                        let full = String::from_utf16_lossy(&exe_buf[..(size as usize)]);
                        Path::new(&full)
                            .file_name()
                            .and_then(|s| s.to_str())
                            .unwrap_or("")
                            .to_string()
                    } else {
                        String::new()
                    }
                } else {
                    String::new()
                }
            } else {
                String::new()
            };

            (cls_name, exe_name)
        }
    }

    pub fn verify_capture_target_safe(hwnd: *mut c_void, title: &str) -> Result<(), UiError> {
        let (class_name, exe_name) = foreground_window_class_and_exe(hwnd);
        if super::is_sensitive_window_meta(title, &class_name, &exe_name) {
            return Err(UiError::VisionCaptureError(format!(
                "capture blocked: sensitive credential/UAC/password-manager window (title='{}', class='{}', exe='{}')",
                title, class_name, exe_name
            )));
        }
        Ok(())
    }
}

pub fn is_sensitive_window_meta(title: &str, class_name: &str, exe_name: &str) -> bool {
    let combined = format!(
        "{} {} {}",
        title.to_ascii_lowercase(),
        class_name.to_ascii_lowercase(),
        exe_name.to_ascii_lowercase()
    );
    const BLOCKED: &[&str] = &[
        "credentialuiclass",
        "credential dialog xaml host",
        "lockscreencontroller",
        "consent.exe",
        "user account control",
        "control de cuentas de usuario",
        "windows security",
        "seguridad de windows",
        "keepass",
        "1password",
        "bitwarden",
        "lastpass",
        "dashlane",
        "enpass",
        "roboform",
    ];
    BLOCKED.iter().any(|p| combined.contains(p))
}

#[cfg(windows)]
pub use ffi::{
    query_window_dpi_scale, try_print_window, verify_capture_target_safe, DpiAwarenessThreadGuard,
};
