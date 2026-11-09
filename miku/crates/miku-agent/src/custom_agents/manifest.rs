use std::collections::BTreeSet;
use std::fs;
use std::path::{Path, PathBuf};

use miku_gatekeeper::{CanonicalPathJail, DenyReason};
use miku_memory::estimate_tokens;
use miku_protocol::sha256_hex;
use serde::{Deserialize, Serialize};

use crate::harness::LlmProvider;
use crate::secrets::validate_secret_alias;
use crate::ToolRegistry;

pub const CUSTOM_AGENT_MANIFEST_FILENAME: &str = "agent.toml";
pub const DEFAULT_SOUL_FILENAME: &str = "SOUL.md";
pub const CUSTOM_AGENT_SCHEMA_VERSION_V1: u16 = 1;

/// Recommended upper threshold of tokens for `SOUL.md` (`1,024` tokens, i.e. 25% of the default
/// 4,096-token working context window) so the pinned `StaticSystemPrefix` does not starve
/// FTS5 RAG chunks and conversation history turns during `SystemPromptAssembler::assemble_within_budget`.
pub const RECOMMENDED_MAX_SOUL_TOKENS: u32 = 1_024;

pub const DEFAULT_CUSTOM_AGENT_MAX_STEPS: usize = 8;
pub const MAX_CUSTOM_AGENT_STEPS_CEILING: usize = 32;
pub const DEFAULT_CUSTOM_AGENT_MAX_CONTEXT_TOKENS: u32 = 4_096;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum CustomAgentError {
    ManifestParseError(String),
    UnsupportedSchemaVersion(u16),
    InvalidAgentId(String),
    DuplicateAgentId(String),
    InvalidField { field: String, reason: String },
    PathJailViolation(String),
    SymlinkForbidden(String),
    InvalidSoulPath(String),
    MissingSoulFile(String),
    EmptySoulFile(String),
    UnknownToolInManifest(String),
    DuplicateToolInManifest(String),
    UndeclaredToolForAgent { agent_id: String, tool_name: String },
    InvalidCredentialAlias(String),
    CredentialAliasOwnedByAnotherAgent {
        alias: String,
        requesting_agent: String,
        owner_agent: String,
    },
    UnauthorizedCredentialAlias {
        requesting_agent: String,
        alias: String,
    },
    CrossAgentSessionAccessDenied {
        requesting_agent: String,
        session_id: String,
        owner_agent: String,
    },
    AgentNotFound(String),
    SessionNotFound(String),
    GatekeeperDenied {
        reason: DenyReason,
        detail: String,
        audit_seq: u64,
    },
    GatekeeperAskRequired {
        req_hash: String,
        audit_seq: u64,
    },
    SecretStoreError(String),
    StorageError(String),
    MemoryError(String),
    IoError(String),
    InvalidIpcRequest(String),
}

impl std::fmt::Display for CustomAgentError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::ManifestParseError(m) => write!(f, "agent.toml parse error: {}", m),
            Self::UnsupportedSchemaVersion(v) => write!(
                f,
                "unsupported agent.toml schema_version {}; expected {}",
                v, CUSTOM_AGENT_SCHEMA_VERSION_V1
            ),
            Self::InvalidAgentId(id) => write!(f, "invalid custom agent id '{}'", id),
            Self::DuplicateAgentId(id) => write!(f, "duplicate agent id '{}'", id),
            Self::InvalidField { field, reason } => {
                write!(f, "invalid field '{}' in agent.toml: {}", field, reason)
            }
            Self::PathJailViolation(m) => write!(f, "agent path jail violation: {}", m),
            Self::SymlinkForbidden(p) => write!(f, "symbolic link or reparse point forbidden: {}", p),
            Self::InvalidSoulPath(m) => write!(f, "invalid soul_path in agent.toml: {}", m),
            Self::MissingSoulFile(p) => write!(f, "missing SOUL.md file at '{}'", p),
            Self::EmptySoulFile(p) => write!(f, "SOUL.md file at '{}' cannot be empty", p),
            Self::UnknownToolInManifest(t) => write!(f, "unknown tool '{}' in agent.toml", t),
            Self::DuplicateToolInManifest(t) => write!(f, "duplicate tool '{}' in agent.toml", t),
            Self::UndeclaredToolForAgent { agent_id, tool_name } => write!(
                f,
                "agent '{}' is not allowed to invoke undeclared tool '{}'",
                agent_id, tool_name
            ),
            Self::InvalidCredentialAlias(m) => write!(f, "invalid credential alias: {}", m),
            Self::CredentialAliasOwnedByAnotherAgent {
                alias,
                requesting_agent,
                owner_agent,
            } => write!(
                f,
                "credential alias '{}' requested by '{}' is owned by '{}'",
                alias, requesting_agent, owner_agent
            ),
            Self::UnauthorizedCredentialAlias {
                requesting_agent,
                alias,
            } => write!(
                f,
                "agent '{}' is forbidden from reading or using credential alias '{}'",
                requesting_agent, alias
            ),
            Self::CrossAgentSessionAccessDenied {
                requesting_agent,
                session_id,
                owner_agent,
            } => write!(
                f,
                "cross-agent session access denied: '{}' cannot access '{}' owned by '{}'",
                requesting_agent, session_id, owner_agent
            ),
            Self::AgentNotFound(id) => write!(f, "custom agent '{}' not found", id),
            Self::SessionNotFound(sid) => write!(f, "session '{}' not found", sid),
            Self::GatekeeperDenied {
                reason,
                detail,
                audit_seq,
            } => write!(
                f,
                "blocked by Gatekeeper ({}, seq={}): {}",
                reason.as_code(),
                audit_seq,
                detail
            ),
            Self::GatekeeperAskRequired { req_hash, audit_seq } => write!(
                f,
                "Gatekeeper confirmation required (req_hash={}, seq={})",
                req_hash, audit_seq
            ),
            Self::SecretStoreError(m) => write!(f, "secret store error: {}", m),
            Self::StorageError(m) => write!(f, "storage error: {}", m),
            Self::MemoryError(m) => write!(f, "memory error: {}", m),
            Self::IoError(m) => write!(f, "I/O error: {}", m),
            Self::InvalidIpcRequest(m) => write!(f, "invalid custom agent IPC request: {}", m),
        }
    }
}

impl std::error::Error for CustomAgentError {}

fn default_soul_path() -> String {
    DEFAULT_SOUL_FILENAME.to_string()
}

fn default_max_steps() -> usize {
    DEFAULT_CUSTOM_AGENT_MAX_STEPS
}

fn default_max_context_tokens() -> u32 {
    DEFAULT_CUSTOM_AGENT_MAX_CONTEXT_TOKENS
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CustomAgentMetadataRaw {
    pub id: String,
    pub display_name: String,
    pub description: String,
    pub provider: LlmProvider,
    pub model: String,
    #[serde(default = "default_soul_path")]
    pub soul_path: String,
    pub allowed_tools: Vec<String>,
    #[serde(default)]
    pub credential_aliases: Vec<String>,
    #[serde(default = "default_max_steps")]
    pub max_steps: usize,
    #[serde(default = "default_max_context_tokens")]
    pub max_context_tokens: u32,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CustomAgentManifestRaw {
    pub schema_version: u16,
    pub agent: CustomAgentMetadataRaw,
}

/// Lightweight declarative descriptor for a custom agent (`agent.toml`).
/// INVARIANT (Lazy Loading): Does NOT hold `SOUL.md` content, `StaticSystemPrefix`,
/// or session history in RAM. Those are loaded on demand only when the agent is invoked.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CustomAgentDescriptor {
    pub schema_version: u16,
    pub id: String,
    pub display_name: String,
    pub description: String,
    pub provider: LlmProvider,
    pub model: String,
    pub agent_dir: PathBuf,
    pub soul_rel_path: String,
    pub canonical_soul_path: String,
    pub allowed_tools: Vec<String>,
    pub credential_aliases: Vec<String>,
    pub max_steps: usize,
    pub max_context_tokens: u32,
    pub manifest_sha256: String,
}

impl CustomAgentDescriptor {
    /// Parses and validates an `agent.toml` string confined inside `agent_dir`.
    /// Never executes any tool or external command.
    pub fn parse_and_validate(
        raw_toml: &str,
        agent_dir: &Path,
        base_registry: &ToolRegistry,
    ) -> Result<Self, CustomAgentError> {
        let parsed: CustomAgentManifestRaw = toml::from_str(raw_toml)
            .map_err(|e| CustomAgentError::ManifestParseError(e.to_string()))?;

        if parsed.schema_version != CUSTOM_AGENT_SCHEMA_VERSION_V1 {
            return Err(CustomAgentError::UnsupportedSchemaVersion(parsed.schema_version));
        }

        validate_custom_agent_id(&parsed.agent.id)?;
        validate_clean_text("display_name", &parsed.agent.display_name, 120)?;
        validate_clean_text("description", &parsed.agent.description, 500)?;
        validate_clean_text("model", &parsed.agent.model, 120)?;

        if parsed.agent.max_steps == 0 || parsed.agent.max_steps > MAX_CUSTOM_AGENT_STEPS_CEILING {
            return Err(CustomAgentError::InvalidField {
                field: "max_steps".to_string(),
                reason: format!("must be in 1..={}", MAX_CUSTOM_AGENT_STEPS_CEILING),
            });
        }
        if parsed.agent.max_context_tokens < 64 || parsed.agent.max_context_tokens > 128_000 {
            return Err(CustomAgentError::InvalidField {
                field: "max_context_tokens".to_string(),
                reason: "must be in 64..=128000".to_string(),
            });
        }

        let mut seen_tools = BTreeSet::new();
        let mut allowed_tools = Vec::with_capacity(parsed.agent.allowed_tools.len());
        for raw_tool in &parsed.agent.allowed_tools {
            let norm = raw_tool.trim().to_ascii_lowercase();
            if norm.is_empty() {
                return Err(CustomAgentError::InvalidField {
                    field: "allowed_tools".to_string(),
                    reason: "tool name cannot be empty".to_string(),
                });
            }
            let _ = base_registry
                .get(&norm)
                .map_err(|_| CustomAgentError::UnknownToolInManifest(raw_tool.clone()))?;
            if !seen_tools.insert(norm.clone()) {
                return Err(CustomAgentError::DuplicateToolInManifest(norm));
            }
            allowed_tools.push(norm);
        }

        let mut seen_aliases = BTreeSet::new();
        let mut credential_aliases = Vec::with_capacity(parsed.agent.credential_aliases.len());
        for raw_alias in &parsed.agent.credential_aliases {
            let trimmed = raw_alias.trim();
            if looks_like_raw_secret_key(trimmed) {
                return Err(CustomAgentError::InvalidCredentialAlias(
                    "credential_aliases must reference an alias name, never a raw secret key".into(),
                ));
            }
            validate_secret_alias(trimmed).map_err(|_| {
                CustomAgentError::InvalidCredentialAlias("alias failed syntax validation".into())
            })?;
            if seen_aliases.insert(trimmed.to_string()) {
                credential_aliases.push(trimmed.to_string());
            }
        }

        let soul_trimmed = parsed.agent.soul_path.trim();
        if soul_trimmed.is_empty() {
            return Err(CustomAgentError::InvalidSoulPath("soul_path cannot be empty".into()));
        }
        let normalized_soul = soul_trimmed.replace('\\', "/");
        if normalized_soul.starts_with('/')
            || (normalized_soul.len() >= 2 && normalized_soul.as_bytes()[1] == b':')
        {
            return Err(CustomAgentError::PathJailViolation(format!(
                "soul_path '{}' must be a relative path inside the agent directory",
                soul_trimmed
            )));
        }
        if !normalized_soul.to_ascii_lowercase().ends_with(".md") {
            return Err(CustomAgentError::InvalidSoulPath(format!(
                "soul_path '{}' must reference a Markdown (.md) file",
                soul_trimmed
            )));
        }

        let jail_root = normalize_os_path_for_agent_jail(agent_dir);
        let jail = CanonicalPathJail::new(&jail_root)
            .map_err(|e| CustomAgentError::PathJailViolation(e.to_string()))?;
        let canonical_soul_path = jail
            .resolve_and_confine(soul_trimmed)
            .map_err(|e| CustomAgentError::PathJailViolation(e.to_string()))?;
        let soul_rel_path = jail.relative_to_root(&canonical_soul_path);
        if soul_rel_path.is_empty() || soul_rel_path == "." {
            return Err(CustomAgentError::InvalidSoulPath(
                "soul_path must point to a file inside the agent directory".into(),
            ));
        }

        let normalized_toml = raw_toml.replace("\r\n", "\n");
        let manifest_sha256 = sha256_hex(normalized_toml.as_bytes());

        Ok(Self {
            schema_version: parsed.schema_version,
            id: parsed.agent.id.trim().to_string(),
            display_name: parsed.agent.display_name.trim().to_string(),
            description: parsed.agent.description.trim().to_string(),
            provider: parsed.agent.provider,
            model: parsed.agent.model.trim().to_string(),
            agent_dir: agent_dir.to_path_buf(),
            soul_rel_path,
            canonical_soul_path,
            allowed_tools,
            credential_aliases,
            max_steps: parsed.agent.max_steps,
            max_context_tokens: parsed.agent.max_context_tokens,
            manifest_sha256,
        })
    }

    /// Loads `agent.toml` from `agent_dir`, verifies no symlinks are used, and confirms
    /// `SOUL.md` exists and is non-empty without retaining `SOUL.md` content in RAM.
    pub fn load_from_dir(
        agent_dir: &Path,
        base_registry: &ToolRegistry,
    ) -> Result<Self, CustomAgentError> {
        ensure_not_symlink(agent_dir)?;
        let manifest_path = agent_dir.join(CUSTOM_AGENT_MANIFEST_FILENAME);
        ensure_not_symlink(&manifest_path)?;

        let raw_toml = fs::read_to_string(&manifest_path).map_err(|e| {
            CustomAgentError::IoError(format!("read {}: {}", manifest_path.display(), e))
        })?;

        let descriptor = Self::parse_and_validate(&raw_toml, agent_dir, base_registry)?;
        let soul_host_path = agent_dir.join(&descriptor.soul_rel_path);
        let _ = read_and_validate_soul_file(&soul_host_path)?;
        Ok(descriptor)
    }

    pub fn declares_tool(&self, tool_name: &str) -> bool {
        let key = tool_name.trim().to_ascii_lowercase();
        self.allowed_tools.iter().any(|t| t == &key)
    }

    pub fn declares_credential_alias(&self, alias: &str) -> bool {
        let key = alias.trim();
        self.credential_aliases.iter().any(|a| a == key)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SoulTokenAuditReport {
    pub agent_id: String,
    pub display_name: String,
    pub soul_rel_path: String,
    pub soul_tokens: u32,
    pub recommended_max_tokens: u32,
    pub exceeds_recommended: bool,
    pub warning_message: Option<String>,
}

/// Validates an agent directory (`agent.toml` + `SOUL.md`) and computes its `SOUL.md`
/// token count using `miku_memory::estimate_tokens` for `miku doctor` diagnostics.
pub fn audit_custom_agent_soul(
    agent_dir: &Path,
    base_registry: &ToolRegistry,
    recommended_max_tokens: u32,
) -> Result<SoulTokenAuditReport, CustomAgentError> {
    let descriptor = CustomAgentDescriptor::load_from_dir(agent_dir, base_registry)?;
    let soul_host_path = agent_dir.join(&descriptor.soul_rel_path);
    let soul_markdown = read_and_validate_soul_file(&soul_host_path)?;
    let soul_tokens = estimate_tokens(&soul_markdown);
    let threshold = if recommended_max_tokens == 0 {
        RECOMMENDED_MAX_SOUL_TOKENS
    } else {
        recommended_max_tokens
    };
    let exceeds_recommended = soul_tokens > threshold;
    let warning_message = if exceeds_recommended {
        Some(format!(
            "Agent '{}' ({}) SOUL.md uses {} tokens (> {} recommended threshold); move reference material to on-demand miku-memory Markdown notes",
            descriptor.id, descriptor.soul_rel_path, soul_tokens, threshold
        ))
    } else {
        None
    };

    Ok(SoulTokenAuditReport {
        agent_id: descriptor.id,
        display_name: descriptor.display_name,
        soul_rel_path: descriptor.soul_rel_path,
        soul_tokens,
        recommended_max_tokens: threshold,
        exceeds_recommended,
        warning_message,
    })
}

pub(crate) fn ensure_not_symlink(path: &Path) -> Result<(), CustomAgentError> {
    if let Ok(meta) = fs::symlink_metadata(path) {
        if meta.file_type().is_symlink() {
            return Err(CustomAgentError::SymlinkForbidden(
                path.to_string_lossy().to_string(),
            ));
        }
    }
    Ok(())
}

pub(crate) fn read_and_validate_soul_file(soul_path: &Path) -> Result<String, CustomAgentError> {
    let meta = fs::symlink_metadata(soul_path)
        .map_err(|_| CustomAgentError::MissingSoulFile(soul_path.to_string_lossy().to_string()))?;
    if meta.file_type().is_symlink() {
        return Err(CustomAgentError::SymlinkForbidden(
            soul_path.to_string_lossy().to_string(),
        ));
    }
    if !meta.is_file() {
        return Err(CustomAgentError::MissingSoulFile(
            soul_path.to_string_lossy().to_string(),
        ));
    }
    let content = fs::read_to_string(soul_path)
        .map_err(|e| CustomAgentError::IoError(format!("read {}: {}", soul_path.display(), e)))?;
    if content.bytes().any(|b| b == 0) {
        return Err(CustomAgentError::InvalidField {
            field: "SOUL.md".to_string(),
            reason: "contains forbidden null byte".to_string(),
        });
    }
    if content.trim().is_empty() {
        return Err(CustomAgentError::EmptySoulFile(
            soul_path.to_string_lossy().to_string(),
        ));
    }
    Ok(content)
}

pub fn normalize_os_path_for_agent_jail(path: &Path) -> String {
    let abs = if path.is_absolute() {
        path.to_path_buf()
    } else {
        std::env::current_dir()
            .unwrap_or_else(|_| Path::new("/workspace").to_path_buf())
            .join(path)
    };
    let s = abs.to_string_lossy().replace('\\', "/");
    let stripped = s.strip_prefix("//?/").unwrap_or(&s);
    if stripped.starts_with('/')
        || (stripped.len() >= 2
            && stripped.as_bytes()[0].is_ascii_alphabetic()
            && stripped.as_bytes()[1] == b':')
    {
        stripped.to_string()
    } else {
        format!("/{}", stripped)
    }
}

pub(crate) fn validate_custom_agent_id(raw_id: &str) -> Result<(), CustomAgentError> {
    let trimmed = raw_id.trim();
    if trimmed.len() < 3
        || trimmed.len() > 64
        || trimmed.starts_with('-')
        || trimmed.ends_with('-')
        || trimmed.contains("--")
        || !trimmed
            .bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-')
    {
        return Err(CustomAgentError::InvalidAgentId(raw_id.to_string()));
    }
    Ok(())
}

pub(crate) fn validate_clean_text(
    field: &str,
    value: &str,
    max_len: usize,
) -> Result<(), CustomAgentError> {
    let trimmed = value.trim();
    if trimmed.is_empty() {
        return Err(CustomAgentError::InvalidField {
            field: field.to_string(),
            reason: "must not be empty".to_string(),
        });
    }
    if trimmed.len() > max_len {
        return Err(CustomAgentError::InvalidField {
            field: field.to_string(),
            reason: format!("exceeds maximum length of {} chars", max_len),
        });
    }
    if value
        .bytes()
        .any(|b| b == 0 || (b < 0x20 && b != b'\n' && b != b'\r' && b != b'\t'))
    {
        return Err(CustomAgentError::InvalidField {
            field: field.to_string(),
            reason: "contains null or control characters".to_string(),
        });
    }
    Ok(())
}

fn looks_like_raw_secret_key(candidate: &str) -> bool {
    let lower = candidate.to_ascii_lowercase();
    lower.starts_with("sk-")
        || lower.starts_with("ghp_")
        || lower.starts_with("gho_")
        || lower.starts_with("aiza")
        || lower.starts_with("xoxb-")
        || lower.starts_with("bearer ")
        || candidate.len() > 64
}
