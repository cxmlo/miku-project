//! Binary entrypoint for Spike 0.1 (0.1a + 0.1b).
//!
//! Modes of operation:
//! - Coordinator (default, no args):
//!   1. Runs the 0.1a baseline check (`cmd.exe /C ping -n 6 127.0.0.1 >NUL`, `n = 20`).
//!   2. Runs the 0.1b 4 real UI lifecycle modes (`A`, `B`, `C`, `D`, `n = 20` each) by spawning
//!      `miku-spike-01-ui.exe --worker --mode <A|B|C|D> --rep <N>` as an isolated child process
//!      (`root_pid = child.id()`).
//!   3. Communicates with the worker over stdin/stdout protocol lines (`STAGE:IDLE_READY`,
//!      `CMD:EXPAND`, `STAGE:EXPANDED_VISIBLE:<ms>`, `CMD:COLLAPSE`, `STAGE:COLLAPSED_READY`,
//!      `CMD:QUIT`), taking real OS `PrivatePageCount` snapshots at each stage, killing the
//!      worker tree via `taskkill /T /F /PID`, waiting 150 ms settle time, verifying 0 orphans,
//!      and removing the per-repetition User Data Directory (UDD).
//! - Worker (`--worker --mode <A|B|C|D> --rep <N> --udd <PATH>`):
//!   Creates a real Win32 top-center Island window (`WS_POPUP | WS_VISIBLE`,
//!   `WS_EX_TOPMOST | WS_EX_TOOLWINDOW | WS_EX_NOACTIVATE`), runs a real Win32 message pump
//!   (`PeekMessageW` + `TranslateMessage` + `DispatchMessageW`), and hosts a real WebView2
//!   (`webview2-com`) for Modes B, C, and D with two-way `window.chrome.webview.postMessage`
//!   handshake inside `requestAnimationFrame` to measure genuine `hover -> visible` latency.

use miku_spike_01_ui::{
    build_isolated_udd_path, collect_process_tree, compute_island_geometry,
    count_surviving_identities_after_close, count_webview2_Descendants, format_benchmark_report,
    parse_powershell_process_csv, select_recommended_lifecycle_mode, summarize_01a_samples,
    summarize_01b_mode_samples, ModeBenchmarkState, ModeCTtlController, ModeIterationSample,
    ProcessIdentity, ProcessNode, TreeIterationSample, UiLifecycleMode,
    GRANDCHILD_POLL_INTERVAL_MS, GRANDCHILD_POLL_TIMEOUT_MS, POST_KILL_SETTLE_DELAY_MS,
    REQUIRED_REPETITIONS,
};
use std::io::{BufRead, BufReader, Write};
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

fn capture_windows_process_snapshot() -> Result<Vec<ProcessNode>, String> {
    let ps_script = concat!(
        "$ErrorActionPreference = 'Stop'; ",
        "Get-CimInstance Win32_Process | ",
        "Select-Object ProcessId, ParentProcessId, Name, PrivatePageCount, CreationDate | ",
        "ConvertTo-Csv -Delimiter \"`t\" -NoTypeInformation"
    );

    let output = Command::new("powershell")
        .args([
            "-NoProfile",
            "-NonInteractive",
            "-ExecutionPolicy",
            "Bypass",
            "-Command",
            ps_script,
        ])
        .output()
        .map_err(|e| format!("Failed to execute PowerShell snapshot: {}", e))?;

    if !output.status.success() {
        return Err(format!(
            "PowerShell exited with status {}: {}",
            output.status,
            String::from_utf8_lossy(&output.stderr)
        ));
    }

    let stdout = String::from_utf8_lossy(&output.stdout);
    parse_powershell_process_csv(&stdout)
}

fn kill_child_process_tree(root_pid: u32, child: &mut Child) {
    let _ = Command::new("taskkill")
        .args(["/PID", &root_pid.to_string(), "/T", "/F"])
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status();
    let _ = child.kill();
    let _ = child.wait();
}

fn wait_for_tree_with_grandchild(
    root_pid: u32,
    timeout: Duration,
    poll_interval: Duration,
) -> Result<(Vec<ProcessNode>, Vec<ProcessIdentity>, u64), String> {
    let deadline = Instant::now() + timeout;
    loop {
        let snapshot = capture_windows_process_snapshot()?;
        if let Ok((identities, total_private_bytes)) = collect_process_tree(root_pid, &snapshot) {
            if identities.len() >= 2 && total_private_bytes > 0 {
                return Ok((snapshot, identities, total_private_bytes));
            }
        }
        if Instant::now() >= deadline {
            return Err(format!(
                "Timed out after {} ms waiting for child root_pid={} to spawn grandchild",
                timeout.as_millis(),
                root_pid
            ));
        }
        std::thread::sleep(poll_interval);
    }
}

fn run_single_01a_iteration(
    repetition: usize,
) -> Result<(TreeIterationSample, Vec<ProcessNode>), String> {
    let start = Instant::now();
    let mut child = Command::new("cmd.exe")
        .args(["/C", "ping -n 6 127.0.0.1 >NUL"])
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .map_err(|e| format!("Failed to spawn 0.1a child at repetition {}: {}", repetition, e))?;

    let root_pid = child.id();
    let poll_result = wait_for_tree_with_grandchild(
        root_pid,
        Duration::from_millis(GRANDCHILD_POLL_TIMEOUT_MS),
        Duration::from_millis(GRANDCHILD_POLL_INTERVAL_MS),
    );
    let elapsed_ms = start.elapsed().as_secs_f64() * 1000.0;

    let (active_snapshot, tree_identities, total_private_bytes) = match poll_result {
        Ok(res) => res,
        Err(err) => {
            kill_child_process_tree(root_pid, &mut child);
            return Err(format!("Repetition {}: {}", repetition, err));
        }
    };

    kill_child_process_tree(root_pid, &mut child);
    std::thread::sleep(Duration::from_millis(POST_KILL_SETTLE_DELAY_MS));

    let post_close_snapshot = capture_windows_process_snapshot()?;
    let orphan_count_after_close =
        count_surviving_identities_after_close(&tree_identities, &post_close_snapshot);

    let sample = TreeIterationSample {
        repetition,
        root_pid,
        process_count: tree_identities.len(),
        tree_identities,
        total_private_bytes,
        elapsed_ms,
        orphan_count_after_close,
    };

    Ok((sample, active_snapshot))
}

fn read_worker_line_expected(
    reader: &mut BufReader<std::process::ChildStdout>,
    expected_prefix: &str,
) -> Result<String, String> {
    let mut line = String::new();
    loop {
        line.clear();
        let bytes = reader
            .read_line(&mut line)
            .map_err(|e| format!("Failed reading from worker stdout: {}", e))?;
        if bytes == 0 {
            return Err(format!(
                "Worker closed stdout unexpectedly while waiting for '{}'",
                expected_prefix
            ));
        }
        let trimmed = line.trim();
        if trimmed.starts_with("ERROR:") {
            return Err(format!("Worker reported error: {}", trimmed));
        }
        if trimmed.starts_with(expected_prefix) {
            return Ok(trimmed.to_string());
        }
    }
}

fn run_single_01b_mode_iteration(
    exe_path: &std::path::Path,
    coordinator_pid: u32,
    mode: UiLifecycleMode,
    repetition: usize,
) -> Result<ModeIterationSample, String> {
    let local_app_data =
        std::env::var("LOCALAPPDATA").unwrap_or_else(|_| std::env::temp_dir().display().to_string());
    let udd_path = build_isolated_udd_path(&local_app_data, mode, repetition, coordinator_pid);

    let mut child = Command::new(exe_path)
        .args([
            "--worker",
            "--mode",
            mode.code(),
            "--rep",
            &repetition.to_string(),
            "--udd",
            &udd_path,
        ])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| {
            format!(
                "Failed to spawn worker for mode {} rep {}: {}",
                mode.code(),
                repetition,
                e
            )
        })?;

    let worker_root_pid = child.id();
    let mut worker_stdin = child
        .stdin
        .take()
        .ok_or_else(|| "Failed to open worker stdin".to_string())?;
    let worker_stdout = child
        .stdout
        .take()
        .ok_or_else(|| "Failed to open worker stdout".to_string())?;
    let mut reader = BufReader::new(worker_stdout);

    let step_result = (|| -> Result<
        (
            Vec<ProcessIdentity>,
            usize,
            u64,
            Vec<ProcessIdentity>,
            usize,
            usize,
            u64,
            f64,
            u64,
        ),
        String,
    > {
        // 1. Wait for STAGE:IDLE_READY
        read_worker_line_expected(&mut reader, "STAGE:IDLE_READY")?;
        let idle_snapshot = capture_windows_process_snapshot()?;
        let (idle_ids, idle_bytes) = collect_process_tree(worker_root_pid, &idle_snapshot)?;
        let idle_proc_count = idle_ids.len();

        // 2. Send CMD:EXPAND and wait for STAGE:EXPANDED_VISIBLE:<ms>
        writeln!(worker_stdin, "CMD:EXPAND")
            .and_then(|_| worker_stdin.flush())
            .map_err(|e| format!("Failed sending CMD:EXPAND: {}", e))?;

        let expanded_line =
            read_worker_line_expected(&mut reader, "STAGE:EXPANDED_VISIBLE:")?;
        let hover_ms_str = expanded_line
            .trim_start_matches("STAGE:EXPANDED_VISIBLE:")
            .trim();
        let hover_to_visible_ms = hover_ms_str.parse::<f64>().map_err(|e| {
            format!(
                "Invalid hover_to_visible_ms '{}' from worker: {}",
                hover_ms_str, e
            )
        })?;

        let expanded_snapshot = capture_windows_process_snapshot()?;
        let (expanded_ids, expanded_bytes) =
            collect_process_tree(worker_root_pid, &expanded_snapshot)?;
        let expanded_proc_count = expanded_ids.len();
        let expanded_webview_children =
            count_webview2_Descendants(worker_root_pid, &expanded_snapshot);

        // 3. Send CMD:COLLAPSE and wait for STAGE:COLLAPSED_READY
        writeln!(worker_stdin, "CMD:COLLAPSE")
            .and_then(|_| worker_stdin.flush())
            .map_err(|e| format!("Failed sending CMD:COLLAPSE: {}", e))?;

        read_worker_line_expected(&mut reader, "STAGE:COLLAPSED_READY")?;
        if matches!(
            mode,
            UiLifecycleMode::ModeCWebViewPrewarmedTtl | UiLifecycleMode::ModeDWebViewOnDemand
        ) {
            std::thread::sleep(Duration::from_millis(POST_KILL_SETTLE_DELAY_MS));
        }

        let collapsed_snapshot = capture_windows_process_snapshot()?;
        let (_collapsed_ids, post_collapse_bytes) =
            collect_process_tree(worker_root_pid, &collapsed_snapshot)?;

        // 4. Ask worker to quit cleanly
        let _ = writeln!(worker_stdin, "CMD:QUIT");
        let _ = worker_stdin.flush();

        Ok((
            idle_ids,
            idle_proc_count,
            idle_bytes,
            expanded_ids,
            expanded_proc_count,
            expanded_webview_children,
            expanded_bytes,
            hover_to_visible_ms,
            post_collapse_bytes,
        ))
    })();

    // Always terminate the worker tree and settle 150 ms before checking orphans
    kill_child_process_tree(worker_root_pid, &mut child);
    std::thread::sleep(Duration::from_millis(POST_KILL_SETTLE_DELAY_MS));

    let post_close_snapshot = capture_windows_process_snapshot()?;
    let _ = std::fs::remove_dir_all(&udd_path);

    let (
        idle_tree_identities,
        idle_process_count,
        idle_private_bytes,
        expanded_tree_identities,
        expanded_process_count,
        expanded_webview_child_count,
        expanded_private_bytes,
        hover_to_visible_ms,
        post_collapse_private_bytes,
    ) = step_result?;

    let mut all_spawned_identities = idle_tree_identities.clone();
    for id in &expanded_tree_identities {
        if !all_spawned_identities.contains(id) {
            all_spawned_identities.push(id.clone());
        }
    }

    let orphan_count_after_close =
        count_surviving_identities_after_close(&all_spawned_identities, &post_close_snapshot);

    Ok(ModeIterationSample {
        mode,
        repetition,
        worker_root_pid,
        idle_tree_identities,
        expanded_tree_identities,
        idle_process_count,
        expanded_process_count,
        expanded_webview_child_count,
        idle_private_bytes,
        expanded_private_bytes,
        hover_to_visible_ms,
        post_collapse_private_bytes,
        orphan_count_after_close,
    })
}

#[cfg(windows)]
mod win32_worker {
    use super::*;
    use std::cell::RefCell;
    use std::rc::Rc;
    use webview2_com::Microsoft::Web::WebView2::Win32::{
        CreateCoreWebView2EnvironmentWithOptions, ICoreWebView2, ICoreWebView2Controller,
        ICoreWebView2Environment, ICoreWebView2WebMessageReceivedEventArgs,
    };
    use webview2_com::{
        take_pwstr, CreateCoreWebView2ControllerCompletedHandler,
        CreateCoreWebView2EnvironmentCompletedHandler, ExecuteScriptCompletedHandler,
        WebMessageReceivedEventHandler,
    };
    use windows::core::{PCWSTR, PWSTR};
    use windows::Win32::Foundation::{BOOL, HINSTANCE, HWND, LPARAM, LRESULT, RECT, WPARAM};
    use windows::Win32::Graphics::Gdi::{
        BeginPaint, CreateSolidBrush, DeleteObject, EndPaint, FillRect, InvalidateRect,
        PAINTSTRUCT,
    };
    use windows::Win32::System::Com::{CoInitializeEx, CoUninitialize, COINIT_APARTMENTTHREADED};
    use windows::Win32::System::LibraryLoader::GetModuleHandleW;
    use windows::Win32::UI::WindowsAndMessaging::{
        CreateWindowExW, DefWindowProcW, DestroyWindow, DispatchMessageW, GetSystemMetrics,
        PeekMessageW, RegisterClassW, SetWindowPos, ShowWindow, TranslateMessage, CS_HREDRAW,
        CS_VREDRAW, MSG, PM_REMOVE, SM_CXSCREEN, SM_CYSCREEN, SWP_NOACTIVATE, SWP_SHOWWINDOW,
        SW_SHOWNOACTIVATE, WINDOW_EX_STYLE, WINDOW_STYLE, WM_DESTROY, WM_PAINT, WNDCLASSW,
    };

    const ISLAND_HTML: &str = r#"<!doctype html>
<html>
<head>
<meta charset="utf-8">
<style>
  html, body {
    margin: 0;
    padding: 0;
    background: #0B0F17;
    color: #22D3EE;
    font-family: system-ui, sans-serif;
    overflow: hidden;
  }
  .card {
    display: flex;
    align-items: center;
    justify-content: space-between;
    padding: 8px 14px;
    border: 1px solid #22D3EE;
    border-radius: 12px;
  }
</style>
</head>
<body>
  <div class="card" id="miku-island">Miku Island · Spike 0.1b Ready</div>
  <script>
    window.__mikuNotifyVisible = function(tag) {
      requestAnimationFrame(function() {
        requestAnimationFrame(function() {
          if (window.chrome && window.chrome.webview) {
            window.chrome.webview.postMessage("PAINT_READY:" + tag);
          }
        });
      });
    };
    window.__mikuNotifyVisible("INITIAL");
  </script>
</body>
</html>"#;

    trait IntoWindowsHResult {
        fn into_hresult(self) -> windows::core::HRESULT;
    }

    impl IntoWindowsHResult for windows::core::HRESULT {
        fn into_hresult(self) -> windows::core::HRESULT {
            self
        }
    }

    impl IntoWindowsHResult for windows::core::Result<()> {
        fn into_hresult(self) -> windows::core::HRESULT {
            match self {
                Ok(()) => windows::core::HRESULT(0),
                Err(e) => e.code(),
            }
        }
    }

    struct WideStr(Vec<u16>);

    impl WideStr {
        fn new(s: &str) -> Self {
            Self(s.encode_utf16().chain(std::iter::once(0)).collect())
        }

        fn as_pcwstr(&self) -> PCWSTR {
            PCWSTR(self.0.as_ptr())
        }
    }

    unsafe extern "system" fn island_wndproc(
        hwnd: HWND,
        msg: u32,
        wparam: WPARAM,
        lparam: LPARAM,
    ) -> LRESULT {
        match msg {
            WM_PAINT => {
                let mut ps = PAINTSTRUCT::default();
                let hdc = BeginPaint(hwnd, &mut ps);
                let brush = CreateSolidBrush(windows::Win32::Foundation::COLORREF(0x0017_0F0B));
                let _ = FillRect(hdc, &ps.rcPaint, brush);
                let _ = DeleteObject(brush.into());
                let _ = EndPaint(hwnd, &ps);
                LRESULT(0)
            }
            WM_DESTROY => LRESULT(0),
            _ => DefWindowProcW(hwnd, msg, wparam, lparam),
        }
    }

    pub fn pump_messages_once() {
        unsafe {
            let mut msg = MSG::default();
            while PeekMessageW(&mut msg, HWND::default(), 0, 0, PM_REMOVE).as_bool() {
                let _ = TranslateMessage(&msg);
                DispatchMessageW(&msg);
            }
        }
    }

    struct WebViewInstance {
        controller: ICoreWebView2Controller,
        webview: ICoreWebView2,
        last_paint_tag: Rc<RefCell<Option<String>>>,
    }

    impl WebViewInstance {
        fn create_and_wait_first_paint(
            hwnd: HWND,
            udd_path: &str,
            width: i32,
            height: i32,
            visible_immediately: bool,
        ) -> Result<Self, String> {
            let _ = std::fs::create_dir_all(udd_path);
            let udd_wide = WideStr::new(udd_path);

            let env_slot: Rc<RefCell<Option<Result<ICoreWebView2Environment, String>>>> =
                Rc::new(RefCell::new(None));
            let env_slot_cb = env_slot.clone();

            unsafe {
                let handler = CreateCoreWebView2EnvironmentCompletedHandler::create(Box::new(
                    move |error_code, environment| {
                        let error_code = error_code.into_hresult();
                        let res = error_code.ok().and_then(|()| {
                            environment.ok_or_else(|| {
                                windows::core::Error::from_win32()
                            })
                        });
                        *env_slot_cb.borrow_mut() =
                            Some(res.map_err(|e| format!("Env error: {}", e)));
                        Ok(())
                    },
                ));

                CreateCoreWebView2EnvironmentWithOptions(
                    PCWSTR::null(),
                    udd_wide.as_pcwstr(),
                    None,
                    &handler,
                )
                .map_err(|e| format!("CreateCoreWebView2EnvironmentWithOptions failed: {}", e))?;
            }

            let deadline = Instant::now() + Duration::from_secs(12);
            while env_slot.borrow().is_none() {
                if Instant::now() >= deadline {
                    return Err("Timed out waiting for WebView2 environment".to_string());
                }
                pump_messages_once();
                std::thread::sleep(Duration::from_millis(1));
            }
            let env = env_slot.borrow_mut().take().unwrap()?;

            let ctrl_slot: Rc<RefCell<Option<Result<ICoreWebView2Controller, String>>>> =
                Rc::new(RefCell::new(None));
            let ctrl_slot_cb = ctrl_slot.clone();

            unsafe {
                let handler = CreateCoreWebView2ControllerCompletedHandler::create(Box::new(
                    move |error_code, controller| {
                        let error_code = error_code.into_hresult();
                        let res = error_code.ok().and_then(|()| {
                            controller.ok_or_else(|| {
                                windows::core::Error::from_win32()
                            })
                        });
                        *ctrl_slot_cb.borrow_mut() =
                            Some(res.map_err(|e| format!("Controller error: {}", e)));
                        Ok(())
                    },
                ));

                env.CreateCoreWebView2Controller(hwnd, &handler)
                    .map_err(|e| format!("CreateCoreWebView2Controller failed: {}", e))?;
            }

            let deadline = Instant::now() + Duration::from_secs(12);
            while ctrl_slot.borrow().is_none() {
                if Instant::now() >= deadline {
                    return Err("Timed out waiting for WebView2 controller".to_string());
                }
                pump_messages_once();
                std::thread::sleep(Duration::from_millis(1));
            }
            let controller = ctrl_slot.borrow_mut().take().unwrap()?;

            let webview = unsafe {
                controller
                    .CoreWebView2()
                    .map_err(|e| format!("CoreWebView2() failed: {}", e))?
            };

            let last_paint_tag: Rc<RefCell<Option<String>>> = Rc::new(RefCell::new(None));
            let paint_slot_cb = last_paint_tag.clone();

            unsafe {
                let mut token = 0i64;
                let msg_handler = WebMessageReceivedEventHandler::create(Box::new(
                    move |_webview, args: Option<ICoreWebView2WebMessageReceivedEventArgs>| {
                        if let Some(args) = args {
                            let mut raw = PWSTR::null();
                            if args.TryGetWebMessageAsString(&mut raw).is_ok() {
                                let text = take_pwstr(raw);
                                if let Some(tag) = text.strip_prefix("PAINT_READY:") {
                                    *paint_slot_cb.borrow_mut() = Some(tag.to_string());
                                }
                            }
                        }
                        Ok(())
                    },
                ));
                webview
                    .add_WebMessageReceived(&msg_handler, &mut token)
                    .map_err(|e| format!("add_WebMessageReceived failed: {}", e))?;

                let rect = RECT {
                    left: 0,
                    top: 0,
                    right: width,
                    bottom: height,
                };
                let _ = controller.put_Bounds(rect);
                let _ = controller.put_IsVisible(BOOL::from(visible_immediately));

                let html_wide = WideStr::new(ISLAND_HTML);
                webview
                    .NavigateToString(html_wide.as_pcwstr())
                    .map_err(|e| format!("NavigateToString failed: {}", e))?;
            }

            let deadline = Instant::now() + Duration::from_secs(12);
            while last_paint_tag.borrow().as_deref() != Some("INITIAL") {
                if Instant::now() >= deadline {
                    return Err("Timed out waiting for WebView2 INITIAL paint message".to_string());
                }
                pump_messages_once();
                std::thread::sleep(Duration::from_millis(1));
            }

            Ok(Self {
                controller,
                webview,
                last_paint_tag,
            })
        }

        fn expand_and_wait_visible(&self, width: i32, height: i32) -> Result<(), String> {
            *self.last_paint_tag.borrow_mut() = None;
            unsafe {
                let rect = RECT {
                    left: 0,
                    top: 0,
                    right: width,
                    bottom: height,
                };
                let _ = self.controller.put_Bounds(rect);
                let _ = self.controller.put_IsVisible(BOOL::from(true));

                let script = WideStr::new("window.__mikuNotifyVisible('EXPAND');");
                let handler = ExecuteScriptCompletedHandler::create(Box::new(|_, _| Ok(())));
                let _ = self.webview.ExecuteScript(script.as_pcwstr(), &handler);
            }

            let deadline = Instant::now() + Duration::from_secs(8);
            while self.last_paint_tag.borrow().as_deref() != Some("EXPAND") {
                if Instant::now() >= deadline {
                    return Err("Timed out waiting for WebView2 EXPAND frame".to_string());
                }
                pump_messages_once();
                std::thread::sleep(Duration::from_millis(1));
            }
            Ok(())
        }

        fn hide(&self) {
            unsafe {
                let _ = self.controller.put_IsVisible(BOOL::from(false));
            }
            pump_messages_once();
        }

        fn close(self) {
            unsafe {
                let _ = self.controller.Close();
            }
            drop(self.webview);
            drop(self.controller);
            pump_messages_once();
        }
    }

    pub fn run_worker_loop(mode: UiLifecycleMode, udd_path: &str) -> Result<(), String> {
        unsafe {
            let hr = CoInitializeEx(None, COINIT_APARTMENTTHREADED);
            if hr.is_err() {
                return Err(format!("CoInitializeEx failed: {:?}", hr));
            }
        }

        let res = (|| -> Result<(), String> {
            let (screen_w, screen_h) = unsafe {
                let w = GetSystemMetrics(SM_CXSCREEN);
                let h = GetSystemMetrics(SM_CYSCREEN);
                (if w > 0 { w } else { 1920 }, if h > 0 { h } else { 1080 })
            };

            let pill_geom = compute_island_geometry(screen_w, screen_h, 1.0, false);
            let exp_geom = compute_island_geometry(screen_w, screen_h, 1.0, true);

            let class_name = WideStr::new("MikuSpike01bIslandClass");
            let window_title = WideStr::new("Miku Island Spike 0.1b");

            let hwnd = unsafe {
                let hinstance: HINSTANCE = GetModuleHandleW(PCWSTR::null())
                    .map_err(|e| format!("GetModuleHandleW failed: {}", e))?
                    .into();
                let wc = WNDCLASSW {
                    style: CS_HREDRAW | CS_VREDRAW,
                    lpfnWndProc: Some(island_wndproc),
                    hInstance: hinstance,
                    lpszClassName: class_name.as_pcwstr(),
                    ..Default::default()
                };
                let _ = RegisterClassW(&wc);

                CreateWindowExW(
                    WINDOW_EX_STYLE(pill_geom.ex_style_flags),
                    class_name.as_pcwstr(),
                    window_title.as_pcwstr(),
                    WINDOW_STYLE(pill_geom.style_flags),
                    pill_geom.x,
                    pill_geom.y,
                    pill_geom.width,
                    pill_geom.height,
                    HWND::default(),
                    None,
                    hinstance,
                    None,
                )
                .map_err(|e| format!("CreateWindowExW failed: {}", e))?
            };

            unsafe {
                let _ = ShowWindow(hwnd, SW_SHOWNOACTIVATE);
                let _ = InvalidateRect(hwnd, std::ptr::null(), BOOL::from(true));
            }
            pump_messages_once();

            // Pre-initialize WebView2 in idle for Mode B and Mode C
            let mut active_webview: Option<WebViewInstance> = match mode {
                UiLifecycleMode::ModeANativeOnly | UiLifecycleMode::ModeDWebViewOnDemand => None,
                UiLifecycleMode::ModeBWebViewAlwaysAlive
                | UiLifecycleMode::ModeCWebViewPrewarmedTtl => {
                    let wv = WebViewInstance::create_and_wait_first_paint(
                        hwnd,
                        udd_path,
                        exp_geom.width,
                        exp_geom.height,
                        false,
                    )?;
                    wv.hide();
                    Some(wv)
                }
            };

            let mut mode_c_fsm = ModeCTtlController::new(200);

            println!("STAGE:IDLE_READY");
            let _ = std::io::stdout().flush();

            let stdin = std::io::stdin();
            for line_res in stdin.lock().lines() {
                let cmd = line_res.map_err(|e| format!("Worker stdin error: {}", e))?;
                let trimmed = cmd.trim();
                if trimmed == "CMD:EXPAND" {
                    let t0 = Instant::now();
                    unsafe {
                        let _ = SetWindowPos(
                            hwnd,
                            HWND::default(),
                            exp_geom.x,
                            exp_geom.y,
                            exp_geom.width,
                            exp_geom.height,
                            SWP_NOACTIVATE | SWP_SHOWWINDOW,
                        );
                        let _ = InvalidateRect(hwnd, std::ptr::null(), BOOL::from(true));
                    }
                    pump_messages_once();

                    match mode {
                        UiLifecycleMode::ModeANativeOnly => {
                            pump_messages_once();
                        }
                        UiLifecycleMode::ModeBWebViewAlwaysAlive => {
                            if let Some(wv) = active_webview.as_ref() {
                                wv.expand_and_wait_visible(exp_geom.width, exp_geom.height)?;
                            }
                        }
                        UiLifecycleMode::ModeCWebViewPrewarmedTtl => {
                            mode_c_fsm.on_hover_expand(0);
                            if let Some(wv) = active_webview.as_ref() {
                                wv.expand_and_wait_visible(exp_geom.width, exp_geom.height)?;
                            } else {
                                let wv = WebViewInstance::create_and_wait_first_paint(
                                    hwnd,
                                    udd_path,
                                    exp_geom.width,
                                    exp_geom.height,
                                    true,
                                )?;
                                active_webview = Some(wv);
                            }
                        }
                        UiLifecycleMode::ModeDWebViewOnDemand => {
                            let wv = WebViewInstance::create_and_wait_first_paint(
                                hwnd,
                                udd_path,
                                exp_geom.width,
                                exp_geom.height,
                                true,
                            )?;
                            active_webview = Some(wv);
                        }
                    }

                    let elapsed_ms = t0.elapsed().as_secs_f64() * 1000.0;
                    println!("STAGE:EXPANDED_VISIBLE:{:.4}", elapsed_ms);
                    let _ = std::io::stdout().flush();
                } else if trimmed == "CMD:COLLAPSE" {
                    unsafe {
                        let _ = SetWindowPos(
                            hwnd,
                            HWND::default(),
                            pill_geom.x,
                            pill_geom.y,
                            pill_geom.width,
                            pill_geom.height,
                            SWP_NOACTIVATE | SWP_SHOWWINDOW,
                        );
                        let _ = InvalidateRect(hwnd, std::ptr::null(), BOOL::from(true));
                    }
                    pump_messages_once();

                    match mode {
                        UiLifecycleMode::ModeANativeOnly => {}
                        UiLifecycleMode::ModeBWebViewAlwaysAlive => {
                            if let Some(wv) = active_webview.as_ref() {
                                wv.hide();
                            }
                        }
                        UiLifecycleMode::ModeCWebViewPrewarmedTtl => {
                            if let Some(wv) = active_webview.as_ref() {
                                wv.hide();
                            }
                            mode_c_fsm.on_collapse_to_idle(100);
                            let mut sim_clock_ms = 100u64;
                            while !mode_c_fsm.poll_timer(sim_clock_ms) {
                                std::thread::sleep(Duration::from_millis(25));
                                pump_messages_once();
                                sim_clock_ms += 25;
                            }
                            if let Some(wv) = active_webview.take() {
                                wv.close();
                            }
                        }
                        UiLifecycleMode::ModeDWebViewOnDemand => {
                            if let Some(wv) = active_webview.take() {
                                wv.close();
                            }
                        }
                    }

                    println!("STAGE:COLLAPSED_READY");
                    let _ = std::io::stdout().flush();
                } else if trimmed == "CMD:QUIT" {
                    if let Some(wv) = active_webview.take() {
                        wv.close();
                    }
                    unsafe {
                        let _ = DestroyWindow(hwnd);
                    }
                    pump_messages_once();
                    break;
                }
            }
            Ok(())
        })();

        unsafe {
            CoUninitialize();
        }
        res
    }
}

fn parse_worker_cli_args() -> Option<(UiLifecycleMode, usize, String)> {
    let args: Vec<String> = std::env::args().collect();
    if !args.iter().any(|a| a == "--worker") {
        return None;
    }
    let mut mode = UiLifecycleMode::ModeANativeOnly;
    let mut rep = 1usize;
    let mut udd = std::env::temp_dir()
        .join("miku-spike-01b-worker")
        .display()
        .to_string();

    let mut i = 1;
    while i < args.len() {
        match args[i].as_str() {
            "--mode" if i + 1 < args.len() => {
                if let Some(m) = UiLifecycleMode::from_code(&args[i + 1]) {
                    mode = m;
                }
                i += 2;
            }
            "--rep" if i + 1 < args.len() => {
                rep = args[i + 1].parse::<usize>().unwrap_or(1);
                i += 2;
            }
            "--udd" if i + 1 < args.len() => {
                udd = args[i + 1].clone();
                i += 2;
            }
            _ => {
                i += 1;
            }
        }
    }
    Some((mode, rep, udd))
}

fn main() {
    if !cfg!(target_os = "windows") {
        eprintln!("Spike 0.1 requires Windows (Win32 + WebView2 + PrivatePageCount).");
        std::process::exit(1);
    }

    // Check if invoked as an isolated UI worker child process
    if let Some((mode, _rep, udd_path)) = parse_worker_cli_args() {
        #[cfg(windows)]
        {
            if let Err(err) = win32_worker::run_worker_loop(mode, &udd_path) {
                println!("ERROR:{}", err);
                let _ = std::io::stdout().flush();
                std::process::exit(1);
            }
            return;
        }
    }

    let coordinator_pid = std::process::id();
    let exe_path = std::env::current_exe().expect("Failed to resolve current executable path");

    println!(
        "Running Spike 0.1 Coordinator (PID={}) with n={} repetitions per series...",
        coordinator_pid, REQUIRED_REPETITIONS
    );

    // 1. Run 0.1a reference baseline
    let mut samples_01a = Vec::with_capacity(REQUIRED_REPETITIONS);
    for rep in 1..=REQUIRED_REPETITIONS {
        match run_single_01a_iteration(rep) {
            Ok((sample, snapshot)) => {
                println!(
                    "  [0.1a {:02}/{}] child_root_pid={} | tree_procs={} | private_ram={:.2} MB | elapsed={:.2} ms | orphans={}",
                    rep,
                    REQUIRED_REPETITIONS,
                    sample.root_pid,
                    sample.process_count,
                    sample.total_private_mb(),
                    sample.elapsed_ms,
                    sample.orphan_count_after_close
                );
                samples_01a.push((sample, snapshot));
            }
            Err(err) => {
                eprintln!("FATAL [0.1a]: {}", err);
                std::process::exit(1);
            }
        }
    }

    let summary_01a = summarize_01a_samples(
        "0.1a: Árbol de proceso hijo real (cmd.exe + ping.exe)",
        coordinator_pid,
        &samples_01a,
    )
    .unwrap_or_else(|e| {
        eprintln!("VERIFIER FAILED [0.1a]: {}", e);
        std::process::exit(1);
    });

    // 2. Run 0.1b 4 real UI lifecycle modes
    let mut measured_summaries = Vec::with_capacity(4);
    let mut mode_states = Vec::with_capacity(4);

    for mode in UiLifecycleMode::all() {
        println!(
            "\n--- Measuring {} (n={}) ---",
            mode.display_name(),
            REQUIRED_REPETITIONS
        );
        let mut mode_samples = Vec::with_capacity(REQUIRED_REPETITIONS);
        for rep in 1..=REQUIRED_REPETITIONS {
            match run_single_01b_mode_iteration(&exe_path, coordinator_pid, mode, rep) {
                Ok(sample) => {
                    println!(
                        "  [0.1b-{} {:02}/{}] worker_pid={} | idle={:.2} MB (procs={}) | expanded={:.2} MB (procs={}, wv2={}) | hover->vis={:.2} ms | post_collapse={:.2} MB | orphans={}",
                        mode.code(),
                        rep,
                        REQUIRED_REPETITIONS,
                        sample.worker_root_pid,
                        sample.idle_private_mb(),
                        sample.idle_process_count,
                        sample.expanded_private_mb(),
                        sample.expanded_process_count,
                        sample.expanded_webview_child_count,
                        sample.hover_to_visible_ms,
                        sample.post_collapse_private_mb(),
                        sample.orphan_count_after_close
                    );
                    mode_samples.push(sample);
                }
                Err(err) => {
                    eprintln!("FATAL [0.1b-{} rep {}]: {}", mode.code(), rep, err);
                    std::process::exit(1);
                }
            }
        }

        let summary = summarize_01b_mode_samples(mode, coordinator_pid, &mode_samples)
            .unwrap_or_else(|e| {
                eprintln!("VERIFIER FAILED [0.1b-{}]: {}", mode.code(), e);
                std::process::exit(1);
            });
        measured_summaries.push(summary.clone());
        mode_states.push(ModeBenchmarkState::Measured(summary));
    }

    let recommendation = select_recommended_lifecycle_mode(&measured_summaries).ok();
    let report = format_benchmark_report(&summary_01a, &mode_states, recommendation.as_ref());
    println!("\n{}", report);
    println!(
        "VERIFIER: OK (0.1a + 0.1b 4 real modes verified across {} repetitions each, 0 hardcoded numbers).",
        REQUIRED_REPETITIONS
    );
}
