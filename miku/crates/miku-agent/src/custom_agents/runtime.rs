use std::collections::{BTreeMap, HashMap};
use std::path::Path;

use miku_gatekeeper::{GatekeeperEngine, GatekeeperVerdict};
use miku_memory::{
    estimate_tokens, AssembledPrompt, ContentChunk, StaticSystemPrefix, SystemPromptAssembler,
};
use miku_protocol::sha256_hex;
use miku_storage::{
    ConversationTurnRecord, Fts5SearchHit, SessionRecord, SingleThreadStoragePool,
};
use serde::{Deserialize, Serialize};

use crate::catalog::{AgentCatalog, AgentManifest, AgentTransportKind, CatalogError};
use crate::custom_agents::manifest::{
    read_and_validate_soul_file, CustomAgentDescriptor, CustomAgentError,
};
use crate::secrets::{SecretManager, Zeroizing};
use crate::token_saver::build_static_system_prefix_from_registry;
use crate::{InternalToolFamily, ToolRegistry};

/// Runtime context for a custom agent, allocated ONLY lazily on first invocation.
/// When an agent is merely registered, listed, or selected in the Island, this struct
/// is NOT instantiated (0 bytes of SOUL.md or prefix in RAM).
#[derive(Debug, Clone)]
pub struct LoadedAgentRuntimeContext {
    pub agent_id: String,
    pub soul_markdown: String,
    pub soul_sha256: String,
    pub soul_tokens: u32,
    pub scoped_tool_registry: ToolRegistry,
    pub static_prefix: StaticSystemPrefix,
    pub invocation_count: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ScopedToolExecutionOutcome {
    pub agent_id: String,
    pub session_id: String,
    pub tool_name: String,
    pub family: InternalToolFamily,
    pub canonical_target: String,
    pub audit_seq: u64,
    pub audit_hash: String,
}

/// Manages custom agents (`F-06`) with declarative registration (`agent.toml` + `SOUL.md`),
/// lazy loading into RAM only upon invocation, strict per-agent session history isolation,
/// strict per-agent credential alias isolation, and scoped tool allowlists + Gatekeeper.
pub struct CustomAgentOrchestrator {
    catalog: AgentCatalog,
    descriptors: BTreeMap<String, CustomAgentDescriptor>,
    loaded_contexts: BTreeMap<String, LoadedAgentRuntimeContext>,
    selected_agent_id: Option<String>,
    session_owners: HashMap<String, String>,
    exclusive_alias_owners: HashMap<String, String>,
    base_tool_registry: ToolRegistry,
    storage: SingleThreadStoragePool,
    secret_manager: SecretManager,
    tool_invocations_executed: u64,
}

impl CustomAgentOrchestrator {
    pub fn new(
        catalog: AgentCatalog,
        base_tool_registry: ToolRegistry,
        storage: SingleThreadStoragePool,
        secret_manager: SecretManager,
    ) -> Self {
        Self {
            catalog,
            descriptors: BTreeMap::new(),
            loaded_contexts: BTreeMap::new(),
            selected_agent_id: None,
            session_owners: HashMap::new(),
            exclusive_alias_owners: HashMap::new(),
            base_tool_registry,
            storage,
            secret_manager,
            tool_invocations_executed: 0,
        }
    }

    pub fn catalog(&self) -> &AgentCatalog {
        &self.catalog
    }

    pub fn base_tool_registry(&self) -> &ToolRegistry {
        &self.base_tool_registry
    }

    pub fn storage(&self) -> &SingleThreadStoragePool {
        &self.storage
    }

    pub fn secret_manager(&self) -> &SecretManager {
        &self.secret_manager
    }

    pub fn secret_manager_mut(&mut self) -> &mut SecretManager {
        &mut self.secret_manager
    }

    pub fn tool_invocations_executed(&self) -> u64 {
        self.tool_invocations_executed
    }

    pub fn selected_agent_id(&self) -> Option<&str> {
        self.selected_agent_id.as_deref()
    }

    pub fn is_agent_context_loaded(&self, agent_id: &str) -> bool {
        self.loaded_contexts.contains_key(agent_id)
    }

    pub fn loaded_agent_count(&self) -> usize {
        self.loaded_contexts.len()
    }

    pub fn loaded_soul_bytes(&self, agent_id: &str) -> usize {
        self.loaded_contexts
            .get(agent_id)
            .map(|ctx| ctx.soul_markdown.len())
            .unwrap_or(0)
    }

    pub fn get_loaded_context(&self, agent_id: &str) -> Option<&LoadedAgentRuntimeContext> {
        self.loaded_contexts.get(agent_id)
    }

    pub fn unload_agent_context(&mut self, agent_id: &str) -> bool {
        self.loaded_contexts.remove(agent_id).is_some()
    }

    pub fn get_descriptor(&self, agent_id: &str) -> Option<&CustomAgentDescriptor> {
        self.descriptors.get(agent_id)
    }

    pub fn list_descriptors(&self) -> Vec<&CustomAgentDescriptor> {
        self.descriptors.values().collect()
    }

    /// Assigns a credential alias exclusively to `owner_agent_id`.
    pub fn assign_exclusive_credential_alias(
        &mut self,
        owner_agent_id: &str,
        alias: &str,
    ) -> Result<(), CustomAgentError> {
        let trimmed_owner = owner_agent_id.trim();
        let trimmed_alias = alias.trim();
        if let Some(existing_owner) = self.exclusive_alias_owners.get(trimmed_alias) {
            if existing_owner != trimmed_owner {
                return Err(CustomAgentError::CredentialAliasOwnedByAnotherAgent {
                    alias: trimmed_alias.to_string(),
                    requesting_agent: trimmed_owner.to_string(),
                    owner_agent: existing_owner.clone(),
                });
            }
        }
        self.exclusive_alias_owners
            .insert(trimmed_alias.to_string(), trimmed_owner.to_string());
        Ok(())
    }

    /// Loads and registers a custom agent from `agent_dir` (`agent.toml` + `SOUL.md`)
    /// without loading `SOUL.md` into RAM and without executing any tools.
    pub fn register_agent_from_dir(
        &mut self,
        agent_dir: &Path,
    ) -> Result<CustomAgentDescriptor, CustomAgentError> {
        let descriptor = CustomAgentDescriptor::load_from_dir(agent_dir, &self.base_tool_registry)?;
        self.register_descriptor(descriptor.clone())?;
        Ok(descriptor)
    }

    pub fn register_descriptor(
        &mut self,
        descriptor: CustomAgentDescriptor,
    ) -> Result<(), CustomAgentError> {
        if self.descriptors.contains_key(&descriptor.id)
            || self.catalog.get(&descriptor.id).is_some()
        {
            return Err(CustomAgentError::DuplicateAgentId(descriptor.id));
        }

        for alias in &descriptor.credential_aliases {
            if let Some(existing_owner) = self.exclusive_alias_owners.get(alias) {
                if existing_owner != &descriptor.id {
                    return Err(CustomAgentError::CredentialAliasOwnedByAnotherAgent {
                        alias: alias.clone(),
                        requesting_agent: descriptor.id.clone(),
                        owner_agent: existing_owner.clone(),
                    });
                }
            }
        }

        let mut capabilities = Vec::new();
        for tool_name in &descriptor.allowed_tools {
            if let Ok(desc) = self.base_tool_registry.get(tool_name) {
                let cap_str = desc.required_capability.as_str().to_string();
                if !capabilities.contains(&cap_str) {
                    capabilities.push(cap_str);
                }
            }
        }

        self.catalog
            .register(AgentManifest {
                id: descriptor.id.clone(),
                display_name: descriptor.display_name.clone(),
                description: descriptor.description.clone(),
                vendor: format!("Custom ({})", descriptor.provider.as_str()),
                version: format!("{}.0.0", descriptor.schema_version),
                transport: AgentTransportKind::McpHost,
                capabilities,
                icon_slug: "custom-agent".to_string(),
                is_builtin: false,
            })
            .map_err(|e| match e {
                CatalogError::DuplicateAgentId(id) => CustomAgentError::DuplicateAgentId(id),
                other => CustomAgentError::ManifestParseError(other.to_string()),
            })?;

        for alias in &descriptor.credential_aliases {
            self.exclusive_alias_owners
                .insert(alias.clone(), descriptor.id.clone());
        }
        self.descriptors.insert(descriptor.id.clone(), descriptor);
        Ok(())
    }

    /// Selects an active custom agent in the Island without loading its `SOUL.md` or sessions into RAM.
    pub fn select_agent(&mut self, agent_id: &str) -> Result<&CustomAgentDescriptor, CustomAgentError> {
        if !self.descriptors.contains_key(agent_id) {
            return Err(CustomAgentError::AgentNotFound(agent_id.to_string()));
        }
        self.selected_agent_id = Some(agent_id.to_string());
        Ok(self.descriptors.get(agent_id).expect("checked above"))
    }

    /// Lazily loads `SOUL.md`, builds the scoped `ToolRegistry`, and pins `StaticSystemPrefix`
    /// on first invocation of `agent_id`.
    pub fn ensure_agent_context_loaded(
        &mut self,
        agent_id: &str,
    ) -> Result<&LoadedAgentRuntimeContext, CustomAgentError> {
        if !self.loaded_contexts.contains_key(agent_id) {
            let descriptor = self
                .descriptors
                .get(agent_id)
                .ok_or_else(|| CustomAgentError::AgentNotFound(agent_id.to_string()))?
                .clone();

            let soul_host_path = descriptor.agent_dir.join(&descriptor.soul_rel_path);
            let soul_markdown = read_and_validate_soul_file(&soul_host_path)?;
            let soul_sha256 = sha256_hex(soul_markdown.as_bytes());
            let soul_tokens = estimate_tokens(&soul_markdown);

            let mut scoped_tool_registry = ToolRegistry::new_empty();
            for tool_name in &descriptor.allowed_tools {
                let tool_desc = self
                    .base_tool_registry
                    .get(tool_name)
                    .map_err(|_| CustomAgentError::UnknownToolInManifest(tool_name.clone()))?
                    .clone();
                let _ = scoped_tool_registry.register(tool_desc);
            }

            let static_prefix = build_static_system_prefix_from_registry(
                &descriptor.id,
                &soul_markdown,
                &scoped_tool_registry,
            );

            self.loaded_contexts.insert(
                agent_id.to_string(),
                LoadedAgentRuntimeContext {
                    agent_id: descriptor.id,
                    soul_markdown,
                    soul_sha256,
                    soul_tokens,
                    scoped_tool_registry,
                    static_prefix,
                    invocation_count: 0,
                },
            );
        }
        Ok(self.loaded_contexts.get(agent_id).expect("context loaded"))
    }

    pub fn create_isolated_session(
        &mut self,
        agent_id: &str,
        session_id: &str,
        project_root: &str,
        created_at_ms: u64,
    ) -> Result<SessionRecord, CustomAgentError> {
        if !self.descriptors.contains_key(agent_id) {
            return Err(CustomAgentError::AgentNotFound(agent_id.to_string()));
        }
        let record = SessionRecord {
            session_id: session_id.trim().to_string(),
            agent_name: agent_id.to_string(),
            project_root: project_root.trim().to_string(),
            created_at_ms,
            state: "active".to_string(),
        };
        self.storage
            .insert_session(record.clone())
            .map_err(|e| CustomAgentError::StorageError(e.to_string()))?;
        self.session_owners
            .insert(record.session_id.clone(), agent_id.to_string());
        Ok(record)
    }

    fn verify_session_ownership(
        &self,
        requesting_agent_id: &str,
        session_id: &str,
    ) -> Result<SessionRecord, CustomAgentError> {
        if !self.descriptors.contains_key(requesting_agent_id) {
            return Err(CustomAgentError::AgentNotFound(requesting_agent_id.into()));
        }
        let session = self
            .storage
            .get_session(session_id)
            .map_err(|e| CustomAgentError::StorageError(e.to_string()))?
            .ok_or_else(|| CustomAgentError::SessionNotFound(session_id.to_string()))?;

        let recorded_owner = self
            .session_owners
            .get(session_id)
            .map(|s| s.as_str())
            .unwrap_or(session.agent_name.as_str());

        if session.agent_name != requesting_agent_id || recorded_owner != requesting_agent_id {
            return Err(CustomAgentError::CrossAgentSessionAccessDenied {
                requesting_agent: requesting_agent_id.to_string(),
                session_id: session_id.to_string(),
                owner_agent: session.agent_name,
            });
        }
        Ok(session)
    }

    pub fn append_isolated_turn(
        &mut self,
        requesting_agent_id: &str,
        session_id: &str,
        turn_id: &str,
        seq: u64,
        role: &str,
        content: &str,
        ts_ms: u64,
    ) -> Result<ConversationTurnRecord, CustomAgentError> {
        let _ = self.verify_session_ownership(requesting_agent_id, session_id)?;
        let turn = ConversationTurnRecord {
            turn_id: turn_id.trim().to_string(),
            session_id: session_id.trim().to_string(),
            seq,
            role: role.trim().to_string(),
            content: content.to_string(),
            token_estimate: estimate_tokens(content),
            ts_ms,
        };
        self.storage
            .insert_turn(turn.clone())
            .map_err(|e| CustomAgentError::StorageError(e.to_string()))?;
        Ok(turn)
    }

    pub fn read_isolated_session_history(
        &self,
        requesting_agent_id: &str,
        session_id: &str,
    ) -> Result<Vec<ConversationTurnRecord>, CustomAgentError> {
        let _ = self.verify_session_ownership(requesting_agent_id, session_id)?;
        self.storage
            .list_turns_for_session(session_id)
            .map_err(|e| CustomAgentError::StorageError(e.to_string()))
    }

    pub fn delete_isolated_session(
        &mut self,
        requesting_agent_id: &str,
        session_id: &str,
    ) -> Result<usize, CustomAgentError> {
        let _ = self.verify_session_ownership(requesting_agent_id, session_id)?;
        let cascaded = self
            .storage
            .delete_session(session_id)
            .map_err(|e| CustomAgentError::StorageError(e.to_string()))?;
        self.session_owners.remove(session_id);
        Ok(cascaded)
    }

    pub fn assemble_isolated_turn_prompt(
        &mut self,
        requesting_agent_id: &str,
        session_id: &str,
        active_user_query: &str,
        rag_candidates: &[Fts5SearchHit],
        chunk_lookup: &BTreeMap<String, ContentChunk>,
        max_budget_tokens_override: Option<u32>,
    ) -> Result<AssembledPrompt, CustomAgentError> {
        let _ = self.verify_session_ownership(requesting_agent_id, session_id)?;
        let default_budget = self
            .descriptors
            .get(requesting_agent_id)
            .map(|d| d.max_context_tokens)
            .unwrap_or(4_096);
        let budget = max_budget_tokens_override.unwrap_or(default_budget);

        let _ = self.ensure_agent_context_loaded(requesting_agent_id)?;
        let prefix = self
            .loaded_contexts
            .get_mut(requesting_agent_id)
            .map(|ctx| {
                ctx.invocation_count += 1;
                ctx.static_prefix.clone()
            })
            .expect("loaded above");

        let history_turns = self
            .storage
            .list_turns_for_session(session_id)
            .map_err(|e| CustomAgentError::StorageError(e.to_string()))?;

        SystemPromptAssembler::assemble_within_budget(
            &prefix,
            rag_candidates,
            chunk_lookup,
            &history_turns,
            active_user_query,
            budget,
        )
        .map_err(|e| CustomAgentError::MemoryError(e.to_string()))
    }

    pub fn resolve_agent_credential_authorized(
        &mut self,
        requesting_agent_id: &str,
        alias: &str,
        session_token: &str,
    ) -> Result<Zeroizing<Vec<u8>>, CustomAgentError> {
        let descriptor = self
            .descriptors
            .get(requesting_agent_id)
            .ok_or_else(|| CustomAgentError::AgentNotFound(requesting_agent_id.to_string()))?;

        let trimmed_alias = alias.trim();
        if let Some(owner) = self.exclusive_alias_owners.get(trimmed_alias) {
            if owner != requesting_agent_id {
                return Err(CustomAgentError::UnauthorizedCredentialAlias {
                    requesting_agent: requesting_agent_id.to_string(),
                    alias: trimmed_alias.to_string(),
                });
            }
        }
        if !descriptor.declares_credential_alias(trimmed_alias) {
            return Err(CustomAgentError::UnauthorizedCredentialAlias {
                requesting_agent: requesting_agent_id.to_string(),
                alias: trimmed_alias.to_string(),
            });
        }

        self.secret_manager
            .get_secret_authorized(trimmed_alias, session_token)
            .map_err(|e| CustomAgentError::SecretStoreError(e.to_string()))
    }

    pub fn invoke_agent_tool_authorized(
        &mut self,
        gatekeeper: &mut GatekeeperEngine,
        requesting_agent_id: &str,
        session_id: &str,
        tool_name: &str,
        target: &str,
        args: &str,
        now_ms: u64,
    ) -> Result<ScopedToolExecutionOutcome, CustomAgentError> {
        let _ = self.verify_session_ownership(requesting_agent_id, session_id)?;
        let _ = self.ensure_agent_context_loaded(requesting_agent_id)?;

        let tool_desc = {
            let ctx = self
                .loaded_contexts
                .get_mut(requesting_agent_id)
                .expect("loaded above");
            ctx.invocation_count += 1;
            ctx.scoped_tool_registry
                .validate_invocation(tool_name, target, args)
                .map_err(|_| CustomAgentError::UndeclaredToolForAgent {
                    agent_id: requesting_agent_id.to_string(),
                    tool_name: tool_name.trim().to_ascii_lowercase(),
                })?
                .clone()
        };

        match gatekeeper.evaluate_request(
            requesting_agent_id,
            tool_desc.required_capability,
            target,
            args,
            now_ms,
        ) {
            GatekeeperVerdict::Allow {
                canonical_target,
                audit_seq,
                audit_hash,
            } => {
                self.tool_invocations_executed += 1;
                Ok(ScopedToolExecutionOutcome {
                    agent_id: requesting_agent_id.to_string(),
                    session_id: session_id.to_string(),
                    tool_name: tool_desc.name,
                    family: tool_desc.family,
                    canonical_target,
                    audit_seq,
                    audit_hash,
                })
            }
            GatekeeperVerdict::AskPending {
                ticket, audit_seq, ..
            } => Err(CustomAgentError::GatekeeperAskRequired {
                req_hash: ticket.req_hash,
                audit_seq,
            }),
            GatekeeperVerdict::Deny {
                reason,
                detail,
                audit_seq,
                ..
            } => Err(CustomAgentError::GatekeeperDenied {
                reason,
                detail,
                audit_seq,
            }),
        }
    }
}
