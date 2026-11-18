//! In-memory Windows.Media.Ocr WinRT COM bridge (zero temporary disk files).

use crate::vision::ocr::{OcrLine, OcrResult, OcrWord};
use crate::win32::UiError;

#[cfg(windows)]
#[allow(clippy::missing_transmute_annotations)]
mod winrt {
    use std::ffi::c_void;
    use std::ptr::null_mut;

    use super::*;

    type Hresult = i32;
    type Hstring = *mut c_void;

    const RO_INIT_MULTITHREADED: u32 = 1;
    const RPC_E_CHANGED_MODE: Hresult = 0x8001_0106u32 as i32;
    const S_FALSE_HR: Hresult = 1;
    const BITMAP_PIXEL_FORMAT_RGBA8: i32 = 30;
    const BITMAP_ALPHA_STRAIGHT: i32 = 1;

    #[repr(C)]
    #[derive(Clone, Copy)]
    struct Guid {
        data1: u32,
        data2: u16,
        data3: u16,
        data4: [u8; 8],
    }

    #[repr(C)]
    #[derive(Default, Clone, Copy)]
    struct WinRtRect {
        x: f32,
        y: f32,
        width: f32,
        height: f32,
    }

    const fn guid(d1: u32, d2: u16, d3: u16, d4: [u8; 8]) -> Guid {
        Guid { data1: d1, data2: d2, data3: d3, data4: d4 }
    }

    const IID_ICRYPTO_BUFFER_STATICS: Guid = guid(0x320b7e22, 0x3cb0, 0x4cdf, [0x86, 0x63, 0x1d, 0x28, 0x91, 0x00, 0x65, 0xeb]);
    const IID_ISOFTWARE_BITMAP_STATICS: Guid = guid(0xdf0385db, 0x672f, 0x4a9d, [0x80, 0x6e, 0xc2, 0x44, 0x2f, 0x34, 0x3e, 0x86]);
    const IID_IOCR_ENGINE_STATICS: Guid = guid(0x5bffa85a, 0x3384, 0x3540, [0x99, 0x40, 0x69, 0x91, 0x20, 0xd4, 0x28, 0xa8]);
    const IID_IASYNC_INFO: Guid = guid(0x00000036, 0x0000, 0x0000, [0xc0, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x46]);

    #[link(name = "kernel32")]
    unsafe extern "system" {
        fn LoadLibraryA(name: *const i8) -> *mut c_void;
        fn GetProcAddress(hmod: *mut c_void, name: *const i8) -> *mut c_void;
        fn Sleep(ms: u32);
    }

    struct ComPtr(*mut c_void);
    impl ComPtr {
        fn null() -> Self { Self(null_mut()) }
        fn is_null(&self) -> bool { self.0.is_null() }
    }
    impl Drop for ComPtr {
        fn drop(&mut self) {
            if !self.0.is_null() {
                unsafe {
                    let vtbl = *(self.0 as *const *const *const c_void);
                    let release: unsafe extern "system" fn(*mut c_void) -> u32 =
                        std::mem::transmute(*vtbl.add(2));
                    let _ = release(self.0);
                }
                self.0 = null_mut();
            }
        }
    }

    struct WinRtFns {
        ro_init: unsafe extern "system" fn(u32) -> Hresult,
        ro_uninit: unsafe extern "system" fn(),
        ro_factory: unsafe extern "system" fn(Hstring, *const Guid, *mut *mut c_void) -> Hresult,
        hstr_create: unsafe extern "system" fn(*const u16, u32, *mut Hstring) -> Hresult,
        hstr_delete: unsafe extern "system" fn(Hstring) -> Hresult,
        hstr_buf: unsafe extern "system" fn(Hstring, *mut u32) -> *const u16,
    }

    unsafe fn load_winrt() -> Result<WinRtFns, UiError> {
        let combase = LoadLibraryA(c"combase.dll".as_ptr());
        let winstr = LoadLibraryA(c"api-ms-win-core-winrt-string-l1-1-0.dll".as_ptr());
        if combase.is_null() || winstr.is_null() {
            return Err(UiError::VisionCaptureError("WinRT runtime DLLs unavailable".into()));
        }
        let s1 = GetProcAddress(combase, c"RoInitialize".as_ptr());
        let s2 = GetProcAddress(combase, c"RoUninitialize".as_ptr());
        let s3 = GetProcAddress(combase, c"RoGetActivationFactory".as_ptr());
        let s4 = GetProcAddress(winstr, c"WindowsCreateString".as_ptr());
        let s5 = GetProcAddress(winstr, c"WindowsDeleteString".as_ptr());
        let s6 = GetProcAddress(winstr, c"WindowsGetStringRawBuffer".as_ptr());
        if s1.is_null() || s2.is_null() || s3.is_null() || s4.is_null() || s5.is_null() || s6.is_null() {
            return Err(UiError::VisionCaptureError("WinRT symbols missing".into()));
        }
        Ok(WinRtFns {
            ro_init: std::mem::transmute(s1),
            ro_uninit: std::mem::transmute(s2),
            ro_factory: std::mem::transmute(s3),
            hstr_create: std::mem::transmute(s4),
            hstr_delete: std::mem::transmute(s5),
            hstr_buf: std::mem::transmute(s6),
        })
    }

    unsafe fn make_hstring(fns: &WinRtFns, s: &str) -> Result<Hstring, UiError> {
        let utf16: Vec<u16> = s.encode_utf16().collect();
        let mut hs: Hstring = null_mut();
        let hr = (fns.hstr_create)(utf16.as_ptr(), utf16.len() as u32, &mut hs);
        if hr < 0 {
            return Err(UiError::VisionCaptureError(format!("WindowsCreateString failed: 0x{:08x}", hr)));
        }
        Ok(hs)
    }

    unsafe fn read_hstring(fns: &WinRtFns, hs: Hstring) -> String {
        if hs.is_null() {
            return String::new();
        }
        let mut len = 0u32;
        let ptr = (fns.hstr_buf)(hs, &mut len);
        let out = if ptr.is_null() || len == 0 {
            String::new()
        } else {
            String::from_utf16_lossy(std::slice::from_raw_parts(ptr, len as usize))
        };
        let _ = (fns.hstr_delete)(hs);
        out
    }

    unsafe fn get_factory(fns: &WinRtFns, class_id: &str, iid: &Guid) -> Result<ComPtr, UiError> {
        let hs = make_hstring(fns, class_id)?;
        let mut out: *mut c_void = null_mut();
        let hr = (fns.ro_factory)(hs, iid, &mut out);
        let _ = (fns.hstr_delete)(hs);
        if hr < 0 || out.is_null() {
            return Err(UiError::VisionCaptureError(format!("RoGetActivationFactory({}) failed: 0x{:08x}", class_id, hr)));
        }
        Ok(ComPtr(out))
    }

    pub fn try_recognize_rgba(width: u32, height: u32, rgba: &[u8]) -> Result<OcrResult, UiError> {
        let expected = (width as usize).saturating_mul(height as usize).saturating_mul(4);
        if width == 0 || height == 0 || rgba.len() < expected {
            return Err(UiError::VisionCaptureError("invalid RGBA buffer for Windows OCR".into()));
        }
        unsafe {
            let fns = load_winrt()?;
            let init_hr = (fns.ro_init)(RO_INIT_MULTITHREADED);
            let should_uninit = init_hr == 0 || init_hr == S_FALSE_HR;
            if init_hr < 0 && init_hr != RPC_E_CHANGED_MODE {
                return Err(UiError::VisionCaptureError(format!("RoInitialize failed: 0x{:08x}", init_hr)));
            }
            struct RoGuard(bool, unsafe extern "system" fn());
            impl Drop for RoGuard { fn drop(&mut self) { if self.0 { unsafe { (self.1)(); } } } }
            let _ro_guard = RoGuard(should_uninit, fns.ro_uninit);

            let crypto = get_factory(&fns, "Windows.Security.Cryptography.CryptographicBuffer", &IID_ICRYPTO_BUFFER_STATICS)?;
            let mut ibuf = ComPtr::null();
            let crypto_vtbl = *(crypto.0 as *const *const *const c_void);
            let create_from_bytes: unsafe extern "system" fn(*mut c_void, u32, *const u8, *mut *mut c_void) -> Hresult =
                std::mem::transmute(*crypto_vtbl.add(9));
            let hr = create_from_bytes(crypto.0, expected as u32, rgba.as_ptr(), &mut ibuf.0);
            if hr < 0 || ibuf.is_null() {
                return Err(UiError::VisionCaptureError(format!("CreateFromByteArray failed: 0x{:08x}", hr)));
            }

            let bmp_stat = get_factory(&fns, "Windows.Graphics.Imaging.SoftwareBitmap", &IID_ISOFTWARE_BITMAP_STATICS)?;
            let mut sw_bmp = ComPtr::null();
            let bmp_vtbl = *(bmp_stat.0 as *const *const *const c_void);
            let create_copy: unsafe extern "system" fn(*mut c_void, *mut c_void, i32, i32, i32, i32, *mut *mut c_void) -> Hresult =
                std::mem::transmute(*bmp_vtbl.add(10));
            let hr = create_copy(bmp_stat.0, ibuf.0, BITMAP_PIXEL_FORMAT_RGBA8, width as i32, height as i32, BITMAP_ALPHA_STRAIGHT, &mut sw_bmp.0);
            if hr < 0 || sw_bmp.is_null() {
                return Err(UiError::VisionCaptureError(format!("SoftwareBitmap failed: 0x{:08x}", hr)));
            }

            let ocr_stat = get_factory(&fns, "Windows.Media.Ocr.OcrEngine", &IID_IOCR_ENGINE_STATICS)?;
            let mut ocr_engine = ComPtr::null();
            let ocr_stat_vtbl = *(ocr_stat.0 as *const *const *const c_void);
            let try_create: unsafe extern "system" fn(*mut c_void, *mut *mut c_void) -> Hresult =
                std::mem::transmute(*ocr_stat_vtbl.add(10));
            let hr = try_create(ocr_stat.0, &mut ocr_engine.0);
            if hr < 0 || ocr_engine.is_null() {
                return Err(UiError::VisionCaptureError("No Windows OCR language pack active".into()));
            }

            let mut async_op = ComPtr::null();
            let eng_vtbl = *(ocr_engine.0 as *const *const *const c_void);
            let recognize_async: unsafe extern "system" fn(*mut c_void, *mut c_void, *mut *mut c_void) -> Hresult =
                std::mem::transmute(*eng_vtbl.add(6));
            let hr = recognize_async(ocr_engine.0, sw_bmp.0, &mut async_op.0);
            if hr < 0 || async_op.is_null() {
                return Err(UiError::VisionCaptureError(format!("RecognizeAsync failed: 0x{:08x}", hr)));
            }

            let mut async_info = ComPtr::null();
            let op_vtbl = *(async_op.0 as *const *const *const c_void);
            let qi: unsafe extern "system" fn(*mut c_void, *const Guid, *mut *mut c_void) -> Hresult =
                std::mem::transmute(*op_vtbl.add(0));
            if qi(async_op.0, &IID_IASYNC_INFO, &mut async_info.0) >= 0 && !async_info.is_null() {
                let info_vtbl = *(async_info.0 as *const *const *const c_void);
                let get_status: unsafe extern "system" fn(*mut c_void, *mut i32) -> Hresult =
                    std::mem::transmute(*info_vtbl.add(7));
                for _ in 0..750 {
                    let mut status = 0i32;
                    if get_status(async_info.0, &mut status) < 0 || status != 0 {
                        break;
                    }
                    Sleep(2);
                }
            }

            let mut ocr_res = ComPtr::null();
            let get_results: unsafe extern "system" fn(*mut c_void, *mut *mut c_void) -> Hresult =
                std::mem::transmute(*op_vtbl.add(8));
            let hr = get_results(async_op.0, &mut ocr_res.0);
            if hr < 0 || ocr_res.is_null() {
                return Err(UiError::VisionCaptureError(format!("OCR GetResults failed: 0x{:08x}", hr)));
            }

            let res_vtbl = *(ocr_res.0 as *const *const *const c_void);
            let get_text: unsafe extern "system" fn(*mut c_void, *mut Hstring) -> Hresult =
                std::mem::transmute(*res_vtbl.add(8));
            let mut full_hs: Hstring = null_mut();
            let _ = get_text(ocr_res.0, &mut full_hs);
            let full_text = read_hstring(&fns, full_hs);

            let mut lines_vec = ComPtr::null();
            let get_lines: unsafe extern "system" fn(*mut c_void, *mut *mut c_void) -> Hresult =
                std::mem::transmute(*res_vtbl.add(6));
            let mut lines = Vec::new();
            if get_lines(ocr_res.0, &mut lines_vec.0) >= 0 && !lines_vec.is_null() {
                let lv_vtbl = *(lines_vec.0 as *const *const *const c_void);
                let l_size_fn: unsafe extern "system" fn(*mut c_void, *mut u32) -> Hresult = std::mem::transmute(*lv_vtbl.add(7));
                let l_get_at: unsafe extern "system" fn(*mut c_void, u32, *mut *mut c_void) -> Hresult = std::mem::transmute(*lv_vtbl.add(6));
                let mut l_count = 0u32;
                let _ = l_size_fn(lines_vec.0, &mut l_count);
                for li in 0..l_count {
                    let mut line_ptr = ComPtr::null();
                    if l_get_at(lines_vec.0, li, &mut line_ptr.0) < 0 || line_ptr.is_null() { continue; }
                    let ln_vtbl = *(line_ptr.0 as *const *const *const c_void);
                    let ln_text_fn: unsafe extern "system" fn(*mut c_void, *mut Hstring) -> Hresult = std::mem::transmute(*ln_vtbl.add(7));
                    let mut ln_hs: Hstring = null_mut();
                    let _ = ln_text_fn(line_ptr.0, &mut ln_hs);
                    let line_text = read_hstring(&fns, ln_hs);
                    let mut words = Vec::new();
                    let mut w_vec = ComPtr::null();
                    let ln_words_fn: unsafe extern "system" fn(*mut c_void, *mut *mut c_void) -> Hresult = std::mem::transmute(*ln_vtbl.add(6));
                    if ln_words_fn(line_ptr.0, &mut w_vec.0) >= 0 && !w_vec.is_null() {
                        let wv_vtbl = *(w_vec.0 as *const *const *const c_void);
                        let w_size_fn: unsafe extern "system" fn(*mut c_void, *mut u32) -> Hresult = std::mem::transmute(*wv_vtbl.add(7));
                        let w_get_at: unsafe extern "system" fn(*mut c_void, u32, *mut *mut c_void) -> Hresult = std::mem::transmute(*wv_vtbl.add(6));
                        let mut w_count = 0u32;
                        let _ = w_size_fn(w_vec.0, &mut w_count);
                        for wi in 0..w_count {
                            let mut wp = ComPtr::null();
                            if w_get_at(w_vec.0, wi, &mut wp.0) < 0 || wp.is_null() { continue; }
                            let w_vtbl = *(wp.0 as *const *const *const c_void);
                            let w_rect_fn: unsafe extern "system" fn(*mut c_void, *mut WinRtRect) -> Hresult = std::mem::transmute(*w_vtbl.add(6));
                            let w_text_fn: unsafe extern "system" fn(*mut c_void, *mut Hstring) -> Hresult = std::mem::transmute(*w_vtbl.add(7));
                            let mut r = WinRtRect::default();
                            let _ = w_rect_fn(wp.0, &mut r);
                            let mut whs: Hstring = null_mut();
                            let _ = w_text_fn(wp.0, &mut whs);
                            words.push(OcrWord {
                                text: read_hstring(&fns, whs),
                                x: r.x.round() as i32,
                                y: r.y.round() as i32,
                                width: r.width.max(0.0).round() as u32,
                                height: r.height.max(0.0).round() as u32,
                                confidence: 0.96,
                            });
                        }
                    }
                    lines.push(OcrLine { text: line_text, words });
                }
            }
            Ok(OcrResult { text: full_text, lines, language: "winrt-profile".to_string() })
        }
    }
}

pub fn try_recognize_rgba_winrt(width: u32, height: u32, rgba: &[u8]) -> Result<OcrResult, UiError> {
    #[cfg(windows)]
    { winrt::try_recognize_rgba(width, height, rgba) }
    #[cfg(not(windows))]
    {
        let _ = (width, height, rgba);
        Err(UiError::VisionCaptureError("Windows.Media.Ocr requires Windows".into()))
    }
}
