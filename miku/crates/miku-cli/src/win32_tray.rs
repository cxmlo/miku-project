//! Win32 System Tray (`NOTIFYICONDATAW` + popup menu) and global hotkeys (`Alt+Space`,
//! `Ctrl+Alt+Shift+K`) for the Step 6.5 resident Miku desktop daemon.

use std::sync::mpsc::{self, Receiver, Sender};

use miku_ui::{
    DEMO_PTT_HOTKEY_ID, KILL_SWITCH_HOTKEY_ID, KILL_SWITCH_WIN32_MODIFIERS,
    KILL_SWITCH_WIN32_VK_K, WIN32_MOD_ALT, WIN32_MOD_NOREPEAT, WIN32_VK_SPACE,
};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TrayRegistrationReport {
    pub tray_icon_active: bool,
    pub ptt_hotkey_registered: bool,
    pub kill_switch_hotkey_registered: bool,
    pub warnings: Vec<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TrayDaemonAction {
    ToggleIsland,
    OpenCustomUiFolder,
    OpenSetup,
    HotkeyPttWake,
    ToggleKillSwitch,
    RunDoctor,
    Exit,
}

#[cfg(target_os = "windows")]
mod win_tray_impl {
    use super::*;
    use std::ffi::c_void;

    const IDM_TOGGLE_ISLAND: usize = 1001;
    const IDM_OPEN_CUSTOM_UI: usize = 1002;
    const IDM_OPEN_SETUP: usize = 1003;
    const IDM_TOGGLE_KILL_SWITCH: usize = 1004;
    const IDM_RUN_DOCTOR: usize = 1005;
    const IDM_EXIT: usize = 1006;

    const WM_CREATE: u32 = 0x0001;
    const WM_DESTROY: u32 = 0x0002;
    const WM_CLOSE: u32 = 0x0010;
    const WM_HOTKEY: u32 = 0x0312;
    const WM_LBUTTONUP: u32 = 0x0202;
    const WM_LBUTTONDBLCLK: u32 = 0x0203;
    const WM_RBUTTONUP: u32 = 0x0205;
    const WM_CONTEXTMENU: u32 = 0x007B;
    const WM_MIKU_TRAY: u32 = 0x8001;

    const WS_POPUP: u32 = 0x8000_0000;
    const WS_EX_TOOLWINDOW: u32 = 0x0000_0080;
    const GWLP_USERDATA: i32 = -21;
    const PM_REMOVE: u32 = 0x0001;
    const MF_STRING: u32 = 0x0000_0000;
    const MF_SEPARATOR: u32 = 0x0000_0800;
    const TPM_RIGHTALIGN: u32 = 0x0008;
    const TPM_BOTTOMALIGN: u32 = 0x0020;
    const TPM_RETURNCMD: u32 = 0x0100;
    const TPM_RIGHTBUTTON: u32 = 0x0002;
    const NIM_ADD: u32 = 0x0000_0000;
    const NIM_DELETE: u32 = 0x0000_0002;
    const NIF_MESSAGE: u32 = 0x0000_0001;
    const NIF_ICON: u32 = 0x0000_0002;
    const NIF_TIP: u32 = 0x0000_0004;
    const IDI_APPLICATION: *const u16 = 32512usize as *const u16;

    #[repr(C)]
    #[derive(Clone, Copy)]
    struct Point {
        x: i32,
        y: i32,
    }

    #[repr(C)]
    #[derive(Clone, Copy)]
    struct Msg {
        hwnd: *mut c_void,
        message: u32,
        wparam: usize,
        lparam: isize,
        time: u32,
        pt: Point,
        l_private: u32,
    }

    #[repr(C)]
    struct WndClassExW {
        cb_size: u32,
        style: u32,
        lpfn_wnd_proc:
            unsafe extern "system" fn(*mut c_void, u32, usize, isize) -> isize,
        cb_cls_extra: i32,
        cb_wnd_extra: i32,
        h_instance: *mut c_void,
        h_icon: *mut c_void,
        h_cursor: *mut c_void,
        hbr_background: *mut c_void,
        lpsz_menu_name: *const u16,
        lpsz_class_name: *const u16,
        h_icon_sm: *mut c_void,
    }

    #[repr(C)]
    struct NotifyIconDataW {
        cb_size: u32,
        h_wnd: *mut c_void,
        u_id: u32,
        u_flags: u32,
        u_callback_message: u32,
        h_icon: *mut c_void,
        sz_tip: [u16; 128],
        dw_state: u32,
        dw_state_mask: u32,
        sz_info: [u16; 256],
        u_timeout_or_version: u32,
        sz_info_title: [u16; 64],
        dw_info_flags: u32,
        guid_item: [u8; 16],
        h_balloon_icon: *mut c_void,
    }

    #[link(name = "user32")]
    extern "system" {
        fn RegisterClassExW(lpwcx: *const WndClassExW) -> u16;
        fn CreateWindowExW(
            dw_ex_style: u32,
            lp_class_name: *const u16,
            lp_window_name: *const u16,
            dw_style: u32,
            x: i32,
            y: i32,
            n_width: i32,
            n_height: i32,
            hwnd_parent: *mut c_void,
            h_menu: *mut c_void,
            h_instance: *mut c_void,
            lp_param: *const c_void,
        ) -> *mut c_void;
        fn DestroyWindow(hwnd: *mut c_void) -> i32;
        fn DefWindowProcW(hwnd: *mut c_void, msg: u32, wparam: usize, lparam: isize) -> isize;
        fn GetWindowLongPtrW(hwnd: *mut c_void, index: i32) -> isize;
        fn SetWindowLongPtrW(hwnd: *mut c_void, index: i32, new_long: isize) -> isize;
        fn PeekMessageW(
            lp_msg: *mut Msg,
            hwnd: *mut c_void,
            w_msg_filter_min: u32,
            w_msg_filter_max: u32,
            w_remove_msg: u32,
        ) -> i32;
        fn TranslateMessage(lp_msg: *const Msg) -> i32;
        fn DispatchMessageW(lp_msg: *const Msg) -> isize;
        fn LoadIconW(h_instance: *mut c_void, lp_icon_name: *const u16) -> *mut c_void;
        fn CreatePopupMenu() -> *mut c_void;
        fn AppendMenuW(
            h_menu: *mut c_void,
            u_flags: u32,
            u_id_new_item: usize,
            lp_new_item: *const u16,
        ) -> i32;
        fn TrackPopupMenu(
            h_menu: *mut c_void,
            u_flags: u32,
            x: i32,
            y: i32,
            n_reserved: i32,
            hwnd: *mut c_void,
            prc_rect: *const c_void,
        ) -> i32;
        fn DestroyMenu(h_menu: *mut c_void) -> i32;
        fn GetCursorPos(lp_point: *mut Point) -> i32;
        fn SetForegroundWindow(hwnd: *mut c_void) -> i32;
        fn RegisterHotKey(hwnd: *mut c_void, id: i32, fs_modifiers: u32, vk: u32) -> i32;
        fn UnregisterHotKey(hwnd: *mut c_void, id: i32) -> i32;
    }

    #[link(name = "kernel32")]
    extern "system" {
        fn GetModuleHandleW(lp_module_name: *const u16) -> *mut c_void;
    }

    #[link(name = "shell32")]
    extern "system" {
        fn Shell_NotifyIconW(dw_message: u32, lp_data: *const NotifyIconDataW) -> i32;
    }

    fn wide_z(s: &str) -> Vec<u16> {
        s.encode_utf16().chain(std::iter::once(0)).collect()
    }

    unsafe fn popup_tray_menu(hwnd: *mut c_void) -> Option<TrayDaemonAction> {
        let menu = CreatePopupMenu();
        if menu.is_null() {
            return None;
        }
        let items: [(u32, usize, &str); 7] = [
            (MF_STRING, IDM_TOGGLE_ISLAND, "Mostrar / Ocultar Isla Miku"),
            (MF_STRING, IDM_OPEN_CUSTOM_UI, "Personalizar UI y Muneco (Abrir Carpeta)"),
            (MF_STRING, IDM_OPEN_SETUP, "Configuracion · Miku (API y Ajustes)"),
            (MF_STRING, IDM_TOGGLE_KILL_SWITCH, "Kill Switch (Pausar / Reanudar)"),
            (MF_STRING, IDM_RUN_DOCTOR, "Ejecutar Diagnostico Doctor"),
            (MF_SEPARATOR, 0, ""),
            (MF_STRING, IDM_EXIT, "Salir de Miku"),
        ];
        for (flags, id, label) in items {
            if flags == MF_SEPARATOR {
                let _ = AppendMenuW(menu, MF_SEPARATOR, 0, std::ptr::null());
            } else {
                let w = wide_z(label);
                let _ = AppendMenuW(menu, flags, id, w.as_ptr());
            }
        }
        let mut pt = Point { x: 0, y: 0 };
        let _ = GetCursorPos(&mut pt);
        let _ = SetForegroundWindow(hwnd);
        let cmd = TrackPopupMenu(
            menu,
            TPM_RETURNCMD | TPM_RIGHTBUTTON | TPM_RIGHTALIGN | TPM_BOTTOMALIGN,
            pt.x,
            pt.y,
            0,
            hwnd,
            std::ptr::null(),
        ) as usize;
        let _ = DestroyMenu(menu);
        match cmd {
            IDM_TOGGLE_ISLAND => Some(TrayDaemonAction::ToggleIsland),
            IDM_OPEN_CUSTOM_UI => Some(TrayDaemonAction::OpenCustomUiFolder),
            IDM_OPEN_SETUP => Some(TrayDaemonAction::OpenSetup),
            IDM_TOGGLE_KILL_SWITCH => Some(TrayDaemonAction::ToggleKillSwitch),
            IDM_RUN_DOCTOR => Some(TrayDaemonAction::RunDoctor),
            IDM_EXIT => Some(TrayDaemonAction::Exit),
            _ => None,
        }
    }

    unsafe extern "system" fn tray_wndproc(
        hwnd: *mut c_void,
        msg: u32,
        wparam: usize,
        lparam: isize,
    ) -> isize {
        if msg == WM_CREATE {
            return 0;
        }
        let tx_ptr = GetWindowLongPtrW(hwnd, GWLP_USERDATA) as *const Sender<TrayDaemonAction>;
        if !tx_ptr.is_null() {
            let tx = &*tx_ptr;
            match msg {
                WM_MIKU_TRAY => {
                    let ev = (lparam as u32) & 0xFFFF;
                    if ev == WM_LBUTTONUP || ev == WM_LBUTTONDBLCLK {
                        let _ = tx.send(TrayDaemonAction::ToggleIsland);
                    } else if ev == WM_RBUTTONUP || ev == WM_CONTEXTMENU {
                        if let Some(act) = popup_tray_menu(hwnd) {
                            let _ = tx.send(act);
                        }
                    }
                    return 0;
                }
                WM_HOTKEY => {
                    if wparam == DEMO_PTT_HOTKEY_ID as usize {
                        let _ = tx.send(TrayDaemonAction::HotkeyPttWake);
                    } else if wparam == KILL_SWITCH_HOTKEY_ID as usize {
                        let _ = tx.send(TrayDaemonAction::ToggleKillSwitch);
                    }
                    return 0;
                }
                WM_CLOSE => {
                    let _ = tx.send(TrayDaemonAction::Exit);
                    return 0;
                }
                _ => {}
            }
        }
        if msg == WM_DESTROY {
            let old = SetWindowLongPtrW(hwnd, GWLP_USERDATA, 0);
            if old != 0 {
                drop(Box::from_raw(old as *mut Sender<TrayDaemonAction>));
            }
            return 0;
        }
        DefWindowProcW(hwnd, msg, wparam, lparam)
    }

    pub struct DesktopTrayHost {
        hwnd: *mut c_void,
        rx: Receiver<TrayDaemonAction>,
        nid: NotifyIconDataW,
        tray_added: bool,
        ptt_registered: bool,
        ks_registered: bool,
        report: TrayRegistrationReport,
    }

    impl DesktopTrayHost {
        pub fn start() -> Result<Self, String> {
            unsafe {
                let hinst = GetModuleHandleW(std::ptr::null());
                let cls_w = wide_z("MikuDesktopTrayDaemonWnd");
                let title_w = wide_z("Miku Tray");
                let wcx = WndClassExW {
                    cb_size: std::mem::size_of::<WndClassExW>() as u32,
                    style: 0,
                    lpfn_wnd_proc: tray_wndproc,
                    cb_cls_extra: 0,
                    cb_wnd_extra: 0,
                    h_instance: hinst,
                    h_icon: std::ptr::null_mut(),
                    h_cursor: std::ptr::null_mut(),
                    hbr_background: std::ptr::null_mut(),
                    lpsz_menu_name: std::ptr::null(),
                    lpsz_class_name: cls_w.as_ptr(),
                    h_icon_sm: std::ptr::null_mut(),
                };
                let _ = RegisterClassExW(&wcx);

                let hwnd = CreateWindowExW(
                    WS_EX_TOOLWINDOW,
                    cls_w.as_ptr(),
                    title_w.as_ptr(),
                    WS_POPUP,
                    0,
                    0,
                    0,
                    0,
                    std::ptr::null_mut(),
                    std::ptr::null_mut(),
                    hinst,
                    std::ptr::null(),
                );
                if hwnd.is_null() {
                    return Err("CreateWindowExW failed for System Tray host".to_string());
                }

                let (tx, rx) = mpsc::channel::<TrayDaemonAction>();
                let boxed_tx = Box::into_raw(Box::new(tx));
                let _ = SetWindowLongPtrW(hwnd, GWLP_USERDATA, boxed_tx as isize);

                let mut nid: NotifyIconDataW = std::mem::zeroed();
                nid.cb_size = std::mem::size_of::<NotifyIconDataW>() as u32;
                nid.h_wnd = hwnd;
                nid.u_id = 1;
                nid.u_flags = NIF_MESSAGE | NIF_ICON | NIF_TIP;
                nid.u_callback_message = WM_MIKU_TRAY;
                nid.h_icon = LoadIconW(std::ptr::null_mut(), IDI_APPLICATION);
                let tip = wide_z("Miku Desktop Orchestrator");
                let n_copy = tip.len().min(nid.sz_tip.len());
                nid.sz_tip[..n_copy].copy_from_slice(&tip[..n_copy]);

                let tray_added = Shell_NotifyIconW(NIM_ADD, &nid) != 0;
                let ptt_registered = RegisterHotKey(
                    hwnd,
                    DEMO_PTT_HOTKEY_ID,
                    WIN32_MOD_ALT | WIN32_MOD_NOREPEAT,
                    WIN32_VK_SPACE,
                ) != 0;
                let ks_registered = RegisterHotKey(
                    hwnd,
                    KILL_SWITCH_HOTKEY_ID,
                    KILL_SWITCH_WIN32_MODIFIERS,
                    KILL_SWITCH_WIN32_VK_K,
                ) != 0;

                let mut warnings = Vec::new();
                if !tray_added {
                    warnings.push("System Tray icon registration skipped/unavailable".to_string());
                }
                if !ptt_registered {
                    warnings.push("Hotkey Alt+Space conflicted with another Windows app".to_string());
                }
                if !ks_registered {
                    warnings.push("Hotkey Ctrl+Alt+Shift+K conflicted with another Windows app".to_string());
                }

                let report = TrayRegistrationReport {
                    tray_icon_active: tray_added,
                    ptt_hotkey_registered: ptt_registered,
                    kill_switch_hotkey_registered: ks_registered,
                    warnings,
                };

                Ok(Self {
                    hwnd,
                    rx,
                    nid,
                    tray_added,
                    ptt_registered,
                    ks_registered,
                    report,
                })
            }
        }

        pub fn report(&self) -> &TrayRegistrationReport {
            &self.report
        }

        /// Synchronizes the global Push-To-Talk `Alt+Space` hotkey registration based on whether
        /// voice STT is enabled. When `false`, unregisters the hotkey to avoid hijacking native
        /// Windows window system menus. Returns whether `Alt+Space` is currently registered.
        pub fn sync_ptt_hotkey(&mut self, enabled: bool) -> bool {
            if self.hwnd.is_null() {
                return false;
            }
            if enabled {
                if !self.ptt_registered {
                    let ok = unsafe {
                        RegisterHotKey(
                            self.hwnd,
                            DEMO_PTT_HOTKEY_ID,
                            WIN32_MOD_ALT | WIN32_MOD_NOREPEAT,
                            WIN32_VK_SPACE,
                        ) != 0
                    };
                    self.ptt_registered = ok;
                    self.report.ptt_hotkey_registered = ok;
                    ok
                } else {
                    true
                }
            } else {
                if self.ptt_registered {
                    let _ = unsafe { UnregisterHotKey(self.hwnd, DEMO_PTT_HOTKEY_ID) };
                    self.ptt_registered = false;
                    self.report.ptt_hotkey_registered = false;
                    true
                } else {
                    true
                }
            }
        }

        pub fn is_ptt_hotkey_registered(&self) -> bool {
            self.ptt_registered
        }

        pub fn poll_actions(&mut self) -> Vec<TrayDaemonAction> {
            unsafe {
                let mut msg: Msg = std::mem::zeroed();
                while PeekMessageW(&mut msg, self.hwnd, 0, 0, PM_REMOVE) != 0 {
                    let _ = TranslateMessage(&msg);
                    let _ = DispatchMessageW(&msg);
                }
            }
            let mut out = Vec::new();
            while let Ok(act) = self.rx.try_recv() {
                out.push(act);
            }
            out
        }

        pub fn shutdown(mut self) {
            self.cleanup();
        }

        fn cleanup(&mut self) {
            unsafe {
                if self.ptt_registered && !self.hwnd.is_null() {
                    let _ = UnregisterHotKey(self.hwnd, DEMO_PTT_HOTKEY_ID);
                    self.ptt_registered = false;
                }
                if self.ks_registered && !self.hwnd.is_null() {
                    let _ = UnregisterHotKey(self.hwnd, KILL_SWITCH_HOTKEY_ID);
                    self.ks_registered = false;
                }
                if self.tray_added {
                    let _ = Shell_NotifyIconW(NIM_DELETE, &self.nid);
                    self.tray_added = false;
                }
                if !self.hwnd.is_null() {
                    let _ = DestroyWindow(self.hwnd);
                    self.hwnd = std::ptr::null_mut();
                }
            }
        }
    }

    impl Drop for DesktopTrayHost {
        fn drop(&mut self) {
            self.cleanup();
        }
    }
}

#[cfg(target_os = "windows")]
pub use win_tray_impl::DesktopTrayHost;

#[cfg(not(target_os = "windows"))]
pub struct DesktopTrayHost {
    ptt_registered: bool,
    report: TrayRegistrationReport,
}

#[cfg(not(target_os = "windows"))]
impl DesktopTrayHost {
    pub fn start() -> Result<Self, String> {
        Ok(Self {
            ptt_registered: false,
            report: TrayRegistrationReport {
                tray_icon_active: false,
                ptt_hotkey_registered: false,
                kill_switch_hotkey_registered: false,
                warnings: Vec::new(),
            },
        })
    }

    pub fn report(&self) -> &TrayRegistrationReport {
        &self.report
    }

    pub fn poll_actions(&mut self) -> Vec<TrayDaemonAction> {
        Vec::new()
    }

    pub fn sync_ptt_hotkey(&mut self, enabled: bool) -> bool {
        self.ptt_registered = enabled;
        self.report.ptt_hotkey_registered = enabled;
        enabled
    }

    pub fn is_ptt_hotkey_registered(&self) -> bool {
        self.ptt_registered
    }

    pub fn shutdown(self) {}
}
