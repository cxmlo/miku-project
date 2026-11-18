use std::fs;
use std::path::PathBuf;

use miku_gatekeeper::Capability;
use serde_json::{json, Value};

use super::agent_tool_exec::{evaluate_and_handle_gatekeeper, truncate_chars};
use super::api_handler::LiveMikuApiHandler;
use super::storage_actor::now_epoch_ms;

fn user_home_dir() -> Option<PathBuf> {
    std::env::var_os("USERPROFILE")
        .or_else(|| std::env::var_os("HOME"))
        .map(PathBuf::from)
}

fn decode_file_uri(uri: &str) -> Option<String> {
    let rest = uri.strip_prefix("file:///")?;
    let mut out = String::with_capacity(rest.len());
    let bytes = rest.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' && i + 2 < bytes.len() {
            if let Ok(hex) = std::str::from_utf8(&bytes[i + 1..i + 3]) {
                if let Ok(b) = u8::from_str_radix(hex, 16) {
                    out.push(b as char);
                    i += 3;
                    continue;
                }
            }
        }
        out.push(bytes[i] as char);
        i += 1;
    }
    Some(out.replace('/', "\\"))
}

pub(super) fn list_antigravity_projects_summary() -> Vec<Value> {
    let Some(home) = user_home_dir() else {
        return Vec::new();
    };
    let dir = home.join(".gemini").join("config").join("projects");
    let Ok(rd) = fs::read_dir(&dir) else {
        return Vec::new();
    };
    let mut projects = Vec::new();
    for entry in rd.filter_map(Result::ok) {
        let p = entry.path();
        if p.extension().and_then(|e| e.to_str()) != Some("json") {
            continue;
        }
        let Ok(raw) = fs::read_to_string(&p) else {
            continue;
        };
        let Ok(v) = serde_json::from_str::<Value>(&raw) else {
            continue;
        };
        let id = v.get("id").and_then(Value::as_str).unwrap_or("").to_string();
        let name = v.get("name").and_then(Value::as_str).unwrap_or("").to_string();
        let mut folders = Vec::new();
        if let Some(arr) = v
            .get("projectResources")
            .and_then(|r| r.get("resources"))
            .and_then(Value::as_array)
        {
            for res in arr {
                let uri = res
                    .get("folderUri")
                    .and_then(Value::as_str)
                    .or_else(|| {
                        res.get("gitFolder")
                            .and_then(|g| g.get("folderUri"))
                            .and_then(Value::as_str)
                    });
                if let Some(u) = uri {
                    if let Some(decoded) = decode_file_uri(u) {
                        folders.push(decoded);
                    }
                }
            }
        }
        projects.push(json!({
            "id": id,
            "name": name,
            "folders": folders,
            "file": p.file_name().and_then(|f| f.to_str()).unwrap_or("")
        }));
    }
    projects
}

pub(super) fn handle_antigravity_projects() -> (u16, Value) {
    let projects = list_antigravity_projects_summary();
    (200, json!({ "ok": true, "count": projects.len(), "projects": projects }))
}

fn is_antigravity_command_preapproved(cmd: &str) -> bool {
    let trimmed = cmd.trim();
    if trimmed.is_empty() {
        return false;
    }
    let Some(home) = user_home_dir() else {
        return false;
    };
    let cfg_path = home.join(".gemini").join("config").join("config.json");
    let Ok(raw) = fs::read_to_string(cfg_path) else {
        return false;
    };
    let Ok(v) = serde_json::from_str::<Value>(&raw) else {
        return false;
    };
    let Some(allows) = v
        .get("userSettings")
        .and_then(|u| u.get("globalPermissionGrants"))
        .and_then(|g| g.get("allow"))
        .and_then(Value::as_array)
    else {
        return false;
    };
    let first_word = trimmed.split_whitespace().next().unwrap_or("");
    allows.iter().filter_map(Value::as_str).any(|rule| {
        if let Some(inner) = rule.trim().strip_prefix("command(").and_then(|s| s.strip_suffix(')')) {
            let prefix = inner.trim();
            !prefix.is_empty()
                && (trimmed == prefix
                    || trimmed.starts_with(&format!("{} ", prefix))
                    || first_word == prefix)
        } else {
            false
        }
    })
}

pub(super) fn handle_hook_relay_event(handler: &LiveMikuApiHandler, parsed: &Value) -> (u16, Value) {
    let agent = parsed.get("agent").and_then(Value::as_str).unwrap_or("antigravity");
    let event = parsed.get("event").and_then(Value::as_str).unwrap_or("PreToolUse");
    let tool = parsed.get("tool_name").and_then(Value::as_str).unwrap_or("");
    let cmd = parsed.get("command").and_then(Value::as_str).unwrap_or("");
    let file_path = parsed.get("file_path").and_then(Value::as_str).unwrap_or("");
    let cwd = parsed.get("cwd").and_then(Value::as_str).unwrap_or("");

    let _ = handler.events_tx.send(json!({
        "type": "hook_event",
        "agent": agent,
        "event": event,
        "tool_name": tool,
        "command": cmd,
        "file_path": file_path,
        "cwd": cwd,
        "ts_ms": now_epoch_ms()
    }));

    if event == "Stop" {
        let _ = handler.events_tx.send(json!({
            "type": "link_notification",
            "notification": {
                "id": format!("hook-stop-{}", now_epoch_ms()),
                "ts_ms": now_epoch_ms(),
                "level": "info",
                "category": "antigravity",
                "title": "Antigravity finalizó un turno",
                "body": if cwd.is_empty() { "Revisa los cambios en el IDE.".to_string() } else { format!("Proyecto: {}", cwd) },
                "source": "antigravity_hook",
                "read": false
            }
        }));
    }

    if event == "PreToolUse" && tool == "run_command" && is_antigravity_command_preapproved(cmd) {
        return (200, json!({ "ok": true, "decision": "allow", "preapproved": true }));
    }

    (200, json!({ "ok": true, "decision": "ask" }))
}

pub(super) fn tool_antigravity_projects(
    handler: &LiveMikuApiHandler,
    agent_id: &str,
    name: &str,
) -> Value {
    if let Err(v) = evaluate_and_handle_gatekeeper(
        handler,
        agent_id,
        name,
        Capability::FsRead,
        "antigravity.projects",
        "",
    ) {
        return v;
    }
    let projects = list_antigravity_projects_summary();
    json!({ "ok": true, "count": projects.len(), "projects": projects })
}

pub(super) fn tool_git_cmd(
    handler: &LiveMikuApiHandler,
    agent_id: &str,
    name: &str,
    args: &Value,
    git_args: &[&str],
) -> Value {
    let repo_path = args
        .get("repo_path")
        .and_then(|v| v.as_str())
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(|s| s.to_string())
        .unwrap_or_else(|| {
            std::env::current_dir()
                .map(|p| p.to_string_lossy().to_string())
                .unwrap_or_else(|_| ".".to_string())
        });
    if let Err(v) = evaluate_and_handle_gatekeeper(
        handler,
        agent_id,
        name,
        Capability::FsRead,
        &format!("git:{}", repo_path),
        &git_args.join(" "),
    ) {
        return v;
    }
    let out = std::process::Command::new("git")
        .arg("-C")
        .arg(&repo_path)
        .args(git_args)
        .output();
    match out {
        Ok(o) => {
            let stdout = String::from_utf8_lossy(&o.stdout);
            let stderr = String::from_utf8_lossy(&o.stderr);
            json!({
                "ok": o.status.success(),
                "repo_path": repo_path,
                "stdout": truncate_chars(&stdout, 2000),
                "stderr": truncate_chars(&stderr, 600)
            })
        }
        Err(e) => json!({ "ok": false, "error": format!("git exec failed: {}", e) }),
    }
}
