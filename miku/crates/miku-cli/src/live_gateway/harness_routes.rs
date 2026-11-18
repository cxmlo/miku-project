use std::fs;
use std::path::{Path, PathBuf};

use serde_json::{json, Value};

use crate::hook_cmd::{
    build_hook_gatekeeper_for_root, discover_all_backups_for_settings, install_miku_hook,
    preview_miku_hook, restore_miku_hook, seed_empty_settings_if_missing, HookTargetAgent,
    NonInteractiveGuiHookPrompter,
};

use super::agent_roster::{default_agents, discover_custom_agents, reconcile_agents};
use super::api_handler::LiveMikuApiHandler;
use super::llm_client::probe_provider_key;
use super::runtime_state::persist_json;
use super::storage_actor::now_epoch_ms;

struct HarnessSpec {
    id: &'static str,
    name: &'static str,
    kind: &'static str,
    cli_bin: &'static str,
    rel_dir: &'static [&'static str],
    config_file: &'static str,
    supports_hook_installer: bool,
    hook_target: Option<HookTargetAgent>,
    integration_mode: &'static str,
}

const HARNESS_SPECS: &[HarnessSpec] = &[
    HarnessSpec {
        id: "antigravity",
        name: "Antigravity IDE",
        kind: "ide",
        cli_bin: "agy",
        rel_dir: &[".gemini", "config"],
        config_file: "hooks.json",
        supports_hook_installer: true,
        hook_target: Some(HookTargetAgent::Antigravity),
        integration_mode: "Hooks ~/.gemini/config/hooks.json (PreToolUse / PostToolUse / Stop) + Proyectos",
    },
    HarnessSpec {
        id: "github",
        name: "Git & GitHub",
        kind: "vcs",
        cli_bin: "gh",
        rel_dir: &[".config", "gh"],
        config_file: "hosts.yml",
        supports_hook_installer: false,
        hook_target: None,
        integration_mode: "Git local (git.EXE) + API REST GitHub (PAT en Bóveda)",
    },
];

pub(super) fn try_dispatch_harness_api(
    handler: &LiveMikuApiHandler,
    method: &str,
    path: &str,
    parsed: &Value,
) -> Option<(u16, Value)> {
    match (method, path) {
        ("GET", "/api/harnesses") => Some(handle_list_harnesses()),
        ("GET", "/api/antigravity/projects") => Some(super::harness_tools::handle_antigravity_projects()),
        ("POST", "/api/hooks/event") => Some(super::harness_tools::handle_hook_relay_event(handler, parsed)),
        ("POST", "/api/harnesses/hook/preview") => Some(handle_hook_preview(parsed)),
        ("POST", "/api/harnesses/hook/install") => Some(handle_hook_install(parsed)),
        ("POST", "/api/harnesses/hook/restore") => Some(handle_hook_restore(parsed)),
        ("POST", "/api/agents/reset") => Some(handle_agents_reset(handler, parsed)),
        ("POST", "/api/secrets/test") => Some(handle_secrets_test(handler, parsed)),
        _ => None,
    }
}

fn user_home_dir() -> Option<PathBuf> {
    std::env::var_os("USERPROFILE")
        .or_else(|| std::env::var_os("HOME"))
        .map(PathBuf::from)
        .filter(|p| p.exists())
}

fn binary_on_path(bin: &str) -> bool {
    let Some(path_env) = std::env::var_os("PATH") else {
        return false;
    };
    let exts: &[&str] = if cfg!(windows) {
        &[".exe", ".cmd", ".bat", ""]
    } else {
        &[""]
    };
    for dir in std::env::split_paths(&path_env) {
        for ext in exts {
            if dir.join(format!("{}{}", bin, ext)).is_file() {
                return true;
            }
        }
    }
    false
}

fn resolve_harness_paths(home: &Path, spec: &HarnessSpec) -> (PathBuf, PathBuf) {
    let mut root = home.to_path_buf();
    for seg in spec.rel_dir {
        root.push(seg);
    }
    let cfg = root.join(spec.config_file);
    (root, cfg)
}

fn handle_list_harnesses() -> (u16, Value) {
    let Some(home) = user_home_dir() else {
        return (
            200,
            json!({ "ok": true, "home": "", "harnesses": [], "git_installed": binary_on_path("git") }),
        );
    };

    let now_ms = now_epoch_ms();
    let mut items = Vec::with_capacity(HARNESS_SPECS.len());
    for spec in HARNESS_SPECS {
        let (root_dir, config_path) = resolve_harness_paths(&home, spec);
        let dir_exists = root_dir.is_dir();
        let cli_installed = binary_on_path(spec.cli_bin);
        let installed = dir_exists || cli_installed;

        let mut hook_installed = false;
        let mut config_exists = config_path.is_file();
        let mut backup_count = 0usize;
        let mut status_note = if installed {
            "Detectado en este equipo"
        } else {
            "No detectado en el sistema"
        }
        .to_string();

        if let Some(agent) = spec.hook_target {
            if dir_exists {
                if let Ok(mut gk) = build_hook_gatekeeper_for_root(&root_dir) {
                    if let Ok(prev) =
                        preview_miku_hook(&root_dir, &config_path, agent, &mut gk, now_ms)
                    {
                        config_exists = prev.file_exists;
                        hook_installed = prev.already_installed;
                        backup_count = prev.backup_count;
                        if hook_installed {
                            status_note = "Hook / MCP de Miku instalado".to_string();
                        } else if prev.file_exists {
                            status_note = "Listo para inyectar hook (backup .bak automático)".to_string();
                        } else {
                            status_note = "Directorio detectado (archivo se creará tras confirmación)".to_string();
                        }
                    }
                }
            }
        } else if spec.id == "github" {
            let git_ok = binary_on_path("git");
            status_note = match (git_ok, cli_installed) {
                (true, true) => "Git y GitHub CLI (`gh`) disponibles".to_string(),
                (true, false) => "Git disponible (`gh` opcional)".to_string(),
                _ => "Git / GitHub CLI no detectados en PATH".to_string(),
            };
        }

        let mut passive_obs_db = false;
        let mut passive_sessions = 0usize;
        if spec.id == "antigravity" && dir_exists {
            passive_obs_db = root_dir.join("conversation_summaries.db").is_file();
            if let Ok(rd) = fs::read_dir(root_dir.join("conversations")) {
                passive_sessions = rd.filter_map(Result::ok).count();
            }
            if !hook_installed && passive_obs_db {
                status_note = format!(
                    "Antigravity detectado ({} sesiones); MCP listo para conectar",
                    passive_sessions
                );
            }
        }

        items.push(json!({
            "id": spec.id,
            "name": spec.name,
            "kind": spec.kind,
            "installed": installed,
            "dir_exists": dir_exists,
            "cli_installed": cli_installed,
            "config_exists": config_exists,
            "config_path": config_path.to_string_lossy(),
            "root_dir": root_dir.to_string_lossy(),
            "supports_hook_installer": spec.supports_hook_installer,
            "hook_installed": hook_installed,
            "backup_count": backup_count,
            "integration_mode": spec.integration_mode,
            "passive_obs_db": passive_obs_db,
            "passive_sessions": passive_sessions,
            "status_note": status_note,
        }));
    }

    (
        200,
        json!({
            "ok": true,
            "home": home.to_string_lossy(),
            "git_installed": binary_on_path("git"),
            "harnesses": items,
        }),
    )
}

fn find_hook_spec(agent_raw: &str) -> Result<(&'static HarnessSpec, HookTargetAgent), (u16, Value)> {
    let agent = HookTargetAgent::parse(agent_raw)
        .map_err(|e| (400, json!({ "ok": false, "error": e.to_string() })))?;
    let spec = HARNESS_SPECS
        .iter()
        .find(|s| s.hook_target == Some(agent))
        .ok_or_else(|| (400, json!({ "ok": false, "error": "harness does not support hooks" })))?;
    Ok((spec, agent))
}

fn handle_hook_preview(parsed: &Value) -> (u16, Value) {
    let agent_raw = parsed.get("agent").and_then(|v| v.as_str()).unwrap_or("");
    let (spec, agent) = match find_hook_spec(agent_raw) {
        Ok(x) => x,
        Err(resp) => return resp,
    };
    let Some(home) = user_home_dir() else {
        return (400, json!({ "ok": false, "error": "USERPROFILE no disponible" }));
    };
    let (root_dir, config_path) = resolve_harness_paths(&home, spec);
    if !root_dir.is_dir() {
        return (
            404,
            json!({
                "ok": false,
                "error": format!("Directorio del agente '{}' no existe aún ({})", spec.name, root_dir.display())
            }),
        );
    }
    let Ok(mut gk) = build_hook_gatekeeper_for_root(&root_dir) else {
        return (500, json!({ "ok": false, "error": "Gatekeeper hook root init error" }));
    };
    match preview_miku_hook(&root_dir, &config_path, agent, &mut gk, now_epoch_ms()) {
        Ok(prev) => (
            200,
            json!({
                "ok": true,
                "agent": prev.agent.as_str(),
                "settings_path": prev.settings_path.to_string_lossy(),
                "file_exists": prev.file_exists,
                "already_installed": prev.already_installed,
                "idempotent_noop": prev.idempotent_noop,
                "backup_count": prev.backup_count,
                "latest_backup_path": prev.latest_backup_path.map(|p| p.to_string_lossy().to_string()),
                "unified_diff": prev.unified_diff,
            }),
        ),
        Err(e) => (400, json!({ "ok": false, "error": e.to_string() })),
    }
}

fn handle_hook_install(parsed: &Value) -> (u16, Value) {
    let confirmed = parsed
        .get("confirmed")
        .or_else(|| parsed.get("confirm"))
        .and_then(|v| v.as_bool())
        .unwrap_or(false);
    if !confirmed {
        return (
            400,
            json!({ "ok": false, "error": "Confirmación explícita requerida tras revisar el diff." }),
        );
    }
    let agent_raw = parsed.get("agent").and_then(|v| v.as_str()).unwrap_or("");
    let (spec, agent) = match find_hook_spec(agent_raw) {
        Ok(x) => x,
        Err(resp) => return resp,
    };
    let Some(home) = user_home_dir() else {
        return (400, json!({ "ok": false, "error": "USERPROFILE no disponible" }));
    };
    let (root_dir, config_path) = resolve_harness_paths(&home, spec);
    if !root_dir.is_dir() {
        return (
            404,
            json!({ "ok": false, "error": format!("{} no está instalado en este equipo", spec.name) }),
        );
    }
    let Ok(mut gk) = build_hook_gatekeeper_for_root(&root_dir) else {
        return (500, json!({ "ok": false, "error": "Gatekeeper init error" }));
    };
    let now_ms = now_epoch_ms();
    if let Err(e) = seed_empty_settings_if_missing(&root_dir, &config_path, &mut gk, now_ms) {
        return (400, json!({ "ok": false, "error": e.to_string() }));
    }
    let prompter = NonInteractiveGuiHookPrompter { confirmed: true };
    match install_miku_hook(&root_dir, &config_path, agent, None, &prompter, &mut gk, now_ms) {
        Ok(out) => {
            let backup_count = discover_all_backups_for_settings(&root_dir, &out.settings_path)
                .map(|b| b.len())
                .unwrap_or(1);
            (
                200,
                json!({
                    "ok": true,
                    "agent": out.agent.as_str(),
                    "applied": out.applied,
                    "idempotent_noop": out.idempotent_noop,
                    "settings_path": out.settings_path.to_string_lossy(),
                    "backup_path": out.backup_path.to_string_lossy(),
                    "backup_count": backup_count,
                    "unified_diff": out.unified_diff,
                    "elapsed_ms": out.elapsed_ms,
                }),
            )
        }
        Err(e) => (400, json!({ "ok": false, "error": e.to_string() })),
    }
}

fn handle_hook_restore(parsed: &Value) -> (u16, Value) {
    let agent_raw = parsed.get("agent").and_then(|v| v.as_str()).unwrap_or("");
    let (spec, _agent) = match find_hook_spec(agent_raw) {
        Ok(x) => x,
        Err(resp) => return resp,
    };
    let Some(home) = user_home_dir() else {
        return (400, json!({ "ok": false, "error": "USERPROFILE no disponible" }));
    };
    let (root_dir, config_path) = resolve_harness_paths(&home, spec);
    let Ok(mut gk) = build_hook_gatekeeper_for_root(&root_dir) else {
        return (500, json!({ "ok": false, "error": "Gatekeeper init error" }));
    };
    match restore_miku_hook(&root_dir, &config_path, None, &mut gk, now_epoch_ms()) {
        Ok(out) => (
            200,
            json!({
                "ok": true,
                "settings_path": out.settings_path.to_string_lossy(),
                "backup_path": out.backup_path.to_string_lossy(),
                "restored_bytes": out.restored_bytes,
                "elapsed_ms": out.elapsed_ms,
            }),
        ),
        Err(e) => (400, json!({ "ok": false, "error": e.to_string() })),
    }
}

fn handle_agents_reset(handler: &LiveMikuApiHandler, parsed: &Value) -> (u16, Value) {
    let only_id = parsed
        .get("id")
        .and_then(|v| v.as_str())
        .map(str::trim)
        .filter(|s| !s.is_empty());

    let snap = {
        let mut st = handler.state.lock().expect("state mutex");
        let defs = default_agents();
        if let Some(target_id) = only_id {
            if let Some(def) = defs.into_iter().find(|d| d.id == target_id) {
                if let Some(slot) = st.agents.iter_mut().find(|a| a.id == target_id) {
                    *slot = def;
                } else {
                    st.agents.push(def);
                }
            }
        } else {
            let mut next = defs;
            let agents_dir = st.agents_dir.clone();
            discover_custom_agents(&agents_dir, &mut next);
            st.agents = next;
        }
        let (reconciled, _) = reconcile_agents(std::mem::take(&mut st.agents));
        st.agents = reconciled;
        persist_json(&st.data_dir.join("agents.json"), &st.agents);
        st.snapshot_json()
    };
    handler.broadcast_state(&snap);
    (
        200,
        json!({
            "ok": true,
            "agents": snap.get("agents").cloned().unwrap_or(Value::Null),
        }),
    )
}

fn handle_secrets_test(handler: &LiveMikuApiHandler, parsed: &Value) -> (u16, Value) {
    let alias = parsed.get("alias").and_then(|v| v.as_str()).unwrap_or("").trim();
    let req_prov = parsed.get("provider").and_then(|v| v.as_str()).map(str::trim);
    let req_model = parsed.get("model").and_then(|v| v.as_str()).map(str::trim);
    let raw_secret = parsed.get("secret").and_then(|v| v.as_str()).map(str::trim).unwrap_or("");
    let req_base_url = parsed
        .get("base_url")
        .and_then(|v| v.as_str())
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(|s| s.to_string());

    let (provider, model, api_key, base_url) = {
        let mut st = handler.state.lock().expect("state mutex");
        let alias_meta = (!alias.is_empty())
            .then(|| st.secret_aliases.get(alias).cloned())
            .flatten();
        let prov = req_prov
            .filter(|s| !s.is_empty())
            .map(str::to_string)
            .or_else(|| alias_meta.as_ref().map(|m| m.provider.clone()))
            .or_else(|| st.settings.get("provider").and_then(|v| v.as_str()).map(str::to_string))
            .unwrap_or_else(|| "openai".to_string());
        let mod_name = req_model
            .filter(|s| !s.is_empty())
            .map(str::to_string)
            .or_else(|| alias_meta.as_ref().and_then(|m| m.model.clone()))
            .or_else(|| st.settings.get("model").and_then(|v| v.as_str()).map(str::to_string))
            .unwrap_or_else(|| default_probe_model(&prov).to_string());
        let key = if !raw_secret.is_empty() {
            raw_secret.to_string()
        } else if !alias.is_empty() {
            st.resolve_secret_by_alias(alias)
                .or_else(|| st.resolve_chat_credential(&prov).map(|(_, k)| k))
                .unwrap_or_default()
        } else {
            st.resolve_chat_credential(&prov)
                .map(|(_, k)| k)
                .unwrap_or_default()
        };
        let b_url = req_base_url.or_else(|| {
            st.settings
                .get("custom_base_url")
                .and_then(|v| v.as_str())
                .map(str::trim)
                .filter(|s| !s.is_empty())
                .map(str::to_string)
        });
        (prov, mod_name, key, b_url)
    };

    if api_key.is_empty() && !provider.eq_ignore_ascii_case("ollama") {
        return (
            200,
            json!({
                "ok": false,
                "status": 400,
                "provider": provider,
                "model": model,
                "error": "No hay clave API disponible para probar este proveedor."
            }),
        );
    }

    match probe_provider_key(&provider, &model, &api_key, base_url.as_deref()) {
        Ok((latency_ms, host)) => (
            200,
            json!({
                "ok": true,
                "status": 200,
                "provider": provider,
                "model": model,
                "latency_ms": latency_ms,
                "host": host,
            }),
        ),
        Err(err) => (
            200,
            json!({
                "ok": false,
                "provider": provider,
                "model": model,
                "status": err.status_code.unwrap_or(502),
                "error": err.message,
            }),
        ),
    }
}

fn default_probe_model(provider: &str) -> &'static str {
    match provider.trim().to_ascii_lowercase().as_str() {
        "anthropic" => "claude-3-5-haiku-latest",
        "gemini" => "gemini-2.5-flash",
        "deepseek" => "deepseek-chat",
        "ollama" => "qwen2.5-coder:7b",
        _ => "gpt-4o-mini",
    }
}
