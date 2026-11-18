//! Native COM `UIAutomationCore.dll` bounded tree walker & `InvokePattern` executor (`COINIT_MULTITHREADED`).

use serde::{Deserialize, Serialize};

use crate::pc_control::backends::UiAutomationActionReceipt;
use crate::pc_control::win32_input::verify_input_target_safe;
use crate::PlatformError;

pub const MAX_UIA_DEPTH: usize = 3;
pub const MAX_UIA_CONTROLS: usize = 40;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct UiaControlNode {
    pub depth: usize,
    pub control_type: String,
    pub control_type_id: i32,
    pub name: String,
    pub automation_id: String,
    pub enabled: bool,
    pub rect: [i32; 4],
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct UiaTreeSnapshot {
    pub window_title: String,
    pub nodes: Vec<UiaControlNode>,
}

pub fn control_type_label(id: i32) -> &'static str {
    match id {
        50000 => "Button",
        50001 => "Calendar",
        50002 => "CheckBox",
        50003 => "ComboBox",
        50004 => "Edit",
        50005 => "Hyperlink",
        50006 => "Image",
        50007 => "ListItem",
        50008 => "List",
        50009 => "Menu",
        50010 => "MenuBar",
        50011 => "MenuItem",
        50018 => "Tab",
        50019 => "TabItem",
        50020 => "Text",
        50021 => "ToolBar",
        50025 => "Custom",
        50026 => "Group",
        50030 => "Document",
        50032 => "Window",
        50033 => "Pane",
        _ => "Control",
    }
}

#[cfg(windows)]
mod uia_com {
    use std::collections::VecDeque;
    use std::ffi::c_void;
    use std::ptr::null_mut;

    use super::*;

    type Hresult = i32;
    type Bstr = *mut u16;

    const COINIT_MULTITHREADED: u32 = 0x0;
    const RPC_E_CHANGED_MODE: Hresult = 0x8001_0106u32 as i32;
    const S_OK_HR: Hresult = 0;
    const S_FALSE_HR: Hresult = 1;
    const CLSCTX_INPROC_SERVER: u32 = 0x1;
    const UIA_INVOKE_PATTERN_ID: i32 = 10000;

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
    struct WinRect {
        left: i32,
        top: i32,
        right: i32,
        bottom: i32,
    }

    const fn guid(d1: u32, d2: u16, d3: u16, d4: [u8; 8]) -> Guid {
        Guid { data1: d1, data2: d2, data3: d3, data4: d4 }
    }

    const CLSID_CUI_AUTOMATION: Guid = guid(0xff48dba4, 0x60ef, 0x4201, [0xaa, 0x87, 0x54, 0x10, 0x3e, 0xef, 0x59, 0x4e]);
    const IID_IUI_AUTOMATION: Guid = guid(0x30cbe57d, 0xd9d0, 0x452a, [0xab, 0x13, 0x7a, 0xc5, 0xac, 0x48, 0x25, 0xee]);
    const IID_IUI_AUTOMATION_INVOKE_PATTERN: Guid = guid(0xfb377fbe, 0x8ea6, 0x46d5, [0x9c, 0x73, 0x64, 0x99, 0x64, 0x2d, 0x30, 0x59]);

    #[link(name = "ole32")]
    extern "system" {
        fn CoInitializeEx(pv_reserved: *mut c_void, dw_co_init: u32) -> Hresult;
        fn CoUninitialize();
        fn CoCreateInstance(
            rclsid: *const Guid,
            p_unk_outer: *mut c_void,
            dw_cls_context: u32,
            riid: *const Guid,
            ppv: *mut *mut c_void,
        ) -> Hresult;
    }

    #[link(name = "oleaut32")]
    extern "system" {
        fn SysFreeString(bstr: Bstr);
        fn SysStringLen(bstr: Bstr) -> u32;
    }

    #[link(name = "user32")]
    extern "system" {
        fn EnumWindows(func: unsafe extern "system" fn(*mut c_void, isize) -> i32, lparam: isize) -> i32;
        fn GetForegroundWindow() -> *mut c_void;
        fn GetWindowTextW(hwnd: *mut c_void, buf: *mut u16, max: i32) -> i32;
        fn IsWindowVisible(hwnd: *mut c_void) -> i32;
    }

    pub struct ComPtr(*mut c_void);
    impl ComPtr {
        fn null() -> Self { Self(null_mut()) }
        fn is_null(&self) -> bool { self.0.is_null() }
        fn clone_addref(&self) -> Self {
            if !self.0.is_null() {
                unsafe {
                    let vtbl = *(self.0 as *const *const *const c_void);
                    let addref: unsafe extern "system" fn(*mut c_void) -> u32 = std::mem::transmute(*vtbl.add(1));
                    let _ = addref(self.0);
                }
            }
            Self(self.0)
        }
    }
    impl Drop for ComPtr {
        fn drop(&mut self) {
            if !self.0.is_null() {
                unsafe {
                    let vtbl = *(self.0 as *const *const *const c_void);
                    let release: unsafe extern "system" fn(*mut c_void) -> u32 = std::mem::transmute(*vtbl.add(2));
                    let _ = release(self.0);
                }
                self.0 = null_mut();
            }
        }
    }

    struct ComMtaGuard(bool);
    impl ComMtaGuard {
        fn enter() -> Self {
            let hr = unsafe { CoInitializeEx(null_mut(), COINIT_MULTITHREADED) };
            let should_uninit = (hr == S_OK_HR || hr == S_FALSE_HR) && hr != RPC_E_CHANGED_MODE;
            Self(should_uninit)
        }
    }
    impl Drop for ComMtaGuard {
        fn drop(&mut self) { if self.0 { unsafe { CoUninitialize(); } } }
    }

    unsafe fn take_bstr(bstr: Bstr) -> String {
        if bstr.is_null() {
            return String::new();
        }
        let len = SysStringLen(bstr) as usize;
        let out = if len > 0 {
            String::from_utf16_lossy(std::slice::from_raw_parts(bstr, len))
        } else {
            String::new()
        };
        SysFreeString(bstr);
        out
    }

    unsafe fn find_target_hwnd(pattern: &str) -> (*mut c_void, String) {
        let fg = GetForegroundWindow();
        if pattern.trim().is_empty() && !fg.is_null() {
            let mut buf = [0u16; 256];
            let len = GetWindowTextW(fg, buf.as_mut_ptr(), 256);
            let title = if len > 0 { String::from_utf16_lossy(&buf[..len as usize]) } else { "Foreground".into() };
            return (fg, title);
        }
        struct Search<'a> { pat: &'a str, hwnd: *mut c_void, title: String }
        unsafe extern "system" fn cb(h: *mut c_void, lp: isize) -> i32 {
            let s = &mut *(lp as *mut Search);
            if IsWindowVisible(h) != 0 {
                let mut buf = [0u16; 256];
                let len = GetWindowTextW(h, buf.as_mut_ptr(), 256);
                if len > 0 {
                    let t = String::from_utf16_lossy(&buf[..len as usize]);
                    if s.pat.is_empty() || t.to_ascii_lowercase().contains(&s.pat.to_ascii_lowercase()) {
                        s.hwnd = h;
                        s.title = t;
                        return 0;
                    }
                }
            }
            1
        }
        let mut s = Search { pat: pattern.trim(), hwnd: null_mut(), title: String::new() };
        let _ = EnumWindows(cb, &mut s as *mut _ as isize);
        if s.hwnd.is_null() && !fg.is_null() {
            return (fg, pattern.to_string());
        }
        (s.hwnd, s.title)
    }

    unsafe fn create_uia_and_walker() -> Result<(ComPtr, ComPtr), PlatformError> {
        let mut auto = ComPtr::null();
        let hr = CoCreateInstance(&CLSID_CUI_AUTOMATION, null_mut(), CLSCTX_INPROC_SERVER, &IID_IUI_AUTOMATION, &mut auto.0);
        if hr < 0 || auto.is_null() {
            return Err(PlatformError::RegistryBackendError(format!("CoCreateInstance(CUIAutomation) failed: 0x{:08x}", hr)));
        }
        let mut walker = ComPtr::null();
        let vtbl = *(auto.0 as *const *const *const c_void);
        let get_walker: unsafe extern "system" fn(*mut c_void, *mut *mut c_void) -> Hresult = std::mem::transmute(*vtbl.add(14));
        let hr_w = get_walker(auto.0, &mut walker.0);
        if hr_w < 0 || walker.is_null() {
            return Err(PlatformError::RegistryBackendError(format!("get_ControlViewWalker failed: 0x{:08x}", hr_w)));
        }
        Ok((auto, walker))
    }

    unsafe fn read_element_meta(el: *mut c_void, depth: usize) -> UiaControlNode {
        let vtbl = *(el as *const *const *const c_void);
        let get_ct: unsafe extern "system" fn(*mut c_void, *mut i32) -> Hresult = std::mem::transmute(*vtbl.add(21));
        let get_name: unsafe extern "system" fn(*mut c_void, *mut Bstr) -> Hresult = std::mem::transmute(*vtbl.add(23));
        let get_en: unsafe extern "system" fn(*mut c_void, *mut i32) -> Hresult = std::mem::transmute(*vtbl.add(28));
        let get_aid: unsafe extern "system" fn(*mut c_void, *mut Bstr) -> Hresult = std::mem::transmute(*vtbl.add(29));
        let get_rect: unsafe extern "system" fn(*mut c_void, *mut WinRect) -> Hresult = std::mem::transmute(*vtbl.add(43));

        let mut ct_id = 0i32;
        let _ = get_ct(el, &mut ct_id);
        let mut b_name: Bstr = null_mut();
        let _ = get_name(el, &mut b_name);
        let mut b_aid: Bstr = null_mut();
        let _ = get_aid(el, &mut b_aid);
        let mut enabled = 0i32;
        let _ = get_en(el, &mut enabled);
        let mut r = WinRect::default();
        let _ = get_rect(el, &mut r);

        UiaControlNode {
            depth,
            control_type: control_type_label(ct_id).to_string(),
            control_type_id: ct_id,
            name: take_bstr(b_name),
            automation_id: take_bstr(b_aid),
            enabled: enabled != 0,
            rect: [r.left, r.top, r.right, r.bottom],
        }
    }

    pub fn walk_tree(window_title_pattern: &str, max_depth: usize, max_controls: usize) -> Result<(UiaTreeSnapshot, Vec<(UiaControlNode, ComPtr)>), PlatformError> {
        verify_input_target_safe()?;
        let _com = ComMtaGuard::enter();
        unsafe {
            let (hwnd, title) = find_target_hwnd(window_title_pattern);
            if hwnd.is_null() {
                return Ok((UiaTreeSnapshot { window_title: title, nodes: Vec::new() }, Vec::new()));
            }
            let (auto, walker) = create_uia_and_walker()?;
            let a_vtbl = *(auto.0 as *const *const *const c_void);
            let el_from_hwnd: unsafe extern "system" fn(*mut c_void, *mut c_void, *mut *mut c_void) -> Hresult = std::mem::transmute(*a_vtbl.add(6));
            let mut root = ComPtr::null();
            if el_from_hwnd(auto.0, hwnd, &mut root.0) < 0 || root.is_null() {
                return Ok((UiaTreeSnapshot { window_title: title, nodes: Vec::new() }, Vec::new()));
            }
            let w_vtbl = *(walker.0 as *const *const *const c_void);
            let first_child: unsafe extern "system" fn(*mut c_void, *mut c_void, *mut *mut c_void) -> Hresult = std::mem::transmute(*w_vtbl.add(4));
            let next_sib: unsafe extern "system" fn(*mut c_void, *mut c_void, *mut *mut c_void) -> Hresult = std::mem::transmute(*w_vtbl.add(6));

            let depth_cap = max_depth.clamp(1, MAX_UIA_DEPTH);
            let count_cap = max_controls.clamp(1, MAX_UIA_CONTROLS);
            let mut queue: VecDeque<(ComPtr, usize)> = VecDeque::new();
            queue.push_back((root, 0));
            let mut nodes = Vec::new();
            let mut handles = Vec::new();

            while let Some((el, depth)) = queue.pop_front() {
                if nodes.len() >= count_cap { break; }
                let meta = read_element_meta(el.0, depth);
                let el_clone = el.clone_addref();
                nodes.push(meta.clone());
                handles.push((meta, el_clone));
                if depth < depth_cap && nodes.len() < count_cap {
                    let mut child = ComPtr::null();
                    if first_child(walker.0, el.0, &mut child.0) >= 0 && !child.is_null() {
                        let mut cur = child;
                        while !cur.is_null() && queue.len() + nodes.len() < count_cap + 8 {
                            let next_el = cur.clone_addref();
                            queue.push_back((cur, depth + 1));
                            let mut sib = ComPtr::null();
                            if next_sib(walker.0, next_el.0, &mut sib.0) < 0 { break; }
                            cur = sib;
                        }
                    }
                }
            }
            Ok((UiaTreeSnapshot { window_title: title, nodes }, handles))
        }
    }

    pub fn invoke_target(window_title_pattern: &str, automation_id_or_name: &str, control_type: &str) -> Result<UiAutomationActionReceipt, PlatformError> {
        let needle = automation_id_or_name.trim().to_ascii_lowercase();
        let ctrl_l = control_type.trim().to_ascii_lowercase();
        let (snap, handles) = walk_tree(window_title_pattern, MAX_UIA_DEPTH, MAX_UIA_CONTROLS)?;
        if snap.nodes.is_empty() {
            return Ok(UiAutomationActionReceipt {
                used_ui_automation_tree: false,
                target_summary: format!("win32_uia_window_not_found:{}|{}|{}", window_title_pattern, automation_id_or_name, control_type),
            });
        }
        for (node, el) in handles {
            let id_match = !needle.is_empty() && node.automation_id.to_ascii_lowercase() == needle;
            let name_match = !needle.is_empty() && node.name.to_ascii_lowercase().contains(&needle);
            let type_ok = ctrl_l.is_empty() || node.control_type.to_ascii_lowercase() == ctrl_l;
            if (id_match || name_match) && (type_ok || id_match) {
                unsafe {
                    let e_vtbl = *(el.0 as *const *const *const c_void);
                    let get_pat: unsafe extern "system" fn(*mut c_void, i32, *mut *mut c_void) -> Hresult = std::mem::transmute(*e_vtbl.add(16));
                    let mut raw_pat = ComPtr::null();
                    if get_pat(el.0, UIA_INVOKE_PATTERN_ID, &mut raw_pat.0) >= 0 && !raw_pat.is_null() {
                        let p_vtbl = *(raw_pat.0 as *const *const *const c_void);
                        let qi: unsafe extern "system" fn(*mut c_void, *const Guid, *mut *mut c_void) -> Hresult = std::mem::transmute(*p_vtbl.add(0));
                        let mut inv = ComPtr::null();
                        if qi(raw_pat.0, &IID_IUI_AUTOMATION_INVOKE_PATTERN, &mut inv.0) >= 0 && !inv.is_null() {
                            let i_vtbl = *(inv.0 as *const *const *const c_void);
                            let invoke_fn: unsafe extern "system" fn(*mut c_void) -> Hresult = std::mem::transmute(*i_vtbl.add(3));
                            if invoke_fn(inv.0) >= 0 {
                                return Ok(UiAutomationActionReceipt {
                                    used_ui_automation_tree: true,
                                    target_summary: format!("win32_uia_invoked:{}|{}|{}|rect={:?}", snap.window_title, node.name, node.control_type, node.rect),
                                });
                            }
                        }
                    }
                    let set_focus: unsafe extern "system" fn(*mut c_void) -> Hresult = std::mem::transmute(*e_vtbl.add(3));
                    let _ = set_focus(el.0);
                    return Ok(UiAutomationActionReceipt {
                        used_ui_automation_tree: true,
                        target_summary: format!("win32_uia_focused_no_invoke:{}|{}|rect={:?}", snap.window_title, node.name, node.rect),
                    });
                }
            }
        }
        Ok(UiAutomationActionReceipt {
            used_ui_automation_tree: false,
            target_summary: format!("win32_uia_control_not_found:{}|{}|scanned={}", snap.window_title, automation_id_or_name, snap.nodes.len()),
        })
    }
}

pub fn inspect_foreground_uia_sync(window_title_pattern: &str, max_depth: usize, max_controls: usize) -> Result<UiaTreeSnapshot, PlatformError> {
    #[cfg(windows)]
    { uia_com::walk_tree(window_title_pattern, max_depth, max_controls).map(|(s, _)| s) }
    #[cfg(not(windows))]
    {
        let _ = (max_depth, max_controls);
        Ok(UiaTreeSnapshot { window_title: window_title_pattern.to_string(), nodes: Vec::new() })
    }
}

pub fn invoke_foreground_uia_sync(window_title_pattern: &str, automation_id_or_name: &str, control_type: &str) -> Result<UiAutomationActionReceipt, PlatformError> {
    #[cfg(windows)]
    { uia_com::invoke_target(window_title_pattern, automation_id_or_name, control_type) }
    #[cfg(not(windows))]
    Ok(UiAutomationActionReceipt {
        used_ui_automation_tree: true,
        target_summary: format!("simulated_uia_invoked:{}|{}|{}", window_title_pattern, automation_id_or_name, control_type),
    })
}
