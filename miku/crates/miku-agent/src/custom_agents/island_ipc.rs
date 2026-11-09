use std::fs;
use std::path::Path;

use miku_gatekeeper::{CanonicalPathJail, Capability, GatekeeperEngine, GatekeeperVerdict};
use miku_protocol::MIKU_PROTOCOL_VERSION_V1;
use serde::{Deserialize, Serialize};

use crate::custom_agents::manifest::{
    normalize_os_path_for_agent_jail, validate_clean_text, validate_custom_agent_id,
    CustomAgentDescriptor, CustomAgentError, CUSTOM_AGENT_MANIFEST_FILENAME,
    CUSTOM_AGENT_SCHEMA_VERSION_V1, DEFAULT_CUSTOM_AGENT_MAX_CONTEXT_TOKENS,
    DEFAULT_CUSTOM_AGENT_MAX_STEPS, DEFAULT_SOUL_FILENAME,
};
use crate::custom_agents::runtime::CustomAgentOrchestrator;
use crate::harness::LlmProvider;

pub const MAX_CUSTOM_AGENT_IPC_BYTES: usize = 16_384;

/// Safe public metadata card exposed to the Island UI (`https://miku.local/*`).
/// INVARIANT: Never contains secret API key values, `SOUL.md` raw prompt text,
/// or any agent's private session history.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct IslandCustomAgentSummary {
    pub agent_id: String,
    pub display_name: String,
    pub description: String,
    pub provider: String,
    pub model: String,
    pub credential_aliases: Vec<String>,
    pub allowed_tools: Vec<String>,
    pub status: String,
    pub context_loaded_in_ram: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct IslandCreateCustomAgentRequest {
    pub id: String,
    pub display_name: String,
    pub description: String,
    pub provider: LlmProvider,
    pub model: String,
    pub soul_markdown: String,
    pub allowed_tools: Vec<String>,
    #[serde(default)]
    pub credential_aliases: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "action", content = "params", rename_all = "snake_case")]
pub enum IslandCustomAgentIpcCommand {
    List,
    Select { agent_id: String },
    Create(IslandCreateCustomAgentRequest),
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct IslandCustomAgentIpcEnvelope {
    pub jsonrpc: String,
    pub v: u8,
    pub id: String,
    pub origin: String,
    pub command: IslandCustomAgentIpcCommand,
    pub timestamp_ms: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct IslandCustomAgentIpcResponse {
    pub jsonrpc: String,
    pub v: u8,
    pub id: String,
    pub selected_agent_id: Option<String>,
    pub agents: Vec<IslandCustomAgentSummary>,
}

pub fn validate_custom_agent_ipc_origin(origin: &str) -> Result<String, CustomAgentError> {
    let trimmed = origin.trim();
    if trimmed.is_empty() {
        return Err(CustomAgentError::InvalidIpcRequest(
            "empty IPC origin is forbidden".to_string(),
        ));
    }
    if trimmed
        .chars()
        .any(|c| c == '\0' || c == '\r' || c == '\n' || c == '@' || c == '\\')
    {
        return Err(CustomAgentError::InvalidIpcRequest(
            "origin contains forbidden control or authority characters".to_string(),
        ));
    }
    let lower = trimmed.to_ascii_lowercase();
    if lower == "https://miku.local" {
        return Ok("https://miku.local/".to_string());
    }
    if let Some(tail) = lower.strip_prefix("https://miku.local/") {
        if tail.contains("..") {
            return Err(CustomAgentError::InvalidIpcRequest(
                "origin path traversal '..' is forbidden".to_string(),
            ));
        }
        return Ok(trimmed.to_string());
    }
    Err(CustomAgentError::InvalidIpcRequest(format!(
        "untrusted origin '{}'; expected 'https://miku.local/*'",
        trimmed
    )))
}

impl CustomAgentOrchestrator {
    /// Builds a safe summary for `descriptor` without loading `SOUL.md` or sessions into RAM.
    pub fn build_island_summary(
        &self,
        descriptor: &CustomAgentDescriptor,
    ) -> IslandCustomAgentSummary {
        let is_selected = self.selected_agent_id() == Some(descriptor.id.as_str());
        let is_loaded = self.is_agent_context_loaded(&descriptor.id);
        let status = match (is_selected, is_loaded) {
            (true, true) => "selected_active",
            (true, false) => "selected_idle",
            (false, true) => "loaded_ready",
            (false, false) => "idle_unloaded",
        };

        IslandCustomAgentSummary {
            agent_id: descriptor.id.clone(),
            display_name: descriptor.display_name.clone(),
            description: descriptor.description.clone(),
            provider: descriptor.provider.as_str().to_string(),
            model: descriptor.model.clone(),
            credential_aliases: descriptor.credential_aliases.clone(),
            allowed_tools: descriptor.allowed_tools.clone(),
            status: status.to_string(),
            context_loaded_in_ram: is_loaded,
        }
    }

    /// Lists all registered custom agents with safe metadata only (`0` lazy loads triggered).
    pub fn list_island_summaries(&self) -> Vec<IslandCustomAgentSummary> {
        self.list_descriptors()
            .into_iter()
            .map(|d| self.build_island_summary(d))
            .collect()
    }

    /// Creates a new custom agent directory (`<agents_root>/<id>/agent.toml` + `SOUL.md`)
    /// after validating `CanonicalPathJail` confinement and `GatekeeperEngine` (`Capability::FsWrite`),
    /// then registers the descriptor lazily (`0` context bytes loaded in RAM).
    pub fn create_custom_agent_from_island(
        &mut self,
        gatekeeper: &mut GatekeeperEngine,
        agents_root: &Path,
        agents_rel_prefix: &str,
        req: &IslandCreateCustomAgentRequest,
        now_ms: u64,
    ) -> Result<IslandCustomAgentSummary, CustomAgentError> {
        validate_custom_agent_id(&req.id)?;
        validate_clean_text("display_name", &req.display_name, 120)?;
        validate_clean_text("description", &req.description, 500)?;
        validate_clean_text("model", &req.model, 120)?;
        if req.soul_markdown.trim().is_empty() {
            return Err(CustomAgentError::EmptySoulFile(DEFAULT_SOUL_FILENAME.to_string()));
        }

        // Confine `<id>` inside `agents_root` using CanonicalPathJail
        let jail_root = normalize_os_path_for_agent_jail(agents_root);
        let jail = CanonicalPathJail::new(&jail_root)
            .map_err(|e| CustomAgentError::PathJailViolation(e.to_string()))?;
        let _ = jail
            .resolve_and_confine(&req.id)
            .map_err(|e| CustomAgentError::PathJailViolation(e.to_string()))?;

        // Authorize creation via Gatekeeper (Capability::FsWrite)
        let clean_prefix = agents_rel_prefix
            .trim()
            .replace('\\', "/")
            .trim_matches('/')
            .to_string();
        let target_rel = if clean_prefix.is_empty() {
            format!("{}/{}", req.id.trim(), CUSTOM_AGENT_MANIFEST_FILENAME)
        } else {
            format!(
                "{}/{}/{}",
                clean_prefix,
                req.id.trim(),
                CUSTOM_AGENT_MANIFEST_FILENAME
            )
        };

        let verdict = gatekeeper.evaluate_request(
            "island-ui",
            Capability::FsWrite,
            &target_rel,
            "create_custom_agent",
            now_ms,
        );
        match verdict {
            GatekeeperVerdict::Allow { .. } => {}
            GatekeeperVerdict::AskPending {
                ticket, audit_seq, ..
            } => {
                return Err(CustomAgentError::GatekeeperAskRequired {
                    req_hash: ticket.req_hash,
                    audit_seq,
                });
            }
            GatekeeperVerdict::Deny {
                reason,
                detail,
                audit_seq,
                ..
            } => {
                return Err(CustomAgentError::GatekeeperDenied {
                    reason,
                    detail,
                    audit_seq,
                });
            }
        }

        let agent_dir = agents_root.join(req.id.trim());
        fs::create_dir_all(&agent_dir)
            .map_err(|e| CustomAgentError::IoError(format!("create_dir_all: {}", e)))?;

        let soul_path = agent_dir.join(DEFAULT_SOUL_FILENAME);
        fs::write(&soul_path, req.soul_markdown.as_bytes())
            .map_err(|e| CustomAgentError::IoError(format!("write SOUL.md: {}", e)))?;

        let manifest_toml = render_agent_toml(req);
        let manifest_path = agent_dir.join(CUSTOM_AGENT_MANIFEST_FILENAME);
        fs::write(&manifest_path, manifest_toml.as_bytes())
            .map_err(|e| CustomAgentError::IoError(format!("write agent.toml: {}", e)))?;

        let descriptor = match self.register_agent_from_dir(&agent_dir) {
            Ok(d) => d,
            Err(e) => {
                let _ = fs::remove_dir_all(&agent_dir);
                return Err(e);
            }
        };

        Ok(self.build_island_summary(&descriptor))
    }
}

/// Handles a raw JSON-RPC 2.0 request from the Island (`Create`, `List`, `Select`),
/// enforcing origin validation, frame size bounds, and zero secret/history leakage.
pub fn handle_island_custom_agent_ipc(
    orchestrator: &mut CustomAgentOrchestrator,
    gatekeeper: &mut GatekeeperEngine,
    agents_root: &Path,
    agents_rel_prefix: &str,
    raw_json: &str,
) -> Result<String, CustomAgentError> {
    if raw_json.is_empty() {
        return Err(CustomAgentError::InvalidIpcRequest(
            "IPC frame cannot be empty".to_string(),
        ));
    }
    if raw_json.len() > MAX_CUSTOM_AGENT_IPC_BYTES {
        return Err(CustomAgentError::InvalidIpcRequest(format!(
            "IPC frame size {} exceeds max {} bytes",
            raw_json.len(),
            MAX_CUSTOM_AGENT_IPC_BYTES
        )));
    }
    if raw_json.as_bytes().contains(&0) {
        return Err(CustomAgentError::InvalidIpcRequest(
            "IPC frame contains forbidden null byte".to_string(),
        ));
    }

    let env: IslandCustomAgentIpcEnvelope = serde_json::from_str(raw_json)
        .map_err(|e| CustomAgentError::InvalidIpcRequest(e.to_string()))?;

    if env.jsonrpc != "2.0" {
        return Err(CustomAgentError::InvalidIpcRequest(format!(
            "expected jsonrpc '2.0', got '{}'",
            env.jsonrpc
        )));
    }
    if env.v != MIKU_PROTOCOL_VERSION_V1 {
        return Err(CustomAgentError::InvalidIpcRequest(format!(
            "unsupported protocol version {}",
            env.v
        )));
    }
    if env.id.trim().is_empty() {
        return Err(CustomAgentError::InvalidIpcRequest(
            "envelope id cannot be empty".to_string(),
        ));
    }
    let _ = validate_custom_agent_ipc_origin(&env.origin)?;

    match &env.command {
        IslandCustomAgentIpcCommand::List => {}
        IslandCustomAgentIpcCommand::Select { agent_id } => {
            let _ = orchestrator.select_agent(agent_id)?;
        }
        IslandCustomAgentIpcCommand::Create(req) => {
            let _ = orchestrator.create_custom_agent_from_island(
                gatekeeper,
                agents_root,
                agents_rel_prefix,
                req,
                env.timestamp_ms,
            )?;
        }
    }

    let resp = IslandCustomAgentIpcResponse {
        jsonrpc: "2.0".to_string(),
        v: MIKU_PROTOCOL_VERSION_V1,
        id: env.id,
        selected_agent_id: orchestrator.selected_agent_id().map(ToOwned::to_owned),
        agents: orchestrator.list_island_summaries(),
    };

    serde_json::to_string(&resp).map_err(|e| CustomAgentError::InvalidIpcRequest(e.to_string()))
}

fn canonical_provider_toml_id(provider: LlmProvider) -> &'static str {
    match provider {
        LlmProvider::DeepSeek => "deep_seek",
        LlmProvider::Anthropic => "anthropic",
        LlmProvider::OpenAi => "open_ai",
        LlmProvider::Gemini => "gemini",
        LlmProvider::Ollama => "ollama",
    }
}

fn render_agent_toml(req: &IslandCreateCustomAgentRequest) -> String {
    let tools_list = req
        .allowed_tools
        .iter()
        .map(|t| format!("\"{}\"", t.replace('\\', "\\\\").replace('"', "\\\"")))
        .collect::<Vec<_>>()
        .join(", ");
    let aliases_list = req
        .credential_aliases
        .iter()
        .map(|a| format!("\"{}\"", a.replace('\\', "\\\\").replace('"', "\\\"")))
        .collect::<Vec<_>>()
        .join(", ");

    format!(
        r#"schema_version = {}

[agent]
id = "{}"
display_name = "{}"
description = "{}"
provider = "{}"
model = "{}"
soul_path = "{}"
allowed_tools = [{}]
credential_aliases = [{}]
max_steps = {}
max_context_tokens = {}
"#,
        CUSTOM_AGENT_SCHEMA_VERSION_V1,
        req.id.trim(),
        req.display_name.trim().replace('"', "\\\""),
        req.description.trim().replace('"', "\\\""),
        canonical_provider_toml_id(req.provider),
        req.model.trim().replace('"', "\\\""),
        DEFAULT_SOUL_FILENAME,
        tools_list,
        aliases_list,
        DEFAULT_CUSTOM_AGENT_MAX_STEPS,
        DEFAULT_CUSTOM_AGENT_MAX_CONTEXT_TOKENS
    )
}
