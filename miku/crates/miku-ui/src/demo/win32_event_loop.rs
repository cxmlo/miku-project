#![allow(unused_imports)]
use crate::prelude::*;

#[cfg(target_os = "windows")]
pub(crate) mod win32_demo_runner {
    use super::*;
    use super::win32_demo_ffi::*;
    use std::mem::{size_of, zeroed};
    use std::ptr::{null, null_mut};
    use std::thread;
    use std::time::Duration;

    pub fn execute_win32_demo_session(
        template_ctrl: &DemoInteractiveController,
        options: &DemoCliOptions,
    ) -> Result<DemoSessionReport, UiError> {
        let monitors = crate::win32::win32_native::enumerate_monitors_win32();
        let primary = select_target_monitor(&monitors, None)?.clone();
        let mut ctrl = template_ctrl.clone_for_monitor(primary)?;
        let t0 = Instant::now();
        let style_cfg = Win32PillStyleConfig::default();
        style_cfg.validate()?;

        let seq = DEMO_WINDOW_SEQ.fetch_add(1, Ordering::Relaxed);
        unsafe {
            let hinstance = GetModuleHandleW(null());
            let class_str = format!("MikuInteractiveDemoPillClass_{}_{}\0", std::process::id(), seq);
            let class_w: Vec<u16> = class_str.encode_utf16().collect();
            let title_w: Vec<u16> = "[DEMO] Miku Native Island\0".encode_utf16().collect();

            let wcx = WndClassExW {
                cb_size: size_of::<WndClassExW>() as Uint,
                style: 0,
                lpfn_wnd_proc: demo_pill_wndproc,
                cb_cls_extra: 0,
                cb_wnd_extra: 0,
                h_instance: hinstance,
                h_icon: null_mut(),
                h_cursor: null_mut(),
                hbr_background: null_mut(),
                lpsz_menu_name: null(),
                lpsz_class_name: class_w.as_ptr(),
                h_icon_sm: null_mut(),
            };
            let _ = RegisterClassExW(&wcx);

            let fg_before = GetForegroundWindow() as usize;
            let anchor = ctrl.pill_geometry();

            let hwnd = CreateWindowExW(
                style_cfg.ex_style,
                class_w.as_ptr(),
                title_w.as_ptr(),
                style_cfg.base_style & !0x1000_0000,
                anchor.x,
                anchor.y,
                anchor.width,
                anchor.height,
                null_mut(),
                null_mut(),
                hinstance,
                null(),
            );
            if hwnd.is_null() {
                return Err(UiError::Win32Error(
                    "CreateWindowExW returned NULL in demo mode".to_string(),
                ));
            }

            let init_rgn = CreateRoundRectRgn(0, 0, anchor.width, anchor.height, 22, 22);
            if !init_rgn.is_null() {
                let _ = SetWindowRgn(hwnd, init_rgn, 1);
            }
            let _ = SetLayeredWindowAttributes(hwnd, 0, 248, LWA_ALPHA);
            let _ = ShowWindow(hwnd, style_cfg.show_cmd);

            // Register global hotkeys on the HWND (graceful fallback if another app holds them)
            let ptt_hk_ok = RegisterHotKey(
                hwnd,
                DEMO_PTT_HOTKEY_ID,
                WIN32_MOD_ALT | WIN32_MOD_NOREPEAT,
                WIN32_VK_SPACE,
            ) != 0;
            let ks_hk_ok = RegisterHotKey(
                hwnd,
                KILL_SWITCH_HOTKEY_ID,
                KILL_SWITCH_WIN32_MODIFIERS,
                KILL_SWITCH_WIN32_VK_K,
            ) != 0;

            let startup_latency_ms = t0.elapsed().as_secs_f64() * 1_000.0;
            let ma_res = SendMessageW(hwnd, WM_MOUSEACTIVATE_MSG, 0, 0);
            let fg_after = GetForegroundWindow() as usize;
            let focus_preserved =
                verify_foreground_focus_invariant(fg_before, fg_after, hwnd as usize, ma_res)?;

            paint_demo_pill_frame(hwnd, anchor.width, anchor.height, &ctrl);

            if options.auto_script {
                // Deterministic scripted execution on the live Win32 window for automated tests & benchmarks
                let mut msg: Msg = zeroed();
                while PeekMessageW(&mut msg, hwnd, 0, 0, PM_REMOVE) != 0 {
                    let _ = TranslateMessage(&msg);
                    let _ = DispatchMessageW(&msg);
                }

                // 1. Hover enter -> resize to HoverPeek (284x52 scaled)
                let _ = ctrl.step_hover_enter(anchor.center_x(), anchor.y + anchor.height / 2, 100)?;
                let g_hover = ctrl.pill_geometry();
                let _ = SetWindowPos(
                    hwnd,
                    null_mut(),
                    g_hover.x,
                    g_hover.y,
                    g_hover.width,
                    g_hover.height,
                    SWP_NOACTIVATE_FLAG | SWP_NOZORDER_FLAG | SWP_SHOWWINDOW_FLAG,
                );
                paint_demo_pill_frame(hwnd, g_hover.width, g_hover.height, &ctrl);

                // 2. Click -> open [DEMO] Gatekeeper Y/N permission modal & approve
                let _ = ctrl.step_click_permission_modal(200)?;
                paint_demo_pill_frame(hwnd, g_hover.width, g_hover.height, &ctrl);
                let _ = ctrl.step_approve_or_deny_modal(true, 250)?;

                // 3. Alt+Space -> PTT listening with 5-band soundwave
                let _ = ctrl.step_ptt_hotkey(300)?;
                paint_demo_pill_frame(hwnd, g_hover.width, g_hover.height, &ctrl);

                // 4. Ctrl+Alt+Shift+K -> Global Kill Switch trip (#F43F5E)
                let _ = ctrl.step_kill_switch_hotkey(400)?;
                paint_demo_pill_frame(hwnd, g_hover.width, g_hover.height, &ctrl);

                // 5. Escape -> Clean teardown
                if ptt_hk_ok {
                    let _ = UnregisterHotKey(hwnd, DEMO_PTT_HOTKEY_ID);
                }
                if ks_hk_ok {
                    let _ = UnregisterHotKey(hwnd, KILL_SWITCH_HOTKEY_ID);
                }
                let _ = DestroyWindow(hwnd);
                let _ = ctrl.step_escape_clean_exit(500)?;
                return ctrl.build_session_report(startup_latency_ms, focus_preserved);
            }

            // Live Interactive Mode (`cargo run --release -p miku-cli -- demo`)
            let max_live_ms = options.duration_ms.unwrap_or(60_000);
            if !options.json_output {
                println!("================================================================");
                println!(" [DEMO] MIKU DESKTOP ORCHESTRATOR · PASTILLA WIN32 EN VIVO");
                println!("================================================================");
                println!(
                    " Geometry : {}x{} @ ({}, {}) | DPI={}",
                    anchor.width, anchor.height, anchor.x, anchor.y, anchor.dpi
                );
                println!(" Sandbox  : 100% In-Memory (0 Disk Writes · 0 API Keys)");
                println!(" Controls :");
                println!("   • Hover Mouse over top-center pill -> Expand & Wake 6-Spring Avatar");
                println!("   • Left-Click on pill               -> Trigger/Approve [DEMO] Y/N Card");
                println!("   • Alt + Space                      -> Simulate PTT Voice & 5-Band Wave");
                println!("   • Ctrl + Alt + Shift + K           -> Trip Emergency Kill Switch (#F43F5E)");
                println!("   • Press ESCAPE                     -> Clean Exit (0 Orphans)");
                println!("================================================================");
            }

            let loop_start = Instant::now();
            let mut is_hovered = false;
            let mut prev_lbutton_down = false;
            let mut cur_w = anchor.width;
            let mut cur_h = anchor.height;

            while loop_start.elapsed().as_millis() < (max_live_ms as u128) {
                let elapsed_ms = loop_start.elapsed().as_millis() as u64;

                // Check Escape key for instant cooperative exit
                if (GetAsyncKeyState(WIN32_VK_ESCAPE) as u16 & 0x8000) != 0 {
                    break;
                }

                // Drain Win32 message queue & handle global hotkeys
                let mut msg: Msg = zeroed();
                let mut should_quit = false;
                while PeekMessageW(&mut msg, null_mut(), 0, 0, PM_REMOVE) != 0 {
                    if msg.message == WM_CLOSE
                        || (msg.message == WM_KEYDOWN && msg.w_param == (WIN32_VK_ESCAPE as usize))
                    {
                        should_quit = true;
                        break;
                    }
                    if msg.message == WM_HOTKEY {
                        let hk_id = msg.w_param as i32;
                        if hk_id == DEMO_PTT_HOTKEY_ID {
                            let _ = ctrl.step_ptt_hotkey(elapsed_ms);
                            paint_demo_pill_frame(hwnd, cur_w, cur_h, &ctrl);
                        } else if hk_id == KILL_SWITCH_HOTKEY_ID {
                            let _ = ctrl.step_kill_switch_hotkey(elapsed_ms);
                            paint_demo_pill_frame(hwnd, cur_w, cur_h, &ctrl);
                        }
                    } else if msg.message == WM_LBUTTONDOWN {
                        if matches!(ctrl.phase(), DemoInteractionPhase::PermissionModalActive { .. }) {
                            let _ = ctrl.step_approve_or_deny_modal(true, elapsed_ms);
                        } else {
                            let _ = ctrl.step_click_permission_modal(elapsed_ms);
                        }
                        paint_demo_pill_frame(hwnd, cur_w, cur_h, &ctrl);
                    }
                    let _ = TranslateMessage(&msg);
                    let _ = DispatchMessageW(&msg);
                }
                if should_quit {
                    break;
                }

                // Rearm Kill Switch if user presses 'R' while tripped
                if ctrl.kill_switch.is_tripped() && (GetAsyncKeyState(VK_R_KEY) as u16 & 0x8000) != 0 {
                    if let Ok(ch) = ctrl.kill_switch.request_rearm(elapsed_ms) {
                        let _ = ctrl.kill_switch.confirm_rearm(
                            &ch.challenge_token,
                            elapsed_ms + 1,
                            &mut ctrl.gatekeeper,
                            ctrl.reloader.runtime_mut(),
                            &ctrl.avatar_dir,
                            &mut ctrl.audit_store,
                            &mut ctrl.bridge,
                        );
                        ctrl.last_demo_banner = ctrl
                            .sandbox
                            .format_demo_labeled_text("Kill Switch Rearmed · Back to Normal");
                        ctrl.phase = DemoInteractionPhase::IdleCompact;
                        paint_demo_pill_frame(hwnd, cur_w, cur_h, &ctrl);
                    }
                }

                // Poll cursor position for hover hysteresis & click on NOACTIVATE window
                let mut pt: Point = zeroed();
                if GetCursorPos(&mut pt) != 0 {
                    let g = ctrl.pill_geometry();
                    let inside = pt.x >= g.x - HOVER_HYSTERESIS_PX
                        && pt.x <= g.x + g.width + HOVER_HYSTERESIS_PX
                        && pt.y >= g.y
                        && pt.y <= g.y + g.height + HOVER_HYSTERESIS_PX;

                    if inside && !is_hovered {
                        is_hovered = true;
                        let _ = ctrl.step_hover_enter(pt.x, pt.y, elapsed_ms);
                        let g_new = ctrl.pill_geometry();
                        cur_w = g_new.width.max(420);
                        cur_h = g_new.height.max(54);
                        let new_x = g_new.center_x() - cur_w / 2;
                        let _ = SetWindowPos(
                            hwnd,
                            null_mut(),
                            new_x,
                            g_new.y,
                            cur_w,
                            cur_h,
                            SWP_NOACTIVATE_FLAG | SWP_NOZORDER_FLAG | SWP_SHOWWINDOW_FLAG,
                        );
                        let rgn = CreateRoundRectRgn(0, 0, cur_w, cur_h, 26, 26);
                        if !rgn.is_null() {
                            let _ = SetWindowRgn(hwnd, rgn, 1);
                        }
                        paint_demo_pill_frame(hwnd, cur_w, cur_h, &ctrl);
                    } else if !inside && is_hovered {
                        is_hovered = false;
                        let _ = ctrl.step_hover_leave(pt.x, pt.y, elapsed_ms);
                        let g_new = ctrl.pill_geometry();
                        cur_w = g_new.width;
                        cur_h = g_new.height;
                        let _ = SetWindowPos(
                            hwnd,
                            null_mut(),
                            g_new.x,
                            g_new.y,
                            cur_w,
                            cur_h,
                            SWP_NOACTIVATE_FLAG | SWP_NOZORDER_FLAG | SWP_SHOWWINDOW_FLAG,
                        );
                        let rgn = CreateRoundRectRgn(0, 0, cur_w, cur_h, 22, 22);
                        if !rgn.is_null() {
                            let _ = SetWindowRgn(hwnd, rgn, 1);
                        }
                        paint_demo_pill_frame(hwnd, cur_w, cur_h, &ctrl);
                    }

                    let lbutton_down = (GetAsyncKeyState(VK_LBUTTON) as u16 & 0x8000) != 0;
                    if inside && lbutton_down && !prev_lbutton_down {
                        if matches!(ctrl.phase(), DemoInteractionPhase::PermissionModalActive { .. }) {
                            let _ = ctrl.step_approve_or_deny_modal(true, elapsed_ms);
                        } else {
                            let _ = ctrl.step_click_permission_modal(elapsed_ms);
                        }
                        paint_demo_pill_frame(hwnd, cur_w, cur_h, &ctrl);
                    }
                    prev_lbutton_down = lbutton_down;
                }

                // Step 6-spring physics if awake; sleep peacefully when settled
                if !ctrl.reloader.runtime().physics.is_at_rest() {
                    let _ = ctrl.reloader.runtime_mut().physics.step(1.0 / 60.0);
                    ctrl.frames_rendered += 1;
                    paint_demo_pill_frame(hwnd, cur_w, cur_h, &ctrl);
                }

                thread::sleep(Duration::from_millis(16));
            }

            if ptt_hk_ok {
                let _ = UnregisterHotKey(hwnd, DEMO_PTT_HOTKEY_ID);
            }
            if ks_hk_ok {
                let _ = UnregisterHotKey(hwnd, KILL_SWITCH_HOTKEY_ID);
            }
            let _ = DestroyWindow(hwnd);
            let _ = ctrl.step_escape_clean_exit(loop_start.elapsed().as_millis() as u64);
            ctrl.build_session_report(startup_latency_ms, focus_preserved)
        }
    }
}

pub fn run_interactive_demo_session(options: &DemoCliOptions) -> Result<DemoSessionReport, UiError> {
    let ctrl = DemoInteractiveController::default_with_resolved_assets()?;
    ctrl.run_live_win32_demo_window(options)
}
