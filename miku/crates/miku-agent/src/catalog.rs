use std::collections::{BTreeMap, HashMap};

use serde::{Deserialize, Serialize};

use crate::bridge::AgentBridgeStatus;

/// Supported transport mechanism for an integrated agent or tool orchestrator.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AgentTransportKind {
    /// Bidirectional stdio hook with sub-15ms fail-open framing (e.g. Claude Code).
    StdioHook,
    /// Model Context Protocol host/client over loopback TCP or stdio (e.g. Cursor).
    McpHost,
    /// Passive log / file-system observation without active IPC hooks (e.g. Antigravity).
    PassiveLog,
    /// REST / Webhook events with HTTP conditional caching (e.g. GitHub).
    WebhookRest,
}

impl AgentTransportKind {
    pub fn as_str(&self) -> &'static str {
        match self {
            AgentTransportKind::StdioHook => "stdio_hook",
            AgentTransportKind::McpHost => "mcp_host",
            AgentTransportKind::PassiveLog => "passive_log",
            AgentTransportKind::WebhookRest => "webhook_rest",
        }
    }
}

/// Declarative metadata defining an agent or integration.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AgentManifest {
    pub id: String,
    pub display_name: String,
    pub description: String,
    pub vendor: String,
    pub version: String,
    pub transport: AgentTransportKind,
    pub capabilities: Vec<String>,
    pub icon_slug: String,
    pub is_builtin: bool,
}

impl AgentManifest {
    pub fn validate(&self) -> Result<(), CatalogError> {
        let id_trimmed = self.id.trim();
        if id_trimmed.is_empty() {
            return Err(CatalogError::InvalidManifest(
                "agent id cannot be empty".to_string(),
            ));
        }
        if !id_trimmed
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
        {
            return Err(CatalogError::InvalidManifest(format!(
                "agent id '{}' contains invalid characters; must be alphanumeric, '-' or '_'",
                self.id
            )));
        }
        if self.display_name.trim().is_empty() {
            return Err(CatalogError::InvalidManifest(
                "agent display_name cannot be empty".to_string(),
            ));
        }
        Ok(())
    }
}

/// Unified UI card representation consumed by Windows Island and mobile Miku-Link.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AgentCard {
    pub agent_id: String,
    pub display_name: String,
    pub status: AgentBridgeStatus,
    pub mood_indicator: String,
    pub capabilities: Vec<String>,
    pub icon_slug: String,
    pub summary: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CatalogError {
    DuplicateAgentId(String),
    InvalidManifest(String),
    ParseError(String),
}

impl std::fmt::Display for CatalogError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            CatalogError::DuplicateAgentId(id) => write!(f, "duplicate agent id '{}'", id),
            CatalogError::InvalidManifest(msg) => write!(f, "invalid agent manifest: {}", msg),
            CatalogError::ParseError(msg) => write!(f, "failed to parse catalog: {}", msg),
        }
    }
}

impl std::error::Error for CatalogError {}

#[derive(Debug, Deserialize)]
struct CatalogTomlWrapper {
    agents: Vec<AgentManifest>,
}

/// Declarative registry of agents and tool integrations (F-04, F-29).
/// Serves as the single source of truth across desktop Island, CLI, and mobile Miku-Link.
#[derive(Debug, Clone, Default)]
pub struct AgentCatalog {
    agents: BTreeMap<String, AgentManifest>,
}

impl AgentCatalog {
    pub fn new() -> Self {
        Self {
            agents: BTreeMap::new(),
        }
    }

    /// Pre-populates the catalog with the 4 standard built-in integrations.
    pub fn with_builtin_agents() -> Self {
        let mut catalog = Self::new();
        catalog
            .register(AgentManifest {
                id: "claude-code".to_string(),
                display_name: "Claude Code".to_string(),
                description: "Anthropic Claude Code CLI interactive programming assistant"
                    .to_string(),
                vendor: "Anthropic".to_string(),
                version: "1.0.0".to_string(),
                transport: AgentTransportKind::StdioHook,
                capabilities: vec![
                    "shell".to_string(),
                    "fs_read".to_string(),
                    "fs_write".to_string(),
                ],
                icon_slug: "claude".to_string(),
                is_builtin: true,
            })
            .expect("built-in claude-code must register");

        catalog
            .register(AgentManifest {
                id: "cursor".to_string(),
                display_name: "Cursor IDE".to_string(),
                description: "Cursor AI editor connected through Model Context Protocol (MCP)"
                    .to_string(),
                vendor: "Anysphere".to_string(),
                version: "0.4.0".to_string(),
                transport: AgentTransportKind::McpHost,
                capabilities: vec![
                    "fs_read".to_string(),
                    "fs_write".to_string(),
                    "shell".to_string(),
                ],
                icon_slug: "cursor".to_string(),
                is_builtin: true,
            })
            .expect("built-in cursor must register");

        catalog
            .register(AgentManifest {
                id: "antigravity".to_string(),
                display_name: "Google Antigravity".to_string(),
                description: "Google Antigravity autonomous workspace agent (passive observation)"
                    .to_string(),
                vendor: "Google".to_string(),
                version: "2.0.0".to_string(),
                transport: AgentTransportKind::PassiveLog,
                capabilities: vec!["fs_read".to_string()],
                icon_slug: "antigravity".to_string(),
                is_builtin: true,
            })
            .expect("built-in antigravity must register");

        catalog
            .register(AgentManifest {
                id: "github".to_string(),
                display_name: "GitHub Integration".to_string(),
                description: "GitHub API and webhook notifications with ETag caching".to_string(),
                vendor: "GitHub".to_string(),
                version: "1.0.0".to_string(),
                transport: AgentTransportKind::WebhookRest,
                capabilities: vec!["network".to_string()],
                icon_slug: "github".to_string(),
                is_builtin: true,
            })
            .expect("built-in github must register");

        catalog
    }

    /// Registers a new manifest in the catalog, rejecting invalid or duplicate IDs.
    pub fn register(&mut self, manifest: AgentManifest) -> Result<(), CatalogError> {
        manifest.validate()?;
        if self.agents.contains_key(&manifest.id) {
            return Err(CatalogError::DuplicateAgentId(manifest.id));
        }
        self.agents.insert(manifest.id.clone(), manifest);
        Ok(())
    }

    /// Retrieves an agent manifest by ID.
    pub fn get(&self, id: &str) -> Option<&AgentManifest> {
        self.agents.get(id)
    }

    /// Lists all registered manifests in deterministic alphabetical order.
    pub fn list_manifests(&self) -> Vec<&AgentManifest> {
        self.agents.values().collect()
    }

    /// Loads agent declarations from a TOML string (`[[agents]]`).
    pub fn load_from_toml(&mut self, toml_str: &str) -> Result<usize, CatalogError> {
        let wrapper: CatalogTomlWrapper =
            toml::from_str(toml_str).map_err(|e| CatalogError::ParseError(e.to_string()))?;
        let count = wrapper.agents.len();
        for manifest in wrapper.agents {
            self.register(manifest)?;
        }
        Ok(count)
    }

    /// Loads agent declarations from a JSON string array (`[...]`).
    pub fn load_from_json(&mut self, json_str: &str) -> Result<usize, CatalogError> {
        let list: Vec<AgentManifest> =
            serde_json::from_str(json_str).map_err(|e| CatalogError::ParseError(e.to_string()))?;
        let count = list.len();
        for manifest in list {
            self.register(manifest)?;
        }
        Ok(count)
    }

    /// Produces a standardized list of `AgentCard` objects for UI rendering,
    /// mapping each registered agent with its active runtime status.
    pub fn render_cards(
        &self,
        statuses: &HashMap<String, AgentBridgeStatus>,
    ) -> Vec<AgentCard> {
        self.agents
            .values()
            .map(|manifest| {
                let status = statuses
                    .get(&manifest.id)
                    .cloned()
                    .unwrap_or(AgentBridgeStatus::Offline);
                let mood_indicator = status.mood_indicator().to_string();
                let summary = match &status {
                    AgentBridgeStatus::Active => {
                        format!("{} is active and connected", manifest.display_name)
                    }
                    AgentBridgeStatus::Idle => {
                        format!("{} is idle and ready", manifest.display_name)
                    }
                    AgentBridgeStatus::Thinking => {
                        format!("{} is processing a request", manifest.display_name)
                    }
                    AgentBridgeStatus::Offline => {
                        format!("{} is currently disconnected", manifest.display_name)
                    }
                    AgentBridgeStatus::Unsupported { reason } => {
                        format!("{}: unsupported ({})", manifest.display_name, reason)
                    }
                    AgentBridgeStatus::Unavailable { reason } => {
                        format!("{}: unavailable ({})", manifest.display_name, reason)
                    }
                    AgentBridgeStatus::Error { message, .. } => {
                        format!("{}: error ({})", manifest.display_name, message)
                    }
                };

                AgentCard {
                    agent_id: manifest.id.clone(),
                    display_name: manifest.display_name.clone(),
                    status,
                    mood_indicator,
                    capabilities: manifest.capabilities.clone(),
                    icon_slug: manifest.icon_slug.clone(),
                    summary,
                }
            })
            .collect()
    }
}
