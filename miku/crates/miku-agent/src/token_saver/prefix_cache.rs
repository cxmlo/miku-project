use miku_memory::{
    estimate_tokens, AssembledPrompt, ContentChunk, StaticSystemPrefix, SystemPromptAssembler,
};
use miku_storage::{ConversationTurnRecord, Fts5SearchHit};
use serde_json::Value;

use crate::harness::types::LlmProvider;
use crate::token_saver::budget::TokenUsageRecord;
use crate::ToolRegistry;

/// Builds a deterministic `StaticSystemPrefix` directly from `miku-memory` (Step 1.6)
/// and the canonical `ToolRegistry` (sorted alphabetically by tool name via `BTreeMap`).
/// Because both soul directives and the sorted tool descriptors are static across turns,
/// the resulting prefix block and its FNV-1a 64-bit hash are guaranteed byte-for-byte identical.
pub fn build_static_system_prefix_from_registry(
    agent_id: &str,
    soul_directives: &str,
    registry: &ToolRegistry,
) -> StaticSystemPrefix {
    let mut tool_entries = Vec::new();
    for desc in registry.list_deterministic() {
        let entry = serde_json::json!({
            "name": desc.name,
            "family": desc.family.as_str(),
            "required_capability": desc.required_capability.as_str(),
            "requires_non_empty_target": desc.requires_non_empty_target,
            "description": desc.description,
        });
        tool_entries.push(entry);
    }
    let canonical_tools_schema = serde_json::to_string(&tool_entries).unwrap_or_else(|_| "[]".to_string());
    StaticSystemPrefix::new(agent_id, soul_directives, &canonical_tools_schema)
}

/// Reuses `SystemPromptAssembler::assemble_within_budget` from `miku-memory` (Step 1.6)
/// without duplicating prompt assembly or token estimation logic.
pub fn assemble_turn_prompt_with_stable_prefix(
    static_prefix: &StaticSystemPrefix,
    rag_hits: &[Fts5SearchHit],
    extra_chunks: &[ContentChunk],
    recent_turns: &[ConversationTurnRecord],
    active_user_query: &str,
    max_total_tokens: usize,
) -> AssembledPrompt {
    SystemPromptAssembler::assemble_within_budget(
        static_prefix,
        rag_hits,
        extra_chunks,
        recent_turns,
        active_user_query,
        max_total_tokens,
    )
}

/// Formats a provider JSON payload while pinning the immutable `StaticSystemPrefix`
/// as the very first message in the request.
/// Across turns where dynamic context or conversation history changes, the static system
/// prefix remains bit-for-bit identical in the payload to maximize KV cache hits.
pub fn format_provider_request_with_stable_prefix(
    provider: LlmProvider,
    model: &str,
    static_prefix: Option<&StaticSystemPrefix>,
    messages: &[(String, String)],
) -> Vec<u8> {
    let mut full_messages: Vec<(String, String)> = Vec::with_capacity(messages.len() + 1);
    if let Some(prefix) = static_prefix {
        full_messages.push(("system".to_string(), prefix.rendered_block().to_string()));
    }
    for (role, content) in messages {
        full_messages.push((role.clone(), content.clone()));
    }

    match provider {
        LlmProvider::DeepSeek | LlmProvider::OpenAi => {
            let msgs: Vec<Value> = full_messages
                .iter()
                .map(|(role, content)| {
                    serde_json::json!({
                        "role": role,
                        "content": content
                    })
                })
                .collect();
            serde_json::json!({
                "model": model,
                "messages": msgs,
            })
            .to_string()
            .into_bytes()
        }
        LlmProvider::Anthropic => {
            let sys_text = static_prefix
                .map(|p| p.rendered_block().to_string())
                .unwrap_or_default();
            let msgs: Vec<Value> = messages
                .iter()
                .map(|(role, content)| {
                    serde_json::json!({
                        "role": role,
                        "content": content
                    })
                })
                .collect();
            serde_json::json!({
                "model": model,
                "system": sys_text,
                "messages": msgs,
                "max_tokens": 2048
            })
            .to_string()
            .into_bytes()
        }
        LlmProvider::Gemini => {
            let contents: Vec<Value> = full_messages
                .iter()
                .map(|(role, content)| {
                    serde_json::json!({
                        "role": if role == "assistant" { "model" } else { "user" },
                        "parts": [{"text": content}]
                    })
                })
                .collect();
            serde_json::json!({
                "contents": contents
            })
            .to_string()
            .into_bytes()
        }
        LlmProvider::Ollama => {
            let msgs: Vec<Value> = full_messages
                .iter()
                .map(|(role, content)| {
                    serde_json::json!({
                        "role": role,
                        "content": content
                    })
                })
                .collect();
            serde_json::json!({
                "model": model,
                "messages": msgs,
                "stream": false
            })
            .to_string()
            .into_bytes()
        }
    }
}

/// Extracts explicit token usage reported by the provider JSON response (`usage` object),
/// or falls back to deterministic token estimation via `miku_memory::estimate_tokens`.
pub fn extract_or_estimate_token_usage(
    response_body: &[u8],
    static_prefix: Option<&StaticSystemPrefix>,
    dynamic_prompt_text: &str,
    reply_text: &str,
    prefix_cached_in_session: bool,
) -> TokenUsageRecord {
    if let Ok(json) = serde_json::from_slice::<Value>(response_body) {
        if let Some(usage) = json.get("usage") {
            let prompt_tokens = usage
                .get("prompt_tokens")
                .or_else(|| usage.get("input_tokens"))
                .and_then(|v| v.as_u64())
                .unwrap_or(0) as u32;
            let cached_tokens = usage
                .get("prompt_cache_hit_tokens")
                .or_else(|| usage.get("cache_read_input_tokens"))
                .or_else(|| {
                    usage
                        .get("prompt_tokens_details")
                        .and_then(|d| d.get("cached_tokens"))
                })
                .and_then(|v| v.as_u64())
                .unwrap_or(0) as u32;
            let completion_tokens = usage
                .get("completion_tokens")
                .or_else(|| usage.get("output_tokens"))
                .and_then(|v| v.as_u64())
                .unwrap_or(0) as u32;

            if prompt_tokens > 0 || cached_tokens > 0 || completion_tokens > 0 {
                let uncached = prompt_tokens.saturating_sub(cached_tokens);
                return TokenUsageRecord {
                    uncached_input_tokens: uncached,
                    cached_input_tokens: cached_tokens,
                    output_tokens: completion_tokens,
                };
            }
        }
    }

    let prefix_tokens = static_prefix.map(|p| p.token_count() as u32).unwrap_or(0);
    let dynamic_tokens = estimate_tokens(dynamic_prompt_text) as u32;
    let output_tokens = estimate_tokens(reply_text) as u32;

    if prefix_cached_in_session {
        TokenUsageRecord {
            uncached_input_tokens: dynamic_tokens,
            cached_input_tokens: prefix_tokens,
            output_tokens,
        }
    } else {
        TokenUsageRecord {
            uncached_input_tokens: prefix_tokens.saturating_add(dynamic_tokens),
            cached_input_tokens: 0,
            output_tokens,
        }
    }
}
