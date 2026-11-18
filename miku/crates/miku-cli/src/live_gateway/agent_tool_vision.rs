//! Multimodal Vision, bounded UIA inspection/interaction, DPI-aware SendInput,
//! browser navigation, and Skill runtime execution tools for Miku's agent loop.

use std::fs;
use std::path::Path;
use std::sync::atomic::AtomicBool;

use miku_browser::validate_browser_url;
use miku_gatekeeper::{Capability, GatekeeperVerdict};
use miku_platform::{
    safe_ui_automation_inspect, safe_ui_automation_invoke, send_mouse_absolute_click,
    send_unicode_text, verify_input_target_safe, virtual_to_sendinput_abs, CaptureTransform,
    InputStep, PcControlBackend, WindowsNativePcControlBackend,
};
use miku_protocol::{base64_encode, PermissionDecisionKind};
use miku_skills::SkillError;
use miku_ui::vision::{
    compress_rgba_to_bounded_png, zeroize_slice, LocalOcrBackend, LocalWindowsOcrBackend,
    ScreenCaptureBackend, VisionCaptureTarget, Win32GdiScreenVisionBackend,
};
use serde_json::{json, Value};

use super::agent_tool_exec::{evaluate_and_handle_gatekeeper, truncate_chars};
use super::api_handler::LiveMikuApiHandler;
use super::app_launcher::launch_with_gatekeeper;
use super::llm_multimodal::LlmImageAttachment;
use super::storage_actor::now_epoch_ms;
use super::token_pipeline::setting_bool;

fn evaluate_pc_gatekeeper(
    handler: &LiveMikuApiHandler,
    agent_id: &str,
    tool_name: &str,
    cap: Capability,
    target: &str,
    raw_args: &str,
    auto_setting_key: Option<&str>,
) -> Result<String, Value> {
    let now_ms = now_epoch_ms();
    let verdict = {
        let mut st = handler.state.lock().expect("state mutex");
        let auto_allowed = auto_setting_key
            .map(|k| setting_bool(&st.settings, k, false))
            .unwrap_or(false)
            && !st.permission_mode.eq_ignore_ascii_case("restricted");
        let initial = st.gatekeeper.evaluate_request(agent_id, cap, target, raw_args, now_ms);
        match initial {
            GatekeeperVerdict::AskPending { ticket, .. } if auto_allowed => st
                .gatekeeper
                .resolve_permission_decision(
                    &ticket.req_hash,
                    PermissionDecisionKind::Allow,
                    false,
                    false,
                    agent_id,
                    cap,
                    target,
                    raw_args,
                    now_ms,
                ),
            other => other,
        }
    };
    match verdict {
        GatekeeperVerdict::Allow { canonical_target, .. } => Ok(canonical_target),
        GatekeeperVerdict::AskPending { ticket, .. } => {
            let _ = handler.events_tx.send(json!({
                "type": "approve",
                "id": ticket.req_hash,
                "title": format!("Gatekeeper: {}", tool_name),
                "command": format!("{}: {}", ticket.capability.as_str(), ticket.canonical_target),
                "diff": format!("Agente: {}\nArgs: {}", agent_id, ticket.canonical_args),
                "capability": ticket.capability.as_str(),
                "target": ticket.canonical_target,
                "agent_id": agent_id
            }));
            Err(json!({
                "ok": false,
                "status": "approval_required",
                "ticket": ticket.req_hash,
                "message": "Gatekeeper requiere aprobación manual del usuario"
            }))
        }
        GatekeeperVerdict::Deny { reason, detail, .. } => Err(json!({
            "ok": false,
            "status": "denied",
            "reason": reason.as_code(),
            "detail": detail
        })),
    }
}

fn format_uia_compact_summary(snap: &miku_platform::UiaTreeSnapshot, tr: &CaptureTransform) -> String {
    let mut lines = Vec::new();
    let w = (tr.width_px.max(1) as f32).max(1.0);
    let h = (tr.height_px.max(1) as f32).max(1.0);
    for node in snap.nodes.iter().take(40) {
        let cx = ((node.rect[0] + node.rect[2]) as f32 * 0.5) - tr.origin_virtual_x as f32;
        let cy = ((node.rect[1] + node.rect[3]) as f32 * 0.5) - tr.origin_virtual_y as f32;
        let nx = ((cx / w) * 1000.0).clamp(0.0, 1000.0).round() as i32;
        let ny = ((cy / h) * 1000.0).clamp(0.0, 1000.0).round() as i32;
        let label = if node.name.is_empty() { &node.automation_id } else { &node.name };
        lines.push(format!(
            "[{}] '{}' id='{}' norm=({},{}) enabled={}",
            node.control_type,
            truncate_chars(label, 48),
            truncate_chars(&node.automation_id, 32),
            nx,
            ny,
            node.enabled
        ));
    }
    lines.join("\n")
}

pub(super) fn tool_screen_capture(
    handler: &LiveMikuApiHandler,
    agent_id: &str,
    name: &str,
    args: &Value,
) -> Value {
    let target = if let (Some(x), Some(y), Some(w), Some(h)) = (
        args.get("x").and_then(|v| v.as_i64()),
        args.get("y").and_then(|v| v.as_i64()),
        args.get("width").and_then(|v| v.as_u64()),
        args.get("height").and_then(|v| v.as_u64()),
    ) {
        VisionCaptureTarget::SelectedRegion {
            x: x as i32,
            y: y as i32,
            width: w as u32,
            height: h as u32,
        }
    } else {
        VisionCaptureTarget::ActiveWindow
    };
    if let Err(v) = evaluate_pc_gatekeeper(
        handler,
        agent_id,
        name,
        Capability::Vision,
        &target.canonical_gatekeeper_target(),
        "screen_capture",
        Some("auto_screen_capture"),
    ) {
        return v;
    }

    let backend = Win32GdiScreenVisionBackend::new();
    let mut frame = match backend.capture_rgba(&target, now_epoch_ms()) {
        Ok(f) => f,
        Err(e) => return json!({ "ok": false, "status": "blocked_or_failed", "error": e.to_string() }),
    };

    let ocr_res = LocalWindowsOcrBackend::new("es-MX")
        .recognize_text(frame.width, frame.height, &frame.rgba, &frame.window_title)
        .ok();
    let uia_snap = safe_ui_automation_inspect(&frame.window_title).ok();
    let uia_summary = uia_snap
        .as_ref()
        .map(|s| format_uia_compact_summary(s, &frame.transform))
        .unwrap_or_default();
    let ocr_text = ocr_res.map(|o| o.text).unwrap_or_default();

    let bounded_res = compress_rgba_to_bounded_png(&frame, 1024, 1024, 250 * 1024);
    zeroize_slice(&mut frame.rgba);
    frame.rgba.clear();

    let mut bounded = match bounded_res {
        Ok(b) => b,
        Err(e) => return json!({ "ok": false, "error": e.to_string() }),
    };
    let b64 = base64_encode(&bounded.png_bytes);
    let png_byte_len = bounded.png_bytes.len();
    zeroize_slice(&mut bounded.png_bytes);
    bounded.png_bytes.clear();

    let attachment = LlmImageAttachment {
        mime_type: "image/png".to_string(),
        base64_data: b64,
        ocr_text: if ocr_text.trim().is_empty() { None } else { Some(ocr_text.clone()) },
        uia_summary: if uia_summary.trim().is_empty() { None } else { Some(uia_summary.clone()) },
        window_title: frame.window_title.clone(),
        width: bounded.width,
        height: bounded.height,
        transform: frame.transform,
    };
    handler.set_turn_vision_attachment(attachment);

    json!({
        "ok": true,
        "window_title": frame.window_title,
        "width": bounded.width,
        "height": bounded.height,
        "png_bytes": png_byte_len,
        "zero_disk_temp_files": true,
        "capture_transform": frame.transform,
        "ocr_excerpt": truncate_chars(&ocr_text, 900),
        "uia_controls_count": uia_snap.as_ref().map(|s| s.nodes.len()).unwrap_or(0),
        "uia_summary": truncate_chars(&uia_summary, 900)
    })
}

pub(super) fn tool_ui_inspect(
    handler: &LiveMikuApiHandler,
    agent_id: &str,
    name: &str,
    args: &Value,
) -> Value {
    let window_title = args.get("window_title").and_then(|v| v.as_str()).unwrap_or("");
    if let Err(v) = evaluate_and_handle_gatekeeper(
        handler,
        agent_id,
        name,
        Capability::UiAutomation,
        &format!("uia:inspect:{}", if window_title.is_empty() { "foreground" } else { window_title }),
        window_title,
    ) {
        return v;
    }
    let tr = handler.last_capture_transform();
    match safe_ui_automation_inspect(window_title) {
        Ok(snap) => {
            let summary = format_uia_compact_summary(&snap, &tr);
            json!({
                "ok": true,
                "window_title": snap.window_title,
                "controls_count": snap.nodes.len(),
                "summary": truncate_chars(&summary, 1400),
                "controls": snap.nodes
            })
        }
        Err(e) => json!({ "ok": false, "error": e.to_string() }),
    }
}

pub(super) fn tool_ui_interact(
    handler: &LiveMikuApiHandler,
    agent_id: &str,
    name: &str,
    args: &Value,
) -> Value {
    let window_title = args.get("window_title").and_then(|v| v.as_str()).unwrap_or("");
    let automation_id = args
        .get("automation_id")
        .or_else(|| args.get("target"))
        .or_else(|| args.get("name"))
        .and_then(|v| v.as_str())
        .unwrap_or("");
    let control_type = args.get("control_type").and_then(|v| v.as_str()).unwrap_or("Button");
    if automation_id.trim().is_empty() {
        return json!({ "ok": false, "error": "automation_id or control name required" });
    }
    if let Err(v) = evaluate_pc_gatekeeper(
        handler,
        agent_id,
        name,
        Capability::UiAutomation,
        &format!("uia:invoke:{}:{}", window_title, automation_id),
        control_type,
        Some("auto_pc_control"),
    ) {
        return v;
    }
    if let Err(e) = verify_input_target_safe() {
        return json!({ "ok": false, "status": "blocked_sensitive_window", "error": e.to_string() });
    }
    match safe_ui_automation_invoke(window_title, automation_id, control_type, None) {
        Ok(receipt) => json!({
            "ok": receipt.used_ui_automation_tree,
            "used_ui_automation_tree": receipt.used_ui_automation_tree,
            "target_summary": receipt.target_summary
        }),
        Err(e) => json!({ "ok": false, "error": e.to_string() }),
    }
}

pub(super) fn tool_pc_input(
    handler: &LiveMikuApiHandler,
    agent_id: &str,
    name: &str,
    args: &Value,
) -> Value {
    let action = args
        .get("action")
        .and_then(|v| v.as_str())
        .unwrap_or("click")
        .trim()
        .to_ascii_lowercase();
    let mut tr: CaptureTransform = args
        .get("capture_transform")
        .or_else(|| args.get("transform"))
        .and_then(|v| serde_json::from_value(v.clone()).ok())
        .unwrap_or_else(|| handler.last_capture_transform());
    if tr.width_px == 0 || tr.height_px == 0 {
        if let Ok(frame) = Win32GdiScreenVisionBackend::new()
            .capture_rgba(&VisionCaptureTarget::ActiveWindow, now_epoch_ms())
        {
            tr = frame.transform;
            if let Ok(mut tr_guard) = handler.last_capture_transform.lock() {
                *tr_guard = tr;
            }
        }
    }

    match action.as_str() {
        "click" | "right_click" => {
            let right = action == "right_click"
                || args.get("button").and_then(|v| v.as_str()) == Some("right");
            let (vx, vy) = if let (Some(nx), Some(ny)) = (
                args.get("x_norm").and_then(|v| v.as_f64()),
                args.get("y_norm").and_then(|v| v.as_f64()),
            ) {
                if nx > 1.0 || ny > 1.0 {
                    tr.normalized_1000_to_virtual(nx as f32, ny as f32)
                } else {
                    tr.screen_coords_to_virtual(nx as f32, ny as f32)
                }
            } else {
                let px = args.get("x").and_then(|v| v.as_i64()).unwrap_or(0) as i32;
                let py = args.get("y").and_then(|v| v.as_i64()).unwrap_or(0) as i32;
                tr.pixel_to_virtual(px, py)
            };
            if let Err(v) = evaluate_pc_gatekeeper(
                handler,
                agent_id,
                name,
                Capability::Input,
                &format!("input.{}:{},{}", action, vx, vy),
                &format!("vx={},vy={},right={}", vx, vy, right),
                Some("auto_pc_control"),
            ) {
                return v;
            }
            let (abs_x, abs_y) = virtual_to_sendinput_abs(vx, vy);
            match send_mouse_absolute_click(vx, vy, right) {
                Ok((cx, cy)) => json!({
                    "ok": true,
                    "action": action,
                    "virtual_x": cx,
                    "virtual_y": cy,
                    "sendinput_abs_x": abs_x,
                    "sendinput_abs_y": abs_y,
                    "capture_transform": tr
                }),
                Err(e) => json!({ "ok": false, "error": e.to_string() }),
            }
        }
        "type" | "type_text" => {
            let text = args.get("text").and_then(|v| v.as_str()).unwrap_or("");
            if text.is_empty() {
                return json!({ "ok": false, "error": "text required" });
            }
            if let Err(v) = evaluate_pc_gatekeeper(
                handler,
                agent_id,
                name,
                Capability::Input,
                "input.type_text",
                &format!("chars={}", text.chars().count()),
                Some("auto_pc_control"),
            ) {
                return v;
            }
            match send_unicode_text(text) {
                Ok(n) => json!({ "ok": true, "action": "type_text", "chars_sent": n }),
                Err(e) => json!({ "ok": false, "error": e.to_string() }),
            }
        }
        "key_combo" | "keys" => {
            let keys: Vec<String> = args
                .get("keys")
                .and_then(|v| v.as_array())
                .map(|arr| arr.iter().filter_map(|s| s.as_str().map(|k| k.to_string())).collect())
                .unwrap_or_default();
            if keys.is_empty() {
                return json!({ "ok": false, "error": "keys array required" });
            }
            if let Err(v) = evaluate_pc_gatekeeper(
                handler,
                agent_id,
                name,
                Capability::Input,
                &format!("input.key_combo:{}", keys.join("+")),
                &keys.join("+"),
                Some("auto_pc_control"),
            ) {
                return v;
            }
            let backend = WindowsNativePcControlBackend::new();
            let abort = AtomicBool::new(false);
            match backend.execute_input(&InputStep::KeyCombo(keys), &abort) {
                Ok(desc) => json!({ "ok": true, "action": "key_combo", "desc": desc }),
                Err(e) => json!({ "ok": false, "error": e.to_string() }),
            }
        }
        other => json!({ "ok": false, "error": format!("unknown pc_input action: {}", other) }),
    }
}

pub(super) fn tool_browser_navigate(
    handler: &LiveMikuApiHandler,
    agent_id: &str,
    args: &Value,
) -> Value {
    let raw_url = args.get("url").and_then(|v| v.as_str()).unwrap_or("").trim();
    if raw_url.is_empty() {
        return json!({ "ok": false, "error": "url required" });
    }
    let normalized = if raw_url.contains("://") {
        raw_url.to_string()
    } else {
        format!("https://{}", raw_url)
    };
    let (_scheme, host, _port) = match validate_browser_url(&normalized, true) {
        Ok(parts) => parts,
        Err(e) => return json!({ "ok": false, "status": "blocked_url", "error": e.to_string() }),
    };
    match launch_with_gatekeeper(handler, agent_id, &normalized) {
        Ok((resolved, simulated)) => json!({
            "ok": true,
            "url": normalized,
            "host": host,
            "label": resolved.label(),
            "simulated": simulated
        }),
        Err(err_json) => err_json,
    }
}

pub(super) fn tool_skill_run(
    handler: &LiveMikuApiHandler,
    agent_id: &str,
    args: &Value,
) -> Value {
    let skill_id = args.get("skill_id").and_then(|v| v.as_str()).unwrap_or("").trim();
    if skill_id.is_empty() {
        return json!({ "ok": false, "error": "skill_id required" });
    }
    let raw_target = args.get("target").and_then(|v| v.as_str()).unwrap_or("**");
    let raw_args = args.get("args").and_then(|v| v.as_str()).unwrap_or("");
    let now_ms = now_epoch_ms();

    let auth_res = {
        let mut st = handler.state.lock().expect("state mutex");
        let state_ref = &mut *st;
        let Some(loaded) = state_ref.skill_engine.get_skill(skill_id).cloned() else {
            return json!({ "ok": false, "error": format!("skill '{}' not found", skill_id) });
        };
        let cap = args
            .get("capability")
            .and_then(|v| v.as_str())
            .and_then(Capability::parse)
            .or_else(|| loaded.manifest.capabilities.first().copied())
            .unwrap_or(Capability::FsRead);
        let auth = state_ref.skill_engine.authorize_skill_action(
            skill_id,
            cap,
            raw_target,
            raw_args,
            &mut state_ref.gatekeeper,
            now_ms,
        );
        auth.map(|a| (a, loaded))
    };

    match auth_res {
        Ok((auth, loaded)) => {
            let entry = Path::new(&loaded.manifest.canonical_entrypoint);
            let ext = entry.extension().and_then(|s| s.to_str()).unwrap_or("").to_ascii_lowercase();
            if matches!(ext.as_str(), "md" | "txt" | "toml" | "json") {
                let doc = fs::read_to_string(entry).unwrap_or_default();
                return json!({
                    "ok": true,
                    "skill_id": auth.skill_id,
                    "version": auth.skill_version,
                    "capability": auth.capability.as_str(),
                    "entrypoint": loaded.manifest.entrypoint_rel,
                    "instructions": truncate_chars(&doc, 1400)
                });
            }
            if ext == "ps1" {
                let backend = WindowsNativePcControlBackend::new();
                let abort = AtomicBool::new(false);
                let mut ps_args = vec![
                    "-NoProfile".to_string(),
                    "-ExecutionPolicy".to_string(),
                    "Bypass".to_string(),
                    "-File".to_string(),
                    loaded.manifest.canonical_entrypoint.clone(),
                ];
                if !raw_args.trim().is_empty() {
                    ps_args.push(raw_args.to_string());
                }
                let cwd = loaded.skill_dir.to_string_lossy().to_string();
                return match backend.run_shell("powershell.exe", &ps_args, Some(&cwd), loaded.manifest.limits.timeout_ms, &abort) {
                    Ok(out) => json!({
                        "ok": out.exit_code == Some(0),
                        "skill_id": auth.skill_id,
                        "exit_code": out.exit_code,
                        "stdout": truncate_chars(&out.stdout, 1000),
                        "stderr": truncate_chars(&out.stderr, 400)
                    }),
                    Err(e) => json!({ "ok": false, "error": e.to_string() }),
                };
            }
            json!({
                "ok": true,
                "skill_id": auth.skill_id,
                "version": auth.skill_version,
                "capability": auth.capability.as_str(),
                "canonical_target": auth.canonical_target,
                "agent": agent_id
            })
        }
        Err(SkillError::GatekeeperApprovalRequired { ticket_hash, capability, .. }) => {
            let _ = handler.events_tx.send(json!({
                "type": "approve",
                "id": ticket_hash,
                "title": format!("Gatekeeper Skill: {}", skill_id),
                "command": format!("skill {}: {}", skill_id, capability.as_str()),
                "diff": format!("Target: {}\nArgs: {}", raw_target, raw_args),
                "capability": capability.as_str(),
                "target": raw_target,
                "agent_id": agent_id
            }));
            json!({ "ok": false, "status": "approval_required", "ticket": ticket_hash })
        }
        Err(e) => json!({ "ok": false, "error": e.to_string() }),
    }
}
