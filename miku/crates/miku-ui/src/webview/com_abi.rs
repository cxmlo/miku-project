#![allow(dead_code)]
//! Minimal zero-dependency COM vtable ABI for Edge WebView2 (`ICoreWebView2Environment`,
//! `ICoreWebView2Controller`, `ICoreWebView2Controller2`, `ICoreWebView2`, `ICoreWebView2Settings`,
//! and STA callback boxes) on Windows x86_64.

#[cfg(target_os = "windows")]
pub(crate) mod win_com {
    use std::ffi::c_void;
    use std::ptr::null_mut;
    use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
    use std::sync::mpsc::Sender;

    use crate::webview::win32_com_ffi::win_ffi::{wide_z, Bool, Dword, Hresult, Hwnd, Rect, S_OK};

    pub const E_NOINTERFACE: Hresult = 0x8000_4002u32 as i32;
    pub const E_POINTER: Hresult = 0x8000_4003u32 as i32;
    pub const CORE_WEBVIEW2_DEFAULT_ENV_TYPE_EVERGREEN: i32 = 0;

    pub const COREWEBVIEW2_PERMISSION_KIND_MICROPHONE: u32 = 2;
    pub const COREWEBVIEW2_PERMISSION_STATE_ALLOW: u32 = 1;
    pub const COREWEBVIEW2_PERMISSION_STATE_DENY: u32 = 2;

    static MIC_PERMISSION_ALLOWED: AtomicBool = AtomicBool::new(false);

    pub fn set_mic_permission_allowed(allowed: bool) {
        MIC_PERMISSION_ALLOWED.store(allowed, Ordering::SeqCst);
    }

    pub fn is_mic_permission_allowed() -> bool {
        MIC_PERMISSION_ALLOWED.load(Ordering::SeqCst)
    }

    #[repr(C)]
    #[derive(Clone, Copy)]
    pub struct Guid {
        pub data1: u32,
        pub data2: u16,
        pub data3: u16,
        pub data4: [u8; 8],
    }

    impl Guid {
        pub const fn from_bytes(b: [u8; 16]) -> Self {
            Self {
                data1: u32::from_le_bytes([b[0], b[1], b[2], b[3]]),
                data2: u16::from_le_bytes([b[4], b[5]]),
                data3: u16::from_le_bytes([b[6], b[7]]),
                data4: [b[8], b[9], b[10], b[11], b[12], b[13], b[14], b[15]],
            }
        }

        pub fn eq(&self, other: &Self) -> bool {
            self.data1 == other.data1
                && self.data2 == other.data2
                && self.data3 == other.data3
                && self.data4 == other.data4
        }
    }

    pub const IID_IUNKNOWN: Guid =
        Guid::from_bytes([0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0xC0, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x46]);
    pub const IID_ENV_COMPLETED: Guid =
        Guid::from_bytes([0x89, 0x33, 0x8A, 0x4E, 0xD8, 0xC9, 0xD2, 0x4B, 0xB6, 0xB5, 0x12, 0x4F, 0xEE, 0x6C, 0xC1, 0x4D]);
    pub const IID_CTRL_COMPLETED: Guid =
        Guid::from_bytes([0xF3, 0x19, 0x48, 0x6C, 0xB7, 0xC9, 0x60, 0x42, 0x81, 0x27, 0xC9, 0xF5, 0xBD, 0xE7, 0xF6, 0x8C]);
    pub const IID_WEB_MSG_RECEIVED: Guid =
        Guid::from_bytes([0x19, 0x3F, 0x21, 0x57, 0xE6, 0x00, 0xFA, 0x49, 0x8E, 0x07, 0x89, 0x8E, 0xA0, 0x1E, 0xCB, 0xD2]);
    pub const IID_CONTROLLER2: Guid =
        Guid::from_bytes([0x3E, 0x90, 0x79, 0xC9, 0xCA, 0xD4, 0x28, 0x42, 0x92, 0xEB, 0x47, 0xEE, 0x3F, 0xA9, 0x6E, 0xAB]);
    pub const IID_CORE_WEBVIEW2_19: Guid =
        Guid::from_bytes([0x54, 0xF9, 0x21, 0x69, 0xB0, 0x79, 0x7F, 0x43, 0xA9, 0x97, 0xC8, 0x58, 0x11, 0x89, 0x7C, 0x68]);
    pub const IID_PERMISSION_REQUESTED: Guid =
        Guid::from_bytes([0xA3, 0xC6, 0xE1, 0x15, 0x2A, 0xC7, 0x40, 0x4D, 0x91, 0xAE, 0x36, 0x93, 0x4B, 0x67, 0x23, 0xD5]);

    #[repr(C)]
    #[derive(Clone, Copy)]
    pub struct ColorRgba {
        pub a: u8,
        pub r: u8,
        pub g: u8,
        pub b: u8,
    }

    #[repr(C)]
    pub struct IUnknownVtbl {
        pub query_interface:
            unsafe extern "system" fn(*mut c_void, *const Guid, *mut *mut c_void) -> Hresult,
        pub add_ref: unsafe extern "system" fn(*mut c_void) -> u32,
        pub release: unsafe extern "system" fn(*mut c_void) -> u32,
    }

    pub unsafe fn com_add_ref(ptr: *mut c_void) {
        if !ptr.is_null() {
            let vtbl = *(ptr as *const *const IUnknownVtbl);
            ((*vtbl).add_ref)(ptr);
        }
    }

    pub unsafe fn com_release(ptr: *mut c_void) {
        if !ptr.is_null() {
            let vtbl = *(ptr as *const *const IUnknownVtbl);
            ((*vtbl).release)(ptr);
        }
    }

    pub unsafe fn com_query_interface(
        ptr: *mut c_void,
        iid: &Guid,
    ) -> Option<*mut c_void> {
        if ptr.is_null() {
            return None;
        }
        let vtbl = *(ptr as *const *const IUnknownVtbl);
        let mut out: *mut c_void = null_mut();
        let hr = ((*vtbl).query_interface)(ptr, iid as *const Guid, &mut out);
        if hr >= 0 && !out.is_null() {
            Some(out)
        } else {
            None
        }
    }

    // ---- Environment / Controller vtables ----
    pub unsafe fn env_create_controller(
        env: *mut c_void,
        hwnd: Hwnd,
        handler: *mut c_void,
    ) -> Hresult {
        let vtbl = *(env as *const *const *const c_void);
        let f: unsafe extern "system" fn(*mut c_void, Hwnd, *mut c_void) -> Hresult =
            std::mem::transmute(*vtbl.add(3));
        f(env, hwnd, handler)
    }

    pub unsafe fn ctrl_put_visible(ctrl: *mut c_void, visible: bool) -> Hresult {
        let vtbl = *(ctrl as *const *const *const c_void);
        let f: unsafe extern "system" fn(*mut c_void, Bool) -> Hresult =
            std::mem::transmute(*vtbl.add(4));
        f(ctrl, if visible { 1 } else { 0 })
    }

    pub unsafe fn ctrl_put_bounds(ctrl: *mut c_void, rect: &Rect) -> Hresult {
        let vtbl = *(ctrl as *const *const *const c_void);
        #[cfg(target_arch = "x86_64")]
        {
            let f: unsafe extern "system" fn(*mut c_void, *const Rect) -> Hresult =
                std::mem::transmute(*vtbl.add(6));
            f(ctrl, rect as *const Rect)
        }
        #[cfg(not(target_arch = "x86_64"))]
        {
            let f: unsafe extern "system" fn(*mut c_void, Rect) -> Hresult =
                std::mem::transmute(*vtbl.add(6));
            f(ctrl, *rect)
        }
    }

    pub unsafe fn ctrl_close(ctrl: *mut c_void) -> Hresult {
        let vtbl = *(ctrl as *const *const *const c_void);
        let f: unsafe extern "system" fn(*mut c_void) -> Hresult =
            std::mem::transmute(*vtbl.add(24));
        f(ctrl)
    }

    pub unsafe fn ctrl_get_core(ctrl: *mut c_void) -> Result<*mut c_void, Hresult> {
        let vtbl = *(ctrl as *const *const *const c_void);
        let f: unsafe extern "system" fn(*mut c_void, *mut *mut c_void) -> Hresult =
            std::mem::transmute(*vtbl.add(25));
        let mut core = null_mut();
        let hr = f(ctrl, &mut core);
        if hr >= 0 && !core.is_null() {
            Ok(core)
        } else {
            Err(hr)
        }
    }

    pub unsafe fn ctrl2_put_default_bg(ctrl: *mut c_void, color: ColorRgba) -> Hresult {
        if let Some(c2) = com_query_interface(ctrl, &IID_CONTROLLER2) {
            let vtbl = *(c2 as *const *const *const c_void);
            let f: unsafe extern "system" fn(*mut c_void, ColorRgba) -> Hresult =
                std::mem::transmute(*vtbl.add(27));
            let hr = f(c2, color);
            com_release(c2);
            hr
        } else {
            E_NOINTERFACE
        }
    }

    pub unsafe fn web_get_settings(web: *mut c_void) -> Option<*mut c_void> {
        let vtbl = *(web as *const *const *const c_void);
        let f: unsafe extern "system" fn(*mut c_void, *mut *mut c_void) -> Hresult =
            std::mem::transmute(*vtbl.add(3));
        let mut settings = null_mut();
        if f(web, &mut settings) >= 0 && !settings.is_null() {
            Some(settings)
        } else {
            None
        }
    }

    pub unsafe fn settings_harden(
        settings: *mut c_void,
        allow_devtools: bool,
        allow_ctx_menu: bool,
    ) {
        let vtbl = *(settings as *const *const *const c_void);
        let put_bool = |slot: usize, v: bool| {
            let f: unsafe extern "system" fn(*mut c_void, Bool) -> Hresult =
                std::mem::transmute(*vtbl.add(slot));
            let _ = f(settings, if v { 1 } else { 0 });
        };
        put_bool(4, true); // IsScriptEnabled
        put_bool(6, true); // IsWebMessageEnabled
        put_bool(8, false); // AreDefaultScriptDialogsEnabled
        put_bool(10, false); // IsStatusBarEnabled
        put_bool(12, allow_devtools); // AreDevToolsEnabled
        put_bool(14, allow_ctx_menu); // AreDefaultContextMenusEnabled
    }

    pub unsafe fn web_navigate(web: *mut c_void, url: &str) -> Hresult {
        let vtbl = *(web as *const *const *const c_void);
        let f: unsafe extern "system" fn(*mut c_void, *const u16) -> Hresult =
            std::mem::transmute(*vtbl.add(5));
        let w = wide_z(url);
        f(web, w.as_ptr())
    }

    pub unsafe fn web_post_message_json(web: *mut c_void, json_str: &str) -> Hresult {
        let vtbl = *(web as *const *const *const c_void);
        let f: unsafe extern "system" fn(*mut c_void, *const u16) -> Hresult =
            std::mem::transmute(*vtbl.add(32));
        let w = wide_z(json_str);
        f(web, w.as_ptr())
    }

    pub unsafe fn web19_set_memory_usage_low(web: *mut c_void, low: bool) -> Hresult {
        if let Some(w19) = com_query_interface(web, &IID_CORE_WEBVIEW2_19) {
            let vtbl = *(w19 as *const *const *const c_void);
            // Slot 120 = ICoreWebView2_19::SetMemoryUsageTargetLevel (0=Normal, 1=Low)
            let f: unsafe extern "system" fn(*mut c_void, i32) -> Hresult =
                std::mem::transmute(*vtbl.add(120));
            let hr = f(w19, if low { 1 } else { 0 });
            com_release(w19);
            hr
        } else {
            E_NOINTERFACE
        }
    }

    pub unsafe fn web_add_message_received(
        web: *mut c_void,
        handler: *mut c_void,
    ) -> Result<i64, Hresult> {
        let vtbl = *(web as *const *const *const c_void);
        let f: unsafe extern "system" fn(*mut c_void, *mut c_void, *mut i64) -> Hresult =
            std::mem::transmute(*vtbl.add(34));
        let mut token = 0i64;
        let hr = f(web, handler, &mut token);
        if hr >= 0 {
            Ok(token)
        } else {
            Err(hr)
        }
    }

    pub unsafe fn web_add_permission_requested(
        web: *mut c_void,
        handler: *mut c_void,
    ) -> Result<i64, Hresult> {
        let vtbl = *(web as *const *const *const c_void);
        let f: unsafe extern "system" fn(*mut c_void, *mut c_void, *mut i64) -> Hresult =
            std::mem::transmute(*vtbl.add(23));
        let mut token = 0i64;
        let hr = f(web, handler, &mut token);
        if hr >= 0 {
            Ok(token)
        } else {
            Err(hr)
        }
    }

    pub unsafe fn args_get_web_message_json(args: *mut c_void) -> Option<String> {
        let vtbl = *(args as *const *const *const c_void);
        let f: unsafe extern "system" fn(*mut c_void, *mut *mut u16) -> Hresult =
            std::mem::transmute(*vtbl.add(4));
        let mut pwstr: *mut u16 = null_mut();
        let hr = f(args, &mut pwstr);
        if hr >= 0 && !pwstr.is_null() {
            let mut len = 0usize;
            while *pwstr.add(len) != 0 {
                len += 1;
            }
            let slice = std::slice::from_raw_parts(pwstr, len);
            let s = String::from_utf16_lossy(slice);
            crate::webview::win32_com_ffi::win_ffi::CoTaskMemFree(pwstr as *mut c_void);
            Some(s)
        } else {
            None
        }
    }

    // ---- Rust-backed COM Callback Box ----
    pub enum ComCallbackSignal {
        EnvReady(Hresult, usize),
        ControllerReady(Hresult, usize),
        WebMessage(String),
        PermissionRequested(u32, u32),
    }

    #[repr(C)]
    struct CallbackVtbl {
        qi: unsafe extern "system" fn(*mut c_void, *const Guid, *mut *mut c_void) -> Hresult,
        add_ref: unsafe extern "system" fn(*mut c_void) -> u32,
        release: unsafe extern "system" fn(*mut c_void) -> u32,
        invoke: unsafe extern "system" fn(*mut c_void, isize, *mut c_void) -> Hresult,
    }

    #[derive(Clone, Copy)]
    pub enum CallbackKind {
        EnvCompleted,
        CtrlCompleted,
        WebMsgReceived,
        PermissionRequested,
    }

    #[repr(C)]
    pub struct ComCallbackObject {
        vtbl: *const CallbackVtbl,
        refs: AtomicU32,
        iid: Guid,
        kind: CallbackKind,
        tx: Sender<ComCallbackSignal>,
    }

    static CB_VTBL: CallbackVtbl = CallbackVtbl {
        qi: cb_qi,
        add_ref: cb_add_ref,
        release: cb_release,
        invoke: cb_invoke,
    };

    unsafe extern "system" fn cb_qi(
        this: *mut c_void,
        riid: *const Guid,
        ppv: *mut *mut c_void,
    ) -> Hresult {
        if ppv.is_null() || riid.is_null() {
            return E_POINTER;
        }
        let obj = &*(this as *const ComCallbackObject);
        let req = &*riid;
        if req.eq(&IID_IUNKNOWN) || req.eq(&obj.iid) {
            *ppv = this;
            cb_add_ref(this);
            S_OK
        } else {
            *ppv = null_mut();
            E_NOINTERFACE
        }
    }

    unsafe extern "system" fn cb_add_ref(this: *mut c_void) -> u32 {
        let obj = &*(this as *const ComCallbackObject);
        obj.refs.fetch_add(1, Ordering::SeqCst) + 1
    }

    unsafe extern "system" fn cb_release(this: *mut c_void) -> u32 {
        let obj = &*(this as *const ComCallbackObject);
        let prev = obj.refs.fetch_sub(1, Ordering::SeqCst);
        if prev == 1 {
            drop(Box::from_raw(this as *mut ComCallbackObject));
            0
        } else {
            prev - 1
        }
    }

    unsafe extern "system" fn cb_invoke(
        this: *mut c_void,
        arg0: isize,
        arg1: *mut c_void,
    ) -> Hresult {
        let obj = &*(this as *const ComCallbackObject);
        match obj.kind {
            CallbackKind::EnvCompleted => {
                let hr = arg0 as Hresult;
                if hr >= 0 && !arg1.is_null() {
                    com_add_ref(arg1);
                }
                let _ = obj.tx.send(ComCallbackSignal::EnvReady(hr, arg1 as usize));
            }
            CallbackKind::CtrlCompleted => {
                let hr = arg0 as Hresult;
                if hr >= 0 && !arg1.is_null() {
                    com_add_ref(arg1);
                }
                let _ = obj
                    .tx
                    .send(ComCallbackSignal::ControllerReady(hr, arg1 as usize));
            }
            CallbackKind::WebMsgReceived => {
                if let Some(json) = args_get_web_message_json(arg1) {
                    let _ = obj.tx.send(ComCallbackSignal::WebMessage(json));
                }
            }
            CallbackKind::PermissionRequested => {
                let args = arg1;
                let mut kind = 0u32;
                let mut state = 0u32;
                if !args.is_null() {
                    let vtbl = *(args as *const *const *const c_void);
                    // Slot 4: get_PermissionKind(&mut kind)
                    let get_kind: unsafe extern "system" fn(*mut c_void, *mut u32) -> Hresult =
                        std::mem::transmute(*vtbl.add(4));
                    if get_kind(args, &mut kind) >= 0 && kind == COREWEBVIEW2_PERMISSION_KIND_MICROPHONE {
                        // Opt-In strict gate: allow microphone only when voice_stt_enabled == true.
                        let next_state = if is_mic_permission_allowed() {
                            COREWEBVIEW2_PERMISSION_STATE_ALLOW
                        } else {
                            COREWEBVIEW2_PERMISSION_STATE_DENY
                        };
                        let put_state: unsafe extern "system" fn(*mut c_void, u32) -> Hresult =
                            std::mem::transmute(*vtbl.add(7));
                        let _ = put_state(args, next_state);
                        state = next_state;
                    }
                }
                let _ = obj
                    .tx
                    .send(ComCallbackSignal::PermissionRequested(kind, state));
            }
        }
        S_OK
    }

    pub fn alloc_com_callback(
        kind: CallbackKind,
        iid: Guid,
        tx: Sender<ComCallbackSignal>,
    ) -> *mut c_void {
        Box::into_raw(Box::new(ComCallbackObject {
            vtbl: &CB_VTBL,
            refs: AtomicU32::new(1),
            iid,
            kind,
            tx,
        })) as *mut c_void
    }

    pub type CreateEnvWithOptionsFn = unsafe extern "system" fn(
        browser_exe_folder: *const u16,
        user_data_folder: *const u16,
        env_options: *mut c_void,
        handler: *mut c_void,
    ) -> Hresult;

    pub type CreateEnvInternalFn = unsafe extern "system" fn(
        check_thread_affinity: Dword,
        runtime_type: i32,
        user_data_folder: *const u16,
        env_options: *mut c_void,
        handler: *mut c_void,
    ) -> Hresult;
}
