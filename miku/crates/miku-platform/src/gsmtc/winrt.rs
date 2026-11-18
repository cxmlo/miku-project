//! Windows WinRT COM ABI vtable bindings for GSMTC (dynamically loaded from `combase.dll`).

use super::{sanitize_media_snapshot, GsmtcControlAction, GsmtcNowPlaying};
use std::ffi::c_void;
use std::ptr::{null, null_mut};
use std::sync::OnceLock;
use std::thread;
use std::time::{Duration, Instant};

type HResult = i32;
type HString = *mut c_void;

const S_OK: HResult = 0;
const S_FALSE: HResult = 1;
const RPC_E_CHANGED_MODE: HResult = 0x8001_0106u32 as i32;
const RO_INIT_MULTITHREADED: u32 = 1;
const ASYNC_COMPLETED: u32 = 1;
const TICKS_PER_SEC: i64 = 10_000_000;

#[repr(C)]
#[derive(Clone, Copy)]
struct Guid {
    data1: u32,
    data2: u16,
    data3: u16,
    data4: [u8; 8],
}

#[repr(C)]
struct HStringHeader {
    _reserved: [u64; 3],
}

// Verified against Windows SDK 10.0.26100.0 asyncinfo.h & windows.media.control.h
const IID_IASYNC_INFO: Guid = Guid {
    data1: 0x0000_0036,
    data2: 0x0000,
    data3: 0x0000,
    data4: [0xC0, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x46],
};
const IID_MANAGER_STATICS: Guid = Guid {
    data1: 0x2050_c4ee,
    data2: 0x11a0,
    data3: 0x57de,
    data4: [0xae, 0xd7, 0xc9, 0x7c, 0x70, 0x33, 0x82, 0x45],
};

#[link(name = "kernel32")]
extern "system" {
    fn LoadLibraryW(lp_lib_file_name: *const u16) -> *mut c_void;
    fn GetProcAddress(h_module: *mut c_void, lp_proc_name: *const u8) -> *mut c_void;
}

type FnRoInitialize = unsafe extern "system" fn(u32) -> HResult;
type FnRoUninitialize = unsafe extern "system" fn();
type FnRoGetActivationFactory =
    unsafe extern "system" fn(HString, *const Guid, *mut *mut c_void) -> HResult;
type FnWinCreateStrRef =
    unsafe extern "system" fn(*const u16, u32, *mut HStringHeader, *mut HString) -> HResult;
type FnWinGetStrBuf = unsafe extern "system" fn(HString, *mut u32) -> *const u16;
type FnWinDelStr = unsafe extern "system" fn(HString) -> HResult;

#[derive(Clone, Copy)]
struct CombaseApi {
    ro_init: FnRoInitialize,
    ro_uninit: FnRoUninitialize,
    ro_get_factory: FnRoGetActivationFactory,
    win_str_ref: FnWinCreateStrRef,
    win_str_buf: FnWinGetStrBuf,
    win_str_del: FnWinDelStr,
}

static COMBASE: OnceLock<Option<CombaseApi>> = OnceLock::new();

fn load_combase() -> Option<CombaseApi> {
    *COMBASE.get_or_init(|| unsafe {
        let wide: Vec<u16> = "combase.dll\0".encode_utf16().collect();
        let module = LoadLibraryW(wide.as_ptr());
        if module.is_null() {
            return None;
        }
        let sym = |name: &[u8]| -> Option<*mut c_void> {
            let p = GetProcAddress(module, name.as_ptr());
            if p.is_null() {
                None
            } else {
                Some(p)
            }
        };
        Some(CombaseApi {
            ro_init: std::mem::transmute::<*mut c_void, FnRoInitialize>(sym(
                b"RoInitialize\0",
            )?),
            ro_uninit: std::mem::transmute::<*mut c_void, FnRoUninitialize>(sym(
                b"RoUninitialize\0",
            )?),
            ro_get_factory: std::mem::transmute::<*mut c_void, FnRoGetActivationFactory>(sym(
                b"RoGetActivationFactory\0",
            )?),
            win_str_ref: std::mem::transmute::<*mut c_void, FnWinCreateStrRef>(sym(
                b"WindowsCreateStringReference\0",
            )?),
            win_str_buf: std::mem::transmute::<*mut c_void, FnWinGetStrBuf>(sym(
                b"WindowsGetStringRawBuffer\0",
            )?),
            win_str_del: std::mem::transmute::<*mut c_void, FnWinDelStr>(sym(
                b"WindowsDeleteString\0",
            )?),
        })
    })
}

struct RoApartment {
    api: CombaseApi,
    should_uninit: bool,
}

impl RoApartment {
    fn enter(api: CombaseApi) -> Result<Self, HResult> {
        let hr = unsafe { (api.ro_init)(RO_INIT_MULTITHREADED) };
        if hr == S_OK || hr == S_FALSE {
            Ok(Self {
                api,
                should_uninit: true,
            })
        } else if hr == RPC_E_CHANGED_MODE {
            Ok(Self {
                api,
                should_uninit: false,
            })
        } else {
            Err(hr)
        }
    }
}

impl Drop for RoApartment {
    fn drop(&mut self) {
        if self.should_uninit {
            unsafe { (self.api.ro_uninit)() };
        }
    }
}

struct ComPtr(*mut c_void);

impl ComPtr {
    fn wrap(ptr: *mut c_void) -> Option<Self> {
        if ptr.is_null() {
            None
        } else {
            Some(Self(ptr))
        }
    }

    fn as_ptr(&self) -> *mut c_void {
        self.0
    }

    unsafe fn vslot(&self, idx: usize) -> *const c_void {
        let vtbl = *(self.0 as *const *const *const c_void);
        *vtbl.add(idx)
    }

    unsafe fn query_interface(&self, iid: &Guid) -> Option<ComPtr> {
        type FnQi = unsafe extern "system" fn(*mut c_void, *const Guid, *mut *mut c_void) -> HResult;
        let f: FnQi = std::mem::transmute(self.vslot(0));
        let mut out: *mut c_void = null_mut();
        let hr = f(self.0, iid as *const Guid, &mut out);
        if hr >= 0 {
            ComPtr::wrap(out)
        } else {
            None
        }
    }
}

impl Drop for ComPtr {
    fn drop(&mut self) {
        if !self.0.is_null() {
            unsafe {
                type FnRelease = unsafe extern "system" fn(*mut c_void) -> u32;
                let f: FnRelease = std::mem::transmute(self.vslot(2));
                f(self.0);
            }
        }
    }
}

fn hstring_take(api: CombaseApi, hstr: HString) -> String {
    if hstr.is_null() {
        return String::new();
    }
    unsafe {
        let mut len: u32 = 0;
        let buf = (api.win_str_buf)(hstr, &mut len);
        let out = if !buf.is_null() && len > 0 {
            let slice = std::slice::from_raw_parts(buf, len as usize);
            String::from_utf16_lossy(slice)
        } else {
            String::new()
        };
        let _ = (api.win_str_del)(hstr);
        out
    }
}

unsafe fn wait_async_completed(op: &ComPtr, timeout_ms: u64) -> bool {
    let Some(info) = op.query_interface(&IID_IASYNC_INFO) else {
        return false;
    };
    type FnGetStatus = unsafe extern "system" fn(*mut c_void, *mut u32) -> HResult;
    type FnVoidCall = unsafe extern "system" fn(*mut c_void) -> HResult;
    let get_status: FnGetStatus = std::mem::transmute(info.vslot(7));
    let cancel: FnVoidCall = std::mem::transmute(info.vslot(9));
    let close: FnVoidCall = std::mem::transmute(info.vslot(10));

    let deadline = Instant::now() + Duration::from_millis(timeout_ms);
    loop {
        let mut status: u32 = 0;
        if get_status(info.as_ptr(), &mut status) < 0 {
            let _ = close(info.as_ptr());
            return false;
        }
        if status == ASYNC_COMPLETED {
            return true;
        }
        if status > ASYNC_COMPLETED || Instant::now() >= deadline {
            let _ = cancel(info.as_ptr());
            let _ = close(info.as_ptr());
            return false;
        }
        thread::sleep(Duration::from_millis(3));
    }
}

unsafe fn await_ptr_op(op: &ComPtr, timeout_ms: u64) -> Option<ComPtr> {
    if !wait_async_completed(op, timeout_ms) {
        return None;
    }
    type FnGetResultsPtr = unsafe extern "system" fn(*mut c_void, *mut *mut c_void) -> HResult;
    let get_results: FnGetResultsPtr = std::mem::transmute(op.vslot(8));
    let mut out: *mut c_void = null_mut();
    if get_results(op.as_ptr(), &mut out) >= 0 {
        ComPtr::wrap(out)
    } else {
        None
    }
}

unsafe fn await_bool_op(op: &ComPtr, timeout_ms: u64) -> bool {
    if !wait_async_completed(op, timeout_ms) {
        return false;
    }
    type FnGetResultsBool = unsafe extern "system" fn(*mut c_void, *mut u8) -> HResult;
    let get_results: FnGetResultsBool = std::mem::transmute(op.vslot(8));
    let mut out: u8 = 0;
    get_results(op.as_ptr(), &mut out) >= 0 && out != 0
}

unsafe fn acquire_current_session(api: CombaseApi, timeout_ms: u64) -> Option<ComPtr> {
    let mut wide: Vec<u16> =
        "Windows.Media.Control.GlobalSystemMediaTransportControlsSessionManager"
            .encode_utf16()
            .collect();
    let chars_len = wide.len() as u32;
    wide.push(0);

    let mut header = HStringHeader { _reserved: [0; 3] };
    let mut hclass: HString = null_mut();
    if (api.win_str_ref)(wide.as_ptr(), chars_len, &mut header, &mut hclass) < 0 {
        return None;
    }

    let mut statics_raw: *mut c_void = null_mut();
    if (api.ro_get_factory)(hclass, &IID_MANAGER_STATICS, &mut statics_raw) < 0 {
        return None;
    }
    let statics = ComPtr::wrap(statics_raw)?;

    // IGlobalSystemMediaTransportControlsSessionManagerStatics::RequestAsync -> slot 6
    type FnRequestAsync = unsafe extern "system" fn(*mut c_void, *mut *mut c_void) -> HResult;
    let request_async: FnRequestAsync = std::mem::transmute(statics.vslot(6));
    let mut op_raw: *mut c_void = null_mut();
    if request_async(statics.as_ptr(), &mut op_raw) < 0 {
        return None;
    }
    let op = ComPtr::wrap(op_raw)?;
    let manager = await_ptr_op(&op, timeout_ms)?;

    // IGlobalSystemMediaTransportControlsSessionManager::GetCurrentSession -> slot 6
    type FnGetCurrentSession =
        unsafe extern "system" fn(*mut c_void, *mut *mut c_void) -> HResult;
    let get_current: FnGetCurrentSession = std::mem::transmute(manager.vslot(6));
    let mut session_raw: *mut c_void = null_mut();
    if get_current(manager.as_ptr(), &mut session_raw) < 0 {
        return None;
    }
    ComPtr::wrap(session_raw)
}

pub(super) fn query_now_playing_winrt(timeout_ms: u64) -> GsmtcNowPlaying {
    let Some(api) = load_combase() else {
        return GsmtcNowPlaying::fallback("fallback");
    };
    let Ok(_apt) = RoApartment::enter(api) else {
        return GsmtcNowPlaying::fallback("fallback");
    };

    unsafe {
        let Some(session) = acquire_current_session(api, timeout_ms) else {
            return GsmtcNowPlaying::fallback("winrt-gsmtc");
        };

        type FnGetHstr = unsafe extern "system" fn(*mut c_void, *mut HString) -> HResult;
        type FnGetPtr = unsafe extern "system" fn(*mut c_void, *mut *mut c_void) -> HResult;
        type FnGetI32 = unsafe extern "system" fn(*mut c_void, *mut i32) -> HResult;
        type FnGetI64 = unsafe extern "system" fn(*mut c_void, *mut i64) -> HResult;

        let get_app: FnGetHstr = std::mem::transmute(session.vslot(6));
        let mut app_hstr: HString = null_mut();
        let app_id = if get_app(session.as_ptr(), &mut app_hstr) >= 0 {
            hstring_take(api, app_hstr)
        } else {
            String::new()
        };

        // PlaybackInfo -> slot 9, get_PlaybackStatus -> slot 7
        let get_pb: FnGetPtr = std::mem::transmute(session.vslot(9));
        let mut pb_raw: *mut c_void = null_mut();
        let status_code = if get_pb(session.as_ptr(), &mut pb_raw) >= 0 {
            if let Some(pb) = ComPtr::wrap(pb_raw) {
                let get_status: FnGetI32 = std::mem::transmute(pb.vslot(7));
                let mut st: i32 = 0;
                if get_status(pb.as_ptr(), &mut st) >= 0 {
                    st
                } else {
                    0
                }
            } else {
                0
            }
        } else {
            0
        };

        // TimelineProperties -> slot 8: get_EndTime (slot 7), get_Position (slot 10)
        let get_tl: FnGetPtr = std::mem::transmute(session.vslot(8));
        let mut tl_raw: *mut c_void = null_mut();
        let (mut duration_sec, mut position_sec) = (0u64, 0u64);
        if get_tl(session.as_ptr(), &mut tl_raw) >= 0 {
            if let Some(tl) = ComPtr::wrap(tl_raw) {
                let get_end: FnGetI64 = std::mem::transmute(tl.vslot(7));
                let get_pos: FnGetI64 = std::mem::transmute(tl.vslot(10));
                let mut end_ticks: i64 = 0;
                let mut pos_ticks: i64 = 0;
                if get_end(tl.as_ptr(), &mut end_ticks) >= 0 && end_ticks > 0 {
                    duration_sec = (end_ticks / TICKS_PER_SEC) as u64;
                }
                if get_pos(tl.as_ptr(), &mut pos_ticks) >= 0 && pos_ticks > 0 {
                    position_sec = (pos_ticks / TICKS_PER_SEC) as u64;
                }
            }
        }

        // TryGetMediaPropertiesAsync -> slot 7
        let try_props: FnGetPtr = std::mem::transmute(session.vslot(7));
        let mut props_op_raw: *mut c_void = null_mut();
        let (mut title, mut artist, mut album) = (String::new(), String::new(), String::new());
        if try_props(session.as_ptr(), &mut props_op_raw) >= 0 {
            if let Some(props_op) = ComPtr::wrap(props_op_raw) {
                if let Some(props) = await_ptr_op(&props_op, timeout_ms) {
                    let get_title: FnGetHstr = std::mem::transmute(props.vslot(6));
                    let get_artist: FnGetHstr = std::mem::transmute(props.vslot(9));
                    let get_album: FnGetHstr = std::mem::transmute(props.vslot(10));

                    let mut h_title: HString = null_mut();
                    if get_title(props.as_ptr(), &mut h_title) >= 0 {
                        title = hstring_take(api, h_title);
                    }
                    let mut h_artist: HString = null_mut();
                    if get_artist(props.as_ptr(), &mut h_artist) >= 0 {
                        artist = hstring_take(api, h_artist);
                    }
                    let mut h_album: HString = null_mut();
                    if get_album(props.as_ptr(), &mut h_album) >= 0 {
                        album = hstring_take(api, h_album);
                    }
                }
            }
        }

        let _ = null::<c_void>();
        sanitize_media_snapshot(
            &title,
            &artist,
            &album,
            &app_id,
            status_code,
            duration_sec,
            position_sec,
            true,
            "winrt-gsmtc",
        )
    }
}

pub(super) fn send_control_action_winrt(action: GsmtcControlAction, timeout_ms: u64) -> bool {
    let Some(api) = load_combase() else {
        return false;
    };
    let Ok(_apt) = RoApartment::enter(api) else {
        return false;
    };

    // Slot indices verified from IGlobalSystemMediaTransportControlsSessionVtbl
    let slot = match action {
        GsmtcControlAction::Play => 10,
        GsmtcControlAction::Pause => 11,
        GsmtcControlAction::Next => 16,
        GsmtcControlAction::Previous => 17,
        GsmtcControlAction::TogglePlayPause => 20,
    };

    unsafe {
        let Some(session) = acquire_current_session(api, timeout_ms) else {
            return false;
        };
        type FnTryAction = unsafe extern "system" fn(*mut c_void, *mut *mut c_void) -> HResult;
        let invoke: FnTryAction = std::mem::transmute(session.vslot(slot));
        let mut op_raw: *mut c_void = null_mut();
        if invoke(session.as_ptr(), &mut op_raw) < 0 {
            return false;
        }
        let Some(op) = ComPtr::wrap(op_raw) else {
            return false;
        };
        await_bool_op(&op, timeout_ms)
    }
}
