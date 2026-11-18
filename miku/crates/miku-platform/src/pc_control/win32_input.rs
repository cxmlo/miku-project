//! Genuine Win32 `SendInput` dispatch, sensitive-window safety guard, and RAII modifier release.

use std::sync::{Arc, Mutex};

use crate::pc_control::coords::virtual_to_sendinput_abs;
use crate::PlatformError;

pub const VK_SHIFT: u16 = 0x10;
pub const VK_CONTROL: u16 = 0x11;
pub const VK_MENU: u16 = 0x12; // Alt
pub const VK_LWIN: u16 = 0x5B;
pub const VK_RWIN: u16 = 0x5C;
pub const KEYEVENTF_KEYUP: u32 = 0x0002;
pub const KEYEVENTF_UNICODE: u32 = 0x0004;

const INPUT_MOUSE: u32 = 0;
const INPUT_KEYBOARD: u32 = 1;
const MOUSEEVENTF_MOVE: u32 = 0x0001;
const MOUSEEVENTF_LEFTDOWN: u32 = 0x0002;
const MOUSEEVENTF_LEFTUP: u32 = 0x0004;
const MOUSEEVENTF_RIGHTDOWN: u32 = 0x0008;
const MOUSEEVENTF_RIGHTUP: u32 = 0x0010;
const MOUSEEVENTF_VIRTUALDESK: u32 = 0x4000;
const MOUSEEVENTF_ABSOLUTE: u32 = 0x8000;

#[cfg(windows)]
mod win32_ffi {
    use std::ffi::c_void;
    use std::path::Path;

    const PROCESS_QUERY_LIMITED_INFORMATION: u32 = 0x1000;
    const DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2: isize = -4;

    #[repr(C)]
    #[derive(Clone, Copy)]
    pub struct MouseInput {
        pub dx: i32,
        pub dy: i32,
        pub mouse_data: u32,
        pub dw_flags: u32,
        pub time: u32,
        pub dw_extra_info: usize,
    }

    #[repr(C)]
    #[derive(Clone, Copy)]
    pub struct KeybdInput {
        pub w_vk: u16,
        pub w_scan: u16,
        pub dw_flags: u32,
        pub time: u32,
        pub dw_extra_info: usize,
    }

    #[repr(C)]
    #[derive(Clone, Copy)]
    pub union InputUnion {
        pub mi: MouseInput,
        pub ki: KeybdInput,
        pub padding: [u8; 32],
    }

    #[repr(C)]
    #[derive(Clone, Copy)]
    pub struct WinInput {
        pub r#type: u32,
        pub u: InputUnion,
    }

    extern "system" {
        pub fn SendInput(c_inputs: u32, p_inputs: *const WinInput, cb_size: i32) -> u32;
        fn GetForegroundWindow() -> *mut c_void;
        fn GetWindowTextW(hwnd: *mut c_void, lp_string: *mut u16, max_count: i32) -> i32;
        fn GetClassNameW(hwnd: *mut c_void, lp_class_name: *mut u16, max_count: i32) -> i32;
        fn GetWindowThreadProcessId(hwnd: *mut c_void, lpdw_process_id: *mut u32) -> u32;
        fn LoadLibraryA(name: *const u8) -> *mut c_void;
        fn GetModuleHandleA(name: *const u8) -> *mut c_void;
        fn GetProcAddress(hmod: *mut c_void, name: *const u8) -> *mut c_void;
        fn OpenProcess(access: u32, inherit: i32, pid: u32) -> *mut c_void;
        fn CloseHandle(handle: *mut c_void) -> i32;
        fn QueryFullProcessImageNameW(
            h_proc: *mut c_void,
            flags: u32,
            buf: *mut u16,
            size: *mut u32,
        ) -> i32;
    }

    pub fn enter_per_monitor_dpi_v2() -> Option<(
        *mut c_void,
        unsafe extern "system" fn(*mut c_void) -> *mut c_void,
    )> {
        unsafe {
            let mut hmod = GetModuleHandleA(c"user32.dll".as_ptr() as *const u8);
            if hmod.is_null() {
                hmod = LoadLibraryA(c"user32.dll".as_ptr() as *const u8);
            }
            if hmod.is_null() {
                return None;
            }
            let sym = GetProcAddress(hmod, c"SetThreadDpiAwarenessContext".as_ptr() as *const u8);
            if sym.is_null() {
                return None;
            }
            let f: unsafe extern "system" fn(*mut c_void) -> *mut c_void =
                std::mem::transmute(sym);
            let prev = f(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2 as *mut c_void);
            Some((prev, f))
        }
    }

    pub fn foreground_window_meta() -> (String, String, String) {
        unsafe {
            let fg = GetForegroundWindow();
            if fg.is_null() {
                return (String::new(), String::new(), String::new());
            }
            let mut title_buf = [0u16; 256];
            let tlen = GetWindowTextW(fg, title_buf.as_mut_ptr(), title_buf.len() as i32);
            let title = if tlen > 0 {
                String::from_utf16_lossy(&title_buf[..tlen as usize])
            } else {
                String::new()
            };

            let mut cls_buf = [0u16; 128];
            let clen = GetClassNameW(fg, cls_buf.as_mut_ptr(), cls_buf.len() as i32);
            let cls = if clen > 0 {
                String::from_utf16_lossy(&cls_buf[..clen as usize])
            } else {
                String::new()
            };

            let mut pid = 0u32;
            let _ = GetWindowThreadProcessId(fg, &mut pid);
            let exe = if pid > 0 {
                let hp = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, pid);
                if !hp.is_null() {
                    let mut pbuf = [0u16; 260];
                    let mut psz = pbuf.len() as u32;
                    let ok = QueryFullProcessImageNameW(hp, 0, pbuf.as_mut_ptr(), &mut psz);
                    let _ = CloseHandle(hp);
                    if ok != 0 && psz > 0 {
                        let s = String::from_utf16_lossy(&pbuf[..psz as usize]);
                        Path::new(&s)
                            .file_name()
                            .and_then(|v| v.to_str())
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
            (title, cls, exe)
        }
    }
}

pub fn is_sensitive_window_metadata(title: &str, class_name: &str, exe_name: &str) -> bool {
    let combined = format!(
        "{} {} {}",
        title.to_ascii_lowercase(),
        class_name.to_ascii_lowercase(),
        exe_name.to_ascii_lowercase()
    );
    const SENSITIVE: &[&str] = &[
        "credentialuiclass",
        "credential dialog xaml host",
        "lockscreencontroller",
        "consent.exe",
        "taskmgr.exe",
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
    SENSITIVE.iter().any(|pat| combined.contains(pat))
}

/// Blocks input automation when a credential dialog, UAC prompt, or password manager has focus.
pub fn verify_input_target_safe() -> Result<(), PlatformError> {
    #[cfg(windows)]
    {
        let (title, class_name, exe_name) = win32_ffi::foreground_window_meta();
        if is_sensitive_window_metadata(&title, &class_name, &exe_name) {
            return Err(PlatformError::PermissionDenied(format!(
                "input blocked on sensitive window (title='{}', class='{}', exe='{}')",
                title, class_name, exe_name
            )));
        }
    }
    Ok(())
}

#[cfg(windows)]
fn dispatch_kbd(w_vk: u16, w_scan: u16, dw_flags: u32) {
    let input = win32_ffi::WinInput {
        r#type: INPUT_KEYBOARD,
        u: win32_ffi::InputUnion {
            ki: win32_ffi::KeybdInput {
                w_vk,
                w_scan,
                dw_flags,
                time: 0,
                dw_extra_info: 0,
            },
        },
    };
    unsafe {
        let _ = win32_ffi::SendInput(1, &input, std::mem::size_of::<win32_ffi::WinInput>() as i32);
    }
}

pub fn send_virtual_key_down(vk: u16) {
    #[cfg(windows)]
    dispatch_kbd(vk, 0, 0);
    #[cfg(not(windows))]
    let _ = vk;
}

pub fn send_virtual_key_up(vk: u16) {
    #[cfg(windows)]
    dispatch_kbd(vk, 0, KEYEVENTF_KEYUP);
    #[cfg(not(windows))]
    let _ = vk;
}

pub fn send_virtual_key_tap(vk: u16) {
    send_virtual_key_down(vk);
    send_virtual_key_up(vk);
}

pub fn parse_modifier_vk(name: &str) -> Option<u16> {
    match name.trim().to_ascii_lowercase().as_str() {
        "ctrl" | "control" | "lctrl" | "rctrl" => Some(VK_CONTROL),
        "alt" | "menu" | "lalt" | "ralt" => Some(VK_MENU),
        "shift" | "lshift" | "rshift" => Some(VK_SHIFT),
        "win" | "windows" | "super" | "meta" | "lwin" => Some(VK_LWIN),
        "rwin" => Some(VK_RWIN),
        _ => None,
    }
}

pub fn parse_virtual_key(name: &str) -> Option<u16> {
    if let Some(m) = parse_modifier_vk(name) {
        return Some(m);
    }
    let clean = name.trim().to_ascii_lowercase();
    if clean.len() == 1 {
        let ch = clean.as_bytes()[0];
        if ch.is_ascii_lowercase() {
            return Some((ch - b'a' + b'A') as u16);
        }
        if ch.is_ascii_digit() {
            return Some(ch as u16);
        }
    }
    match clean.as_str() {
        "enter" | "return" => Some(0x0D),
        "tab" => Some(0x09),
        "esc" | "escape" => Some(0x1B),
        "space" | "espacio" => Some(0x20),
        "backspace" | "back" => Some(0x08),
        "delete" | "del" => Some(0x2E),
        "insert" | "ins" => Some(0x2D),
        "home" => Some(0x24),
        "end" => Some(0x23),
        "pageup" | "pgup" => Some(0x21),
        "pagedown" | "pgdn" => Some(0x22),
        "left" => Some(0x25),
        "up" => Some(0x26),
        "right" => Some(0x27),
        "down" => Some(0x28),
        "f1" => Some(0x70),
        "f2" => Some(0x71),
        "f3" => Some(0x72),
        "f4" => Some(0x73),
        "f5" => Some(0x74),
        "f6" => Some(0x75),
        "f7" => Some(0x76),
        "f8" => Some(0x77),
        "f9" => Some(0x78),
        "f10" => Some(0x79),
        "f11" => Some(0x7A),
        "f12" => Some(0x7B),
        _ => None,
    }
}

pub fn send_unicode_text(text: &str) -> Result<usize, PlatformError> {
    verify_input_target_safe()?;
    #[cfg(windows)]
    {
        let mut count = 0usize;
        for ch in text.encode_utf16() {
            if ch == b'\n' as u16 {
                send_virtual_key_tap(0x0D);
            } else if ch == b'\t' as u16 {
                send_virtual_key_tap(0x09);
            } else {
                dispatch_kbd(0, ch, KEYEVENTF_UNICODE);
                dispatch_kbd(0, ch, KEYEVENTF_UNICODE | KEYEVENTF_KEYUP);
            }
            count += 1;
        }
        Ok(count)
    }
    #[cfg(not(windows))]
    Ok(text.len())
}

/// Converts virtual desktop `(vx, vy)` into Per-Monitor DPI Aware `0..65535` SendInput coordinates and clicks.
pub fn send_mouse_absolute_click(vx: i32, vy: i32, right_click: bool) -> Result<(i32, i32), PlatformError> {
    verify_input_target_safe()?;
    #[cfg(windows)]
    {
        let prev = win32_ffi::enter_per_monitor_dpi_v2();
        let (abs_x, abs_y) = virtual_to_sendinput_abs(vx, vy);
        let (down_flag, up_flag) = if right_click {
            (MOUSEEVENTF_RIGHTDOWN, MOUSEEVENTF_RIGHTUP)
        } else {
            (MOUSEEVENTF_LEFTDOWN, MOUSEEVENTF_LEFTUP)
        };
        let events = [
            win32_ffi::WinInput {
                r#type: INPUT_MOUSE,
                u: win32_ffi::InputUnion {
                    mi: win32_ffi::MouseInput {
                        dx: abs_x,
                        dy: abs_y,
                        mouse_data: 0,
                        dw_flags: MOUSEEVENTF_MOVE | MOUSEEVENTF_ABSOLUTE | MOUSEEVENTF_VIRTUALDESK,
                        time: 0,
                        dw_extra_info: 0,
                    },
                },
            },
            win32_ffi::WinInput {
                r#type: INPUT_MOUSE,
                u: win32_ffi::InputUnion {
                    mi: win32_ffi::MouseInput {
                        dx: abs_x,
                        dy: abs_y,
                        mouse_data: 0,
                        dw_flags: down_flag | MOUSEEVENTF_ABSOLUTE | MOUSEEVENTF_VIRTUALDESK,
                        time: 0,
                        dw_extra_info: 0,
                    },
                },
            },
            win32_ffi::WinInput {
                r#type: INPUT_MOUSE,
                u: win32_ffi::InputUnion {
                    mi: win32_ffi::MouseInput {
                        dx: abs_x,
                        dy: abs_y,
                        mouse_data: 0,
                        dw_flags: up_flag | MOUSEEVENTF_ABSOLUTE | MOUSEEVENTF_VIRTUALDESK,
                        time: 0,
                        dw_extra_info: 0,
                    },
                },
            },
        ];
        unsafe {
            let _ = win32_ffi::SendInput(
                events.len() as u32,
                events.as_ptr(),
                std::mem::size_of::<win32_ffi::WinInput>() as i32,
            );
            if let Some((prev_ctx, restore)) = prev {
                if !prev_ctx.is_null() {
                    let _ = restore(prev_ctx);
                }
            }
        }
        Ok((abs_x, abs_y))
    }
    #[cfg(not(windows))]
    Ok(virtual_to_sendinput_abs(vx, vy))
}

/// RAII Guard that records and tracks held modifier keys (`Ctrl`, `Alt`, `Shift`, `Win`).
/// When dropped (on task finish, error, panic, or Kill Switch abort), dispatches `KEYEVENTF_KEYUP`
/// for all pressed modifiers in reverse order so modifiers never stay stuck.
#[derive(Debug)]
pub struct KeyComboGuard {
    modifiers_pressed: Vec<u16>,
    active: bool,
    test_release_sink: Option<Arc<Mutex<Vec<u16>>>>,
}

impl KeyComboGuard {
    pub fn new(modifiers: Vec<u16>) -> Self {
        Self { modifiers_pressed: modifiers, active: true, test_release_sink: None }
    }

    pub fn from_key_names<I, S>(names: I) -> Self
    where
        I: IntoIterator<Item = S>,
        S: AsRef<str>,
    {
        let mut modifiers = Vec::new();
        for name in names {
            if let Some(vk) = parse_modifier_vk(name.as_ref()) {
                if !modifiers.contains(&vk) {
                    modifiers.push(vk);
                }
            }
        }
        Self::new(modifiers)
    }

    pub fn set_test_release_sink(&mut self, sink: Arc<Mutex<Vec<u16>>>) {
        self.test_release_sink = Some(sink);
    }

    pub fn press_modifiers(&self) {
        if !self.active {
            return;
        }
        for &vk in &self.modifiers_pressed {
            send_virtual_key_down(vk);
        }
    }

    pub fn release_all(&mut self) {
        if !self.active {
            return;
        }
        self.active = false;
        for &vk in self.modifiers_pressed.iter().rev() {
            if let Some(sink) = &self.test_release_sink {
                if let Ok(mut g) = sink.lock() {
                    g.push(vk);
                }
            }
            send_virtual_key_up(vk);
        }
    }

    pub fn modifiers(&self) -> &[u16] { &self.modifiers_pressed }
    pub fn is_empty(&self) -> bool { self.modifiers_pressed.is_empty() }
}

impl Drop for KeyComboGuard {
    fn drop(&mut self) { self.release_all(); }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_key_combo_guard_releases_modifiers_in_reverse_on_drop() {
        let sink = Arc::new(Mutex::new(Vec::new()));
        {
            let mut guard = KeyComboGuard::from_key_names(["ctrl", "alt", "shift", "k"]);
            guard.set_test_release_sink(Arc::clone(&sink));
            assert_eq!(guard.modifiers(), &[VK_CONTROL, VK_MENU, VK_SHIFT]);
        }
        assert_eq!(sink.lock().unwrap().clone(), vec![VK_SHIFT, VK_MENU, VK_CONTROL]);
    }

    #[test]
    fn test_key_combo_guard_handles_windows_key() {
        let sink = Arc::new(Mutex::new(Vec::new()));
        {
            let mut guard = KeyComboGuard::from_key_names(["win", "r"]);
            guard.set_test_release_sink(Arc::clone(&sink));
            assert_eq!(guard.modifiers(), &[VK_LWIN]);
        }
        assert_eq!(*sink.lock().unwrap(), vec![VK_LWIN]);
    }
}
