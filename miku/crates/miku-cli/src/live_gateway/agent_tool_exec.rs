//! Gatekeeper-verified execution of Miku agent tools against real local crates.

use std::fs;
use std::path::Path;
use std::sync::atomic::AtomicBool;

use miku_browser::detect_installed_browsers;
use miku_gatekeeper::{Capability, GatekeeperVerdict};
use miku_platform::{
    matches_immutable_shell_denylist, PcControlBackend, WindowsNativePcControlBackend,
};
use miku_protocol::CaptureTransform;
use serde_json::{json, Value};

use super::api_handler::LiveMikuApiHandler;
use super::media_routes::dispatch_media_vk;
use super::storage_actor::now_epoch_ms;

pub(super) fn truncate_chars(text: &str, max_chars: usize) -> String {
    if text.chars().count() <= max_chars {
        text.to_string()
    } else {
        let mut s: String = text.chars().take(max_chars).collect();
        s.push_str("...");
        s
    }
}

pub(super) fn evaluate_and_handle_gatekeeper(
    handler: &LiveMikuApiHandler,
    agent_id: &str,
    tool_name: &str,
    cap: Capability,
    target: &str,
    raw_args: &str,
) -> Result<String, Value> {
    let now_ms = now_epoch_ms();
    let verdict = {
        let mut st = handler.state.lock().expect("state mutex");
        st.gatekeeper
            .evaluate_request(agent_id, cap, target, raw_args, now_ms)
    };
    match verdict {
        GatekeeperVerdict::Allow {
            canonical_target, ..
        } => Ok(canonical_target),
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

pub(super) fn execute_single_tool(
    handler: &LiveMikuApiHandler,
    agent_id: &str,
    name: &str,
    args: &Value,
) -> Value {
    match name {
        "system_info" => tool_system_info(handler, agent_id, name),
        "memory_search" => tool_memory_search(handler, agent_id, name, args),
        "skills_list" => tool_skills_list(handler, agent_id, name),
        "apps_detect" => tool_apps_detect(handler, agent_id, name),
        "fs_list" => tool_fs_list(handler, agent_id, name, args),
        "fs_read" => tool_fs_read(handler, agent_id, name, args),
        "fs_write" => tool_fs_write(handler, agent_id, name, args),
        "shell_exec" => tool_shell_exec(handler, agent_id, name, args),
        "clipboard_read" => tool_clipboard_read(handler, agent_id, name),
        "clipboard_write" => tool_clipboard_write(handler, agent_id, name, args),
        "media_control" => tool_media_control(handler, agent_id, name, args),
        "open_app_or_url" => tool_open_app_or_url(handler, agent_id, args),
        "mouse_click" | "input_click" => tool_mouse_click(handler, agent_id, name, args),
        "key_combo" | "input_key_combo" => tool_key_combo(handler, agent_id, name, args),
        "ui_automation_invoke" => tool_ui_automation_invoke(handler, agent_id, name, args),
        "screen_coords_to_virtual" => tool_screen_coords_to_virtual(args),
        "screen_capture" | "capture_screen" => {
            super::agent_tool_vision::tool_screen_capture(handler, agent_id, name, args)
        }
        "ui_inspect" => super::agent_tool_vision::tool_ui_inspect(handler, agent_id, name, args),
        "ui_interact" => super::agent_tool_vision::tool_ui_interact(handler, agent_id, name, args),
        "pc_input" => super::agent_tool_vision::tool_pc_input(handler, agent_id, name, args),
        "browser_navigate" => super::agent_tool_vision::tool_browser_navigate(handler, agent_id, args),
        "skill_run" => super::agent_tool_vision::tool_skill_run(handler, agent_id, args),
        "antigravity_projects" => super::harness_tools::tool_antigravity_projects(handler, agent_id, name),
        "git_status" => super::harness_tools::tool_git_cmd(handler, agent_id, name, args, &["status", "-sb"]),
        "git_diff" => super::harness_tools::tool_git_cmd(handler, agent_id, name, args, &["diff", "--stat"]),
        _ => json!({ "ok": false, "error": format!("herramienta desconocida: {}", name) }),
    }
}

fn tool_system_info(handler: &LiveMikuApiHandler, agent_id: &str, name: &str) -> Value {
    if let Err(v) =
        evaluate_and_handle_gatekeeper(handler, agent_id, name, Capability::FsRead, "system.info", "")
    {
        return v;
    }
    let st = handler.state.lock().expect("state mutex");
    json!({
        "ok": true,
        "os": std::env::consts::OS,
        "provider": st.settings.get("provider").and_then(|v| v.as_str()).unwrap_or("openai"),
        "model": st.settings.get("model").and_then(|v| v.as_str()).unwrap_or("gpt-4o-mini"),
        "agents_count": st.agents.len(),
        "skills_count": st.skill_engine.active_skill_count(),
        "mcp_count": st.mcp_servers.len(),
        "kill_switch": st.gatekeeper.is_kill_switch_active(),
        "data_dir": st.data_dir.to_string_lossy()
    })
}

fn tool_memory_search(
    handler: &LiveMikuApiHandler,
    agent_id: &str,
    name: &str,
    args: &Value,
) -> Value {
    let query = args.get("query").and_then(|v| v.as_str()).unwrap_or("").trim();
    if query.is_empty() {
        return json!({ "ok": false, "error": "query requerida" });
    }
    if let Err(v) = evaluate_and_handle_gatekeeper(
        handler,
        agent_id,
        name,
        Capability::FsRead,
        "memory.fts5",
        query,
    ) {
        return v;
    }
    let limit = args
        .get("limit")
        .and_then(|v| v.as_u64())
        .unwrap_or(5)
        .clamp(1, 5) as usize;
    match handler.storage.search_fts5(query, limit) {
        Ok(hits) => {
            let items: Vec<Value> = hits
                .into_iter()
                .map(|h| {
                    json!({
                        "doc_id": h.doc_id,
                        "title": h.title,
                        "snippet": truncate_chars(&h.snippet, 160),
                        "score": h.bm25_score
                    })
                })
                .collect();
            json!({ "ok": true, "hits": items })
        }
        Err(e) => json!({ "ok": false, "error": e }),
    }
}

fn tool_skills_list(handler: &LiveMikuApiHandler, agent_id: &str, name: &str) -> Value {
    if let Err(v) = evaluate_and_handle_gatekeeper(
        handler,
        agent_id,
        name,
        Capability::FsRead,
        "skills.catalog",
        "",
    ) {
        return v;
    }
    let st = handler.state.lock().expect("state mutex");
    let list: Vec<Value> = st
        .skill_engine
        .active_skills()
        .values()
        .map(|s| {
            json!({
                "id": s.manifest.id,
                "version": s.manifest.version.to_string(),
                "capabilities": s.manifest.capabilities.iter().map(|c| c.as_str()).collect::<Vec<_>>()
            })
        })
        .collect();
    json!({ "ok": true, "count": list.len(), "skills": list })
}

fn tool_apps_detect(handler: &LiveMikuApiHandler, agent_id: &str, name: &str) -> Value {
    if let Err(v) = evaluate_and_handle_gatekeeper(
        handler,
        agent_id,
        name,
        Capability::FsRead,
        "apps.detect",
        "",
    ) {
        return v;
    }
    let browsers: Vec<Value> = detect_installed_browsers()
        .into_iter()
        .map(|b| json!({ "name": b.engine.slug(), "path": b.executable_path }))
        .collect();
    let mcp: Vec<Value> = {
        let st = handler.state.lock().expect("state mutex");
        st.mcp_servers
            .iter()
            .map(|m| json!({ "name": m.name, "command": m.command }))
            .collect()
    };
    json!({ "ok": true, "browsers": browsers, "mcp": mcp })
}

fn tool_fs_list(
    handler: &LiveMikuApiHandler,
    agent_id: &str,
    name: &str,
    args: &Value,
) -> Value {
    let rel = args.get("path").and_then(|v| v.as_str()).unwrap_or(".");
    let canon = match evaluate_and_handle_gatekeeper(
        handler,
        agent_id,
        name,
        Capability::FsRead,
        rel,
        "fs_list",
    ) {
        Ok(c) => c,
        Err(v) => return v,
    };
    match fs::read_dir(Path::new(&canon)) {
        Ok(rd) => {
            let mut entries: Vec<Value> = rd
                .flatten()
                .take(40)
                .map(|e| {
                    let is_dir = e.path().is_dir();
                    let bytes = e.metadata().map(|m| m.len()).unwrap_or(0);
                    json!({
                        "name": e.file_name().to_string_lossy(),
                        "kind": if is_dir { "dir" } else { "file" },
                        "bytes": bytes
                    })
                })
                .collect();
            entries.sort_by(|a, b| {
                a.get("name")
                    .and_then(|v| v.as_str())
                    .cmp(&b.get("name").and_then(|v| v.as_str()))
            });
            json!({ "ok": true, "path": canon, "entries": entries })
        }
        Err(e) => json!({ "ok": false, "error": e.to_string() }),
    }
}

fn tool_fs_read(
    handler: &LiveMikuApiHandler,
    agent_id: &str,
    name: &str,
    args: &Value,
) -> Value {
    let path = args.get("path").and_then(|v| v.as_str()).unwrap_or("").trim();
    if path.is_empty() {
        return json!({ "ok": false, "error": "path requerido" });
    }
    let canon = match evaluate_and_handle_gatekeeper(
        handler,
        agent_id,
        name,
        Capability::FsRead,
        path,
        "fs_read",
    ) {
        Ok(c) => c,
        Err(v) => return v,
    };
    let backend = WindowsNativePcControlBackend::new();
    match backend.fs_read(&canon) {
        Ok(content) => json!({
            "ok": true,
            "path": canon,
            "content": truncate_chars(&content, 1_200)
        }),
        Err(e) => json!({ "ok": false, "error": e.to_string() }),
    }
}

fn tool_fs_write(
    handler: &LiveMikuApiHandler,
    agent_id: &str,
    name: &str,
    args: &Value,
) -> Value {
    let path = args.get("path").and_then(|v| v.as_str()).unwrap_or("").trim();
    let content = args.get("content").and_then(|v| v.as_str()).unwrap_or("");
    if path.is_empty() {
        return json!({ "ok": false, "error": "path requerido" });
    }
    let raw_args = format!("fs_write|bytes={}", content.len());
    let canon = match evaluate_and_handle_gatekeeper(
        handler,
        agent_id,
        name,
        Capability::FsWrite,
        path,
        &raw_args,
    ) {
        Ok(c) => c,
        Err(v) => return v,
    };
    let backend = WindowsNativePcControlBackend::new();
    match backend.fs_write(&canon, content, true) {
        Ok(bytes) => json!({ "ok": true, "path": canon, "bytes_written": bytes }),
        Err(e) => json!({ "ok": false, "error": e.to_string() }),
    }
}

fn tool_shell_exec(
    handler: &LiveMikuApiHandler,
    agent_id: &str,
    name: &str,
    args: &Value,
) -> Value {
    let cmd = args
        .get("command")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .trim();
    if cmd.is_empty() {
        return json!({ "ok": false, "error": "command requerido" });
    }
    let (prog, prog_args): (&str, Vec<String>) = if cfg!(windows) {
        ("cmd.exe", vec!["/C".to_string(), cmd.to_string()])
    } else {
        ("sh", vec!["-c".to_string(), cmd.to_string()])
    };
    if matches_immutable_shell_denylist(cmd, &[]).is_some()
        || matches_immutable_shell_denylist(prog, &prog_args).is_some()
    {
        return json!({
            "ok": false,
            "status": "denied",
            "reason": "immutable_shell_denylist",
            "detail": "comando bloqueado por la lista inmutable de Gatekeeper"
        });
    }
    if let Err(v) =
        evaluate_and_handle_gatekeeper(handler, agent_id, name, Capability::Shell, cmd, cmd)
    {
        return v;
    }
    let cwd = handler
        .state
        .lock()
        .expect("state mutex")
        .data_dir
        .to_string_lossy()
        .to_string();
    let backend = WindowsNativePcControlBackend::new();
    let abort = AtomicBool::new(false);
    match backend.run_shell(prog, &prog_args, Some(&cwd), 10_000, &abort) {
        Ok(res) => json!({
            "ok": res.exit_code == Some(0),
            "exit_code": res.exit_code,
            "stdout": truncate_chars(&res.stdout, 900),
            "stderr": truncate_chars(&res.stderr, 400)
        }),
        Err(e) => json!({ "ok": false, "error": e.to_string() }),
    }
}

fn tool_clipboard_read(handler: &LiveMikuApiHandler, agent_id: &str, name: &str) -> Value {
    if let Err(v) = evaluate_and_handle_gatekeeper(
        handler,
        agent_id,
        name,
        Capability::Clipboard,
        "clipboard:read",
        "",
    ) {
        return v;
    }
    let backend = WindowsNativePcControlBackend::new();
    match backend.clipboard_read() {
        Ok(txt) => json!({ "ok": true, "text": truncate_chars(&txt, 800) }),
        Err(e) => json!({ "ok": false, "error": e.to_string() }),
    }
}

fn tool_clipboard_write(
    handler: &LiveMikuApiHandler,
    agent_id: &str,
    name: &str,
    args: &Value,
) -> Value {
    let text = args.get("text").and_then(|v| v.as_str()).unwrap_or("");
    if let Err(v) = evaluate_and_handle_gatekeeper(
        handler,
        agent_id,
        name,
        Capability::Clipboard,
        "clipboard:write",
        &format!("bytes={}", text.len()),
    ) {
        return v;
    }
    let backend = WindowsNativePcControlBackend::new();
    match backend.clipboard_write(text) {
        Ok(()) => json!({ "ok": true, "bytes": text.len() }),
        Err(e) => json!({ "ok": false, "error": e.to_string() }),
    }
}

fn tool_media_control(
    handler: &LiveMikuApiHandler,
    agent_id: &str,
    name: &str,
    args: &Value,
) -> Value {
    let action = args
        .get("action")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .trim();
    let vk: u8 = match action {
        "play_pause" | "toggle" => 0xB3,
        "next" => 0xB0,
        "prev" => 0xB1,
        "mute" => 0xAD,
        "vol_down" => 0xAE,
        "vol_up" => 0xAF,
        _ => return json!({ "ok": false, "error": "acción media no soportada" }),
    };
    if let Err(v) = evaluate_and_handle_gatekeeper(
        handler,
        agent_id,
        name,
        Capability::Input,
        &format!("media.{}", action),
        action,
    ) {
        return v;
    }
    dispatch_media_vk(vk);
    json!({ "ok": true, "action": action })
}

fn tool_open_app_or_url(handler: &LiveMikuApiHandler, agent_id: &str, args: &Value) -> Value {
    let target = args
        .get("target")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .trim();
    match super::app_launcher::launch_with_gatekeeper(handler, agent_id, target) {
        Ok((resolved, simulated)) => json!({
            "ok": true,
            "kind": resolved.kind_str(),
            "label": resolved.label(),
            "target": resolved.display_target(),
            "simulated": simulated
        }),
        Err(err_val) => err_val,
    }
}

fn tool_mouse_click(handler: &LiveMikuApiHandler, agent_id: &str, name: &str, args: &Value) -> Value {
    let transform: CaptureTransform = args.get("capture_transform").or_else(|| args.get("transform"))
        .and_then(|v| serde_json::from_value(v.clone()).ok()).unwrap_or_else(|| handler.last_capture_transform());
    let (vx, vy) = if let (Some(nx), Some(ny)) = (args.get("x_norm").and_then(|v| v.as_f64()), args.get("y_norm").and_then(|v| v.as_f64())) {
        if nx > 1.0 || ny > 1.0 {
            transform.normalized_1000_to_virtual(nx as f32, ny as f32)
        } else {
            transform.screen_coords_to_virtual(nx as f32, ny as f32)
        }
    } else {
        let px = args.get("x").and_then(|v| v.as_i64()).unwrap_or(0) as i32;
        let py = args.get("y").and_then(|v| v.as_i64()).unwrap_or(0) as i32;
        transform.pixel_to_virtual(px, py)
    };
    if let Err(v) = evaluate_and_handle_gatekeeper(handler, agent_id, name, Capability::Input,
        &format!("mouse.click:{},{}", vx, vy), &format!("vx={},vy={}", vx, vy)) {
        return v;
    }
    let backend = WindowsNativePcControlBackend::new();
    let abort = AtomicBool::new(false);
    let step = miku_platform::InputStep::MouseClick { x: vx, y: vy };
    match backend.execute_input(&step, &abort) {
        Ok(desc) => json!({ "ok": true, "virtual_x": vx, "virtual_y": vy, "transform_applied": transform, "desc": desc }),
        Err(e) => json!({ "ok": false, "error": e.to_string() }),
    }
}

fn tool_screen_coords_to_virtual(args: &Value) -> Value {
    let transform: CaptureTransform = args.get("capture_transform").or_else(|| args.get("transform"))
        .and_then(|v| serde_json::from_value(v.clone()).ok()).unwrap_or_default();
    let (vx, vy) = if let (Some(nx), Some(ny)) = (args.get("x_norm").and_then(|v| v.as_f64()), args.get("y_norm").and_then(|v| v.as_f64())) {
        if nx > 1.0 || ny > 1.0 {
            transform.normalized_1000_to_virtual(nx as f32, ny as f32)
        } else {
            transform.screen_coords_to_virtual(nx as f32, ny as f32)
        }
    } else {
        let px = args.get("x").and_then(|v| v.as_i64()).unwrap_or(0) as i32;
        let py = args.get("y").and_then(|v| v.as_i64()).unwrap_or(0) as i32;
        transform.pixel_to_virtual(px, py)
    };
    json!({ "ok": true, "virtual_x": vx, "virtual_y": vy, "transform": transform })
}

fn tool_key_combo(handler: &LiveMikuApiHandler, agent_id: &str, name: &str, args: &Value) -> Value {
    let keys = args.get("keys").and_then(|v| v.as_array())
        .map(|arr| arr.iter().filter_map(|s| s.as_str().map(|k| k.to_string())).collect::<Vec<_>>())
        .unwrap_or_default();
    if keys.is_empty() {
        return json!({ "ok": false, "error": "keys array required" });
    }
    if let Err(v) = evaluate_and_handle_gatekeeper(handler, agent_id, name, Capability::Input,
        &format!("input.key_combo:{}", keys.join("+")), &keys.join("+")) {
        return v;
    }
    let backend = WindowsNativePcControlBackend::new();
    let abort = AtomicBool::new(false);
    let step = miku_platform::InputStep::KeyCombo(keys.clone());
    match backend.execute_input(&step, &abort) {
        Ok(desc) => json!({ "ok": true, "keys": keys, "desc": desc }),
        Err(e) => json!({ "ok": false, "error": e.to_string() }),
    }
}

fn tool_ui_automation_invoke(handler: &LiveMikuApiHandler, agent_id: &str, name: &str, args: &Value) -> Value {
    let window_title = args.get("window_title").and_then(|v| v.as_str()).unwrap_or("");
    let automation_id = args.get("automation_id").and_then(|v| v.as_str()).unwrap_or("");
    let control_type = args.get("control_type").and_then(|v| v.as_str()).unwrap_or("Button");
    if let Err(v) = evaluate_and_handle_gatekeeper(handler, agent_id, name, Capability::UiAutomation,
        &format!("uia:{}:{}", window_title, automation_id), &format!("type={}", control_type)) {
        return v;
    }
    let backend = WindowsNativePcControlBackend::new();
    match backend.ui_automation_invoke(window_title, automation_id, control_type) {
        Ok(receipt) => json!({ "ok": true, "used_ui_automation_tree": receipt.used_ui_automation_tree, "target_summary": receipt.target_summary }),
        Err(e) => json!({ "ok": false, "error": e.to_string() }),
    }
}
