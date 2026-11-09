use std::time::Instant;

use miku_gatekeeper::GatekeeperEngine;
use miku_protocol::{JSONRPC_VERSION_2_0, MIKU_PROTOCOL_VERSION_V1};
use miku_storage::sanitize_fts5_query;
use serde::{Deserialize, Serialize};

use crate::markdown_vault::{MarkdownMemoryVault, SESSION_DOC_ID_PREFIX};
use crate::MemoryError;

pub const CTRL_K_SHORTCUT_ACCEL: &str = "Ctrl+K";
pub const DEFAULT_ISLAND_SEARCH_MAX_RESULTS: usize = 10;
pub const HARD_MAX_ISLAND_SEARCH_RESULTS: usize = 25;
pub const ISLAND_TRUSTED_ORIGIN_PREFIX: &str = "https://miku.local";

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum IslandSearchSourceKind {
    MarkdownNote,
    SessionHistory,
}

#[derive(Debug, Default, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum IslandSearchFilter {
    #[default]
    All,
    MarkdownOnly,
    SessionsOnly,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct IslandSearchResultItem {
    pub doc_id: String,
    pub source_kind: IslandSearchSourceKind,
    pub title: String,
    pub source_locator: String,
    pub tags: String,
    pub bm25_score: f64,
    pub highlighted_snippet: String,
    pub updated_at_ms: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct IslandMemorySearchIpcRequest {
    pub jsonrpc: String,
    pub v: u8,
    pub id: String,
    pub origin: String,
    pub shortcut: String,
    pub raw_query: String,
    #[serde(default)]
    pub filter: IslandSearchFilter,
    pub limit: usize,
    pub agent_id: String,
    pub timestamp_ms: u64,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct IslandMemorySearchIpcResponse {
    pub jsonrpc: String,
    pub v: u8,
    pub id: String,
    pub reply_to: String,
    pub palette_open: bool,
    pub sanitized_terms: Vec<String>,
    pub results: Vec<IslandSearchResultItem>,
    pub returned_count: usize,
    pub total_indexed_corpus_docs: usize,
    pub truncated_to_limit: bool,
    pub search_latency_ms: f64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum IslandSearchIpcError {
    UnauthorizedOrigin,
    UnsupportedShortcut(String),
    InvalidFrame(String),
    MemoryBackendError(String),
}

impl std::fmt::Display for IslandSearchIpcError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            IslandSearchIpcError::UnauthorizedOrigin => {
                write!(f, "unauthorized Island IPC origin")
            }
            IslandSearchIpcError::UnsupportedShortcut(s) => {
                write!(f, "unsupported search shortcut '{}'", redact_sensitive_text(s))
            }
            IslandSearchIpcError::InvalidFrame(msg) => {
                write!(f, "invalid Island search IPC frame: {}", redact_sensitive_text(msg))
            }
            IslandSearchIpcError::MemoryBackendError(msg) => {
                write!(f, "memory search failure: {}", redact_sensitive_text(msg))
            }
        }
    }
}

impl std::error::Error for IslandSearchIpcError {}

impl From<MemoryError> for IslandSearchIpcError {
    fn from(err: MemoryError) -> Self {
        IslandSearchIpcError::MemoryBackendError(redact_sensitive_text(&err.to_string()))
    }
}

#[derive(Debug, Default)]
pub struct IslandMemorySearchController {
    palette_open: bool,
    completed_queries: u64,
    last_query_latency_ms: f64,
}

impl IslandMemorySearchController {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn is_palette_open(&self) -> bool {
        self.palette_open
    }

    pub fn completed_queries(&self) -> u64 {
        self.completed_queries
    }

    pub fn last_query_latency_ms(&self) -> f64 {
        self.last_query_latency_ms
    }

    pub fn trigger_ctrl_k(&mut self) -> bool {
        self.palette_open = true;
        self.palette_open
    }

    pub fn close_on_escape(&mut self) {
        self.palette_open = false;
    }

    /// Processes a raw JSON IPC request emitted by the Island `Ctrl+K` command palette
    /// (`Ctrl+K` -> IPC JSON -> FTS5 BM25 -> bounded Island results).
    /// Never returns the full corpus; caps results at `HARD_MAX_ISLAND_SEARCH_RESULTS` (25).
    pub fn handle_ipc_search_json(
        &mut self,
        raw_json: &str,
        vault: &mut MarkdownMemoryVault,
        gatekeeper: &mut GatekeeperEngine,
    ) -> Result<String, IslandSearchIpcError> {
        if raw_json.trim().is_empty() || raw_json.len() > 16_384 || raw_json.as_bytes().contains(&0) {
            return Err(IslandSearchIpcError::InvalidFrame(
                "IPC search frame is empty, oversized, or contains null bytes".to_string(),
            ));
        }

        let req: IslandMemorySearchIpcRequest = serde_json::from_str(raw_json).map_err(|_| {
            IslandSearchIpcError::InvalidFrame("malformed JSON-RPC search request".to_string())
        })?;

        let resp = self.execute_search_request(&req, vault, gatekeeper)?;
        serde_json::to_string(&resp).map_err(|_| {
            IslandSearchIpcError::InvalidFrame("failed to encode search response".to_string())
        })
    }

    pub fn execute_search_request(
        &mut self,
        req: &IslandMemorySearchIpcRequest,
        vault: &mut MarkdownMemoryVault,
        gatekeeper: &mut GatekeeperEngine,
    ) -> Result<IslandMemorySearchIpcResponse, IslandSearchIpcError> {
        if req.jsonrpc != JSONRPC_VERSION_2_0 || req.v != MIKU_PROTOCOL_VERSION_V1 {
            return Err(IslandSearchIpcError::InvalidFrame(
                "unsupported jsonrpc or protocol version".to_string(),
            ));
        }
        if req.id.trim().is_empty() || req.agent_id.trim().is_empty() {
            return Err(IslandSearchIpcError::InvalidFrame(
                "request id and agent_id must not be empty".to_string(),
            ));
        }
        validate_island_origin(&req.origin)?;

        if !req.shortcut.trim().eq_ignore_ascii_case(CTRL_K_SHORTCUT_ACCEL) {
            return Err(IslandSearchIpcError::UnsupportedShortcut(
                req.shortcut.clone(),
            ));
        }

        self.palette_open = true;
        let t0 = Instant::now();

        let requested_limit = if req.limit == 0 {
            DEFAULT_ISLAND_SEARCH_MAX_RESULTS
        } else {
            req.limit
        };
        let bounded_limit = requested_limit.clamp(1, HARD_MAX_ISLAND_SEARCH_RESULTS);
        let sanitized_terms = sanitize_fts5_query(&req.raw_query);
        let total_indexed_corpus_docs = vault
            .storage()
            .fts5_document_count()
            .map_err(MemoryError::from)?;

        let fetch_window = (bounded_limit.saturating_mul(2)).max(bounded_limit);
        let raw_hits = vault.search_authorized(
            gatekeeper,
            req.agent_id.trim(),
            &req.raw_query,
            fetch_window,
            req.timestamp_ms,
        )?;

        let mut results = Vec::with_capacity(bounded_limit);
        for hit in raw_hits {
            let is_session = hit.doc_id.starts_with(SESSION_DOC_ID_PREFIX);
            let source_kind = if is_session {
                IslandSearchSourceKind::SessionHistory
            } else {
                IslandSearchSourceKind::MarkdownNote
            };

            match req.filter {
                IslandSearchFilter::All => {}
                IslandSearchFilter::MarkdownOnly => {
                    if source_kind != IslandSearchSourceKind::MarkdownNote {
                        continue;
                    }
                }
                IslandSearchFilter::SessionsOnly => {
                    if source_kind != IslandSearchSourceKind::SessionHistory {
                        continue;
                    }
                }
            }

            let source_locator = if let Some(rel) = vault.relative_path_for_doc(&hit.doc_id) {
                rel.to_string()
            } else if let Some(chunk) = vault.context_engine().get_cached_chunk(&hit.doc_id) {
                chunk.source_path.clone()
            } else {
                hit.doc_id.clone()
            };

            results.push(IslandSearchResultItem {
                doc_id: hit.doc_id,
                source_kind,
                title: escape_text_for_island_dom(&redact_sensitive_text(&hit.title)),
                source_locator: escape_text_for_island_dom(&source_locator),
                tags: escape_text_for_island_dom(&hit.tags),
                bm25_score: hit.bm25_score,
                highlighted_snippet: escape_text_for_island_dom(&redact_sensitive_text(
                    &hit.snippet,
                )),
                updated_at_ms: hit.updated_at_ms,
            });

            if results.len() >= bounded_limit {
                break;
            }
        }

        let search_latency_ms = t0.elapsed().as_secs_f64() * 1_000.0;
        self.completed_queries += 1;
        self.last_query_latency_ms = search_latency_ms;

        let returned_count = results.len();
        let truncated_to_limit =
            total_indexed_corpus_docs > returned_count && returned_count == bounded_limit;

        Ok(IslandMemorySearchIpcResponse {
            jsonrpc: JSONRPC_VERSION_2_0.to_string(),
            v: MIKU_PROTOCOL_VERSION_V1,
            id: format!("island-search-ack-{}", self.completed_queries),
            reply_to: req.id.clone(),
            palette_open: self.palette_open,
            sanitized_terms,
            results,
            returned_count,
            total_indexed_corpus_docs,
            truncated_to_limit,
            search_latency_ms,
        })
    }
}

fn validate_island_origin(origin: &str) -> Result<(), IslandSearchIpcError> {
    let trimmed = origin.trim();
    let lower = trimmed.to_ascii_lowercase();
    if lower == ISLAND_TRUSTED_ORIGIN_PREFIX {
        return Ok(());
    }
    if let Some(rest) = lower.strip_prefix("https://miku.local/") {
        if !rest.contains("..") && !rest.contains('\\') && !rest.contains('@') {
            return Ok(());
        }
    }
    Err(IslandSearchIpcError::UnauthorizedOrigin)
}

fn escape_text_for_island_dom(input: &str) -> String {
    let mut safe = String::with_capacity(input.len());
    for ch in input.chars() {
        match ch {
            '\0' | '\r' => {}
            '<' => safe.push_str("&lt;"),
            '>' => safe.push_str("&gt;"),
            '&' => safe.push_str("&amp;"),
            '"' => safe.push_str("&quot;"),
            '\'' => safe.push_str("&#x27;"),
            c if c.is_control() && c != '\n' && c != '\t' => {}
            c => safe.push(c),
        }
    }
    safe
}

pub fn redact_sensitive_text(raw: &str) -> String {
    let mut tokens = Vec::new();
    for word in raw.split_whitespace() {
        let clean = word.trim_matches(|c: char| {
            c == '"' || c == '\'' || c == ',' || c == ';' || c == '(' || c == ')'
        });
        let lower = clean.to_ascii_lowercase();
        let is_secret = lower.starts_with("sk-")
            || lower.starts_with("ghp_")
            || lower.starts_with("github_pat_")
            || lower.starts_with("xoxb-")
            || lower.starts_with("ai_key_");
        let is_sensitive_os_path = lower.contains(":\\windows\\")
            || lower.contains(":/windows/")
            || lower.contains("/etc/shadow")
            || lower.contains("/etc/passwd")
            || lower.contains("appdata\\local\\microsoft\\credentials");
        if is_secret {
            tokens.push("[REDACTED_SECRET]".to_string());
        } else if is_sensitive_os_path {
            tokens.push("[REDACTED_PATH]".to_string());
        } else {
            tokens.push(word.to_string());
        }
    }
    tokens.join(" ")
}
