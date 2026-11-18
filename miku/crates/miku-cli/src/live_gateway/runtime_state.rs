use std::collections::{BTreeMap, HashMap, VecDeque};
use std::fs;
use std::path::{Path, PathBuf};

#[cfg(target_os = "windows")]
use miku_agent::secrets::WindowsCredentialBackend;
use miku_agent::secrets::{EncryptedMemoryBackend, SecretManager, SecretStoreBackend};
use miku_gatekeeper::{
    CanonicalPathJail, Capability, GatekeeperEngine, PolicyEffect, ProjectProfile,
};
use miku_gateway::lan_pairing::{LanPairingConfig, LanPairingManager};
use miku_gateway::DEFAULT_LOOPBACK_PORT;
use miku_protocol::sha256_hex;
use miku_skills::{SkillEngine, SKILL_MANIFEST_FILENAME};
use serde::{Deserialize, Serialize};
use serde_json::{json, Map, Value};

use super::agent_roster::{default_agents, discover_custom_agents, reconcile_agents};
use super::storage_actor::{now_epoch_ms, AgentProfileDto, McpServerEntry, SecretAliasIndexEntry};
use super::telemetry::UsageTelemetryState;

const INTERNAL_SECRET_SESSION_TOKEN: &str = "miku-live-gateway-vault-session";
const GATEKEEPER_DEFAULT_TTL_MS: u64 = 60_000;

pub(super) fn normalize_permission_mode(raw: &str) -> Option<&'static str> {
    match raw.trim().to_ascii_lowercase().as_str() {
        "full" | "toda_mi_pc" | "toda mi pc" | "pc" => Some("full"),
        "sandbox" => Some("sandbox"),
        "restricted" | "restringido" | "read_only" => Some("restricted"),
        _ => None,
    }
}

pub(super) fn build_gatekeeper_profile_for_mode(root_str: &str, mode: &str) -> ProjectProfile {
    let profile = ProjectProfile::new("miku-workspace", root_str, GATEKEEPER_DEFAULT_TTL_MS);
    match mode {
        "full" => profile
            .with_rule(Capability::FsRead, "**", PolicyEffect::Allow, true)
            .with_rule(Capability::FsWrite, "**", PolicyEffect::Allow, false)
            .with_rule(Capability::Clipboard, "clipboard:*", PolicyEffect::Allow, false)
            .with_rule(Capability::Network, "*", PolicyEffect::Allow, false)
            .with_rule(Capability::Input, "*", PolicyEffect::Allow, true)
            .with_rule(Capability::Vision, "*", PolicyEffect::Allow, false)
            .with_rule(Capability::UiAutomation, "*", PolicyEffect::Allow, false)
            .with_rule(Capability::Browser, "*", PolicyEffect::Allow, false)
            .with_rule(Capability::Shell, "*", PolicyEffect::Allow, false)
            .with_rule(Capability::Shell, "*git push*", PolicyEffect::Deny, true)
            .with_rule(Capability::Shell, "*rm -rf*", PolicyEffect::Deny, true)
            .with_rule(Capability::Shell, "*format *", PolicyEffect::Deny, true),
        "restricted" => profile
            .with_rule(Capability::FsRead, "**", PolicyEffect::Allow, true)
            .with_rule(Capability::Network, "chat.prompt", PolicyEffect::Allow, false)
            .with_rule(Capability::Input, "media.*", PolicyEffect::Allow, true)
            .with_rule(Capability::Vision, "*", PolicyEffect::Deny, false)
            .with_rule(Capability::UiAutomation, "*", PolicyEffect::Deny, false)
            .with_rule(Capability::FsWrite, "**", PolicyEffect::Deny, false)
            .with_rule(Capability::Clipboard, "*", PolicyEffect::Deny, false)
            .with_rule(Capability::Shell, "*", PolicyEffect::Deny, true)
            .with_rule(Capability::Browser, "*", PolicyEffect::Deny, false),
        _ => profile
            .with_rule(Capability::FsRead, "**", PolicyEffect::Allow, true)
            .with_rule(Capability::FsWrite, "workspace/**", PolicyEffect::Allow, false)
            .with_rule(Capability::FsWrite, "notes/**", PolicyEffect::Ask, false)
            .with_rule(Capability::Clipboard, "clipboard:*", PolicyEffect::Ask, false)
            .with_rule(Capability::Network, "*", PolicyEffect::Allow, false)
            .with_rule(Capability::Input, "media.*", PolicyEffect::Allow, true)
            .with_rule(Capability::Input, "input.*", PolicyEffect::Ask, false)
            .with_rule(Capability::Input, "mouse.*", PolicyEffect::Ask, false)
            .with_rule(Capability::Vision, "screen:*", PolicyEffect::Allow, false)
            .with_rule(Capability::Vision, "screen_capture:*", PolicyEffect::Allow, false)
            .with_rule(Capability::UiAutomation, "uia:inspect:*", PolicyEffect::Allow, false)
            .with_rule(Capability::UiAutomation, "*", PolicyEffect::Ask, false)
            .with_rule(Capability::Browser, "open_url:https://*", PolicyEffect::Allow, false)
            .with_rule(Capability::Browser, "open_url:http://localhost*", PolicyEffect::Allow, false)
            .with_rule(Capability::Browser, "open_url:http://127.0.0.1*", PolicyEffect::Allow, false)
            .with_rule(Capability::Browser, "open_app:*", PolicyEffect::Allow, false)
            .with_rule(Capability::Shell, "cargo test*", PolicyEffect::Ask, false)
            .with_rule(Capability::Shell, "*git push*", PolicyEffect::Deny, true)
            .with_rule(Capability::Shell, "*rm -rf*", PolicyEffect::Deny, true)
            .with_rule(Capability::Shell, "*format *", PolicyEffect::Deny, true),
    }
}

pub(super) struct LiveRuntimeState {
    pub use_memory_secrets: bool,
    pub data_dir: PathBuf,
    pub skills_dir: PathBuf,
    pub agents_dir: PathBuf,
    pub uploads_jail: CanonicalPathJail,
    pub gatekeeper: GatekeeperEngine,
    pub permission_mode: String,
    pub secrets: SecretManager,
    pub secret_aliases: BTreeMap<String, SecretAliasIndexEntry>,
    pub skill_engine: SkillEngine,
    pub mcp_servers: Vec<McpServerEntry>,
    pub agents: Vec<AgentProfileDto>,
    pub settings: Map<String, Value>,
    pub telemetry: UsageTelemetryState,
    pub pairing: LanPairingManager,
    pub pair_url: String,
    pub link_device: String,
    pub(super) link_devices: HashMap<String, super::link_routes::LinkDeviceMeta>,
    pub(super) link_notifications: VecDeque<super::link_routes::LinkNotificationItem>,
    pub link_notif_seq: u64,
    pub next_seq: u64,
}

impl LiveRuntimeState {
    pub(super) fn new(
        data_dir: &Path,
        port: u16,
        use_memory_secrets: bool,
    ) -> Result<Self, String> {
        fs::create_dir_all(data_dir).map_err(|e| format!("create data dir: {}", e))?;
        let skills_dir = data_dir.join("skills");
        let agents_dir = data_dir.join("agents");
        let uploads_dir = data_dir.join("uploads");
        fs::create_dir_all(&skills_dir).map_err(|e| e.to_string())?;
        fs::create_dir_all(&agents_dir).map_err(|e| e.to_string())?;
        fs::create_dir_all(&uploads_dir).map_err(|e| e.to_string())?;

        let root_str = data_dir.to_string_lossy().to_string();
        let uploads_jail = CanonicalPathJail::new(&uploads_dir.to_string_lossy())
            .map_err(|e| e.to_string())?;

        let settings = load_json_or_default::<Map<String, Value>>(&data_dir.join("settings.json"))
            .unwrap_or_default();
        let permission_mode = settings
            .get("permission_mode")
            .and_then(|v| v.as_str())
            .and_then(normalize_permission_mode)
            .unwrap_or("sandbox")
            .to_string();

        let profile = build_gatekeeper_profile_for_mode(&root_str, &permission_mode);
        let gatekeeper = GatekeeperEngine::new(profile).map_err(|e| e.to_string())?;

        let vault_profile = build_gatekeeper_profile_for_mode(&root_str, "sandbox");
        let vault_gk = GatekeeperEngine::new(vault_profile).map_err(|e| e.to_string())?;
        let backend: Box<dyn SecretStoreBackend> = if use_memory_secrets {
            Box::new(EncryptedMemoryBackend::new())
        } else {
            #[cfg(target_os = "windows")]
            {
                Box::new(WindowsCredentialBackend::new())
            }
            #[cfg(not(target_os = "windows"))]
            {
                Box::new(EncryptedMemoryBackend::new())
            }
        };
        let secrets = SecretManager::new(
            backend,
            INTERNAL_SECRET_SESSION_TOKEN.to_string(),
            vault_gk,
        );

        let telemetry = load_json_or_default::<UsageTelemetryState>(&data_dir.join("telemetry.json"))
            .unwrap_or_default();
        let mcp_servers =
            load_json_or_default::<Vec<McpServerEntry>>(&data_dir.join("mcp.json"))
                .unwrap_or_default();
        let mut secret_aliases =
            load_json_or_default::<BTreeMap<String, SecretAliasIndexEntry>>(
                &data_dir.join("secret_aliases.json"),
            )
            .unwrap_or_default();
        secret_aliases.retain(|k, _| !k.trim().is_empty());

        let raw_agents =
            load_json_or_default::<Vec<AgentProfileDto>>(&data_dir.join("agents.json"))
                .unwrap_or_else(default_agents);
        let (mut agents, agents_migrated) = reconcile_agents(raw_agents);
        discover_custom_agents(&agents_dir, &mut agents);
        if agents_migrated || !data_dir.join("agents.json").exists() {
            persist_json(&data_dir.join("agents.json"), &agents);
        }

        let mut skill_engine = SkillEngine::new();
        reload_skills_from_root(&skills_dir, &mut skill_engine, &gatekeeper);

        let pairing_cfg = LanPairingConfig {
            enabled_by_user: false,
            lan_bind_host: "127.0.0.1".to_string(),
            port: if port == 0 {
                DEFAULT_LOOPBACK_PORT
            } else {
                port
            },
            pinned_cert_sha256: sha256_hex(b"miku-local-spki-pin-v1"),
            ..LanPairingConfig::default()
        };

        Ok(Self {
            use_memory_secrets,
            data_dir: data_dir.to_path_buf(),
            skills_dir,
            agents_dir,
            uploads_jail,
            gatekeeper,
            permission_mode,
            secrets,
            secret_aliases,
            skill_engine,
            mcp_servers,
            agents,
            settings,
            telemetry,
            pairing: LanPairingManager::new(pairing_cfg),
            pair_url: String::new(),
            link_device: String::new(),
            link_devices: HashMap::new(),
            link_notifications: super::link_routes::init_notification_queue(),
            link_notif_seq: 0,
            next_seq: 1,
        })
    }

    pub(super) fn apply_permission_mode(&mut self, raw_mode: &str) -> Result<&'static str, String> {
        let mode = normalize_permission_mode(raw_mode)
            .ok_or_else(|| format!("modo de permisos desconocido: {}", raw_mode))?;
        let was_ks = self.gatekeeper.is_kill_switch_active();
        let root_str = self.data_dir.to_string_lossy().to_string();
        let mut next_engine = GatekeeperEngine::new(build_gatekeeper_profile_for_mode(&root_str, mode))
            .map_err(|e| e.to_string())?;
        if was_ks {
            let _ = next_engine.trigger_kill_switch(now_epoch_ms());
        }
        self.gatekeeper = next_engine;
        self.permission_mode = mode.to_string();
        self.settings
            .insert("permission_mode".to_string(), json!(mode));
        persist_json(&self.data_dir.join("settings.json"), &self.settings);
        Ok(mode)
    }

    pub(super) fn record_llm_usage_with_cache(
        &mut self,
        model: &str,
        input_tokens: u64,
        cached_input_tokens: u64,
        output_tokens: u64,
        latency_ms: u64,
        is_error: bool,
    ) {
        self.telemetry.record_with_cache(
            model,
            input_tokens,
            cached_input_tokens,
            output_tokens,
            latency_ms,
            is_error,
            now_epoch_ms(),
        );
        persist_json(&self.data_dir.join("telemetry.json"), &self.telemetry);
    }

    pub(super) fn record_local_route(&mut self, est_tokens_saved: u64) {
        self.telemetry
            .record_local_route(est_tokens_saved, now_epoch_ms());
        persist_json(&self.data_dir.join("telemetry.json"), &self.telemetry);
    }

    pub(super) fn add_tokens_saved(&mut self, est_tokens_saved: u64) {
        if est_tokens_saved > 0 {
            self.telemetry.add_tokens_saved(est_tokens_saved);
            persist_json(&self.data_dir.join("telemetry.json"), &self.telemetry);
        }
    }

    pub(super) fn reset_telemetry(&mut self) {
        self.telemetry = UsageTelemetryState::default();
        persist_json(&self.data_dir.join("telemetry.json"), &self.telemetry);
    }

    pub(super) fn telemetry_snapshot(&self) -> Value {
        let active_provider = self
            .settings
            .get("provider")
            .and_then(|v| v.as_str())
            .unwrap_or("deepseek");
        let active_model = self
            .settings
            .get("model")
            .and_then(|v| v.as_str())
            .unwrap_or("deepseek-chat");
        self.telemetry
            .to_snapshot_json(active_provider, active_model)
    }

    pub(super) fn secret_names(&self) -> Vec<String> {
        let mut names: BTreeMap<String, ()> = self
            .secrets
            .list_summaries()
            .into_iter()
            .map(|s| (s.alias, ()))
            .collect();
        for alias in self.secret_aliases.keys() {
            names.insert(alias.clone(), ());
        }
        names.into_keys().collect()
    }

    pub(super) fn resolve_secret_by_alias(&mut self, alias: &str) -> Option<String> {
        let buf = self
            .secrets
            .get_secret_authorized(alias, INTERNAL_SECRET_SESSION_TOKEN)
            .ok()?;
        let key = String::from_utf8_lossy(buf.as_slice()).trim().to_string();
        (!key.is_empty()).then_some(key)
    }

    pub(super) fn resolve_chat_credential(&mut self, preferred_provider: &str) -> Option<(String, String)> {
        let prov_lc = preferred_provider.trim().to_ascii_lowercase();
        let mut ordered_aliases: Vec<String> = Vec::new();
        for (alias, meta) in &self.secret_aliases {
            if meta.provider.eq_ignore_ascii_case(&prov_lc) {
                ordered_aliases.push(alias.clone());
            }
        }
        for s in self.secrets.list_summaries() {
            if s.provider.eq_ignore_ascii_case(&prov_lc) && !ordered_aliases.contains(&s.alias) {
                ordered_aliases.push(s.alias);
            }
        }
        if !prov_lc.is_empty() && !ordered_aliases.contains(&prov_lc) {
            ordered_aliases.push(prov_lc.clone());
        }
        for alias in self.secret_names() {
            if !ordered_aliases.contains(&alias) {
                ordered_aliases.push(alias);
            }
        }
        for alias in ordered_aliases {
            if let Ok(buf) = self
                .secrets
                .get_secret_authorized(&alias, INTERNAL_SECRET_SESSION_TOKEN)
            {
                let key = String::from_utf8_lossy(buf.as_slice()).trim().to_string();
                if !key.is_empty() {
                    return Some((alias, key));
                }
            }
        }
        None
    }

    pub(super) fn rules_json(&self) -> Vec<Value> {
        self.gatekeeper
            .profile()
            .rules
            .iter()
            .enumerate()
            .map(|(idx, r)| {
                let tier = match r.effect {
                    PolicyEffect::Allow => "allow",
                    PolicyEffect::Ask => "ask",
                    PolicyEffect::Deny => "deny",
                };
                json!({
                    "id": format!("rule_{}", idx + 1),
                    "name": format!("{}: {}", r.capability.as_str(), r.pattern),
                    "tier": tier,
                    "pc_only": r.pc_only
                })
            })
            .collect()
    }

    pub(super) fn skills_json(&self) -> Vec<Value> {
        self.skill_engine
            .active_skills()
            .values()
            .map(|loaded| {
                json!({
                    "id": loaded.manifest.id,
                    "name": loaded.manifest.name,
                    "version": loaded.manifest.version.to_string(),
                    "capabilities": loaded.manifest.capabilities.iter().map(|c| c.as_str()).collect::<Vec<_>>()
                })
            })
            .collect()
    }

    pub(super) fn snapshot_json(&self) -> Value {
        let secrets = self.secret_names();
        let has_api_key = !secrets.is_empty();
        let has_groq_key = secrets.iter().any(|s| s == "groq");
        let provider = self
            .settings
            .get("provider")
            .and_then(|v| v.as_str())
            .unwrap_or("openai");
        let model = self
            .settings
            .get("model")
            .and_then(|v| v.as_str())
            .unwrap_or("gpt-4o-mini");

        json!({
            "ok": true,
            "agents": self.agents,
            "secrets": secrets,
            "rules": self.rules_json(),
            "skills": self.skills_json(),
            "mcp": self.mcp_servers,
            "kill_switch": self.gatekeeper.is_kill_switch_active(),
            "permission_mode": self.permission_mode,
            "telemetry": self.telemetry_snapshot(),
            "has_api_key": has_api_key,
            "has_groq_key": has_groq_key,
            "provider": provider,
            "model": model,
            "link_connected": self.pairing.is_lan_enabled(),
            "link_paired_count": self.link_devices.len(),
            "link_device": self.link_device,
            "link_devices": self.link_devices.values().collect::<Vec<_>>(),
            "link_unread_notifications": self.link_notifications.iter().filter(|n| !n.read).count(),
            "pair_url": self.pair_url,
            "settings": self.settings,
            "custom_agents_dir": self.agents_dir.to_string_lossy(),
            "skills_dir": self.skills_dir.to_string_lossy()
        })
    }
}

pub(super) fn reload_skills_from_root(
    skills_dir: &Path,
    engine: &mut SkillEngine,
    gatekeeper: &GatekeeperEngine,
) -> Vec<String> {
    let mut warnings = Vec::new();
    let now_ms = now_epoch_ms();
    if let Ok(rd) = fs::read_dir(skills_dir) {
        let mut dirs: Vec<PathBuf> = rd
            .flatten()
            .map(|e| e.path())
            .filter(|p| p.is_dir() && p.join(SKILL_MANIFEST_FILENAME).exists())
            .collect();
        dirs.sort();
        for dir in dirs {
            if let Err(err) = engine.load_or_reload_from_dir(&dir, gatekeeper, now_ms) {
                warnings.push(format!("{}: {}", dir.display(), err));
            }
        }
    }
    warnings
}

fn load_json_or_default<T: for<'de> Deserialize<'de>>(path: &Path) -> Option<T> {
    let raw = fs::read_to_string(path).ok()?;
    serde_json::from_str(&raw).ok()
}

pub(super) fn persist_json<T: Serialize>(path: &Path, value: &T) {
    if let Ok(raw) = serde_json::to_string_pretty(value) {
        let _ = fs::write(path, raw);
    }
}

pub(super) fn parse_query_param(query: Option<&str>, key: &str) -> String {
    let raw = query.unwrap_or("");
    for pair in raw.split('&') {
        if let Some((k, v)) = pair.split_once('=') {
            if k == key {
                return percent_decode_lossy(v);
            }
        }
    }
    String::new()
}

pub(super) fn percent_decode_lossy(input: &str) -> String {
    let replaced = input.replace('+', " ");
    let bytes = replaced.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' && i + 2 < bytes.len() {
            if let (Some(h), Some(l)) = (
                (bytes[i + 1] as char).to_digit(16),
                (bytes[i + 2] as char).to_digit(16),
            ) {
                out.push(((h << 4) | l) as u8);
                i += 3;
                continue;
            }
        }
        out.push(bytes[i]);
        i += 1;
    }
    String::from_utf8_lossy(&out).to_string()
}
