use std::collections::BTreeSet;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::Arc;
use std::thread::JoinHandle;

use miku_gatekeeper::GatekeeperEngine;
use serde::{Deserialize, Serialize};
use tokio::sync::mpsc;

use crate::{normalize_os_path_for_jail, SkillEngine, SkillError, SkillReloadOutcome};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum SkillWatchEventKind {
    ManifestModified,
    DirectoryRescan,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SkillWatchEvent {
    pub skill_dir: PathBuf,
    pub kind: SkillWatchEventKind,
    pub timestamp_ms: u64,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct WatcherBatchSummary {
    pub raw_events_received: usize,
    pub coalesced_dirs_processed: usize,
    pub reloads_applied: usize,
    pub reloads_unchanged: usize,
    pub reloads_rejected: usize,
}

/// Zero-polling OS Event-Driven Directory Watcher (`F-11`, `ADR-008`).
///
/// Architecture:
/// - **0 polling timers in idle**: Never uses `std::thread::sleep` loops or `tokio::time::interval`.
/// - **Windows (`#[cfg(target_os = "windows")]`)**: Uses Win32 kernel change notification handles
///   (`FindFirstChangeNotificationW` + manual-reset `CreateEventW` stop handle + `WaitForMultipleObjects`
///   with `INFINITE` timeout). The kernel wakes the watcher thread only when a file/attribute change
///   occurs in the watched skills tree or when `shutdown()` signals the stop event (`< 1 ms` wake).
/// - **Event Coalescing**: Rapid consecutive notifications on the same skill folder are deduplicated
///   per batch before applying transactional hot-reload in `SkillEngine`.
pub struct SkillFsWatcher {
    watched_root: PathBuf,
    event_tx: mpsc::UnboundedSender<SkillWatchEvent>,
    event_rx: mpsc::UnboundedReceiver<SkillWatchEvent>,
    running: Arc<AtomicBool>,
    os_wakeup_count: Arc<AtomicUsize>,
    #[cfg(target_os = "windows")]
    stop_event_raw: usize,
    worker_thread: Option<JoinHandle<()>>,
}

impl SkillFsWatcher {
    pub fn start(watched_root: &Path) -> Result<Self, SkillError> {
        if !watched_root.is_dir() {
            return Err(SkillError::WatcherError(format!(
                "watched root '{}' is not an existing directory",
                watched_root.display()
            )));
        }

        let (event_tx, event_rx) = mpsc::unbounded_channel();
        let running = Arc::new(AtomicBool::new(true));
        let os_wakeup_count = Arc::new(AtomicUsize::new(0));

        #[cfg(target_os = "windows")]
        {
            let (stop_event_raw, handle) = Self::spawn_win32_event_watcher(
                watched_root.to_path_buf(),
                event_tx.clone(),
                Arc::clone(&running),
                Arc::clone(&os_wakeup_count),
            )?;
            Ok(Self {
                watched_root: watched_root.to_path_buf(),
                event_tx,
                event_rx,
                running,
                os_wakeup_count,
                stop_event_raw,
                worker_thread: Some(handle),
            })
        }

        #[cfg(not(target_os = "windows"))]
        {
            Ok(Self {
                watched_root: watched_root.to_path_buf(),
                event_tx,
                event_rx,
                running,
                os_wakeup_count,
                worker_thread: None,
            })
        }
    }

    pub fn is_running(&self) -> bool {
        self.running.load(Ordering::SeqCst)
    }

    pub fn watched_root(&self) -> &Path {
        &self.watched_root
    }

    pub fn os_wakeup_count(&self) -> usize {
        self.os_wakeup_count.load(Ordering::SeqCst)
    }

    /// Emits a filesystem change notification event into the watcher's event channel.
    /// Used both by the Win32 kernel notification callback and by deterministic event-injection tests.
    pub fn notify_skill_dir_changed(
        &self,
        skill_dir: &Path,
        timestamp_ms: u64,
    ) -> Result<(), SkillError> {
        if !self.is_running() {
            return Err(SkillError::WatcherError(
                "watcher is already shut down".to_string(),
            ));
        }
        self.os_wakeup_count.fetch_add(1, Ordering::SeqCst);
        self.event_tx
            .send(SkillWatchEvent {
                skill_dir: skill_dir.to_path_buf(),
                kind: SkillWatchEventKind::ManifestModified,
                timestamp_ms,
            })
            .map_err(|e| SkillError::WatcherError(e.to_string()))
    }

    /// Waits asynchronously for the next OS watcher event (or returns `None` if shut down),
    /// then drains any queued burst events and applies them transactionally with path coalescing.
    pub async fn recv_and_apply_coalesced(
        &mut self,
        engine: &mut SkillEngine,
        gatekeeper: &GatekeeperEngine,
    ) -> Option<WatcherBatchSummary> {
        let first = self.event_rx.recv().await?;
        let mut batch = vec![first];
        while let Ok(ev) = self.event_rx.try_recv() {
            batch.push(ev);
        }
        Some(self.apply_event_batch(&batch, engine, gatekeeper))
    }

    /// Drains all currently queued events without blocking and applies them with coalescing.
    pub fn drain_pending_and_apply(
        &mut self,
        engine: &mut SkillEngine,
        gatekeeper: &GatekeeperEngine,
    ) -> WatcherBatchSummary {
        let mut batch = Vec::new();
        while let Ok(ev) = self.event_rx.try_recv() {
            batch.push(ev);
        }
        self.apply_event_batch(&batch, engine, gatekeeper)
    }

    fn apply_event_batch(
        &self,
        batch: &[SkillWatchEvent],
        engine: &mut SkillEngine,
        gatekeeper: &GatekeeperEngine,
    ) -> WatcherBatchSummary {
        let raw_events_received = batch.len();
        let mut seen_keys = BTreeSet::new();
        let mut unique_dirs: Vec<(PathBuf, u64)> = Vec::new();

        // Coalesce burst notifications per skill folder while preserving the latest timestamp
        for ev in batch.iter().rev() {
            match ev.kind {
                SkillWatchEventKind::ManifestModified => {
                    let key = normalize_os_path_for_jail(&ev.skill_dir);
                    if seen_keys.insert(key) {
                        unique_dirs.push((ev.skill_dir.clone(), ev.timestamp_ms));
                    }
                }
                SkillWatchEventKind::DirectoryRescan => {
                    if let Ok(entries) = std::fs::read_dir(&ev.skill_dir) {
                        for entry in entries.flatten() {
                            let p = entry.path();
                            if p.is_dir() && p.join(crate::SKILL_MANIFEST_FILENAME).is_file() {
                                let key = normalize_os_path_for_jail(&p);
                                if seen_keys.insert(key) {
                                    unique_dirs.push((p, ev.timestamp_ms));
                                }
                            }
                        }
                    }
                }
            }
        }
        unique_dirs.reverse();

        let mut reloads_applied = 0usize;
        let mut reloads_unchanged = 0usize;
        let mut reloads_rejected = 0usize;

        for (dir, ts) in &unique_dirs {
            match engine.load_or_reload_from_dir(dir, gatekeeper, *ts) {
                Ok(SkillReloadOutcome::Loaded { .. })
                | Ok(SkillReloadOutcome::Reloaded { .. }) => {
                    reloads_applied += 1;
                }
                Ok(SkillReloadOutcome::Unchanged { .. }) => {
                    reloads_unchanged += 1;
                }
                Err(_) => {
                    reloads_rejected += 1;
                }
            }
        }

        WatcherBatchSummary {
            raw_events_received,
            coalesced_dirs_processed: unique_dirs.len(),
            reloads_applied,
            reloads_unchanged,
            reloads_rejected,
        }
    }

    /// Deterministically stops the OS event watcher, signaling the Win32 stop event handle
    /// and joining the background kernel wait thread with zero leaked handles or threads.
    pub fn shutdown(&mut self) {
        if !self.running.swap(false, Ordering::SeqCst) {
            return;
        }
        #[cfg(target_os = "windows")]
        {
            if self.stop_event_raw != 0 {
                unsafe {
                    win32_ffi::SetEvent(self.stop_event_raw as win32_ffi::HANDLE);
                }
            }
        }
        if let Some(handle) = self.worker_thread.take() {
            let _ = handle.join();
        }
        #[cfg(target_os = "windows")]
        {
            if self.stop_event_raw != 0 {
                unsafe {
                    win32_ffi::CloseHandle(self.stop_event_raw as win32_ffi::HANDLE);
                }
                self.stop_event_raw = 0;
            }
        }
    }

    #[cfg(target_os = "windows")]
    fn spawn_win32_event_watcher(
        watched_root: PathBuf,
        event_tx: mpsc::UnboundedSender<SkillWatchEvent>,
        running: Arc<AtomicBool>,
        os_wakeup_count: Arc<AtomicUsize>,
    ) -> Result<(usize, JoinHandle<()>), SkillError> {
        use std::os::windows::ffi::OsStrExt;

        let mut wide_path: Vec<u16> = watched_root.as_os_str().encode_wide().collect();
        wide_path.push(0);

        let notify_filter = win32_ffi::FILE_NOTIFY_CHANGE_FILE_NAME
            | win32_ffi::FILE_NOTIFY_CHANGE_DIR_NAME
            | win32_ffi::FILE_NOTIFY_CHANGE_SIZE
            | win32_ffi::FILE_NOTIFY_CHANGE_LAST_WRITE;

        let change_handle = unsafe {
            win32_ffi::FindFirstChangeNotificationW(wide_path.as_ptr(), 1, notify_filter)
        };
        if change_handle == win32_ffi::INVALID_HANDLE_VALUE || change_handle.is_null() {
            return Err(SkillError::WatcherError(format!(
                "FindFirstChangeNotificationW failed for '{}'",
                watched_root.display()
            )));
        }

        let stop_event =
            unsafe { win32_ffi::CreateEventW(std::ptr::null(), 1, 0, std::ptr::null()) };
        if stop_event.is_null() || stop_event == win32_ffi::INVALID_HANDLE_VALUE {
            unsafe {
                win32_ffi::FindCloseChangeNotification(change_handle);
            }
            return Err(SkillError::WatcherError(
                "CreateEventW failed for watcher stop handle".to_string(),
            ));
        }

        let change_raw = change_handle as usize;
        let stop_raw = stop_event as usize;

        let join_handle = std::thread::Builder::new()
            .name("miku-skills-os-watcher".to_string())
            .spawn(move || {
                let handles = [
                    change_raw as win32_ffi::HANDLE,
                    stop_raw as win32_ffi::HANDLE,
                ];
                while running.load(Ordering::SeqCst) {
                    let wait_rc = unsafe {
                        win32_ffi::WaitForMultipleObjects(
                            2,
                            handles.as_ptr(),
                            0,
                            win32_ffi::INFINITE,
                        )
                    };
                    if wait_rc == win32_ffi::WAIT_OBJECT_0 {
                        if !running.load(Ordering::SeqCst) {
                            break;
                        }
                        os_wakeup_count.fetch_add(1, Ordering::SeqCst);
                        let _ = event_tx.send(SkillWatchEvent {
                            skill_dir: watched_root.clone(),
                            kind: SkillWatchEventKind::DirectoryRescan,
                            timestamp_ms: 0,
                        });
                        let next_ok = unsafe {
                            win32_ffi::FindNextChangeNotification(change_raw as win32_ffi::HANDLE)
                        };
                        if next_ok == 0 {
                            break;
                        }
                    } else {
                        break;
                    }
                }
                unsafe {
                    win32_ffi::FindCloseChangeNotification(change_raw as win32_ffi::HANDLE);
                }
            })
            .map_err(|e| SkillError::WatcherError(e.to_string()))?;

        Ok((stop_raw, join_handle))
    }
}

impl Drop for SkillFsWatcher {
    fn drop(&mut self) {
        self.shutdown();
    }
}

#[cfg(target_os = "windows")]
mod win32_ffi {
    pub type HANDLE = *mut core::ffi::c_void;
    pub type BOOL = i32;
    pub type DWORD = u32;
    pub type LPCWSTR = *const u16;

    pub const INVALID_HANDLE_VALUE: HANDLE = (-1isize) as HANDLE;
    pub const WAIT_OBJECT_0: DWORD = 0x0000_0000;
    pub const INFINITE: DWORD = 0xFFFF_FFFF;

    pub const FILE_NOTIFY_CHANGE_FILE_NAME: DWORD = 0x0000_0001;
    pub const FILE_NOTIFY_CHANGE_DIR_NAME: DWORD = 0x0000_0002;
    pub const FILE_NOTIFY_CHANGE_SIZE: DWORD = 0x0000_0008;
    pub const FILE_NOTIFY_CHANGE_LAST_WRITE: DWORD = 0x0000_0010;

    #[link(name = "kernel32")]
    extern "system" {
        pub fn FindFirstChangeNotificationW(
            lp_path_name: LPCWSTR,
            b_watch_subtree: BOOL,
            dw_notify_filter: DWORD,
        ) -> HANDLE;
        pub fn FindNextChangeNotification(h_change_handle: HANDLE) -> BOOL;
        pub fn FindCloseChangeNotification(h_change_handle: HANDLE) -> BOOL;
        pub fn CreateEventW(
            lp_event_attributes: *const core::ffi::c_void,
            b_manual_reset: BOOL,
            b_initial_state: BOOL,
            lp_name: LPCWSTR,
        ) -> HANDLE;
        pub fn SetEvent(h_event: HANDLE) -> BOOL;
        pub fn CloseHandle(h_object: HANDLE) -> BOOL;
        pub fn WaitForMultipleObjects(
            n_count: DWORD,
            lp_handles: *const HANDLE,
            b_wait_all: BOOL,
            dw_milliseconds: DWORD,
        ) -> DWORD;
    }
}
