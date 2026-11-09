use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Instant, SystemTime, UNIX_EPOCH};

use miku_gatekeeper::{Capability, DenyReason, GatekeeperEngine, PolicyEffect, ProjectProfile};
use miku_protocol::{JSONRPC_VERSION_2_0, MIKU_PROTOCOL_VERSION_V1};
use miku_storage::{ConversationTurnRecord, SessionRecord, SingleThreadStoragePool};

use crate::island_search_ipc::{
    IslandMemorySearchController, IslandMemorySearchIpcRequest, IslandSearchFilter,
    IslandSearchIpcError, IslandSearchSourceKind, CTRL_K_SHORTCUT_ACCEL,
    HARD_MAX_ISLAND_SEARCH_RESULTS,
};
use crate::markdown_vault::MarkdownMemoryVault;
use crate::{MemoryError, StaticSystemPrefix};

static TEMP_COUNTER: AtomicU64 = AtomicU64::new(1);

struct TempVaultSandbox {
    root_dir: PathBuf,
    canonical_root_str: String,
}

impl TempVaultSandbox {
    fn new(label: &str) -> Self {
        let seq = TEMP_COUNTER.fetch_add(1, Ordering::Relaxed);
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0);
        let folder = format!("miku_mem_3_9_{}_{}_{}_{}", label, std::process::id(), seq, nanos);
        let root_dir = std::env::temp_dir().join(folder);
        let _ = fs::remove_dir_all(&root_dir);
        fs::create_dir_all(root_dir.join("notes")).expect("create temp notes dir");
        let canonical_root_str = normalize_root_for_jail(&root_dir);
        Self { root_dir, canonical_root_str }
    }

    fn root_path(&self) -> &Path {
        &self.root_dir
    }

    fn canonical_root(&self) -> &str {
        &self.canonical_root_str
    }
}

impl Drop for TempVaultSandbox {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.root_dir);
    }
}

fn normalize_root_for_jail(p: &Path) -> String {
    let raw = p.to_string_lossy().replace('\\', "/");
    let trimmed = raw.trim_end_matches('/');
    if trimmed.starts_with('/') || (trimmed.len() >= 2 && trimmed.as_bytes()[1] == b':') {
        trimmed.to_string()
    } else {
        format!("/{}", trimmed)
    }
}

fn compute_percentile(sorted_ms: &[f64], q: f64) -> f64 {
    if sorted_ms.is_empty() {
        return 0.0;
    }
    let idx = ((sorted_ms.len() as f64 - 1.0) * q).round() as usize;
    sorted_ms[idx.min(sorted_ms.len() - 1)]
}

fn build_notes_gatekeeper(canonical_root: &str) -> GatekeeperEngine {
    let profile = ProjectProfile::new("miku-3-9", canonical_root, 60_000)
        .with_rule(Capability::FsRead, "notes/**", PolicyEffect::Allow, false)
        .with_rule(Capability::FsWrite, "notes/**", PolicyEffect::Allow, false);
    GatekeeperEngine::new(profile).expect("gatekeeper")
}

// LEDGER-3.9-1: Index 500 Markdown notes + session records, verify BM25 ranking
//               and measure local FTS5 search < 10.0 ms with warmup + multi-sample
#[test]
fn test_index_500_markdown_notes_and_session_records_bm25_ranking_and_warmed_search_under_10ms() {
    let sandbox = TempVaultSandbox::new("500_notes_bench");
    let pool = SingleThreadStoragePool::new_in_memory().expect("in-memory WAL pool");

    for s_idx in 0..10_usize {
        let sess_id = format!("sess_hist_{:02}", s_idx);
        pool.insert_session(SessionRecord {
            session_id: sess_id.clone(),
            agent_name: if s_idx % 2 == 0 { "claude_code" } else { "cursor" }.to_string(),
            project_root: sandbox.canonical_root().to_string(),
            created_at_ms: 1_762_500_000 + (s_idx as u64),
            state: "active".to_string(),
        })
        .expect("insert session");

        for t_seq in 1..=3_u64 {
            let content = if s_idx == 4 && t_seq == 2 {
                "Decision arquitectonica critica: el subsistema de sincronizacion incremental por hash sha256 evita reindexar notas intactas en fts5.".to_string()
            } else {
                format!("Turno historico {} de la sesion {} revisando compilacion y pruebas.", t_seq, sess_id)
            };
            pool.insert_turn(ConversationTurnRecord {
                turn_id: format!("t_{:02}_{}", s_idx, t_seq),
                session_id: sess_id.clone(),
                seq: t_seq,
                role: if t_seq % 2 == 1 { "user" } else { "assistant" }.to_string(),
                content,
                token_estimate: 20,
                ts_ms: 1_762_500_100 + (s_idx as u64) * 10 + t_seq,
            })
            .expect("insert turn");
        }
    }

    let mut gk = build_notes_gatekeeper(sandbox.canonical_root());
    let mut vault = MarkdownMemoryVault::new(sandbox.root_path(), pool, 32);

    for i in 0..500_usize {
        let rel_path = format!("notes/note_{:03}.md", i);
        let (title, tags, body) = if i == 242 {
            (
                "Optimizacion de Indice Lexico SQLite FTS5 BM25".to_string(),
                "fts5 bm25 sqlite memoria latencia".to_string(),
                "La busqueda local FTS5 utiliza ranking BM25 ponderado en titulo, etiquetas y cuerpo para responder en menos de diez milisegundos.".to_string(),
            )
        } else if i == 388 {
            (
                "Notas sobre SQLite basico".to_string(),
                "sqlite".to_string(),
                "Uso general de tablas locales y consultas simples.".to_string(),
            )
        } else {
            (
                format!("Nota Tecnica Modulo {:03}", i),
                format!("modulo_{:03} arquitectura rust", i % 25),
                format!("Documentacion persistente del componente numero {} con invariantes.", i),
            )
        };
        let md_text = format!("# {}\n<!-- miku-tags: {} -->\n\n{}\n", title, tags, body);
        fs::write(sandbox.root_path().join(&rel_path), md_text.as_bytes()).expect("write seed md");
    }

    let sync_report = vault
        .sync_vault_directory(&mut gk, "claude_code", "notes", 1_762_501_000)
        .expect("sync 500 notes");
    let indexed_sessions = vault.index_all_sessions(1_762_501_010).expect("index sessions");

    assert_eq!(sync_report.scanned_files, 500);
    assert_eq!(sync_report.indexed_new_files, 500);
    assert_eq!(sync_report.reindexed_modified_files, 0);
    assert_eq!(indexed_sessions, 30);
    assert_eq!(vault.tracked_note_count(), 500);
    assert!(vault.storage().fts5_document_count().expect("doc count") >= 530);

    let ranked = vault
        .search_authorized(&mut gk, "claude_code", "fts5 bm25 sqlite latencia", 5, 1_762_501_100)
        .expect("authorized BM25 search");
    assert!(!ranked.is_empty());
    assert!(ranked[0].title.contains("note_242.md"));
    assert!(ranked[0].bm25_score > ranked.get(1).map(|h| h.bm25_score).unwrap_or(0.0));

    let session_hits = vault
        .search_authorized(&mut gk, "claude_code", "sincronizacion incremental hash sha256", 5, 1_762_501_105)
        .expect("session history search");
    assert!(!session_hits.is_empty());
    assert!(session_hits[0].doc_id.contains("sess_hist_04"));

    for w in 0..5_u64 {
        let _ = vault
            .search_authorized(&mut gk, "claude_code", "fts5 bm25 sqlite memoria latencia", 10, 1_762_501_200 + w)
            .expect("warmup search");
    }

    let mut samples_ms = Vec::with_capacity(25);
    for s in 0..25_u64 {
        let t0 = Instant::now();
        let hits = vault
            .search_authorized(&mut gk, "claude_code", "fts5 bm25 sqlite memoria latencia", 10, 1_762_501_300 + s)
            .expect("measured search");
        let elapsed_ms = t0.elapsed().as_secs_f64() * 1_000.0;
        assert!(!hits.is_empty());
        samples_ms.push(elapsed_ms);
    }

    samples_ms.sort_by(|a, b| a.partial_cmp(b).unwrap());
    let p50_ms = compute_percentile(&samples_ms, 0.50);
    let p95_ms = compute_percentile(&samples_ms, 0.95);
    println!(
        "BENCH-3.9 FTS5 500 notes + 30 session records (25 samples): p50={:.4} ms, p95={:.4} ms (budget < 10.0 ms)",
        p50_ms, p95_ms
    );
    assert!(p50_ms < 10.0, "FTS5 search p50 {:.4} ms exceeded 10.0 ms budget", p50_ms);
}

// LEDGER-3.9-2: Edit a .md file and verify only the modified file is reindexed
//               via SHA-256 hash, verify note deletion purges stale results, and rebuild
#[test]
fn test_markdown_edit_sha256_incremental_sync_skips_intact_docs_and_purges_deleted_notes() {
    let sandbox = TempVaultSandbox::new("incremental_sha256");
    let pool = SingleThreadStoragePool::new_in_memory().expect("pool");
    let mut gk = build_notes_gatekeeper(sandbox.canonical_root());
    let mut vault = MarkdownMemoryVault::new(sandbox.root_path(), pool, 24);

    vault
        .write_markdown_note(&mut gk, "claude_code", "notes/alpha.md", "Arquitectura Alpha", "alpha core", "El modulo alpha gestiona el ciclo principal.", 1_000)
        .expect("write alpha");
    vault
        .write_markdown_note(&mut gk, "claude_code", "notes/beta.md", "Arquitectura Beta", "beta cache", "El modulo beta contiene obsoleto_token_v1.", 1_010)
        .expect("write beta");
    vault
        .write_markdown_note(&mut gk, "claude_code", "notes/gamma.md", "Arquitectura Gamma", "gamma temp", "Nota efimera con unico_gamma_borrable_99.", 1_020)
        .expect("write gamma");

    let alpha_hash_before = vault.get_tracked_note("notes/alpha.md").expect("alpha").file_sha256.clone();
    let beta_hash_before = vault.get_tracked_note("notes/beta.md").expect("beta").file_sha256.clone();

    let sync_no_change = vault.sync_vault_directory(&mut gk, "claude_code", "notes", 2_000).expect("sync 1");
    assert_eq!(sync_no_change.scanned_files, 3);
    assert_eq!(sync_no_change.indexed_new_files, 0);
    assert_eq!(sync_no_change.reindexed_modified_files, 0);
    assert_eq!(sync_no_change.skipped_unchanged_files, 3);

    fs::write(
        sandbox.root_path().join("notes/beta.md"),
        b"# Arquitectura Beta\n<!-- miku-tags: beta cache v2 -->\n\nEl modulo beta ahora implementa sincronizacion_reactiva_v2.\n",
    )
    .expect("edit beta on disk");

    let sync_after_edit = vault.sync_vault_directory(&mut gk, "claude_code", "notes", 3_000).expect("sync 2");
    assert_eq!(sync_after_edit.scanned_files, 3);
    assert_eq!(sync_after_edit.indexed_new_files, 0);
    assert_eq!(sync_after_edit.reindexed_modified_files, 1);
    assert_eq!(sync_after_edit.skipped_unchanged_files, 2);

    assert_eq!(alpha_hash_before, vault.get_tracked_note("notes/alpha.md").unwrap().file_sha256);
    assert_ne!(beta_hash_before, vault.get_tracked_note("notes/beta.md").unwrap().file_sha256);

    assert!(vault.search_authorized(&mut gk, "claude_code", "obsoleto_token_v1", 5, 3_100).unwrap().is_empty());
    assert_eq!(vault.search_authorized(&mut gk, "claude_code", "sincronizacion_reactiva_v2", 5, 3_110).unwrap().len(), 1);

    fs::remove_file(sandbox.root_path().join("notes/gamma.md")).expect("delete gamma.md");
    let sync_after_delete = vault.sync_vault_directory(&mut gk, "claude_code", "notes", 4_000).expect("sync 3");
    assert_eq!(sync_after_delete.scanned_files, 2);
    assert_eq!(sync_after_delete.deleted_stale_files, 1);
    assert_eq!(sync_after_delete.reindexed_modified_files, 0);
    assert_eq!(sync_after_delete.skipped_unchanged_files, 2);
    assert!(vault.search_authorized(&mut gk, "claude_code", "unico_gamma_borrable_99", 5, 4_100).unwrap().is_empty());

    vault.storage().clear_fts5_documents().expect("wipe fts5");
    assert_eq!(vault.storage().fts5_document_count().expect("count"), 0);
    let rebuild_report = vault.rebuild_fts5_index_from_sources(&mut gk, "claude_code", "notes", 5_000).expect("rebuild");
    assert_eq!(rebuild_report.indexed_new_files, 2);
    assert_eq!(vault.search_authorized(&mut gk, "claude_code", "sincronizacion_reactiva_v2", 5, 5_100).unwrap().len(), 1);
}

// LEDGER-3.9-3: Multi-term BM25 ranking, empty/malformed queries, and FTS5 sanitization
#[test]
fn test_bm25_ranking_empty_malformed_queries_and_fts5_input_sanitization() {
    let sandbox = TempVaultSandbox::new("bm25_sanitization");
    let pool = SingleThreadStoragePool::new_in_memory().expect("pool");
    let mut gk = build_notes_gatekeeper(sandbox.canonical_root());
    let mut vault = MarkdownMemoryVault::new(sandbox.root_path(), pool, 24);

    vault
        .write_markdown_note(&mut gk, "claude_code", "notes/exact_match.md", "Motor FTS5 BM25 y Cache KV Estable", "fts5 bm25 cache_kv sqlite", "El motor FTS5 BM25 combina busqueda lexica con prefijo estable para cache KV.", 1_000)
        .expect("write doc 1");
    vault
        .write_markdown_note(&mut gk, "claude_code", "notes/partial_match.md", "Notas Generales de Base de Datos", "sqlite almacenamiento", "Incluye mencion breve a FTS5 dentro de una nota generalista.", 1_010)
        .expect("write doc 2");

    let multi_hits = vault.search_authorized(&mut gk, "claude_code", "fts5 bm25 cache_kv", 5, 2_000).expect("multi");
    assert_eq!(multi_hits.len(), 2);
    assert!(multi_hits[0].title.contains("exact_match.md"));
    assert!(multi_hits[0].bm25_score > multi_hits[1].bm25_score);

    for empty_or_op in ["", "   \t\n ", "a", "AND OR NOT NEAR", "; -- /* */"] {
        let hits = vault.search_authorized(&mut gk, "claude_code", empty_or_op, 5, 2_100).expect("empty query");
        assert!(hits.is_empty(), "expected empty hits for '{}'", empty_or_op);
    }

    let hostile_queries = [
        r#"fts5 "unclosed_quote * ^ NEAR/3 ( OR DROP TABLE memory_fts5; --"#,
        "bm25'); DELETE FROM sessions; --",
        "sqlite\0\x01\x07 AND NOT *",
        "((([[[{{{ fts5 }}}]]])))",
    ];
    for hostile in hostile_queries {
        assert!(vault.search_authorized(&mut gk, "claude_code", hostile, 5, 2_200).is_ok());
    }
    assert!(!vault.search_authorized(&mut gk, "claude_code", "fts5 bm25", 5, 2_300).unwrap().is_empty());
}

// LEDGER-3.9-4: Gatekeeper enforces Capability::FsRead / FsWrite on every file;
//               when Gatekeeper denies, zero files are read and zero results returned
#[test]
fn test_gatekeeper_authorizes_every_markdown_read_and_blocks_denied_paths_with_zero_leaks() {
    let sandbox = TempVaultSandbox::new("gatekeeper_isolation");
    fs::create_dir_all(sandbox.root_path().join("notes/public")).expect("public dir");
    fs::create_dir_all(sandbox.root_path().join("notes/private")).expect("private dir");
    fs::write(
        sandbox.root_path().join("notes/private/forbidden_vault.md"),
        b"# Claves Privadas\nContenido restringido token_privado_restringido_777.\n",
    )
    .expect("seed private file");

    let pool = SingleThreadStoragePool::new_in_memory().expect("pool");
    let profile = ProjectProfile::new("miku-3-9", sandbox.canonical_root(), 60_000)
        .with_rule(Capability::FsRead, "notes/public/**", PolicyEffect::Allow, false)
        .with_rule(Capability::FsWrite, "notes/public/**", PolicyEffect::Allow, false)
        .with_rule(Capability::FsRead, "notes/private/**", PolicyEffect::Deny, true)
        .with_rule(Capability::FsWrite, "notes/private/**", PolicyEffect::Deny, true);

    let mut gk = GatekeeperEngine::new(profile).expect("gk");
    let mut vault = MarkdownMemoryVault::new(sandbox.root_path(), pool, 24);

    let reads_before = vault.disk_reads_performed();
    let denied_sync = vault.sync_markdown_file(&mut gk, "claude_code", "notes/private/forbidden_vault.md", "private", 1_000);
    assert!(matches!(denied_sync, Err(MemoryError::GatekeeperDenied { reason: DenyReason::ExplicitDenyRule, .. })));
    assert_eq!(vault.disk_reads_performed(), reads_before);

    let traversal_sync = vault.sync_markdown_file(&mut gk, "claude_code", "../../etc/outside.md", "escape", 1_010);
    assert!(matches!(traversal_sync, Err(MemoryError::GatekeeperDenied { reason: DenyReason::PathJailViolation, .. })));
    assert_eq!(vault.disk_reads_performed(), reads_before);

    vault
        .write_markdown_note(&mut gk, "claude_code", "notes/public/roadmap.md", "Roadmap Publico", "roadmap", "Objetivo verificable de arquitectura limpia.", 1_020)
        .expect("write public note");
    assert_eq!(vault.search_authorized(&mut gk, "claude_code", "arquitectura limpia", 5, 1_030).unwrap().len(), 1);

    let locked_profile = ProjectProfile::new("miku-3-9-locked", sandbox.canonical_root(), 60_000)
        .with_rule(Capability::FsRead, "notes/public/**", PolicyEffect::Deny, true);
    let mut locked_gk = GatekeeperEngine::new(locked_profile).expect("locked gk");
    let blocked_hits = vault.search_authorized(&mut locked_gk, "claude_code", "arquitectura limpia", 5, 1_040).expect("locked search");
    assert!(blocked_hits.is_empty());
}

// LEDGER-3.9-5: On-demand selective retrieval injects ONLY relevant fragments
//               and strictly respects the context token budget
#[test]
fn test_on_demand_selective_retrieval_injects_only_relevant_fragments_within_token_budget() {
    let sandbox = TempVaultSandbox::new("selective_rag_budget");
    let pool = SingleThreadStoragePool::new_in_memory().expect("pool");
    pool.insert_session(SessionRecord {
        session_id: "sess_rag".to_string(),
        agent_name: "claude_code".to_string(),
        project_root: sandbox.canonical_root().to_string(),
        created_at_ms: 1_000,
        state: "active".to_string(),
    })
    .expect("session");

    let mut gk = build_notes_gatekeeper(sandbox.canonical_root());
    let mut vault = MarkdownMemoryVault::new(sandbox.root_path(), pool, 24);

    for i in 0..40_usize {
        let rel = format!("notes/topic_{:02}.md", i);
        let (title, tags, body) = if i == 7 {
            (
                "Protocolo de Tunel Miku-Link".to_string(),
                "miku_link tunel quic seguridad".to_string(),
                "El tunel miku_link exige emparejamiento explicito con token firmado y loopback por defecto.".to_string(),
            )
        } else {
            (
                format!("Documento Irrelevante {:02}", i),
                format!("ruido_{:02}", i),
                format!("Contenido de relleno sobre subsistema secundario numero {}.", i),
            )
        };
        vault.write_markdown_note(&mut gk, "claude_code", &rel, &title, &tags, &body, 1_100 + i as u64).expect("write");
    }

    let prefix = StaticSystemPrefix::new(
        "claude_code",
        "Eres el arquitecto principal de Miku Desktop Orchestrator.",
        "fs_read, fs_write, memory_search",
    );

    let budget_tokens = 180_u32;
    let res = vault
        .retrieve_on_demand_within_budget(&mut gk, "claude_code", &prefix, "sess_rag", "Explica el emparejamiento del tunel miku_link", 3, budget_tokens, 2_000)
        .expect("selective retrieval");

    assert!(res.query_requires_memory);
    assert!(!res.selected_hits.is_empty());
    assert!(res.selected_hits.len() <= 3);
    assert!(res.assembled_prompt.retained_chunks_count >= 1 && res.assembled_prompt.retained_chunks_count <= 3);
    assert!(res.assembled_prompt.total_tokens <= budget_tokens);
    assert!(res.assembled_prompt.dynamic_tail.contains("notes/topic_07.md"));
    assert!(!res.assembled_prompt.dynamic_tail.contains("notes/topic_39.md"));
    assert_eq!(res.assembled_prompt.prefix_sha256, prefix.prefix_sha256);

    let casual = vault
        .retrieve_on_demand_within_budget(&mut gk, "claude_code", &prefix, "sess_rag", "hola", 3, budget_tokens, 2_010)
        .expect("casual retrieval");
    assert!(!casual.query_requires_memory);
    assert_eq!(casual.selected_hits.len(), 0);
    assert_eq!(casual.assembled_prompt.rag_tokens, 0);
}

// LEDGER-3.9-6: Island search flow: Ctrl+K -> IPC -> FTS5 search -> bounded
//               results in the Island without returning the full corpus
#[test]
fn test_ctrl_k_island_ipc_search_flow_returns_bounded_results_without_full_corpus() {
    let sandbox = TempVaultSandbox::new("ctrl_k_ipc");
    let pool = SingleThreadStoragePool::new_in_memory().expect("pool");
    pool.insert_session(SessionRecord {
        session_id: "sess_island_01".to_string(),
        agent_name: "claude_code".to_string(),
        project_root: sandbox.canonical_root().to_string(),
        created_at_ms: 1_000,
        state: "active".to_string(),
    })
    .expect("session");
    pool.insert_turn(ConversationTurnRecord {
        turn_id: "turn_island_01".to_string(),
        session_id: "sess_island_01".to_string(),
        seq: 1,
        role: "assistant".to_string(),
        content: "Se aprobo el ticket de arquitectura_island_fts5 con verificacion SHA-256.".to_string(),
        token_estimate: 14,
        ts_ms: 1_050,
    })
    .expect("turn");

    let mut gk = build_notes_gatekeeper(sandbox.canonical_root());
    let mut vault = MarkdownMemoryVault::new(sandbox.root_path(), pool, 24);

    for idx in 0..60_usize {
        let rel = format!("notes/island_doc_{:02}.md", idx);
        vault
            .write_markdown_note(
                &mut gk,
                "claude_code",
                &rel,
                &format!("Nota Buscador Island {:02}", idx),
                "arquitectura_island_fts5 ctrl_k",
                &format!("Fragmento indexado numero {} para arquitectura_island_fts5.", idx),
                1_100 + idx as u64,
            )
            .expect("write note");
    }
    vault.index_all_sessions(1_200).expect("index sessions");

    let mut controller = IslandMemorySearchController::new();
    assert!(!controller.is_palette_open());
    assert!(controller.trigger_ctrl_k());
    assert!(controller.is_palette_open());

    let ipc_req = IslandMemorySearchIpcRequest {
        jsonrpc: JSONRPC_VERSION_2_0.to_string(),
        v: MIKU_PROTOCOL_VERSION_V1,
        id: "req-ctrl-k-01".to_string(),
        origin: "https://miku.local/index.html".to_string(),
        shortcut: CTRL_K_SHORTCUT_ACCEL.to_string(),
        raw_query: "arquitectura_island_fts5".to_string(),
        filter: IslandSearchFilter::All,
        limit: 999,
        agent_id: "claude_code".to_string(),
        timestamp_ms: 2_000,
    };
    let raw_req_json = serde_json::to_string(&ipc_req).expect("serialize req");

    let raw_resp_json = controller.handle_ipc_search_json(&raw_req_json, &mut vault, &mut gk).expect("ipc");
    let resp: crate::island_search_ipc::IslandMemorySearchIpcResponse =
        serde_json::from_str(&raw_resp_json).expect("deserialize resp");

    assert_eq!(resp.reply_to, "req-ctrl-k-01");
    assert!(resp.palette_open);
    assert_eq!(resp.total_indexed_corpus_docs, 61);
    assert_eq!(resp.returned_count, HARD_MAX_ISLAND_SEARCH_RESULTS);
    assert_eq!(resp.results.len(), HARD_MAX_ISLAND_SEARCH_RESULTS);
    assert!(resp.truncated_to_limit);
    assert!(resp.search_latency_ms < 10.0);

    let sess_req = IslandMemorySearchIpcRequest {
        id: "req-ctrl-k-02".to_string(),
        filter: IslandSearchFilter::SessionsOnly,
        limit: 8,
        ..ipc_req.clone()
    };
    let sess_resp = controller.execute_search_request(&sess_req, &mut vault, &mut gk).expect("sessions");
    assert_eq!(sess_resp.returned_count, 1);
    assert_eq!(sess_resp.results[0].source_kind, IslandSearchSourceKind::SessionHistory);

    let untrusted_req = IslandMemorySearchIpcRequest {
        origin: "https://evil.example.org".to_string(),
        ..ipc_req
    };
    let untrusted_err = controller.execute_search_request(&untrusted_req, &mut vault, &mut gk).expect_err("origin");
    assert_eq!(untrusted_err, IslandSearchIpcError::UnauthorizedOrigin);

    controller.close_on_escape();
    assert!(!controller.is_palette_open());
}
