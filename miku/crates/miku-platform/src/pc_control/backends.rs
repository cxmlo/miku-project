use std::collections::BTreeMap;
use std::fs;
use std::path::Path;
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Mutex;
use std::thread;
use std::time::{Duration, Instant};

#[cfg(windows)]
use std::os::windows::process::CommandExt;

use serde::{Deserialize, Serialize};

use crate::{PlatformError, WIN32_CREATE_NO_WINDOW};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ShellExecOutput {
    pub exit_code: Option<i32>,
    pub stdout: String,
    pub stderr: String,
    pub aborted_by_kill_switch: bool,
    pub timed_out: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct UiAutomationActionReceipt {
    pub used_ui_automation_tree: bool,
    pub target_summary: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum InputStep {
    KeyCombo(Vec<String>),
    TypeText(String),
    MouseClick { x: i32, y: i32 },
}

pub trait PcControlBackend {
    fn fs_read(&self, canonical_path: &str) -> Result<String, PlatformError>;
    fn fs_write(
        &self,
        canonical_path: &str,
        content: &str,
        create_dirs: bool,
    ) -> Result<usize, PlatformError>;
    fn run_shell(
        &self,
        program: &str,
        args: &[String],
        cwd: Option<&str>,
        timeout_ms: u64,
        abort_flag: &AtomicBool,
    ) -> Result<ShellExecOutput, PlatformError>;
    fn clipboard_read(&self) -> Result<String, PlatformError>;
    fn clipboard_write(&self, text: &str) -> Result<(), PlatformError>;
    fn ui_automation_invoke(
        &self,
        window_title_pattern: &str,
        automation_id_or_name: &str,
        control_type: &str,
    ) -> Result<UiAutomationActionReceipt, PlatformError>;
    fn ui_automation_pixel_fallback(
        &self,
        window_title_pattern: &str,
        x: i32,
        y: i32,
        reason: &str,
    ) -> Result<UiAutomationActionReceipt, PlatformError>;
    fn execute_input(
        &self,
        step: &InputStep,
        abort_flag: &AtomicBool,
    ) -> Result<String, PlatformError>;
}

#[derive(Default)]
pub struct SandboxPcControlBackend {
    files: Mutex<BTreeMap<String, String>>,
    clipboard: Mutex<String>,
    pub executed_log: Mutex<Vec<String>>,
    pub simulated_shell_step_sleep_ms: u64,
}

impl SandboxPcControlBackend {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn seed_file(&self, canonical_path: &str, content: &str) {
        self.files
            .lock()
            .expect("files lock")
            .insert(canonical_path.to_string(), content.to_string());
    }

    pub fn recorded_actions(&self) -> Vec<String> {
        self.executed_log.lock().expect("log lock").clone()
    }
}

impl PcControlBackend for SandboxPcControlBackend {
    fn fs_read(&self, canonical_path: &str) -> Result<String, PlatformError> {
        self.executed_log
            .lock()
            .expect("log lock")
            .push(format!("fs_read:{}", canonical_path));
        self.files
            .lock()
            .expect("files lock")
            .get(canonical_path)
            .cloned()
            .ok_or_else(|| {
                PlatformError::RegistryBackendError(format!(
                    "sandbox file not found: {}",
                    canonical_path
                ))
            })
    }

    fn fs_write(
        &self,
        canonical_path: &str,
        content: &str,
        _create_dirs: bool,
    ) -> Result<usize, PlatformError> {
        self.executed_log
            .lock()
            .expect("log lock")
            .push(format!("fs_write:{}", canonical_path));
        self.files
            .lock()
            .expect("files lock")
            .insert(canonical_path.to_string(), content.to_string());
        Ok(content.len())
    }

    fn run_shell(
        &self,
        program: &str,
        args: &[String],
        _cwd: Option<&str>,
        _timeout_ms: u64,
        abort_flag: &AtomicBool,
    ) -> Result<ShellExecOutput, PlatformError> {
        for _ in 0..8 {
            if abort_flag.load(Ordering::SeqCst) {
                return Ok(ShellExecOutput {
                    exit_code: None,
                    stdout: String::new(),
                    stderr: "aborted by kill switch".to_string(),
                    aborted_by_kill_switch: true,
                    timed_out: false,
                });
            }
            if self.simulated_shell_step_sleep_ms > 0 {
                thread::sleep(Duration::from_millis(self.simulated_shell_step_sleep_ms));
            }
        }
        let line = format!("shell:{} {}", program, args.join(" "));
        self.executed_log.lock().expect("log lock").push(line.clone());
        Ok(ShellExecOutput {
            exit_code: Some(0),
            stdout: format!("ok:{}", line),
            stderr: String::new(),
            aborted_by_kill_switch: false,
            timed_out: false,
        })
    }

    fn clipboard_read(&self) -> Result<String, PlatformError> {
        self.executed_log
            .lock()
            .expect("log lock")
            .push("clipboard_read".to_string());
        Ok(self.clipboard.lock().expect("clipboard lock").clone())
    }

    fn clipboard_write(&self, text: &str) -> Result<(), PlatformError> {
        self.executed_log
            .lock()
            .expect("log lock")
            .push(format!("clipboard_write:{}", text.len()));
        *self.clipboard.lock().expect("clipboard lock") = text.to_string();
        Ok(())
    }

    fn ui_automation_invoke(
        &self,
        window_title_pattern: &str,
        automation_id_or_name: &str,
        control_type: &str,
    ) -> Result<UiAutomationActionReceipt, PlatformError> {
        let summary = format!(
            "uia_tree:{}|{}|{}",
            window_title_pattern, automation_id_or_name, control_type
        );
        self.executed_log
            .lock()
            .expect("log lock")
            .push(summary.clone());
        Ok(UiAutomationActionReceipt {
            used_ui_automation_tree: true,
            target_summary: summary,
        })
    }

    fn ui_automation_pixel_fallback(
        &self,
        window_title_pattern: &str,
        x: i32,
        y: i32,
        reason: &str,
    ) -> Result<UiAutomationActionReceipt, PlatformError> {
        let summary = format!(
            "pixel_fallback:{}|{},{}|reason={}",
            window_title_pattern, x, y, reason
        );
        self.executed_log
            .lock()
            .expect("log lock")
            .push(summary.clone());
        Ok(UiAutomationActionReceipt {
            used_ui_automation_tree: false,
            target_summary: summary,
        })
    }

    fn execute_input(
        &self,
        step: &InputStep,
        abort_flag: &AtomicBool,
    ) -> Result<String, PlatformError> {
        if abort_flag.load(Ordering::SeqCst) {
            return Err(PlatformError::DaemonStartupFailed(
                "input aborted immediately by kill switch".to_string(),
            ));
        }
        let desc = match step {
            InputStep::KeyCombo(keys) => format!("input_keys:{}", keys.join("+")),
            InputStep::TypeText(text) => format!("input_type:{}", text.len()),
            InputStep::MouseClick { x, y } => format!("input_click:{},{}", x, y),
        };
        self.executed_log
            .lock()
            .expect("log lock")
            .push(desc.clone());
        Ok(desc)
    }
}

#[derive(Debug, Default)]
pub struct WindowsNativePcControlBackend;

impl WindowsNativePcControlBackend {
    pub fn new() -> Self {
        Self
    }
}

#[cfg(windows)]
mod win32_pc_ffi {
    use super::*;
    use std::ffi::c_void;

    pub type WinHwnd = *mut c_void;
    pub type WinHandle = *mut c_void;
    pub type WinBool = i32;
    pub type WinUint = u32;
    pub type WinSizeT = usize;

    const CF_UNICODETEXT: WinUint = 13;
    const GMEM_MOVEABLE: WinUint = 0x0002;

    #[link(name = "user32")]
    unsafe extern "system" {
        fn OpenClipboard(hwnd_new_owner: WinHwnd) -> WinBool;
        fn CloseClipboard() -> WinBool;
        fn EmptyClipboard() -> WinBool;
        fn GetClipboardData(format: WinUint) -> WinHandle;
        fn SetClipboardData(format: WinUint, h_mem: WinHandle) -> WinHandle;
    }

    #[link(name = "kernel32")]
    unsafe extern "system" {
        fn GlobalAlloc(flags: WinUint, bytes: WinSizeT) -> WinHandle;
        fn GlobalLock(h_mem: WinHandle) -> *mut c_void;
        fn GlobalUnlock(h_mem: WinHandle) -> WinBool;
        fn GlobalFree(h_mem: WinHandle) -> WinHandle;
    }

    struct ClipboardGuard;
    impl Drop for ClipboardGuard {
        fn drop(&mut self) {
            // SAFETY: always matches successful `OpenClipboard`.
            unsafe {
                let _ = CloseClipboard();
            }
        }
    }

    struct GlobalLockGuard(WinHandle);
    impl Drop for GlobalLockGuard {
        fn drop(&mut self) {
            // SAFETY: always matches successful `GlobalLock`.
            unsafe {
                let _ = GlobalUnlock(self.0);
            }
        }
    }

    pub fn read_clipboard_utf16() -> Result<String, PlatformError> {
        // SAFETY: Win32 clipboard API guarded by `ClipboardGuard` and `GlobalLockGuard`.
        unsafe {
            if OpenClipboard(std::ptr::null_mut()) == 0 {
                return Err(PlatformError::RegistryBackendError(
                    "OpenClipboard failed during clipboard_read".to_string(),
                ));
            }
            let _clip_guard = ClipboardGuard;
            let h_data = GetClipboardData(CF_UNICODETEXT);
            if h_data.is_null() {
                return Ok(String::new());
            }
            let ptr = GlobalLock(h_data) as *const u16;
            if ptr.is_null() {
                return Err(PlatformError::RegistryBackendError(
                    "GlobalLock failed reading CF_UNICODETEXT".to_string(),
                ));
            }
            let _lock_guard = GlobalLockGuard(h_data);
            let mut len = 0usize;
            while len < 1_048_576 && *ptr.add(len) != 0 {
                len += 1;
            }
            let slice = std::slice::from_raw_parts(ptr, len);
            Ok(String::from_utf16_lossy(slice))
        }
    }

    pub fn write_clipboard_utf16(text: &str) -> Result<(), PlatformError> {
        let mut wide: Vec<u16> = text.encode_utf16().collect();
        wide.push(0);
        let byte_len = wide.len() * std::mem::size_of::<u16>();

        // SAFETY: allocates moveable UTF-16 buffer and transfers ownership on `SetClipboardData`.
        unsafe {
            let h_mem = GlobalAlloc(GMEM_MOVEABLE, byte_len);
            if h_mem.is_null() {
                return Err(PlatformError::RegistryBackendError(
                    "GlobalAlloc failed for CF_UNICODETEXT".to_string(),
                ));
            }
            {
                let dst = GlobalLock(h_mem) as *mut u16;
                if dst.is_null() {
                    let _ = GlobalFree(h_mem);
                    return Err(PlatformError::RegistryBackendError(
                        "GlobalLock failed writing CF_UNICODETEXT".to_string(),
                    ));
                }
                let _lock = GlobalLockGuard(h_mem);
                std::ptr::copy_nonoverlapping(wide.as_ptr(), dst, wide.len());
            }
            if OpenClipboard(std::ptr::null_mut()) == 0 {
                let _ = GlobalFree(h_mem);
                return Err(PlatformError::RegistryBackendError(
                    "OpenClipboard failed during clipboard_write".to_string(),
                ));
            }
            let _clip = ClipboardGuard;
            if EmptyClipboard() == 0 || SetClipboardData(CF_UNICODETEXT, h_mem).is_null() {
                let _ = GlobalFree(h_mem);
                return Err(PlatformError::RegistryBackendError(
                    "SetClipboardData(CF_UNICODETEXT) failed".to_string(),
                ));
            }
            Ok(())
        }
    }
}

impl PcControlBackend for WindowsNativePcControlBackend {
    fn fs_read(&self, canonical_path: &str) -> Result<String, PlatformError> {
        fs::read_to_string(Path::new(canonical_path)).map_err(|e| {
            PlatformError::RegistryBackendError(format!(
                "fs_read failed for '{}': {}",
                canonical_path, e
            ))
        })
    }

    fn fs_write(
        &self,
        canonical_path: &str,
        content: &str,
        create_dirs: bool,
    ) -> Result<usize, PlatformError> {
        let path = Path::new(canonical_path);
        if create_dirs {
            if let Some(parent) = path.parent() {
                fs::create_dir_all(parent).map_err(|e| {
                    PlatformError::RegistryBackendError(format!(
                        "create_dir_all failed for '{}': {}",
                        parent.display(),
                        e
                    ))
                })?;
            }
        }
        fs::write(path, content.as_bytes()).map_err(|e| {
            PlatformError::RegistryBackendError(format!(
                "fs_write failed for '{}': {}",
                canonical_path, e
            ))
        })?;
        Ok(content.len())
    }

    fn run_shell(
        &self,
        program: &str,
        args: &[String],
        cwd: Option<&str>,
        timeout_ms: u64,
        abort_flag: &AtomicBool,
    ) -> Result<ShellExecOutput, PlatformError> {
        if abort_flag.load(Ordering::SeqCst) {
            return Ok(ShellExecOutput {
                exit_code: None,
                stdout: String::new(),
                stderr: "aborted before spawn by kill switch".to_string(),
                aborted_by_kill_switch: true,
                timed_out: false,
            });
        }

        let mut cmd = Command::new(program);
        cmd.args(args)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        if let Some(dir) = cwd {
            cmd.current_dir(dir);
        }
        #[cfg(windows)]
        {
            cmd.creation_flags(WIN32_CREATE_NO_WINDOW);
        }

        let mut child = cmd.spawn().map_err(|e| {
            PlatformError::DaemonStartupFailed(format!(
                "failed to spawn shell command '{}': {}",
                program, e
            ))
        })?;

        let started = Instant::now();
        let timeout = Duration::from_millis(timeout_ms);
        loop {
            if abort_flag.load(Ordering::SeqCst) {
                let _ = child.kill();
                let _ = child.wait();
                return Ok(ShellExecOutput {
                    exit_code: None,
                    stdout: String::new(),
                    stderr: "child terminated immediately by kill switch".to_string(),
                    aborted_by_kill_switch: true,
                    timed_out: false,
                });
            }
            if let Some(_status) = child.try_wait().map_err(|e| {
                PlatformError::DaemonStartupFailed(format!("child try_wait error: {}", e))
            })? {
                let out = child.wait_with_output().map_err(|e| {
                    PlatformError::DaemonStartupFailed(format!("wait_with_output error: {}", e))
                })?;
                return Ok(ShellExecOutput {
                    exit_code: out.status.code(),
                    stdout: String::from_utf8_lossy(&out.stdout).to_string(),
                    stderr: String::from_utf8_lossy(&out.stderr).to_string(),
                    aborted_by_kill_switch: false,
                    timed_out: false,
                });
            }
            if started.elapsed() >= timeout {
                let _ = child.kill();
                let _ = child.wait();
                return Ok(ShellExecOutput {
                    exit_code: None,
                    stdout: String::new(),
                    stderr: format!("shell command timed out after {} ms", timeout_ms),
                    aborted_by_kill_switch: false,
                    timed_out: true,
                });
            }
            thread::sleep(Duration::from_millis(2));
        }
    }

    fn clipboard_read(&self) -> Result<String, PlatformError> {
        #[cfg(windows)]
        {
            win32_pc_ffi::read_clipboard_utf16()
        }
        #[cfg(not(windows))]
        {
            Err(PlatformError::RegistryBackendError(
                "Windows clipboard requires Windows".to_string(),
            ))
        }
    }

    fn clipboard_write(&self, text: &str) -> Result<(), PlatformError> {
        #[cfg(windows)]
        {
            win32_pc_ffi::write_clipboard_utf16(text)
        }
        #[cfg(not(windows))]
        {
            let _ = text;
            Err(PlatformError::RegistryBackendError(
                "Windows clipboard requires Windows".to_string(),
            ))
        }
    }

    fn ui_automation_invoke(
        &self,
        window_title_pattern: &str,
        automation_id_or_name: &str,
        control_type: &str,
    ) -> Result<UiAutomationActionReceipt, PlatformError> {
        crate::pc_control::win32_uia::safe_ui_automation_invoke(
            window_title_pattern,
            automation_id_or_name,
            control_type,
            None,
        )
    }

    fn ui_automation_pixel_fallback(
        &self,
        window_title_pattern: &str,
        x: i32,
        y: i32,
        reason: &str,
    ) -> Result<UiAutomationActionReceipt, PlatformError> {
        Ok(UiAutomationActionReceipt {
            used_ui_automation_tree: false,
            target_summary: format!(
                "win32_pixel_fallback:{}|{},{}|{}",
                window_title_pattern, x, y, reason
            ),
        })
    }

    fn execute_input(
        &self,
        step: &InputStep,
        abort_flag: &AtomicBool,
    ) -> Result<String, PlatformError> {
        if abort_flag.load(Ordering::SeqCst) {
            return Err(PlatformError::DaemonStartupFailed(
                "input execution aborted by kill switch".to_string(),
            ));
        }
        match step {
            InputStep::KeyCombo(keys) => {
                crate::pc_control::win32_input::verify_input_target_safe()?;
                let mut guard = crate::pc_control::win32_input::KeyComboGuard::from_key_names(keys);
                guard.press_modifiers();
                for key in keys {
                    if crate::pc_control::win32_input::parse_modifier_vk(key).is_none() {
                        if let Some(vk) = crate::pc_control::win32_input::parse_virtual_key(key) {
                            if abort_flag.load(Ordering::SeqCst) {
                                return Err(PlatformError::DaemonStartupFailed("input aborted by kill switch".into()));
                            }
                            crate::pc_control::win32_input::send_virtual_key_tap(vk);
                        }
                    }
                }
                if abort_flag.load(Ordering::SeqCst) {
                    return Err(PlatformError::DaemonStartupFailed("input aborted by kill switch".into()));
                }
                guard.release_all();
                Ok(format!("win32_sendinput_keys:{}", keys.join("+")))
            }
            InputStep::TypeText(t) => {
                let n = crate::pc_control::win32_input::send_unicode_text(t)?;
                Ok(format!("win32_sendinput_text:{}u16", n))
            }
            InputStep::MouseClick { x, y } => {
                let (ax, ay) = crate::pc_control::win32_input::send_mouse_absolute_click(*x, *y, false)?;
                Ok(format!("win32_sendinput_click:virt={},{}->abs={},{}", x, y, ax, ay))
            }
        }
    }
}
