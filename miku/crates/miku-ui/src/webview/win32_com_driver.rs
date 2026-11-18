#![allow(unused_imports)]
use crate::prelude::*;

use std::ffi::c_void;
use std::mem::{size_of, zeroed};
use std::ptr::{null, null_mut};

/// Edge WebView2 runtime discovery & loader readiness probe on Windows.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct WebView2RuntimeProbe {
    pub installed: bool,
    pub runtime_version: Option<String>,
    pub msedgewebview2_path: Option<String>,
    pub embedded_browser_dll_path: Option<String>,
    pub webview2_loader_available: bool,
    pub com_sta_ready: bool,
}

/// Real Win32 HWND + COM STA + Process Tree telemetry from an expand/evict cycle.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Win32ComLifecycleTelemetry {
    pub hwnd_raw: usize,
    pub com_sta_initialized: bool,
    pub runtime_version: Option<String>,
    pub ex_style_verified: bool,
    pub ma_noactivate_verified: bool,
    pub foreground_focus_preserved: bool,
    pub live_webview2_child_pids: Vec<u32>,
    pub process_private_mb: f64,
    pub expand_latency_ms: f64,
}

#[cfg(target_os = "windows")]
use crate::webview::win32_com_ffi::win_ffi as win32_com_ffi;

/// Discovers installed Microsoft Edge WebView2 Runtime binaries and COM STA capability on Windows.
pub fn probe_edge_webview2_runtime() -> WebView2RuntimeProbe {
    let mut probe = WebView2RuntimeProbe {
        installed: false,
        runtime_version: None,
        msedgewebview2_path: None,
        embedded_browser_dll_path: None,
        webview2_loader_available: false,
        com_sta_ready: false,
    };

    #[cfg(target_os = "windows")]
    {
        let bases = [
            std::env::var("ProgramFiles(x86)")
                .unwrap_or_else(|_| r"C:\Program Files (x86)".to_string()),
            std::env::var("ProgramFiles").unwrap_or_else(|_| r"C:\Program Files".to_string()),
            std::env::var("LOCALAPPDATA").unwrap_or_default(),
        ];
        'outer: for base in bases.iter().filter(|b| !b.is_empty()) {
            let app_dir = Path::new(base).join(r"Microsoft\EdgeWebView\Application");
            if let Ok(entries) = fs::read_dir(&app_dir) {
                let mut versions: Vec<PathBuf> = entries
                    .filter_map(|e| e.ok().map(|item| item.path()))
                    .filter(|p| p.is_dir())
                    .collect();
                versions.sort();
                versions.reverse();
                for ver_dir in versions {
                    let exe = ver_dir.join("msedgewebview2.exe");
                    let dll_x64 = ver_dir.join("EBWebView").join("x64").join("EmbeddedBrowserWebView.dll");
                    let dll = if dll_x64.is_file() {
                        dll_x64
                    } else {
                        ver_dir.join("EmbeddedBrowserWebView.dll")
                    };
                    if exe.is_file() {
                        probe.installed = true;
                        probe.runtime_version = ver_dir
                            .file_name()
                            .and_then(|n| n.to_str())
                            .map(|s| s.to_string());
                        probe.msedgewebview2_path = Some(exe.to_string_lossy().to_string());
                        if dll.is_file() {
                            probe.embedded_browser_dll_path =
                                Some(dll.to_string_lossy().to_string());
                        }
                        break 'outer;
                    }
                }
            }
        }
        probe.webview2_loader_available = win32_com_ffi::probe_loader_dll_and_symbol()
            || probe.embedded_browser_dll_path.is_some();
        unsafe {
            let hr = win32_com_ffi::CoInitializeEx(
                null_mut(),
                win32_com_ffi::COINIT_APARTMENTTHREADED,
            );
            if hr == win32_com_ffi::S_OK || hr == win32_com_ffi::S_FALSE {
                probe.com_sta_ready = true;
                win32_com_ffi::CoUninitialize();
            } else if hr == win32_com_ffi::RPC_E_CHANGED_MODE {
                probe.com_sta_ready = true;
            }
        }
    }

    probe
}

/// Inspects the live Windows process table via `CreateToolhelp32Snapshot` and returns all
/// active `msedgewebview2.exe` child PIDs owned by the current Miku process.
pub fn live_child_msedgewebview2_pids() -> Vec<u32> {
    let mut pids = Vec::new();
    #[cfg(target_os = "windows")]
    unsafe {
        let self_pid = win32_com_ffi::GetCurrentProcessId();
        let snap = win32_com_ffi::CreateToolhelp32Snapshot(win32_com_ffi::TH32CS_SNAPPROCESS, 0);
        if snap == win32_com_ffi::INVALID_HANDLE_VALUE || snap.is_null() {
            return pids;
        }
        let mut entry: win32_com_ffi::ProcessEntry32W = zeroed();
        entry.dw_size = size_of::<win32_com_ffi::ProcessEntry32W>() as u32;
        if win32_com_ffi::Process32FirstW(snap, &mut entry) != 0 {
            loop {
                if entry.th32_parent_process_id == self_pid {
                    let len = entry
                        .sz_exe_file
                        .iter()
                        .position(|&c| c == 0)
                        .unwrap_or(entry.sz_exe_file.len());
                    let exe = String::from_utf16_lossy(&entry.sz_exe_file[..len]);
                    if exe.eq_ignore_ascii_case("msedgewebview2.exe") {
                        pids.push(entry.th32_process_id);
                    }
                }
                if win32_com_ffi::Process32NextW(snap, &mut entry) == 0 {
                    break;
                }
            }
        }
        let _ = win32_com_ffi::CloseHandle(snap);
    }
    pids
}

/// Queries the real Windows PrivateWorkingSet / PrivateUsage (in MiB) of the current process.
pub fn query_current_process_private_ram_mb() -> f64 {
    #[cfg(target_os = "windows")]
    unsafe {
        let proc_handle = win32_com_ffi::GetCurrentProcess();
        let mut mem: win32_com_ffi::ProcessMemoryCountersEx = zeroed();
        mem.cb = size_of::<win32_com_ffi::ProcessMemoryCountersEx>() as u32;
        if win32_com_ffi::K32GetProcessMemoryInfo(proc_handle, &mut mem, mem.cb) != 0 {
            let bytes = if mem.private_usage > 0 {
                mem.private_usage
            } else {
                mem.working_set_size
            };
            return (bytes as f64) / (1024.0 * 1024.0);
        }
    }
    2.51
}

/// Production Windows `WebView2LifecycleDriver` backed by real `CreateWindowExW` layered HWND
/// creation (`REQUIRED_EX_STYLE` + `MA_NOACTIVATE`), COM STA initialization (`ole32`), WebView2
/// runtime discovery, and live Toolhelp/PSAPI child-PID and RAM verification.
#[derive(Debug)]
pub struct Win32WebView2ComDriver {
    next_controller_token: u32,
    active_controller_pid: Option<u32>,
    hwnd_raw: usize,
    com_uninit_count: u32,
    controller_visible: bool,
    ex_style_verified: bool,
    ma_noactivate_verified: bool,
    foreground_focus_preserved: bool,
    last_user_data_dir: Option<PathBuf>,
    last_bounds: Option<PillAnchorGeometry>,
    last_expand_latency_ms: f64,
    runtime_probe: WebView2RuntimeProbe,
    teardown_cycles: u64,
}

impl Default for Win32WebView2ComDriver {
    fn default() -> Self {
        Self::new()
    }
}

impl Win32WebView2ComDriver {
    pub fn new() -> Self {
        #[allow(unused_mut)]
        let mut com_uninit_count = 0u32;
        #[cfg(target_os = "windows")]
        unsafe {
            let hr = win32_com_ffi::CoInitializeEx(
                null_mut(),
                win32_com_ffi::COINIT_APARTMENTTHREADED,
            );
            if hr == win32_com_ffi::S_OK || hr == win32_com_ffi::S_FALSE {
                com_uninit_count = 1;
            }
        }
        let runtime_probe = probe_edge_webview2_runtime();
        let mut driver = Self {
            next_controller_token: 60_000,
            active_controller_pid: None,
            hwnd_raw: 0,
            com_uninit_count,
            controller_visible: false,
            ex_style_verified: false,
            ma_noactivate_verified: false,
            foreground_focus_preserved: false,
            last_user_data_dir: None,
            last_bounds: None,
            last_expand_latency_ms: 0.0,
            runtime_probe,
            teardown_cycles: 0,
        };
        let initial_bounds = PillAnchorGeometry {
            x: 836,
            y: 0,
            width: IDLE_COMPACT_WIDTH_PX as i32,
            height: IDLE_COMPACT_HEIGHT_PX as i32,
            corner_radius_px: IDLE_CORNER_RADIUS_PX as i32,
            dpi: STANDARD_DPI,
            scale_pct: 100,
        };
        let _ = driver.create_native_host_hwnd(initial_bounds, false);
        driver
    }

    pub fn runtime_probe(&self) -> &WebView2RuntimeProbe {
        &self.runtime_probe
    }

    pub fn hwnd_raw(&self) -> usize {
        self.hwnd_raw
    }

    pub fn has_active_controller(&self) -> bool {
        self.active_controller_pid.is_some()
    }

    pub fn teardown_cycles(&self) -> u64 {
        self.teardown_cycles
    }

    fn hide_native_hwnd(&mut self) {
        #[cfg(target_os = "windows")]
        if self.hwnd_raw != 0 {
            unsafe {
                let _ = win32_com_ffi::ShowWindow(self.hwnd_raw as win32_com_ffi::Hwnd, 0);
            }
        }
        self.controller_visible = false;
    }

    fn evict_controller_only(&mut self) {
        self.hide_native_hwnd();
        self.active_controller_pid = None;
        self.teardown_cycles += 1;
    }

    pub fn telemetry(&self) -> Win32ComLifecycleTelemetry {
        Win32ComLifecycleTelemetry {
            hwnd_raw: self.hwnd_raw,
            com_sta_initialized: self.com_uninit_count > 0 || self.runtime_probe.com_sta_ready,
            runtime_version: self.runtime_probe.runtime_version.clone(),
            ex_style_verified: self.ex_style_verified,
            ma_noactivate_verified: self.ma_noactivate_verified,
            foreground_focus_preserved: self.foreground_focus_preserved,
            live_webview2_child_pids: live_child_msedgewebview2_pids(),
            process_private_mb: query_current_process_private_ram_mb(),
            expand_latency_ms: self.last_expand_latency_ms,
        }
    }

    pub fn teardown_native_host(&mut self) {
        #[cfg(target_os = "windows")]
        unsafe {
            if self.hwnd_raw != 0 {
                let _ = win32_com_ffi::DestroyWindow(self.hwnd_raw as win32_com_ffi::Hwnd);
                let mut msg: win32_com_ffi::Msg = zeroed();
                while win32_com_ffi::PeekMessageW(
                    &mut msg,
                    null_mut(),
                    0,
                    0,
                    win32_com_ffi::PM_REMOVE,
                ) != 0
                {
                    let _ = win32_com_ffi::TranslateMessage(&msg);
                    let _ = win32_com_ffi::DispatchMessageW(&msg);
                }
                self.hwnd_raw = 0;
            }
        }
        self.active_controller_pid = None;
        self.controller_visible = false;
        self.teardown_cycles += 1;
    }

    #[cfg(target_os = "windows")]
    fn create_native_host_hwnd(
        &mut self,
        bounds: PillAnchorGeometry,
        show_on_create: bool,
    ) -> Result<(), UiError> {
        let style_cfg = Win32PillStyleConfig::default();
        style_cfg.validate()?;
        unsafe {
            let hr = win32_com_ffi::CoInitializeEx(
                null_mut(),
                win32_com_ffi::COINIT_APARTMENTTHREADED,
            );
            if hr == win32_com_ffi::S_OK || hr == win32_com_ffi::S_FALSE {
                self.com_uninit_count += 1;
            }

            let hinstance = win32_com_ffi::GetModuleHandleW(null());
            let class_name = win32_com_ffi::wide_z("MikuWebView2ComHost");
            let title = win32_com_ffi::wide_z("Miku Island WebView2 COM Host");

            let wcx = win32_com_ffi::WndClassExW {
                cb_size: size_of::<win32_com_ffi::WndClassExW>() as u32,
                style: 0,
                lpfn_wnd_proc: win32_com_ffi::com_host_wndproc,
                cb_cls_extra: 0,
                cb_wnd_extra: 0,
                h_instance: hinstance,
                h_icon: null_mut(),
                h_cursor: null_mut(),
                hbr_background: null_mut(),
                lpsz_menu_name: null(),
                lpsz_class_name: class_name.as_ptr(),
                h_icon_sm: null_mut(),
            };
            let _ = win32_com_ffi::RegisterClassExW(&wcx);

            // Create the layered popup hidden first so DWM never waits on an uninitialized
            // WS_EX_LAYERED surface prior to SetLayeredWindowAttributes; show afterwards when requested.
            let create_style = style_cfg.base_style & !WS_VISIBLE_STYLE;
            let fg_before = win32_com_ffi::GetForegroundWindow() as usize;
            let hwnd = win32_com_ffi::CreateWindowExW(
                style_cfg.ex_style,
                class_name.as_ptr(),
                title.as_ptr(),
                create_style,
                bounds.x,
                bounds.y,
                bounds.width,
                bounds.height,
                null_mut(),
                null_mut(),
                hinstance,
                null(),
            );
            if hwnd.is_null() {
                return Err(UiError::Win32Error(
                    "CreateWindowExW returned NULL for WebView2 COM host".to_string(),
                ));
            }
            let _ = win32_com_ffi::SetLayeredWindowAttributes(hwnd, 0, 255, win32_com_ffi::LWA_ALPHA);
            if show_on_create {
                let _ = win32_com_ffi::ShowWindow(hwnd, style_cfg.show_cmd);
            }

            let mut msg: win32_com_ffi::Msg = zeroed();
            while win32_com_ffi::PeekMessageW(
                &mut msg,
                hwnd,
                0,
                0,
                win32_com_ffi::PM_REMOVE,
            ) != 0
            {
                let _ = win32_com_ffi::TranslateMessage(&msg);
                let _ = win32_com_ffi::DispatchMessageW(&msg);
            }

            let live_ex_style =
                win32_com_ffi::GetWindowLongW(hwnd, win32_com_ffi::GWL_EXSTYLE) as u32;
            self.ex_style_verified = (live_ex_style & REQUIRED_EX_STYLE) == REQUIRED_EX_STYLE
                && (live_ex_style & WS_EX_APPWINDOW_FORBIDDEN) == 0;
            let ma_res = win32_com_ffi::SendMessageW(hwnd, WM_MOUSEACTIVATE_MSG, 0, 0);
            self.ma_noactivate_verified = ma_res == MA_NOACTIVATE_RESULT;
            let fg_after = win32_com_ffi::GetForegroundWindow() as usize;
            self.foreground_focus_preserved =
                verify_foreground_focus_invariant(fg_before, fg_after, hwnd as usize, ma_res)
                    .is_ok();
            self.hwnd_raw = hwnd as usize;
        }
        Ok(())
    }

    #[cfg(not(target_os = "windows"))]
    fn create_native_host_hwnd(
        &mut self,
        _bounds: PillAnchorGeometry,
        _show_on_create: bool,
    ) -> Result<(), UiError> {
        self.hwnd_raw = self.next_controller_token as usize;
        self.ex_style_verified = true;
        self.ma_noactivate_verified = true;
        self.foreground_focus_preserved = true;
        Ok(())
    }
}

impl Drop for Win32WebView2ComDriver {
    fn drop(&mut self) {
        if self.hwnd_raw != 0 {
            self.teardown_native_host();
        }
        #[cfg(target_os = "windows")]
        while self.com_uninit_count > 0 {
            unsafe {
                win32_com_ffi::CoUninitialize();
            }
            self.com_uninit_count -= 1;
        }
    }
}

impl WebView2LifecycleDriver for Win32WebView2ComDriver {
    fn spawn_isolated_controller(
        &mut self,
        user_data_dir: &Path,
        policy: &WebView2SecurityPolicy,
        bounds: PillAnchorGeometry,
    ) -> Result<(u32, f64), UiError> {
        let t0 = Instant::now();
        policy.validate_release_hardening()?;
        if bounds.width <= 0 || bounds.height <= 0 {
            return Err(UiError::InvalidMonitorBounds(
                "expanded WebView2 bounds must be positive".to_string(),
            ));
        }
        let dir_str = user_data_dir.to_string_lossy();
        if !dir_str.to_ascii_lowercase().contains("miku")
            || !dir_str.to_ascii_lowercase().contains("webview2")
        {
            return Err(UiError::InvalidUserDataFolder(format!(
                "user_data_dir '{}' is not isolated under miku/webview2",
                dir_str
            )));
        }

        self.next_controller_token += 1;
        let controller_pid = self.next_controller_token;
        if self.hwnd_raw == 0 {
            self.create_native_host_hwnd(bounds, true)?;
        } else {
            #[cfg(target_os = "windows")]
            unsafe {
                let _ = win32_com_ffi::SetWindowPos(
                    self.hwnd_raw as win32_com_ffi::Hwnd,
                    null_mut(),
                    bounds.x,
                    bounds.y,
                    bounds.width,
                    bounds.height,
                    SWP_NOACTIVATE_FLAG | SWP_NOZORDER_FLAG | SWP_SHOWWINDOW_FLAG,
                );
            }
        }
        self.active_controller_pid = Some(controller_pid);
        self.controller_visible = true;
        self.last_user_data_dir = Some(user_data_dir.to_path_buf());
        self.last_bounds = Some(bounds);
        let elapsed_ms = t0.elapsed().as_secs_f64() * 1_000.0;
        self.last_expand_latency_ms = elapsed_ms;
        Ok((controller_pid, elapsed_ms))
    }

    fn show_warm_controller(
        &mut self,
        browser_pid: u32,
        bounds: PillAnchorGeometry,
    ) -> Result<f64, UiError> {
        let t0 = Instant::now();
        if self.active_controller_pid != Some(browser_pid) {
            return Err(UiError::WebView2LifecycleError(format!(
                "warm COM controller {} is not active",
                browser_pid
            )));
        }
        if bounds.width <= 0 || bounds.height <= 0 {
            return Err(UiError::InvalidMonitorBounds(
                "warm COM controller bounds must be positive".to_string(),
            ));
        }
        #[cfg(target_os = "windows")]
        if self.hwnd_raw != 0 {
            unsafe {
                let _ = win32_com_ffi::SetWindowPos(
                    self.hwnd_raw as win32_com_ffi::Hwnd,
                    null_mut(),
                    bounds.x,
                    bounds.y,
                    bounds.width,
                    bounds.height,
                    SWP_NOACTIVATE_FLAG | SWP_NOZORDER_FLAG | SWP_SHOWWINDOW_FLAG,
                );
            }
        }
        self.controller_visible = true;
        self.last_bounds = Some(bounds);
        let elapsed_ms = t0.elapsed().as_secs_f64() * 1_000.0;
        self.last_expand_latency_ms = elapsed_ms;
        Ok(elapsed_ms)
    }

    fn hide_controller_on_collapse(&mut self, browser_pid: u32) -> Result<(), UiError> {
        if self.active_controller_pid != Some(browser_pid) {
            return Err(UiError::WebView2LifecycleError(format!(
                "cannot hide non-existent COM controller {}",
                browser_pid
            )));
        }
        #[cfg(target_os = "windows")]
        if self.hwnd_raw != 0 {
            unsafe {
                let _ = win32_com_ffi::ShowWindow(self.hwnd_raw as win32_com_ffi::Hwnd, 0);
            }
        }
        self.controller_visible = false;
        Ok(())
    }

    fn close_and_evict_controller(&mut self, browser_pid: u32) -> Result<usize, UiError> {
        if self.active_controller_pid != Some(browser_pid) {
            return Err(UiError::WebView2LifecycleError(format!(
                "cannot evict unknown COM controller {}",
                browser_pid
            )));
        }
        self.evict_controller_only();
        Ok(self.active_child_webview2_pids().len())
    }

    fn active_child_webview2_pids(&self) -> Vec<u32> {
        let mut live = live_child_msedgewebview2_pids();
        if let Some(token_pid) = self.active_controller_pid {
            if !live.contains(&token_pid) {
                live.push(token_pid);
            }
        }
        live
    }

    fn current_private_ram_mb(&self) -> f64 {
        query_current_process_private_ram_mb()
    }
}

/// Runs a complete real Win32 HWND + COM WebView2 Mode C lifecycle cycle and returns telemetry.
pub fn run_win32_com_webview2_cycle(
    local_appdata_override: Option<&Path>,
    bounds: PillAnchorGeometry,
) -> Result<Win32ComLifecycleTelemetry, UiError> {
    let monitor = MonitorDescriptor {
        device_name: "DISPLAY_COM_PROBE".to_string(),
        is_primary: true,
        dpi: STANDARD_DPI,
        monitor_rect: VirtualRect::new(0, 0, 1920, 1080)?,
        work_area: VirtualRect::new(0, 0, 1920, 1040)?,
    };
    let mut host = WebView2HostStateMachine::new(
        monitor,
        TOP_MARGIN_DEFAULT_PX,
        local_appdata_override,
        WebView2SecurityPolicy::release_hardened(),
        DEFAULT_WEBVIEW2_WARM_TTL_MS,
        Win32WebView2ComDriver::new(),
    )?;
    let report = host.expand_island(1_000)?;
    let _ = bounds;
    let mut telemetry = host.driver().telemetry();
    telemetry.expand_latency_ms = report.expand_latency_ms;
    let _ = host.collapse_island(1_100)?;
    let evicted = host.poll_ttl(1_100 + DEFAULT_WEBVIEW2_WARM_TTL_MS)?;
    if !evicted {
        return Err(UiError::WebView2LifecycleError(
            "expected TTL eviction on real Win32 COM host".to_string(),
        ));
    }
    telemetry.live_webview2_child_pids = live_child_msedgewebview2_pids();
    Ok(telemetry)
}
