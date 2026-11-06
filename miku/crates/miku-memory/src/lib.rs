#![allow(
    unknown_lints,
    clippy::too_many_arguments,
    clippy::manual_range_contains,
    clippy::needless_range_loop,
    clippy::large_enum_variant,
    clippy::chunks_exact_to_as_chunks,
    clippy::type_complexity,
    clippy::invisible_characters
)]

use std::collections::BTreeMap;

use miku_gatekeeper::{Capability, DenyReason, GatekeeperEngine, GatekeeperVerdict};
use miku_protocol::sha256_hex;
use miku_storage::{
    ConversationTurnRecord, Fts5SearchHit, MemoryFts5Document, SingleThreadStoragePool,
    StorageError,
};
use serde::{Deserialize, Serialize};

pub const DEFAULT_MAX_LINES_PER_CHUNK: usize = 32;
pub const MIN_SYSTEM_BUDGET_TOKENS: u32 = 16;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ChunkKind {
    Markdown,
    SourceCode,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ContentChunk {
    pub chunk_id: String,
    pub source_path: String,
    pub kind: ChunkKind,
    pub section_title: String,
    pub start_line: usize,
    pub end_line: usize,
    pub token_count: u32,
    pub chunk_sha256: String,
    pub content: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct StaticSystemPrefix {
    pub agent_id: String,
    pub soul_directives: String,
    pub tool_contracts_summary: String,
    pub rendered_prefix: String,
    pub prefix_sha256: String,
    pub prefix_tokens: u32,
}

impl StaticSystemPrefix {
    pub fn new(agent_id: &str, soul_directives: &str, tool_contracts_summary: &str) -> Self {
        let rendered_prefix = format!(
            r##"[MIKU_STATIC_SYSTEM_PREFIX_V1]
agent_id: {}
soul: {}
tools: {}
[/MIKU_STATIC_SYSTEM_PREFIX_V1]"##,
            agent_id.trim(),
            soul_directives.trim(),
            tool_contracts_summary.trim()
        );
        let prefix_sha256 = sha256_hex(rendered_prefix.as_bytes());
        let prefix_tokens = estimate_tokens(&rendered_prefix);
        Self {
            agent_id: agent_id.trim().to_string(),
            soul_directives: soul_directives.trim().to_string(),
            tool_contracts_summary: tool_contracts_summary.trim().to_string(),
            rendered_prefix,
            prefix_sha256,
            prefix_tokens,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AssembledPrompt {
    pub static_prefix: String,
    pub prefix_sha256: String,
    pub dynamic_tail: String,
    pub full_prompt: String,
    pub prefix_tokens: u32,
    pub rag_tokens: u32,
    pub history_tokens: u32,
    pub query_tokens: u32,
    pub total_tokens: u32,
    pub max_budget_tokens: u32,
    pub retained_chunks_count: usize,
    pub evicted_chunks_count: usize,
    pub retained_turns_count: usize,
    pub evicted_turns_count: usize,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MemoryError {
    EmptyInput(String),
    BudgetTooSmall {
        required_pinned_tokens: u32,
        max_budget_tokens: u32,
    },
    GatekeeperDenied {
        reason: DenyReason,
        detail: String,
        audit_seq: u64,
    },
    GatekeeperAskRequired {
        req_hash: String,
        audit_seq: u64,
    },
    StorageFailure(String),
}

impl std::fmt::Display for MemoryError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            MemoryError::EmptyInput(msg) => write!(f, "empty memory input: {}", msg),
            MemoryError::BudgetTooSmall {
                required_pinned_tokens,
                max_budget_tokens,
            } => write!(
                f,
                "token budget {} is smaller than pinned requirement {}",
                max_budget_tokens, required_pinned_tokens
            ),
            MemoryError::GatekeeperDenied {
                reason,
                detail,
                audit_seq,
            } => write!(
                f,
                "Gatekeeper denied memory ingestion ({}, seq={}): {}",
                reason.as_code(),
                audit_seq,
                detail
            ),
            MemoryError::GatekeeperAskRequired {
                req_hash,
                audit_seq,
            } => write!(
                f,
                "Gatekeeper requires interactive approval (req_hash={}, seq={})",
                req_hash, audit_seq
            ),
            MemoryError::StorageFailure(msg) => write!(f, "storage failure: {}", msg),
        }
    }
}

impl std::error::Error for MemoryError {}

impl From<StorageError> for MemoryError {
    fn from(err: StorageError) -> Self {
        MemoryError::StorageFailure(err.to_string())
    }
}

pub fn estimate_tokens(text: &str) -> u32 {
    let trimmed = text.trim();
    if trimmed.is_empty() {
        return 0;
    }

    let mut tokens = 0u32;
    let mut in_word = false;
    let mut word_len = 0usize;

    for b in trimmed.bytes() {
        if b.is_ascii_alphanumeric() || b == b'_' {
            in_word = true;
            word_len += 1;
        } else {
            if in_word {
                tokens += 1 + (word_len / 8) as u32;
                in_word = false;
                word_len = 0;
            }
            if !b.is_ascii_whitespace() {
                tokens += 1;
            }
        }
    }
    if in_word {
        tokens += 1 + (word_len / 8) as u32;
    }

    tokens.max(1)
}

pub struct SemanticChunker;

impl SemanticChunker {
    pub fn chunk_document(
        source_path: &str,
        raw_text: &str,
        kind: ChunkKind,
        max_lines_per_chunk: usize,
    ) -> Vec<ContentChunk> {
        let max_lines = max_lines_per_chunk.max(1);
        let lines: Vec<&str> = raw_text.lines().collect();
        if lines.is_empty() {
            return Vec::new();
        }

        let mut chunks = Vec::new();
        let mut current_lines: Vec<&str> = Vec::new();
        let mut current_start_line = 1usize;
        let mut current_title = match kind {
            ChunkKind::Markdown => "root".to_string(),
            ChunkKind::SourceCode => "module".to_string(),
        };
        let mut inside_code_fence = false;

        for (idx, &line) in lines.iter().enumerate() {
            let line_no = idx + 1;
            let trimmed = line.trim();

            if kind == ChunkKind::Markdown && trimmed.starts_with("```") {
                inside_code_fence = !inside_code_fence;
            }

            let boundary_title = if !inside_code_fence {
                Self::detect_boundary_title(trimmed, kind)
            } else {
                None
            };

            let should_flush = if current_lines.is_empty() {
                false
            } else if boundary_title.is_some() {
                true
            } else {
                !inside_code_fence && current_lines.len() >= max_lines
            };

            if should_flush {
                let end_line = line_no - 1;
                chunks.push(Self::build_chunk(
                    source_path,
                    kind,
                    &current_title,
                    current_start_line,
                    end_line,
                    &current_lines,
                    chunks.len() + 1,
                ));
                current_lines.clear();
                current_start_line = line_no;
            }

            if let Some(new_title) = boundary_title {
                current_title = new_title;
            }

            current_lines.push(line);
        }

        if !current_lines.is_empty() {
            let end_line = lines.len();
            chunks.push(Self::build_chunk(
                source_path,
                kind,
                &current_title,
                current_start_line,
                end_line,
                &current_lines,
                chunks.len() + 1,
            ));
        }

        chunks
    }

    fn detect_boundary_title(trimmed: &str, kind: ChunkKind) -> Option<String> {
        match kind {
            ChunkKind::Markdown => {
                if let Some(rest) = trimmed
                    .strip_prefix("### ")
                    .or_else(|| trimmed.strip_prefix("## "))
                    .or_else(|| trimmed.strip_prefix("# "))
                {
                    let clean = rest.trim();
                    if !clean.is_empty() {
                        return Some(clean.to_string());
                    }
                }
                None
            }
            ChunkKind::SourceCode => {
                let prefixes = [
                    "pub async fn ",
                    "async fn ",
                    "pub fn ",
                    "fn ",
                    "pub struct ",
                    "struct ",
                    "pub enum ",
                    "enum ",
                    "impl ",
                    "export function ",
                ];
                for p in prefixes {
                    if let Some(rest) = trimmed.strip_prefix(p) {
                        let ident: String = rest
                            .chars()
                            .take_while(|c| c.is_ascii_alphanumeric() || *c == '_' || *c == ':')
                            .collect();
                        if !ident.is_empty() {
                            return Some(ident);
                        }
                    }
                }
                None
            }
        }
    }

    fn build_chunk(
        source_path: &str,
        kind: ChunkKind,
        section_title: &str,
        start_line: usize,
        end_line: usize,
        lines: &[&str],
        ordinal: usize,
    ) -> ContentChunk {
        let content = lines.join("\n");
        let chunk_sha256 = sha256_hex(content.as_bytes());
        let token_count = estimate_tokens(&content);
        let safe_path = source_path
            .replace('\\', "/")
            .replace(['/', '.', ':'], "_");
        let chunk_id = format!(
            "chk_{}_{}_{}_{}",
            safe_path, ordinal, start_line, end_line
        );
        ContentChunk {
            chunk_id,
            source_path: source_path.replace('\\', "/"),
            kind,
            section_title: section_title.to_string(),
            start_line,
            end_line,
            token_count,
            chunk_sha256,
            content,
        }
    }
}

pub struct SystemPromptAssembler;

impl SystemPromptAssembler {
    pub fn assemble_within_budget(
        prefix: &StaticSystemPrefix,
        rag_candidates: &[Fts5SearchHit],
        chunk_lookup: &BTreeMap<String, ContentChunk>,
        history_turns: &[ConversationTurnRecord],
        active_user_query: &str,
        max_budget_tokens: u32,
    ) -> Result<AssembledPrompt, MemoryError> {
        let query_trimmed = active_user_query.trim();
        if query_trimmed.is_empty() {
            return Err(MemoryError::EmptyInput(
                "active_user_query must not be empty".to_string(),
            ));
        }

        let query_block = format!("[ACTIVE_USER_QUERY]\n{}\n[/ACTIVE_USER_QUERY]", query_trimmed);
        let query_tokens = estimate_tokens(&query_block);
        let required_pinned = prefix.prefix_tokens + query_tokens;

        if max_budget_tokens < required_pinned {
            return Err(MemoryError::BudgetTooSmall {
                required_pinned_tokens: required_pinned,
                max_budget_tokens,
            });
        }

        let mut remaining_budget = max_budget_tokens - required_pinned;

        // 1. Select highest-priority FTS5 RAG chunks first (already sorted by BM25 descending)
        let mut retained_rag_blocks: Vec<String> = Vec::new();
        let mut rag_tokens = 0u32;
        let mut evicted_chunks_count = 0usize;

        for hit in rag_candidates {
            let formatted = if let Some(chunk) = chunk_lookup.get(&hit.doc_id) {
                format!(
                    "[MEMORY_CHUNK id={} path={}:{}-{} title=\"{}\" bm25={:.2}]\n{}\n[/MEMORY_CHUNK]",
                    chunk.chunk_id,
                    chunk.source_path,
                    chunk.start_line,
                    chunk.end_line,
                    chunk.section_title,
                    hit.bm25_score,
                    chunk.content
                )
            } else {
                format!(
                    "[MEMORY_CHUNK id={} title=\"{}\" bm25={:.2}]\n{}\n[/MEMORY_CHUNK]",
                    hit.doc_id, hit.title, hit.bm25_score, hit.snippet
                )
            };

            let cost = estimate_tokens(&formatted);
            if cost <= remaining_budget {
                remaining_budget -= cost;
                rag_tokens += cost;
                retained_rag_blocks.push(formatted);
            } else {
                evicted_chunks_count += 1;
            }
        }

        // 2. Select most recent conversation turns (newest to oldest), evicting oldest first
        let mut retained_turns_rev: Vec<String> = Vec::new();
        let mut history_tokens = 0u32;
        let mut evicted_turns_count = 0usize;

        for turn in history_turns.iter().rev() {
            let line = format!("[TURN seq={} role={}] {}", turn.seq, turn.role, turn.content);
            let cost = estimate_tokens(&line);
            if cost <= remaining_budget {
                remaining_budget -= cost;
                history_tokens += cost;
                retained_turns_rev.push(line);
            } else {
                evicted_turns_count += 1;
            }
        }
        retained_turns_rev.reverse();

        let mut tail_sections: Vec<String> = Vec::new();
        if !retained_rag_blocks.is_empty() {
            tail_sections.push(retained_rag_blocks.join("\n"));
        }
        if !retained_turns_rev.is_empty() {
            tail_sections.push(retained_turns_rev.join("\n"));
        }
        tail_sections.push(query_block);

        let dynamic_tail = tail_sections.join("\n\n");
        let full_prompt = format!("{}\n\n{}", prefix.rendered_prefix, dynamic_tail);
        let total_tokens = prefix.prefix_tokens + rag_tokens + history_tokens + query_tokens;

        Ok(AssembledPrompt {
            static_prefix: prefix.rendered_prefix.clone(),
            prefix_sha256: prefix.prefix_sha256.clone(),
            dynamic_tail,
            full_prompt,
            prefix_tokens: prefix.prefix_tokens,
            rag_tokens,
            history_tokens,
            query_tokens,
            total_tokens,
            max_budget_tokens,
            retained_chunks_count: retained_rag_blocks.len(),
            evicted_chunks_count,
            retained_turns_count: retained_turns_rev.len(),
            evicted_turns_count,
        })
    }
}

pub struct ContextEngine {
    storage: SingleThreadStoragePool,
    chunk_cache: BTreeMap<String, ContentChunk>,
    max_lines_per_chunk: usize,
}

impl ContextEngine {
    pub fn new(storage: SingleThreadStoragePool, max_lines_per_chunk: usize) -> Self {
        Self {
            storage,
            chunk_cache: BTreeMap::new(),
            max_lines_per_chunk: max_lines_per_chunk.max(1),
        }
    }

    pub fn storage(&self) -> &SingleThreadStoragePool {
        &self.storage
    }

    pub fn cached_chunk_count(&self) -> usize {
        self.chunk_cache.len()
    }

    pub fn get_cached_chunk(&self, chunk_id: &str) -> Option<&ContentChunk> {
        self.chunk_cache.get(chunk_id)
    }

    pub fn ingest_document_with_gatekeeper(
        &mut self,
        gatekeeper: &mut GatekeeperEngine,
        agent: &str,
        raw_relative_path: &str,
        raw_content: &str,
        kind: ChunkKind,
        tags: &str,
        now_ms: u64,
    ) -> Result<Vec<ContentChunk>, MemoryError> {
        if raw_content.trim().is_empty() {
            return Err(MemoryError::EmptyInput(
                "raw_content must not be empty".to_string(),
            ));
        }

        let verdict = gatekeeper.evaluate_request(
            agent,
            Capability::FsRead,
            raw_relative_path,
            "miku_memory_ingest",
            now_ms,
        );

        let canonical_target = match verdict {
            GatekeeperVerdict::Allow {
                canonical_target, ..
            } => canonical_target,
            GatekeeperVerdict::AskPending {
                ticket, audit_seq, ..
            } => {
                return Err(MemoryError::GatekeeperAskRequired {
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
                return Err(MemoryError::GatekeeperDenied {
                    reason,
                    detail,
                    audit_seq,
                });
            }
        };

        let chunks = SemanticChunker::chunk_document(
            &canonical_target,
            raw_content,
            kind,
            self.max_lines_per_chunk,
        );

        let tx = self.storage.begin_transaction()?;
        for chunk in &chunks {
            self.storage.upsert_fts5_document(MemoryFts5Document {
                doc_id: chunk.chunk_id.clone(),
                title: format!("{} ({})", chunk.section_title, chunk.source_path),
                tags: tags.to_string(),
                body: chunk.content.clone(),
                updated_at_ms: now_ms,
            })?;
            self.chunk_cache
                .insert(chunk.chunk_id.clone(), chunk.clone());
        }
        tx.commit()?;

        Ok(chunks)
    }

    pub fn build_working_memory_prompt(
        &self,
        prefix: &StaticSystemPrefix,
        session_id: &str,
        active_user_query: &str,
        fts5_top_k: usize,
        max_budget_tokens: u32,
    ) -> Result<AssembledPrompt, MemoryError> {
        let rag_hits = self
            .storage
            .search_fts5(active_user_query, fts5_top_k.max(1))?;
        let history_turns = self.storage.list_turns_for_session(session_id)?;

        SystemPromptAssembler::assemble_within_budget(
            prefix,
            &rag_hits,
            &self.chunk_cache,
            &history_turns,
            active_user_query,
            max_budget_tokens,
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use miku_gatekeeper::{PolicyEffect, ProjectProfile};
    use miku_storage::SessionRecord;

    // LEDGER-1.6-1
    #[test]
    fn test_semantic_markdown_and_code_chunker_preserves_boundaries_and_line_spans() {
        let md_doc = r##"# Introduccion a Miku
Miku vive en una Island superior de Windows.
Consume menos de 30 MB en reposo.

## Seguridad Gatekeeper
Toda herramienta pasa por el Gatekeeper deny-by-default.
```rust
fn check_policy() -> bool {
    // Este bloque de codigo no debe partirse
    true
}
```

### Auditoria SHA-256
Cada decision se firma en una cadena SHA-256."##;

        let md_chunks = SemanticChunker::chunk_document("docs/arch.md", md_doc, ChunkKind::Markdown, 10);
        assert_eq!(md_chunks.len(), 3);
        assert_eq!(md_chunks[0].section_title, "Introduccion a Miku");
        assert_eq!(md_chunks[0].start_line, 1);
        assert_eq!(md_chunks[0].end_line, 4);

        assert_eq!(md_chunks[1].section_title, "Seguridad Gatekeeper");
        assert_eq!(md_chunks[1].start_line, 5);
        assert_eq!(md_chunks[1].end_line, 13);
        assert!(md_chunks[1].content.contains("fn check_policy()"));

        assert_eq!(md_chunks[2].section_title, "Auditoria SHA-256");
        assert_eq!(md_chunks[2].start_line, 14);
        assert_eq!(md_chunks[2].end_line, 15);

        let rust_code = r##"pub fn alpha_handler(x: u32) -> u32 {
    x + 1
}

pub fn beta_dispatcher(name: &str) -> bool {
    !name.is_empty()
}"##;
        let code_chunks =
            SemanticChunker::chunk_document("src/handlers.rs", rust_code, ChunkKind::SourceCode, 10);
        assert_eq!(code_chunks.len(), 2);
        assert_eq!(code_chunks[0].section_title, "alpha_handler");
        assert_eq!(code_chunks[0].start_line, 1);
        assert_eq!(code_chunks[0].end_line, 4);
        assert_eq!(code_chunks[1].section_title, "beta_dispatcher");
        assert_eq!(code_chunks[1].start_line, 5);
        assert_eq!(code_chunks[1].end_line, 7);
        assert_eq!(
            code_chunks[0].chunk_sha256,
            sha256_hex(code_chunks[0].content.as_bytes())
        );
    }

    // LEDGER-1.6-2
    #[test]
    fn test_deterministic_token_counter_and_sliding_window_budget_eviction() {
        let prefix = StaticSystemPrefix::new(
            "claude_code",
            "Actua como arquitecto principal en Rust.",
            "fs_read, fs_write, shell_exec, memory_query",
        );

        let mut turns = Vec::new();
        for seq in 1..=8_u64 {
            turns.push(ConversationTurnRecord {
                turn_id: format!("t_{}", seq),
                session_id: "sess_window".to_string(),
                seq,
                role: if seq % 2 == 1 { "user" } else { "assistant" }.to_string(),
                content: format!(
                    "Mensaje de historial numero {} con detalles de arquitectura y pruebas unitarias.",
                    seq
                ),
                token_estimate: 16,
                ts_ms: 1_000 + seq,
            });
        }

        let rag_hits = vec![
            Fts5SearchHit {
                doc_id: "chk_1".to_string(),
                title: "Gatekeeper SHA-256".to_string(),
                tags: "security".to_string(),
                bm25_score: 4.5,
                snippet: "El Gatekeeper valida cada ruta en la jaula canonica.".to_string(),
                updated_at_ms: 1_000,
            },
            Fts5SearchHit {
                doc_id: "chk_2".to_string(),
                title: "SQLite WAL".to_string(),
                tags: "storage".to_string(),
                bm25_score: 2.1,
                snippet: "Persistencia ACID sobre tokio current_thread.".to_string(),
                updated_at_ms: 1_005,
            },
        ];

        let empty_cache = BTreeMap::new();
        let active_query = "Revisa la latencia del Gatekeeper y el presupuesto de tokens.";

        for budget in [95_u32, 130, 180, 350] {
            let assembled = SystemPromptAssembler::assemble_within_budget(
                &prefix,
                &rag_hits,
                &empty_cache,
                &turns,
                active_query,
                budget,
            )
            .expect("assemble within budget");

            assert!(
                assembled.total_tokens <= budget,
                "total_tokens {} exceeded budget {}",
                assembled.total_tokens,
                budget
            );
            assert!(assembled.full_prompt.starts_with(&prefix.rendered_prefix));
            assert!(assembled.dynamic_tail.contains(active_query));
        }

        // Verify tight budget evicts older turns while preserving the newest turn (seq=8)
        let tight = SystemPromptAssembler::assemble_within_budget(
            &prefix,
            &rag_hits,
            &empty_cache,
            &turns,
            active_query,
            180,
        )
        .expect("tight budget assembly");
        assert!(tight.evicted_turns_count > 0);
        assert!(tight.retained_turns_count >= 1);
        assert!(tight.dynamic_tail.contains("[TURN seq=8"));
        assert!(!tight.dynamic_tail.contains("[TURN seq=1 "));
    }

    // LEDGER-1.6-3
    #[test]
    fn test_system_prompt_assembler_stable_prefix_hash_and_fts5_rag_injection() {
        let pool = SingleThreadStoragePool::new_in_memory().expect("pool");
        pool.insert_session(SessionRecord {
            session_id: "sess_kv".to_string(),
            agent_name: "claude_code".to_string(),
            project_root: "C:/workspace/miku".to_string(),
            created_at_ms: 1_000,
            state: "active".to_string(),
        })
        .expect("session");

        let profile = ProjectProfile::new("miku", "C:/workspace/miku", 60_000).with_rule(
            Capability::FsRead,
            "docs/**",
            PolicyEffect::Allow,
            false,
        );
        let mut gk = GatekeeperEngine::new(profile).expect("gk");
        let mut engine = ContextEngine::new(pool, 12);

        let doc = r##"# Motor FTS5 BM25
El motor FTS5 indexa documentos locales en SQLite WAL.
Responde consultas lexicas en menos de 2 milisegundos."##;

        engine
            .ingest_document_with_gatekeeper(
                &mut gk,
                "claude_code",
                "docs/storage.md",
                doc,
                ChunkKind::Markdown,
                "fts5 wal sqlite",
                1_100,
            )
            .expect("ingest allowed doc");

        let prefix = StaticSystemPrefix::new(
            "claude_code",
            "Eres el orquestador Miku con ahorro estricto de tokens.",
            "fs_read, memory_query",
        );

        let p1 = engine
            .build_working_memory_prompt(
                &prefix,
                "sess_kv",
                "consultas lexicas fts5 sqlite wal",
                3,
                400,
            )
            .expect("prompt 1");

        let p2 = engine
            .build_working_memory_prompt(
                &prefix,
                "sess_kv",
                "otra consulta distinta sobre fts5",
                3,
                400,
            )
            .expect("prompt 2");

        // KV-Cache invariant (F-09): static_prefix and prefix_sha256 are 100% identical across turns
        assert_eq!(p1.prefix_sha256, p2.prefix_sha256);
        assert_eq!(p1.static_prefix, p2.static_prefix);
        assert_eq!(p1.retained_chunks_count, 1);
        assert!(p1.dynamic_tail.contains("docs/storage.md:1-3"));
    }

    // LEDGER-1.6-4
    #[test]
    fn test_gatekeeper_enforced_context_ingestion_blocks_jailed_or_denied_files() {
        let pool = SingleThreadStoragePool::new_in_memory().expect("pool");
        let profile = ProjectProfile::new("miku", "C:/workspace/miku", 60_000)
            .with_rule(Capability::FsRead, "src/**", PolicyEffect::Allow, false)
            .with_rule(Capability::FsRead, "src/secrets.rs", PolicyEffect::Deny, false);

        let mut gk = GatekeeperEngine::new(profile).expect("gk");
        let mut engine = ContextEngine::new(pool, 16);

        // 1. Path traversal outside jail is blocked
        let traversal_err = engine
            .ingest_document_with_gatekeeper(
                &mut gk,
                "claude_code",
                r"..\..\Windows\System32\drivers\etc\hosts",
                "127.0.0.1 localhost",
                ChunkKind::Markdown,
                "hosts",
                2_000,
            )
            .expect_err("must block path traversal");
        assert!(matches!(
            traversal_err,
            MemoryError::GatekeeperDenied {
                reason: DenyReason::PathJailViolation,
                ..
            }
        ));

        // 2. Explicit Deny rule on src/secrets.rs overrides Allow on src/**
        let deny_err = engine
            .ingest_document_with_gatekeeper(
                &mut gk,
                "claude_code",
                "src/secrets.rs",
                "pub const API_KEY: &str = \"secret\";",
                ChunkKind::SourceCode,
                "secrets",
                2_010,
            )
            .expect_err("must block explicit deny rule");
        assert!(matches!(
            deny_err,
            MemoryError::GatekeeperDenied {
                reason: DenyReason::ExplicitDenyRule,
                ..
            }
        ));

        assert_eq!(engine.cached_chunk_count(), 0);

        // 3. Allowed file src/lib.rs succeeds and is audited
        let chunks = engine
            .ingest_document_with_gatekeeper(
                &mut gk,
                "claude_code",
                "src/lib.rs",
                "pub fn init_orchestrator() -> bool {\n    true\n}",
                ChunkKind::SourceCode,
                "core orchestrator",
                2_020,
            )
            .expect("allowed file must ingest");
        assert_eq!(chunks.len(), 1);
        assert_eq!(engine.cached_chunk_count(), 1);
        assert!(gk.audit_log().verify_chain().is_ok());
        assert_eq!(gk.audit_log().len(), 3);
    }
}
