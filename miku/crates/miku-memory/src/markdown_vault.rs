use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::{Path, PathBuf};
use std::time::Instant;

use miku_gatekeeper::{Capability, GatekeeperEngine, GatekeeperVerdict};
use miku_protocol::sha256_hex;
use miku_storage::{
    sanitize_fts5_query, ConversationTurnRecord, Fts5SearchHit, MemoryFts5Document, SessionRecord,
    SingleThreadStoragePool,
};
use serde::{Deserialize, Serialize};

use crate::{
    estimate_tokens, AssembledPrompt, ChunkKind, ContentChunk, ContextEngine, MemoryError,
    SemanticChunker, StaticSystemPrefix, SystemPromptAssembler,
};

pub const SESSION_DOC_ID_PREFIX: &str = "sess_turn_";

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MarkdownNoteMetadata {
    pub relative_path: String,
    pub canonical_path: String,
    pub title: String,
    pub tags: String,
    pub file_sha256: String,
    pub chunk_ids: Vec<String>,
    pub updated_at_ms: u64,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct VaultSyncReport {
    pub scanned_files: usize,
    pub indexed_new_files: usize,
    pub reindexed_modified_files: usize,
    pub skipped_unchanged_files: usize,
    pub deleted_stale_files: usize,
    pub indexed_session_records: usize,
    pub total_fts5_documents: usize,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SelectiveRetrievalResult {
    pub query_requires_memory: bool,
    pub selected_hits: Vec<Fts5SearchHit>,
    pub assembled_prompt: AssembledPrompt,
    pub search_latency_ms: f64,
}

pub struct MarkdownMemoryVault {
    project_root: PathBuf,
    context_engine: ContextEngine,
    tracked_notes: BTreeMap<String, MarkdownNoteMetadata>,
    doc_to_relative_path: BTreeMap<String, String>,
    session_doc_ids: BTreeSet<String>,
    disk_reads_performed: u64,
    disk_writes_performed: u64,
}

impl MarkdownMemoryVault {
    pub fn new(project_root: impl Into<PathBuf>, storage: SingleThreadStoragePool, max_lines_per_chunk: usize) -> Self {
        Self {
            project_root: project_root.into(),
            context_engine: ContextEngine::new(storage, max_lines_per_chunk),
            tracked_notes: BTreeMap::new(),
            doc_to_relative_path: BTreeMap::new(),
            session_doc_ids: BTreeSet::new(),
            disk_reads_performed: 0,
            disk_writes_performed: 0,
        }
    }

    pub fn project_root(&self) -> &Path { &self.project_root }
    pub fn context_engine(&self) -> &ContextEngine { &self.context_engine }
    pub fn storage(&self) -> &SingleThreadStoragePool { self.context_engine.storage() }
    pub fn tracked_note_count(&self) -> usize { self.tracked_notes.len() }
    pub fn get_tracked_note(&self, relative_path: &str) -> Option<&MarkdownNoteMetadata> {
        self.tracked_notes.get(&normalize_rel_path(relative_path))
    }
    pub fn relative_path_for_doc(&self, doc_id: &str) -> Option<&str> {
        self.doc_to_relative_path.get(doc_id).map(|s| s.as_str())
    }
    pub fn disk_reads_performed(&self) -> u64 { self.disk_reads_performed }
    pub fn disk_writes_performed(&self) -> u64 { self.disk_writes_performed }

    /// Saves a long-term memory note as a human-readable Markdown (`.md`) file on disk
    /// after enforcing Gatekeeper `Capability::FsWrite`, then synchronizes the FTS5 index.
    pub fn write_markdown_note(
        &mut self,
        gatekeeper: &mut GatekeeperEngine,
        agent: &str,
        relative_path: &str,
        title: &str,
        tags: &str,
        markdown_body: &str,
        now_ms: u64,
    ) -> Result<MarkdownNoteMetadata, MemoryError> {
        let clean_rel = validate_markdown_relative_path(relative_path)?;
        if title.trim().is_empty() || markdown_body.trim().is_empty() {
            return Err(MemoryError::EmptyInput(
                "markdown note title and body must not be empty".to_string(),
            ));
        }

        let canonical_target = authorize_gatekeeper_path(
            gatekeeper,
            agent,
            Capability::FsWrite,
            &clean_rel,
            "miku_memory_vault_write",
            now_ms,
        )?;

        let formatted_md = format_markdown_note(title.trim(), tags.trim(), markdown_body.trim());
        let host_path = self.resolve_host_path(&clean_rel);
        if let Some(parent) = host_path.parent() {
            fs::create_dir_all(parent).map_err(|_| {
                MemoryError::StorageFailure("failed to create markdown directory".to_string())
            })?;
        }
        fs::write(&host_path, formatted_md.as_bytes()).map_err(|_| {
            MemoryError::StorageFailure(format!("failed to write note '{}'", clean_rel))
        })?;
        self.disk_writes_performed += 1;

        self.index_markdown_bytes(&clean_rel, &canonical_target, &formatted_md, tags.trim(), now_ms)
    }

    /// Synchronizes a single `.md` file from disk into SQLite FTS5 if its SHA-256 hash changed.
    /// Returns `Ok(true)` if the file was indexed/reindexed, or `Ok(false)` if unchanged (0 reindex).
    pub fn sync_markdown_file(
        &mut self,
        gatekeeper: &mut GatekeeperEngine,
        agent: &str,
        relative_path: &str,
        fallback_tags: &str,
        now_ms: u64,
    ) -> Result<bool, MemoryError> {
        let clean_rel = validate_markdown_relative_path(relative_path)?;
        let canonical_target = authorize_gatekeeper_path(
            gatekeeper,
            agent,
            Capability::FsRead,
            &clean_rel,
            "miku_memory_vault_read",
            now_ms,
        )?;

        let host_path = self.resolve_host_path(&clean_rel);
        if !host_path.exists() {
            return self.purge_deleted_note(&clean_rel);
        }

        let raw_content = fs::read_to_string(&host_path).map_err(|_| {
            MemoryError::StorageFailure(format!("failed to read markdown file '{}'", clean_rel))
        })?;
        self.disk_reads_performed += 1;

        let current_sha256 = sha256_hex(raw_content.as_bytes());
        if let Some(existing) = self.tracked_notes.get(&clean_rel) {
            if existing.file_sha256 == current_sha256 {
                return Ok(false);
            }
        }

        let extracted_tags = extract_markdown_tags(&raw_content, fallback_tags);
        self.index_markdown_bytes(&clean_rel, &canonical_target, &raw_content, &extracted_tags, now_ms)?;
        Ok(true)
    }

    /// Incrementally synchronizes all `.md` files in `vault_rel_dir` using SHA-256 modification hashes:
    /// - New `.md` files are indexed.
    /// - Modified `.md` files (SHA-256 changed) have their old chunks replaced.
    /// - Intact `.md` files (SHA-256 identical) are skipped (`0` reindex operations).
    /// - Deleted `.md` files are purged from FTS5 and chunk cache so no stale results remain.
    pub fn sync_vault_directory(
        &mut self,
        gatekeeper: &mut GatekeeperEngine,
        agent: &str,
        vault_rel_dir: &str,
        now_ms: u64,
    ) -> Result<VaultSyncReport, MemoryError> {
        let clean_dir = normalize_rel_path(vault_rel_dir);
        let dir_probe = if clean_dir.is_empty() || clean_dir == "." {
            "notes".to_string()
        } else {
            format!("{}/_dir_scan.md", clean_dir)
        };
        let _ = authorize_gatekeeper_path(
            gatekeeper,
            agent,
            Capability::FsRead,
            &dir_probe,
            "miku_memory_vault_scan",
            now_ms,
        )?;

        let host_dir = self.resolve_host_path(&clean_dir);
        let mut discovered_rel_paths: BTreeSet<String> = BTreeSet::new();
        if host_dir.exists() {
            collect_markdown_files_recursively(&self.project_root, &host_dir, &mut discovered_rel_paths)?;
        }

        let mut report = VaultSyncReport {
            scanned_files: discovered_rel_paths.len(),
            ..VaultSyncReport::default()
        };

        let prefix_filter = if clean_dir.is_empty() || clean_dir == "." {
            String::new()
        } else {
            format!("{}/", clean_dir)
        };
        let tracked_keys: Vec<String> = self
            .tracked_notes
            .keys()
            .filter(|k| prefix_filter.is_empty() || k.starts_with(&prefix_filter))
            .cloned()
            .collect();

        for tracked_rel in tracked_keys {
            if !discovered_rel_paths.contains(&tracked_rel) && self.purge_deleted_note(&tracked_rel)? {
                report.deleted_stale_files += 1;
            }
        }

        for rel_path in discovered_rel_paths {
            let was_known = self.tracked_notes.contains_key(&rel_path);
            let changed = self.sync_markdown_file(gatekeeper, agent, &rel_path, "memory", now_ms)?;
            if changed {
                if was_known {
                    report.reindexed_modified_files += 1;
                } else {
                    report.indexed_new_files += 1;
                }
            } else {
                report.skipped_unchanged_files += 1;
            }
        }

        report.indexed_session_records = self.session_doc_ids.len();
        report.total_fts5_documents = self.storage().fts5_document_count()?;
        Ok(report)
    }

    /// Indexes a session and all its conversation turns into SQLite FTS5 (`F-23`).
    pub fn index_session_history(&mut self, session_id: &str, now_ms: u64) -> Result<usize, MemoryError> {
        let Some(session) = self.storage().get_session(session_id)? else {
            return Ok(0);
        };
        let turns = self.storage().list_turns_for_session(session_id)?;
        self.index_session_and_turns(&session, &turns, now_ms)
    }

    /// Indexes all sessions and conversation turns currently persisted in `SingleThreadStoragePool`.
    pub fn index_all_sessions(&mut self, now_ms: u64) -> Result<usize, MemoryError> {
        let sessions = self.storage().list_all_sessions()?;
        let mut total_indexed = 0usize;
        for session in sessions {
            let turns = self.storage().list_turns_for_session(&session.session_id)?;
            total_indexed += self.index_session_and_turns(&session, &turns, now_ms)?;
        }
        Ok(total_indexed)
    }

    /// Rebuilds the entire SQLite FTS5 index from scratch using the Markdown files on disk
    /// (the source of truth) and the persisted session records in `SingleThreadStoragePool`.
    pub fn rebuild_fts5_index_from_sources(
        &mut self,
        gatekeeper: &mut GatekeeperEngine,
        agent: &str,
        vault_rel_dir: &str,
        now_ms: u64,
    ) -> Result<VaultSyncReport, MemoryError> {
        self.storage().clear_fts5_documents()?;
        self.context_engine.clear_cached_chunks();
        self.tracked_notes.clear();
        self.doc_to_relative_path.clear();
        self.session_doc_ids.clear();

        let mut report = self.sync_vault_directory(gatekeeper, agent, vault_rel_dir, now_ms)?;
        let session_docs = self.index_all_sessions(now_ms)?;
        report.indexed_session_records = session_docs;
        report.total_fts5_documents = self.storage().fts5_document_count()?;
        Ok(report)
    }

    /// Executes a local BM25 search across Markdown notes and session history,
    /// enforcing Gatekeeper `Capability::FsRead` on every matched Markdown note.
    pub fn search_authorized(
        &mut self,
        gatekeeper: &mut GatekeeperEngine,
        agent: &str,
        raw_query: &str,
        limit: usize,
        now_ms: u64,
    ) -> Result<Vec<Fts5SearchHit>, MemoryError> {
        self.search_authorized_filtered(gatekeeper, agent, raw_query, true, true, limit, now_ms)
    }

    /// Executes a type-filtered BM25 search where source-kind filtering and Gatekeeper
    /// authorization are applied before `limit` truncation.
    pub fn search_authorized_filtered(
        &mut self,
        gatekeeper: &mut GatekeeperEngine,
        agent: &str,
        raw_query: &str,
        include_markdown: bool,
        include_sessions: bool,
        limit: usize,
        now_ms: u64,
    ) -> Result<Vec<Fts5SearchHit>, MemoryError> {
        if limit == 0 || (!include_markdown && !include_sessions) {
            return Ok(Vec::new());
        }
        let storage = self.context_engine.storage();
        let doc_to_rel = &self.doc_to_relative_path;
        let hits = storage.search_fts5_filtered(raw_query, limit, |doc_id| {
            let is_session = doc_id.starts_with(SESSION_DOC_ID_PREFIX);
            if (is_session && !include_sessions) || (!is_session && !include_markdown) {
                return false;
            }
            if let Some(rel_path) = doc_to_rel.get(doc_id) {
                let verdict = gatekeeper.evaluate_request(
                    agent,
                    Capability::FsRead,
                    rel_path,
                    "miku_memory_search_hit",
                    now_ms,
                );
                if !matches!(verdict, GatekeeperVerdict::Allow { .. }) {
                    return false;
                }
            }
            true
        })?;
        Ok(hits)
    }

    /// Retrieves and injects ONLY relevant memory fragments on demand when the query requires it,
    /// strictly respecting `max_budget_tokens` via `SystemPromptAssembler::assemble_within_budget`.
    pub fn retrieve_on_demand_within_budget(
        &mut self,
        gatekeeper: &mut GatekeeperEngine,
        agent: &str,
        prefix: &StaticSystemPrefix,
        session_id: &str,
        active_user_query: &str,
        fts5_top_k: usize,
        max_budget_tokens: u32,
        now_ms: u64,
    ) -> Result<SelectiveRetrievalResult, MemoryError> {
        let t0 = Instant::now();
        let requires_memory = query_requires_memory_lookup(active_user_query);

        let selected_hits = if requires_memory && fts5_top_k > 0 {
            self.search_authorized(gatekeeper, agent, active_user_query, fts5_top_k, now_ms)?
        } else {
            Vec::new()
        };
        let search_latency_ms = t0.elapsed().as_secs_f64() * 1_000.0;

        let history_turns = self.storage().list_turns_for_session(session_id)?;
        let assembled_prompt = SystemPromptAssembler::assemble_within_budget(
            prefix,
            &selected_hits,
            self.context_engine.chunk_cache(),
            &history_turns,
            active_user_query,
            max_budget_tokens,
        )?;

        Ok(SelectiveRetrievalResult {
            query_requires_memory: requires_memory,
            selected_hits,
            assembled_prompt,
            search_latency_ms,
        })
    }

    fn purge_deleted_note(&mut self, clean_rel: &str) -> Result<bool, MemoryError> {
        let Some(removed_meta) = self.tracked_notes.remove(clean_rel) else {
            return Ok(false);
        };
        let tx = self.storage().begin_transaction()?;
        for chunk_id in &removed_meta.chunk_ids {
            let _ = self.storage().delete_fts5_document(chunk_id)?;
            let _ = self.context_engine.remove_cached_chunk(chunk_id);
            self.doc_to_relative_path.remove(chunk_id);
        }
        tx.commit()?;
        Ok(true)
    }

    fn index_markdown_bytes(
        &mut self,
        clean_rel: &str,
        canonical_target: &str,
        raw_content: &str,
        tags: &str,
        now_ms: u64,
    ) -> Result<MarkdownNoteMetadata, MemoryError> {
        let file_sha256 = sha256_hex(raw_content.as_bytes());
        let note_title = extract_markdown_title(raw_content, clean_rel);
        let chunks = SemanticChunker::chunk_document(
            clean_rel,
            raw_content,
            ChunkKind::Markdown,
            self.context_engine.max_lines_per_chunk(),
        );

        let tx = self.storage().begin_transaction()?;
        if let Some(prev) = self.tracked_notes.get(clean_rel) {
            for old_id in &prev.chunk_ids {
                let _ = self.storage().delete_fts5_document(old_id)?;
                let _ = self.context_engine.remove_cached_chunk(old_id);
                self.doc_to_relative_path.remove(old_id);
            }
        }

        let mut chunk_ids = Vec::with_capacity(chunks.len());
        for chunk in chunks {
            self.storage().upsert_fts5_document(MemoryFts5Document {
                doc_id: chunk.chunk_id.clone(),
                title: format!("{} · {} ({})", note_title, chunk.section_title, clean_rel),
                tags: tags.to_string(),
                body: chunk.content.clone(),
                updated_at_ms: now_ms,
            })?;
            chunk_ids.push(chunk.chunk_id.clone());
            self.doc_to_relative_path.insert(chunk.chunk_id.clone(), clean_rel.to_string());
            self.context_engine.insert_cached_chunk(chunk);
        }
        tx.commit()?;

        let meta = MarkdownNoteMetadata {
            relative_path: clean_rel.to_string(),
            canonical_path: canonical_target.to_string(),
            title: note_title,
            tags: tags.to_string(),
            file_sha256,
            chunk_ids,
            updated_at_ms: now_ms,
        };
        self.tracked_notes.insert(clean_rel.to_string(), meta.clone());
        Ok(meta)
    }

    fn index_session_and_turns(
        &mut self,
        session: &SessionRecord,
        turns: &[ConversationTurnRecord],
        now_ms: u64,
    ) -> Result<usize, MemoryError> {
        let tx = self.storage().begin_transaction()?;
        let mut count = 0usize;

        for turn in turns {
            let doc_id = format!("{}{}_{}", SESSION_DOC_ID_PREFIX, session.session_id, turn.turn_id);
            let title = format!(
                "Session {} ({}) · Turn #{} [{}]",
                session.session_id, session.agent_name, turn.seq, turn.role
            );
            let tags = format!("session {} {} {}", session.session_id, session.agent_name, turn.role);
            self.storage().upsert_fts5_document(MemoryFts5Document {
                doc_id: doc_id.clone(),
                title: title.clone(),
                tags,
                body: turn.content.clone(),
                updated_at_ms: turn.ts_ms.max(now_ms),
            })?;
            let chunk = ContentChunk {
                chunk_id: doc_id.clone(),
                source_path: format!("session://{}/{}", session.session_id, turn.turn_id),
                kind: ChunkKind::Markdown,
                section_title: title,
                start_line: turn.seq as usize,
                end_line: turn.seq as usize,
                token_count: turn.token_estimate.max(estimate_tokens(&turn.content)),
                chunk_sha256: sha256_hex(turn.content.as_bytes()),
                content: turn.content.clone(),
            };
            self.context_engine.insert_cached_chunk(chunk);
            self.session_doc_ids.insert(doc_id);
            count += 1;
        }

        tx.commit()?;
        Ok(count)
    }

    fn resolve_host_path(&self, clean_rel: &str) -> PathBuf {
        let mut p = self.project_root.clone();
        for seg in clean_rel.split('/') {
            if !seg.is_empty() && seg != "." {
                p.push(seg);
            }
        }
        p
    }
}

fn authorize_gatekeeper_path(
    gatekeeper: &mut GatekeeperEngine,
    agent: &str,
    capability: Capability,
    relative_path: &str,
    args_tag: &str,
    now_ms: u64,
) -> Result<String, MemoryError> {
    let verdict = gatekeeper.evaluate_request(agent, capability, relative_path, args_tag, now_ms);
    match verdict {
        GatekeeperVerdict::Allow { canonical_target, .. } => Ok(canonical_target),
        GatekeeperVerdict::AskPending { ticket, audit_seq, .. } => {
            Err(MemoryError::GatekeeperAskRequired { req_hash: ticket.req_hash, audit_seq })
        }
        GatekeeperVerdict::Deny { reason, detail, audit_seq, .. } => {
            Err(MemoryError::GatekeeperDenied { reason, detail, audit_seq })
        }
    }
}

fn validate_markdown_relative_path(raw: &str) -> Result<String, MemoryError> {
    let clean = normalize_rel_path(raw);
    if clean.is_empty() {
        return Err(MemoryError::EmptyInput("markdown relative path must not be empty".to_string()));
    }
    if !clean.to_ascii_lowercase().ends_with(".md") {
        return Err(MemoryError::EmptyInput("long-term memory note path must end with .md".to_string()));
    }
    Ok(clean)
}

fn normalize_rel_path(raw: &str) -> String {
    raw.trim().replace('\\', "/").trim_start_matches("./").trim_matches('/').to_string()
}

fn format_markdown_note(title: &str, tags: &str, body: &str) -> String {
    format!("# {}\n<!-- miku-tags: {} -->\n\n{}\n", title, tags, body)
}

fn extract_markdown_title(raw_md: &str, fallback_path: &str) -> String {
    raw_md
        .lines()
        .find_map(|line| line.trim().strip_prefix("# ").map(str::trim))
        .filter(|s| !s.is_empty())
        .map(ToOwned::to_owned)
        .unwrap_or_else(|| fallback_path.to_string())
}

fn extract_markdown_tags(raw_md: &str, fallback_tags: &str) -> String {
    raw_md
        .lines()
        .take(8)
        .find_map(|line| line.trim().strip_prefix("<!-- miku-tags:")?.strip_suffix("-->"))
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(ToOwned::to_owned)
        .unwrap_or_else(|| fallback_tags.to_string())
}

fn query_requires_memory_lookup(raw_query: &str) -> bool {
    let terms = sanitize_fts5_query(raw_query);
    let trivial = ["hola", "buenos", "dias", "gracias", "ok", "vale", "hello"];
    !terms.is_empty() && !(terms.len() == 1 && trivial.contains(&terms[0].as_str()))
}

fn collect_markdown_files_recursively(
    project_root: &Path,
    current_dir: &Path,
    out: &mut BTreeSet<String>,
) -> Result<(), MemoryError> {
    let entries = fs::read_dir(current_dir)
        .map_err(|_| MemoryError::StorageFailure("failed to read vault directory entries".to_string()))?;
    for entry in entries {
        let path = entry
            .map_err(|_| MemoryError::StorageFailure("failed to read directory entry".to_string()))?
            .path();
        if path.is_dir() {
            collect_markdown_files_recursively(project_root, &path, out)?;
        } else if path.extension().and_then(|e| e.to_str()).map(|e| e.eq_ignore_ascii_case("md")).unwrap_or(false) {
            if let Ok(rel) = path.strip_prefix(project_root) {
                let rel_str = normalize_rel_path(&rel.to_string_lossy());
                if !rel_str.is_empty() {
                    out.insert(rel_str);
                }
            }
        }
    }
    Ok(())
}
