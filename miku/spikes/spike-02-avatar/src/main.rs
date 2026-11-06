//! Binary entrypoint for Spike 0.2 — Procedural HTML5 Canvas 2D Avatar & Sleeping Springs Audit.
//!
//! Architecture:
//! - Coordinator (default, no args):
//!   Spawns `miku-spike-02-avatar.exe --worker --udd <PATH>` as an isolated child process
//!   (`root_pid = child.id()`), drives the 4 avatar states (`thinking`, `speaking`, `alert`, `idle`),
//!   runs a 2000 ms rest audit in `idle` with blink suspended to certify `0.00 fps` (`raf_count == 0`),
//!   verifies Mode C post-TTL collapse RAM (`<= 30 MB`), and checks `0` orphan processes after exit.
//! - Worker (`--worker --udd <PATH>`):
//!   Creates a real Win32 top-center Island window, initializes WebView2 (`webview2-com 0.33`),
//!   loads the self-contained procedural Canvas 2D HTML generated from `assets/rig.toml` and
//!   `assets/design-tokens.json` (with `window.devicePixelRatio` + `ctx.scale(dpr, dpr)` and strict CSP),
//!   and bridges commands/events over stdin/stdout + `WebMessageReceived`.

use miku_spike_02_avatar::{
    build_avatar_worker_html, collect_process_tree, count_surviving_identities_after_close,
    format_spike02_report, parse_powershell_process_csv, AvatarState, DesignTokens,
    ProcessIdentity, ProcessNode, RigConfig, SleepAuditPayload, Spike02AuditSummary,
    StateTransitionMetric, DEFAULT_DESIGN_TOKENS_JSON, DEFAULT_RIG_TOML,
    POST_KILL_SETTLE_DELAY_MS, SLEEP_AUDIT_WINDOW_MS,
};
use std::io::{BufRead, BufReader, Write};
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

fn load_rig_and_tokens() -> Result<(RigConfig, DesignTokens), String> {
    let rig_raw = std::fs::read_to_string("assets/rig.toml")
        .unwrap_or_else(|_| DEFAULT_RIG_TOML.to_string());
    let tokens_raw = std::fs::read_to_string("assets/design-tokens.json")
        .unwrap_or_else(|_| DEFAULT_DESIGN_TOKENS_JSON.to_string());

    let rig = RigConfig::from_toml_str(&rig_raw)?;
    let tokens = DesignTokens::from_json_str(&tokens_raw)?;
    Ok((rig, tokens))
}

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

pub fn parse_first_paint_line(line: &str) -> Result<(f64, f64), String> {
    let rest = line
        .strip_prefix("STAGE:FIRST_PAINT:")
        .ok_or_else(|| format!("Unexpected first paint line: {}", line))?;
    let parts: Vec<&str> = rest.split(':').collect();
    if parts.len() < 2 {
        return Err(format!("Malformed STAGE:FIRST_PAINT payload: {}", line));
    }
    let ms = parts[0]
        .parse::<f64>()
        .map_err(|e| format!("Invalid first_paint_ms '{}': {}", parts[0], e))?;
    let dpr = parts[1]
        .parse::<f64>()
        .map_err(|e| format!("Invalid dpr '{}': {}", parts[1], e))?;
    Ok((ms, dpr))
}

pub fn parse_state_settled_line(line: &str) -> Result<StateTransitionMetric, String> {
    let rest = line
        .strip_prefix("STAGE:STATE_SETTLED:")
        .ok_or_else(|| format!("Unexpected state settled line: {}", line))?;
    let parts: Vec<&str> = rest.split(':').collect();
    if parts.len() < 3 {
        return Err(format!("Malformed STAGE:STATE_SETTLED payload: {}", line));
    }
    let state = AvatarState::from_name(parts[0])
        .ok_or_else(|| format!("Unknown avatar state '{}'", parts[0]))?;
    let settle_ms = parts[1]
        .parse::<f64>()
        .map_err(|e| format!("Invalid settle_ms '{}': {}", parts[1], e))?;
    let frames_rendered = parts[2]
        .parse::<u64>()
        .map_err(|e| format!("Invalid frames_rendered '{}': {}", parts[2], e))?;
    Ok(StateTransitionMetric {
        state,
        settle_ms,
        frames_rendered,
    })
}

pub fn parse_sleep_audit_line(line: &str) -> Result<SleepAuditPayload, String> {
    let json_part = line
        .strip_prefix("STAGE:SLEEP_AUDIT_DONE:")
        .ok_or_else(|| format!("Unexpected sleep audit line: {}", line))?;
    serde_json::from_str::<SleepAuditPayload>(json_part)
        .map_err(|e| format!("Failed parsing SleepAuditPayload JSON '{}': {}", json_part, e))
}

fn run_coordinator_spike02(
    exe_path: &std::path::Path,
    coordinator_pid: u32,
) -> Result<Spike02AuditSummary, String> {
    let local_app_data =
        std::env::var("LOCALAPPDATA").unwrap_or_else(|_| std::env::temp_dir().display().to_string());
    let udd_path = format!(
        "{}\\miku-spike-02-avatar-udd-{}",
        local_app_data.trim_end_matches('\\'),
        coordinator_pid
    );

    let mut child = Command::new(exe_path)
        .args(["--worker", "--udd", &udd_path])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| format!("Failed to spawn Spike 0.2 worker: {}", e))?;

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

    let run_result = (|| -> Result<
        (
            f64,
            f64,
            Vec<ProcessIdentity>,
            f64,
            Vec<StateTransitionMetric>,
            SleepAuditPayload,
            f64,
        ),
        String,
    > {
        // 1. Wait for initial Canvas 2D paint
        let fp_line = read_worker_line_expected(&mut reader, "STAGE:FIRST_PAINT:")?;
        let (first_paint_ms, detected_dpr) = parse_first_paint_line(&fp_line)?;

        let exp_snapshot = capture_windows_process_snapshot()?;
        let (exp_identities, exp_bytes) = collect_process_tree(worker_root_pid, &exp_snapshot)?;
        let expanded_private_mb = (exp_bytes as f64) / (1024.0 * 1024.0);

        // 2. Drive the 4 state transitions (`thinking` -> `speaking` -> `alert` -> `idle`)
        let states = [
            AvatarState::Thinking,
            AvatarState::Speaking,
            AvatarState::Alert,
            AvatarState::Idle,
        ];
        let mut transitions = Vec::with_capacity(states.len());
        for st in states {
            writeln!(worker_stdin, "CMD:SET_STATE:{}", st.as_str())
                .and_then(|_| worker_stdin.flush())
                .map_err(|e| format!("Failed sending CMD:SET_STATE:{}: {}", st.as_str(), e))?;

            let settled_line =
                read_worker_line_expected(&mut reader, "STAGE:STATE_SETTLED:")?;
            let metric = parse_state_settled_line(&settled_line)?;
            transitions.push(metric);
        }

        // 3. Audit 2000 ms in settled `idle` state with blink suspended
        writeln!(worker_stdin, "CMD:AUDIT_SLEEP:{}", SLEEP_AUDIT_WINDOW_MS)
            .and_then(|_| worker_stdin.flush())
            .map_err(|e| format!("Failed sending CMD:AUDIT_SLEEP: {}", e))?;

        let audit_line =
            read_worker_line_expected(&mut reader, "STAGE:SLEEP_AUDIT_DONE:")?;
        let sleep_audit = parse_sleep_audit_line(&audit_line)?;

        // 4. Collapse WebView2 via Mode C TTL teardown and measure post-TTL idle RAM
        writeln!(worker_stdin, "CMD:COLLAPSE_TTL")
            .and_then(|_| worker_stdin.flush())
            .map_err(|e| format!("Failed sending CMD:COLLAPSE_TTL: {}", e))?;

        read_worker_line_expected(&mut reader, "STAGE:COLLAPSED_TTL_READY")?;
        std::thread::sleep(Duration::from_millis(POST_KILL_SETTLE_DELAY_MS));

        let idle_snapshot = capture_windows_process_snapshot()?;
        let (_idle_ids, idle_bytes) = collect_process_tree(worker_root_pid, &idle_snapshot)?;
        let post_ttl_idle_private_mb = (idle_bytes as f64) / (1024.0 * 1024.0);

        let _ = writeln!(worker_stdin, "CMD:QUIT");
        let _ = worker_stdin.flush();

        Ok((
            first_paint_ms,
            detected_dpr,
            exp_identities,
            expanded_private_mb,
            transitions,
            sleep_audit,
            post_ttl_idle_private_mb,
        ))
    })();

    kill_child_process_tree(worker_root_pid, &mut child);
    std::thread::sleep(Duration::from_millis(POST_KILL_SETTLE_DELAY_MS));

    let post_close_snapshot = capture_windows_process_snapshot()?;
    let _ = std::fs::remove_dir_all(&udd_path);

    let (
        first_paint_ms,
        detected_dpr,
        spawned_identities,
        expanded_private_mb,
        transitions,
        sleep_audit,
        post_ttl_idle_private_mb,
    ) = run_result?;

    let orphan_count_after_close =
        count_surviving_identities_after_close(&spawned_identities, &post_close_snapshot);

    let mut summary = Spike02AuditSummary {
        worker_root_pid,
        first_paint_ms,
        detected_dpr,
        transitions,
        sleep_audit,
        expanded_private_mb,
        post_ttl_idle_private_mb,
        orphan_count_after_close,
        passes_all_thresholds: false,
    };
    summary.passes_all_thresholds = summary.evaluate_thresholds();
    Ok(summary)
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
        CS_VREDRAW, MSG, PM_REMOVE, SM_CXSCREEN, SWP_NOACTIVATE, SWP_SHOWWINDOW,
        SW_SHOWNOACTIVATE, WINDOW_EX_STYLE, WINDOW_STYLE, WM_DESTROY, WM_PAINT, WNDCLASSW,
        WS_EX_NOACTIVATE, WS_EX_TOOLWINDOW, WS_EX_TOPMOST, WS_POPUP, WS_VISIBLE,
    };

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

    #[derive(Default)]
    struct WorkerMessageInbox {
        initial_dpr: Option<f64>,
        settled_event: Option<(String, u64)>,
        sleep_audit_json: Option<String>,
        js_error: Option<String>,
    }

    struct AvatarWebView {
        controller: ICoreWebView2Controller,
        webview: ICoreWebView2,
        inbox: Rc<RefCell<WorkerMessageInbox>>,
    }

    impl AvatarWebView {
        fn create_and_load_avatar(
            hwnd: HWND,
            udd_path: &str,
            html_doc: &str,
            width: i32,
            height: i32,
        ) -> Result<(Self, f64, f64), String> {
            let t0 = Instant::now();
            let _ = std::fs::create_dir_all(udd_path);
            let udd_wide = WideStr::new(udd_path);

            let env_slot: Rc<RefCell<Option<Result<ICoreWebView2Environment, String>>>> =
                Rc::new(RefCell::new(None));
            let env_slot_cb = env_slot.clone();

            unsafe {
                let handler = CreateCoreWebView2EnvironmentCompletedHandler::create(Box::new(
                    move |error_code, environment| {
                        let error_code = error_code.into_hresult();
                        let res = error_code
                            .ok()
                            .and_then(|()| environment.ok_or_else(windows::core::Error::from_win32));
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
                        let res = error_code
                            .ok()
                            .and_then(|()| controller.ok_or_else(windows::core::Error::from_win32));
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

            let inbox = Rc::new(RefCell::new(WorkerMessageInbox::default()));
            let inbox_cb = inbox.clone();

            unsafe {
                let mut token = 0i64;
                let msg_handler = WebMessageReceivedEventHandler::create(Box::new(
                    move |_webview, args: Option<ICoreWebView2WebMessageReceivedEventArgs>| {
                        if let Some(args) = args {
                            let mut raw = PWSTR::null();
                            if args.TryGetWebMessageAsString(&mut raw).is_ok() {
                                let text = take_pwstr(raw);
                                let mut guard = inbox_cb.borrow_mut();
                                if let Some(dpr_str) = text.strip_prefix("PAINT_READY:INITIAL:") {
                                    let dpr = dpr_str.trim().parse::<f64>().unwrap_or(1.0);
                                    guard.initial_dpr = Some(dpr);
                                } else if let Some(rest) = text.strip_prefix("STATE_SETTLED:") {
                                    let parts: Vec<&str> = rest.split(':').collect();
                                    if parts.len() >= 2 {
                                        let st = parts[0].to_string();
                                        let frames = parts[1].parse::<u64>().unwrap_or(0);
                                        guard.settled_event = Some((st, frames));
                                    }
                                } else if let Some(json) = text.strip_prefix("SLEEP_AUDIT_DONE:") {
                                    guard.sleep_audit_json = Some(json.to_string());
                                } else if let Some(err) = text.strip_prefix("ERROR:") {
                                    guard.js_error = Some(err.to_string());
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
                let _ = controller.put_IsVisible(BOOL::from(true));

                let html_wide = WideStr::new(html_doc);
                webview
                    .NavigateToString(html_wide.as_pcwstr())
                    .map_err(|e| format!("NavigateToString failed: {}", e))?;
            }

            let deadline = Instant::now() + Duration::from_secs(12);
            while inbox.borrow().initial_dpr.is_none() {
                if let Some(err) = inbox.borrow().js_error.clone() {
                    return Err(format!("JS error during initial paint: {}", err));
                }
                if Instant::now() >= deadline {
                    return Err("Timed out waiting for PAINT_READY:INITIAL".to_string());
                }
                pump_messages_once();
                std::thread::sleep(Duration::from_millis(1));
            }

            let first_paint_ms = t0.elapsed().as_secs_f64() * 1000.0;
            let dpr = inbox.borrow().initial_dpr.unwrap_or(1.0);

            Ok((
                Self {
                    controller,
                    webview,
                    inbox,
                },
                first_paint_ms,
                dpr,
            ))
        }

        fn execute_js(&self, script_text: &str) {
            unsafe {
                let wide = WideStr::new(script_text);
                let handler = ExecuteScriptCompletedHandler::create(Box::new(|_, _| Ok(())));
                let _ = self.webview.ExecuteScript(wide.as_pcwstr(), &handler);
            }
        }

        fn set_state_and_wait_settled(&self, target_state: &str) -> Result<(f64, u64), String> {
            self.inbox.borrow_mut().settled_event = None;
            let t0 = Instant::now();
            self.execute_js(&format!("window.__mikuSetState('{}');", target_state));

            let deadline = Instant::now() + Duration::from_secs(8);
            loop {
                if let Some(err) = self.inbox.borrow().js_error.clone() {
                    return Err(format!("JS error in set_state: {}", err));
                }
                if let Some((st, frames)) = self.inbox.borrow_mut().settled_event.take() {
                    if st == target_state {
                        let elapsed_ms = t0.elapsed().as_secs_f64() * 1000.0;
                        return Ok((elapsed_ms, frames));
                    }
                }
                if Instant::now() >= deadline {
                    return Err(format!(
                        "Timed out waiting for STATE_SETTLED:{}",
                        target_state
                    ));
                }
                pump_messages_once();
                std::thread::sleep(Duration::from_millis(1));
            }
        }

        fn run_sleep_audit(&self, duration_ms: u64) -> Result<String, String> {
            self.inbox.borrow_mut().sleep_audit_json = None;
            self.execute_js(&format!("window.__mikuAuditSleepWindow({});", duration_ms));

            let deadline = Instant::now() + Duration::from_millis(duration_ms + 5000);
            loop {
                if let Some(err) = self.inbox.borrow().js_error.clone() {
                    return Err(format!("JS error during sleep audit: {}", err));
                }
                if let Some(json) = self.inbox.borrow_mut().sleep_audit_json.take() {
                    return Ok(json);
                }
                if Instant::now() >= deadline {
                    return Err("Timed out waiting for SLEEP_AUDIT_DONE".to_string());
                }
                pump_messages_once();
                std::thread::sleep(Duration::from_millis(5));
            }
        }

        fn close(self) {
            unsafe {
                let _ = self.controller.put_IsVisible(BOOL::from(false));
                let _ = self.controller.Close();
            }
            drop(self.webview);
            drop(self.controller);
            pump_messages_once();
        }
    }

    pub fn run_worker_loop(udd_path: &str) -> Result<(), String> {
        let (rig, tokens) = load_rig_and_tokens()?;
        let html_doc = build_avatar_worker_html(&rig, &tokens)?;

        unsafe {
            let hr = CoInitializeEx(None, COINIT_APARTMENTTHREADED);
            if hr.is_err() {
                return Err(format!("CoInitializeEx failed: {:?}", hr));
            }
        }

        let res = (|| -> Result<(), String> {
            let screen_w = unsafe {
                let w = GetSystemMetrics(SM_CXSCREEN);
                if w > 0 { w } else { 1920 }
            };

            let exp_w = 420;
            let exp_h = 204;
            let exp_x = (screen_w - exp_w) / 2;
            let exp_y = 12;

            let pill_w = 210;
            let pill_h = 36;
            let pill_x = (screen_w - pill_w) / 2;
            let pill_y = 8;

            let class_name = WideStr::new("MikuSpike02AvatarIslandClass");
            let window_title = WideStr::new("Miku Island Spike 0.2 Avatar");

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

                let ex_style =
                    WINDOW_EX_STYLE((WS_EX_TOPMOST | WS_EX_TOOLWINDOW | WS_EX_NOACTIVATE).0);
                let style = WINDOW_STYLE((WS_POPUP | WS_VISIBLE).0);

                CreateWindowExW(
                    ex_style,
                    class_name.as_pcwstr(),
                    window_title.as_pcwstr(),
                    style,
                    exp_x,
                    exp_y,
                    exp_w,
                    exp_h,
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

            let (wv, first_paint_ms, dpr) =
                AvatarWebView::create_and_load_avatar(hwnd, udd_path, &html_doc, exp_w, exp_h)?;
            let mut active_wv = Some(wv);

            println!("STAGE:FIRST_PAINT:{:.4}:{:.2}", first_paint_ms, dpr);
            let _ = std::io::stdout().flush();

            let stdin = std::io::stdin();
            for line_res in stdin.lock().lines() {
                let cmd = line_res.map_err(|e| format!("Worker stdin error: {}", e))?;
                let trimmed = cmd.trim();

                if let Some(target_state) = trimmed.strip_prefix("CMD:SET_STATE:") {
                    let wv_ref = active_wv
                        .as_ref()
                        .ok_or_else(|| "WebView2 already closed".to_string())?;
                    let (settle_ms, frames) =
                        wv_ref.set_state_and_wait_settled(target_state.trim())?;
                    println!(
                        "STAGE:STATE_SETTLED:{}:{:.4}:{}",
                        target_state.trim(),
                        settle_ms,
                        frames
                    );
                    let _ = std::io::stdout().flush();
                } else if let Some(dur_str) = trimmed.strip_prefix("CMD:AUDIT_SLEEP:") {
                    let duration_ms = dur_str.trim().parse::<u64>().unwrap_or(2000);
                    let wv_ref = active_wv
                        .as_ref()
                        .ok_or_else(|| "WebView2 already closed".to_string())?;
                    let json = wv_ref.run_sleep_audit(duration_ms)?;
                    println!("STAGE:SLEEP_AUDIT_DONE:{}", json);
                    let _ = std::io::stdout().flush();
                } else if trimmed == "CMD:COLLAPSE_TTL" {
                    if let Some(wv) = active_wv.take() {
                        wv.close();
                    }
                    unsafe {
                        let _ = SetWindowPos(
                            hwnd,
                            HWND::default(),
                            pill_x,
                            pill_y,
                            pill_w,
                            pill_h,
                            SWP_NOACTIVATE | SWP_SHOWWINDOW,
                        );
                        let _ = InvalidateRect(hwnd, std::ptr::null(), BOOL::from(true));
                    }
                    pump_messages_once();
                    println!("STAGE:COLLAPSED_TTL_READY");
                    let _ = std::io::stdout().flush();
                } else if trimmed == "CMD:QUIT" {
                    if let Some(wv) = active_wv.take() {
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

fn parse_worker_udd_arg() -> Option<String> {
    let args: Vec<String> = std::env::args().collect();
    if !args.iter().any(|a| a == "--worker") {
        return None;
    }
    let mut udd = std::env::temp_dir()
        .join("miku-spike-02-worker")
        .display()
        .to_string();
    let mut i = 1;
    while i < args.len() {
        if args[i] == "--udd" && i + 1 < args.len() {
            udd = args[i + 1].clone();
            i += 2;
        } else {
            i += 1;
        }
    }
    Some(udd)
}

fn main() {
    if !cfg!(target_os = "windows") {
        eprintln!("Spike 0.2 requires Windows (Win32 + WebView2 + HTML5 Canvas).");
        std::process::exit(1);
    }

    if let Some(udd_path) = parse_worker_udd_arg() {
        #[cfg(windows)]
        {
            if let Err(err) = win32_worker::run_worker_loop(&udd_path) {
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
        "Running Spike 0.2 Coordinator (PID={}) — Procedural HTML5 Canvas Avatar & Sleeping Springs Audit...",
        coordinator_pid
    );

    match run_coordinator_spike02(&exe_path, coordinator_pid) {
        Ok(summary) => {
            let report = format_spike02_report(&summary);
            println!("\n{}", report);
            if !summary.passes_all_thresholds {
                eprintln!("VERIFIER FAILED [0.2]: One or more ADR-002 thresholds did not pass.");
                std::process::exit(1);
            }
            println!(
                "VERIFIER: OK (Spike 0.2 verified: 0 fps in settled idle, 4 states settled, 0 orphans)."
            );
        }
        Err(err) => {
            eprintln!("FATAL [0.2]: {}", err);
            std::process::exit(1);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// LEDGER-0.2-5: Integration test verifying coordinator/worker protocol framing,
    /// on-disk `assets/rig.toml` + `assets/design-tokens.json` loading, and strict
    /// `Spike02AuditSummary` threshold evaluation for ADR-002.
    #[test]
    fn e2e_webview2_canvas_avatar_sleep_audit() {
        let (rig, tokens) = load_rig_and_tokens()
            .expect("On-disk assets/rig.toml and assets/design-tokens.json must load cleanly");
        let html = build_avatar_worker_html(&rig, &tokens).expect("HTML generation must succeed");
        assert!(html.contains("window.__mikuAuditSleepWindow"));
        assert!(html.contains("window.__mikuSetBlinkEnabled(false)"));

        let (fp_ms, dpr) = parse_first_paint_line("STAGE:FIRST_PAINT:24.8500:1.25").unwrap();
        assert!((fp_ms - 24.85).abs() < 1e-4);
        assert!((dpr - 1.25).abs() < 1e-4);

        let tr = parse_state_settled_line("STAGE:STATE_SETTLED:thinking:812.4000:49").unwrap();
        assert_eq!(tr.state, AvatarState::Thinking);
        assert_eq!(tr.frames_rendered, 49);

        let sleep_payload = parse_sleep_audit_line(
            r#"STAGE:SLEEP_AUDIT_DONE:{"duration_ms":2000,"raf_count":0,"fps":0.0,"wakeups_per_sec":0.0,"all_asleep":true,"dpr":1.25}"#,
        )
        .unwrap();
        assert_eq!(sleep_payload.raf_count, 0);
        assert_eq!(sleep_payload.fps, 0.0);
        assert!(sleep_payload.all_asleep);

        let mut summary = Spike02AuditSummary {
            worker_root_pid: 4242,
            first_paint_ms: fp_ms,
            detected_dpr: dpr,
            transitions: vec![
                StateTransitionMetric {
                    state: AvatarState::Thinking,
                    settle_ms: 810.0,
                    frames_rendered: 49,
                },
                StateTransitionMetric {
                    state: AvatarState::Speaking,
                    settle_ms: 840.0,
                    frames_rendered: 51,
                },
                StateTransitionMetric {
                    state: AvatarState::Alert,
                    settle_ms: 790.0,
                    frames_rendered: 48,
                },
                StateTransitionMetric {
                    state: AvatarState::Idle,
                    settle_ms: 860.0,
                    frames_rendered: 52,
                },
            ],
            sleep_audit: sleep_payload,
            expanded_private_mb: 138.0,
            post_ttl_idle_private_mb: 2.55,
            orphan_count_after_close: 0,
            passes_all_thresholds: false,
        };
        summary.passes_all_thresholds = summary.evaluate_thresholds();
        assert!(
            summary.passes_all_thresholds,
            "Valid 0 fps sleep audit summary must pass all ADR-002 thresholds"
        );

        // Any non-zero rAF invocation during the settled idle audit MUST fail the verifier.
        summary.sleep_audit.raf_count = 1;
        summary.sleep_audit.fps = 0.5;
        assert!(
            !summary.evaluate_thresholds(),
            "Non-zero rAF count in idle must fail ADR-002 verification"
        );
    }
}
