#![allow(dead_code)]
//! Live Win32 Dynamic Island pill window backed by real `ICoreWebView2Controller` COM hosting,
//! non-activating layered rounded geometry, bidirectional `chrome.webview` IPC, and zero-orphan teardown.

use crate::prelude::*;
use std::sync::mpsc::{self, Receiver, Sender};
use std::time::{Duration, Instant};

#[cfg(target_os = "windows")]
use crate::webview::com_abi::win_com;
#[cfg(target_os = "windows")]
use crate::webview::win32_com_ffi::win_ffi;

#[derive(Debug, Clone)]
pub struct LiveWebView2Config {
    pub url: String,
    pub auth_token: Option<String>,
    pub bounds: PillAnchorGeometry,
    pub security_policy: WebView2SecurityPolicy,
    pub user_data_dir_override: Option<PathBuf>,
    pub transparent_bg: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LiveWebView2Event {
    ControllerReady,
    ControllerError(String),
    WebMessageJson(String),
    PermissionRequested { kind: u32, state: u32 },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum LiveWindowKind {
    #[default]
    FloatingIsland,
    FramelessDialog,
}

pub struct LiveWebView2Window {
    pub(super) hwnd_raw: usize,
    pub(super) bounds: PillAnchorGeometry,
    user_data_dir: PathBuf,
    url: String,
    auth_token: Option<String>,
    security_policy: WebView2SecurityPolicy,
    pub(super) window_kind: LiveWindowKind,
    pub(super) transparent_bg: bool,
    pub(super) cursor_ignored: bool,
    pub(super) visible: bool,
    controller_ready: bool,
    open_latency_ms: f64,
    #[cfg(target_os = "windows")]
    com_uninit: bool,
    #[cfg(target_os = "windows")]
    loader_module: usize,
    #[cfg(target_os = "windows")]
    env_ptr: usize,
    #[cfg(target_os = "windows")]
    pub(super) ctrl_ptr: usize,
    #[cfg(target_os = "windows")]
    pub(super) web_ptr: usize,
    #[cfg(target_os = "windows")]
    msg_handler_ptr: usize,
    #[cfg(target_os = "windows")]
    perm_handler_ptr: usize,
    #[cfg(target_os = "windows")]
    cb_tx: Sender<win_com::ComCallbackSignal>,
    #[cfg(target_os = "windows")]
    cb_rx: Receiver<win_com::ComCallbackSignal>,
}

fn validate_live_navigation_target(
    uri: &str,
    policy: &WebView2SecurityPolicy,
) -> Result<(), UiError> {
    if let NavigationVerdict::AllowLocalOrigin { .. } = evaluate_navigation_target(uri, policy) {
        return Ok(());
    }
    let trimmed = uri.trim();
    let lower = trimmed.to_ascii_lowercase();
    let loopback_ok = (lower.starts_with("http://127.0.0.1:") || lower.starts_with("http://127.0.0.1/"))
        && !trimmed.contains('@')
        && !trimmed.contains('\\')
        && !trimmed.contains("..")
        && !trimmed.chars().any(|c| c == '\0' || c == '\r' || c == '\n');
    if loopback_ok {
        return Ok(());
    }
    Err(UiError::UnsafeNavigationBlocked(format!(
        "navigation target denied: {}",
        uri
    )))
}

impl LiveWebView2Window {
    pub fn open(config: LiveWebView2Config) -> Result<Self, UiError> {
        Self::open_with_kind(config, LiveWindowKind::FloatingIsland, "Miku")
    }

    pub fn open_dialog(config: LiveWebView2Config, title: &str) -> Result<Self, UiError> {
        Self::open_with_kind(config, LiveWindowKind::FramelessDialog, title)
    }

    fn open_with_kind(
        config: LiveWebView2Config,
        window_kind: LiveWindowKind,
        title: &str,
    ) -> Result<Self, UiError> {
        let t0 = Instant::now();
        config.security_policy.validate_release_hardening()?;
        validate_live_navigation_target(&config.url, &config.security_policy)?;
        let user_data_dir =
            resolve_isolated_webview2_user_data_dir(config.user_data_dir_override.as_deref())?;
        fs::create_dir_all(&user_data_dir)
            .map_err(|e| UiError::InvalidUserDataFolder(e.to_string()))?;

        #[cfg(target_os = "windows")]
        {
            let mut com_uninit = false;
            unsafe {
                let hr = win_ffi::CoInitializeEx(
                    std::ptr::null_mut(),
                    win_ffi::COINIT_APARTMENTTHREADED,
                );
                if hr == win_ffi::S_OK || hr == win_ffi::S_FALSE {
                    com_uninit = true;
                }
            }

            let (cb_tx, cb_rx) = mpsc::channel();
            let transparent_bg = match window_kind {
                LiveWindowKind::FloatingIsland => config.transparent_bg,
                LiveWindowKind::FramelessDialog => false,
            };
            let hwnd = match window_kind {
                LiveWindowKind::FloatingIsland => {
                    super::transparent_frame::create_live_pill_hwnd(config.bounds, transparent_bg)?
                }
                LiveWindowKind::FramelessDialog => {
                    super::transparent_frame::create_settings_dialog_hwnd(config.bounds, title)?
                }
            };
            let mut win = Self {
                hwnd_raw: hwnd as usize,
                bounds: config.bounds,
                user_data_dir,
                url: config.url,
                auth_token: config.auth_token,
                security_policy: config.security_policy,
                window_kind,
                transparent_bg,
                cursor_ignored: false,
                visible: true,
                controller_ready: false,
                open_latency_ms: 0.0,
                com_uninit,
                loader_module: 0,
                env_ptr: 0,
                ctrl_ptr: 0,
                web_ptr: 0,
                msg_handler_ptr: 0,
                perm_handler_ptr: 0,
                cb_tx,
                cb_rx,
            };
            win.start_webview2_environment()?;
            win.open_latency_ms = t0.elapsed().as_secs_f64() * 1_000.0;
            Ok(win)
        }

        #[cfg(not(target_os = "windows"))]
        {
            Ok(Self {
                hwnd_raw: 0x1001,
                bounds: config.bounds,
                user_data_dir,
                url: config.url,
                auth_token: config.auth_token,
                security_policy: config.security_policy,
                transparent_bg: config.transparent_bg,
                cursor_ignored: false,
                visible: true,
                controller_ready: true,
                open_latency_ms: t0.elapsed().as_secs_f64() * 1_000.0,
            })
        }
    }

    pub fn hwnd_raw(&self) -> usize {
        self.hwnd_raw
    }

    pub fn bounds(&self) -> PillAnchorGeometry {
        self.bounds
    }

    pub fn open_latency_ms(&self) -> f64 {
        self.open_latency_ms
    }

    pub fn is_controller_ready(&self) -> bool {
        self.controller_ready
    }

    pub fn user_data_dir(&self) -> &Path {
        &self.user_data_dir
    }

    #[cfg(target_os = "windows")]
    fn start_webview2_environment(&mut self) -> Result<(), UiError> {
        let probe = probe_edge_webview2_runtime();
        let user_data_w = win_ffi::wide_z(&self.user_data_dir.to_string_lossy());
        let env_cb = win_com::alloc_com_callback(
            win_com::CallbackKind::EnvCompleted,
            win_com::IID_ENV_COMPLETED,
            self.cb_tx.clone(),
        );

        unsafe {
            // 1. Try standard WebView2Loader.dll beside executable or system PATH
            let loader_w = win_ffi::wide_z("WebView2Loader.dll");
            let h_loader = win_ffi::LoadLibraryW(loader_w.as_ptr());
            if !h_loader.is_null() {
                let sym = b"CreateCoreWebView2EnvironmentWithOptions\0";
                let proc_ptr = win_ffi::GetProcAddress(h_loader, sym.as_ptr() as *const i8);
                if !proc_ptr.is_null() {
                    self.loader_module = h_loader as usize;
                    let func: win_com::CreateEnvWithOptionsFn = std::mem::transmute(proc_ptr);
                    let hr = func(
                        std::ptr::null(),
                        user_data_w.as_ptr(),
                        std::ptr::null_mut(),
                        env_cb,
                    );
                    win_com::com_release(env_cb);
                    if hr >= 0 {
                        return Ok(());
                    }
                    return Err(UiError::WebView2LifecycleError(format!(
                        "CreateCoreWebView2EnvironmentWithOptions failed HRESULT=0x{:08x}",
                        hr as u32
                    )));
                }
                let _ = win_ffi::FreeLibrary(h_loader);
            }

            // 2. Fallback to EdgeWebView2 runtime's own EmbeddedBrowserWebView.dll (zero sidecar DLLs needed)
            if let Some(ref dll_path) = probe.embedded_browser_dll_path {
                let dll_w = win_ffi::wide_z(dll_path);
                let h_emb = win_ffi::LoadLibraryExW(
                    dll_w.as_ptr(),
                    std::ptr::null_mut(),
                    win_ffi::LOAD_WITH_ALTERED_SEARCH_PATH,
                );
                if !h_emb.is_null() {
                    let sym = b"CreateWebViewEnvironmentWithOptionsInternal\0";
                    let proc_ptr = win_ffi::GetProcAddress(h_emb, sym.as_ptr() as *const i8);
                    if !proc_ptr.is_null() {
                        self.loader_module = h_emb as usize;
                        let func: win_com::CreateEnvInternalFn = std::mem::transmute(proc_ptr);
                        let hr = func(
                            1,
                            win_com::CORE_WEBVIEW2_DEFAULT_ENV_TYPE_EVERGREEN,
                            user_data_w.as_ptr(),
                            std::ptr::null_mut(),
                            env_cb,
                        );
                        win_com::com_release(env_cb);
                        if hr >= 0 {
                            return Ok(());
                        }
                        return Err(UiError::WebView2LifecycleError(format!(
                            "CreateWebViewEnvironmentWithOptionsInternal failed HRESULT=0x{:08x}",
                            hr as u32
                        )));
                    }
                    let _ = win_ffi::FreeLibrary(h_emb);
                }
            }
            win_com::com_release(env_cb);
        }

        Err(UiError::WebView2LifecycleError(
            "WebView2 Runtime loader unavailable".to_string(),
        ))
    }

    pub fn pump_events(&mut self) -> Vec<LiveWebView2Event> {
        let mut out = Vec::new();
        #[cfg(target_os = "windows")]
        {
            unsafe {
                let mut msg: win_ffi::Msg = std::mem::zeroed();
                while win_ffi::PeekMessageW(
                    &mut msg,
                    std::ptr::null_mut(),
                    0,
                    0,
                    win_ffi::PM_REMOVE,
                ) != 0
                {
                    let _ = win_ffi::TranslateMessage(&msg);
                    let _ = win_ffi::DispatchMessageW(&msg);
                }
            }

            while let Ok(sig) = self.cb_rx.try_recv() {
                match sig {
                    win_com::ComCallbackSignal::EnvReady(hr, ptr) => {
                        if hr < 0 || ptr == 0 {
                            out.push(LiveWebView2Event::ControllerError(format!(
                                "Environment init HRESULT=0x{:08x}",
                                hr as u32
                            )));
                            continue;
                        }
                        self.env_ptr = ptr;
                        let ctrl_cb = win_com::alloc_com_callback(
                            win_com::CallbackKind::CtrlCompleted,
                            win_com::IID_CTRL_COMPLETED,
                            self.cb_tx.clone(),
                        );
                        unsafe {
                            let call_hr = win_com::env_create_controller(
                                ptr as *mut std::ffi::c_void,
                                self.hwnd_raw as win_ffi::Hwnd,
                                ctrl_cb,
                            );
                            win_com::com_release(ctrl_cb);
                            if call_hr < 0 {
                                out.push(LiveWebView2Event::ControllerError(format!(
                                    "CreateCoreWebView2Controller HRESULT=0x{:08x}",
                                    call_hr as u32
                                )));
                            }
                        }
                    }
                    win_com::ComCallbackSignal::ControllerReady(hr, ptr) => {
                        if hr < 0 || ptr == 0 {
                            out.push(LiveWebView2Event::ControllerError(format!(
                                "Controller init HRESULT=0x{:08x}",
                                hr as u32
                            )));
                            continue;
                        }
                        self.ctrl_ptr = ptr;
                        unsafe {
                            let ctrl = ptr as *mut std::ffi::c_void;
                            if self.transparent_bg {
                                let bg_hr = win_com::ctrl2_put_default_bg(
                                    ctrl,
                                    win_com::ColorRgba {
                                        a: 0,
                                        r: 0,
                                        g: 0,
                                        b: 0,
                                    },
                                );
                                if bg_hr < 0 {
                                    out.push(LiveWebView2Event::ControllerError(format!(
                                        "ctrl2_put_default_bg HRESULT=0x{:08x}",
                                        bg_hr as u32
                                    )));
                                }
                            }
                            let rect = win_ffi::Rect {
                                left: 0,
                                top: 0,
                                right: self.bounds.width,
                                bottom: self.bounds.height,
                            };
                            let _ = win_com::ctrl_put_bounds(ctrl, &rect);
                            let _ = win_com::ctrl_put_visible(ctrl, self.visible);
                            if let Ok(web) = win_com::ctrl_get_core(ctrl) {
                                self.web_ptr = web as usize;
                                if let Some(settings) = win_com::web_get_settings(web) {
                                    win_com::settings_harden(
                                        settings,
                                        self.security_policy.are_devtools_enabled,
                                        self.security_policy.are_default_context_menus_enabled,
                                    );
                                    win_com::com_release(settings);
                                }
                                let msg_cb = win_com::alloc_com_callback(
                                    win_com::CallbackKind::WebMsgReceived,
                                    win_com::IID_WEB_MSG_RECEIVED,
                                    self.cb_tx.clone(),
                                );
                                self.msg_handler_ptr = msg_cb as usize;
                                let _ = win_com::web_add_message_received(web, msg_cb);
                                let perm_cb = win_com::alloc_com_callback(
                                    win_com::CallbackKind::PermissionRequested,
                                    win_com::IID_PERMISSION_REQUESTED,
                                    self.cb_tx.clone(),
                                );
                                self.perm_handler_ptr = perm_cb as usize;
                                let _ = win_com::web_add_permission_requested(web, perm_cb);
                                let _ = win_com::web_navigate(web, &self.url);
                                if let Some(ref tok) = self.auth_token {
                                    let auth_json = format!(
                                        "{{\"type\":\"auth\",\"token\":{}}}",
                                        serde_json::to_string(tok).unwrap_or_else(|_| "\"\"".into())
                                    );
                                    let _ = win_com::web_post_message_json(web, &auth_json);
                                }
                            }
                        }
                        self.controller_ready = true;
                        out.push(LiveWebView2Event::ControllerReady);
                    }
                    win_com::ComCallbackSignal::WebMessage(json) => {
                        out.push(LiveWebView2Event::WebMessageJson(json));
                    }
                    win_com::ComCallbackSignal::PermissionRequested(kind, state) => {
                        out.push(LiveWebView2Event::PermissionRequested { kind, state });
                    }
                }
            }
        }
        out
    }

    pub fn close_and_drain(mut self) -> Vec<u32> {
        self.teardown_internal(true);
        live_child_msedgewebview2_pids()
    }

    pub fn close_without_drain(mut self) {
        self.teardown_internal(false);
    }

    fn teardown_internal(&mut self, wait_for_children: bool) {
        let _ = wait_for_children;
        #[cfg(target_os = "windows")]
        unsafe {
            let had_live = self.web_ptr != 0
                || self.msg_handler_ptr != 0
                || self.perm_handler_ptr != 0
                || self.ctrl_ptr != 0
                || self.env_ptr != 0
                || self.hwnd_raw != 0
                || self.loader_module != 0
                || self.com_uninit;
            if !had_live {
                self.controller_ready = false;
                return;
            }
            if self.web_ptr != 0 {
                win_com::com_release(self.web_ptr as *mut std::ffi::c_void);
                self.web_ptr = 0;
            }
            if self.msg_handler_ptr != 0 {
                win_com::com_release(self.msg_handler_ptr as *mut std::ffi::c_void);
                self.msg_handler_ptr = 0;
            }
            if self.perm_handler_ptr != 0 {
                win_com::com_release(self.perm_handler_ptr as *mut std::ffi::c_void);
                self.perm_handler_ptr = 0;
            }
            if self.ctrl_ptr != 0 {
                let ctrl = self.ctrl_ptr as *mut std::ffi::c_void;
                let _ = win_com::ctrl_close(ctrl);
                win_com::com_release(ctrl);
                self.ctrl_ptr = 0;
            }
            if self.env_ptr != 0 {
                win_com::com_release(self.env_ptr as *mut std::ffi::c_void);
                self.env_ptr = 0;
            }
            if self.hwnd_raw != 0 {
                let _ = win_ffi::DestroyWindow(self.hwnd_raw as win_ffi::Hwnd);
                self.hwnd_raw = 0;
            }
            let mut msg: win_ffi::Msg = std::mem::zeroed();
            while win_ffi::PeekMessageW(&mut msg, std::ptr::null_mut(), 0, 0, win_ffi::PM_REMOVE) != 0
            {
                let _ = win_ffi::TranslateMessage(&msg);
                let _ = win_ffi::DispatchMessageW(&msg);
            }
            if wait_for_children {
                // Wait for Edge child processes to unwind deterministically while pumping STA messages
                let deadline = Instant::now() + Duration::from_millis(7_500);
                while !live_child_msedgewebview2_pids().is_empty() && Instant::now() < deadline {
                    while win_ffi::PeekMessageW(
                        &mut msg,
                        std::ptr::null_mut(),
                        0,
                        0,
                        win_ffi::PM_REMOVE,
                    ) != 0
                    {
                        let _ = win_ffi::TranslateMessage(&msg);
                        let _ = win_ffi::DispatchMessageW(&msg);
                    }
                    std::thread::sleep(Duration::from_millis(20));
                }
            }
            if self.loader_module != 0 {
                let _ = win_ffi::FreeLibrary(self.loader_module as win_ffi::Hmodule);
                self.loader_module = 0;
            }
            if self.com_uninit {
                win_ffi::CoUninitialize();
                self.com_uninit = false;
            }
        }
        self.controller_ready = false;
    }
}

impl Drop for LiveWebView2Window {
    fn drop(&mut self) {
        self.teardown_internal(true);
    }
}
