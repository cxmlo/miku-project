pub mod island_ipc;
pub mod manifest;
pub mod runtime;

pub use island_ipc::{
    handle_island_custom_agent_ipc, validate_custom_agent_ipc_origin,
    IslandCreateCustomAgentRequest, IslandCustomAgentIpcCommand, IslandCustomAgentIpcEnvelope,
    IslandCustomAgentIpcResponse, IslandCustomAgentSummary, MAX_CUSTOM_AGENT_IPC_BYTES,
};
pub use manifest::{
    audit_custom_agent_soul, normalize_os_path_for_agent_jail, CustomAgentDescriptor,
    CustomAgentError, CustomAgentManifestRaw, CustomAgentMetadataRaw, SoulTokenAuditReport,
    CUSTOM_AGENT_MANIFEST_FILENAME, CUSTOM_AGENT_SCHEMA_VERSION_V1,
    DEFAULT_CUSTOM_AGENT_MAX_CONTEXT_TOKENS, DEFAULT_CUSTOM_AGENT_MAX_STEPS,
    DEFAULT_SOUL_FILENAME, MAX_CUSTOM_AGENT_STEPS_CEILING, RECOMMENDED_MAX_SOUL_TOKENS,
};
pub use runtime::{
    CustomAgentOrchestrator, LoadedAgentRuntimeContext, ScopedToolExecutionOutcome,
};

#[cfg(test)]
mod tests;
