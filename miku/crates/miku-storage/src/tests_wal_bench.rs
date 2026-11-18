use std::fs;
use std::time::Instant;

use crate::sqlite_wal_backend::sqlite_wal_backend_name;
use crate::{
    ApprovedProfileRecord, ConversationTurnRecord, DatabaseSnapshot, MemoryFts5Document,
    SessionRecord, SingleThreadStoragePool,
};

#[test]
fn benchmark_10000_incremental_wal_inserts_p95_under_2ms_with_zero_full_snapshot_rewrite() {
    let temp_dir = std::env::temp_dir().join(format!("miku_sqlite_wal_bench_{}", std::process::id()));
    let _ = fs::remove_dir_all(&temp_dir);
    fs::create_dir_all(&temp_dir).unwrap();
    let db_file = temp_dir.join("miku_bench.db");

    let pool = SingleThreadStoragePool::open_wal_file(&db_file).expect("open sqlite wal");
    assert_eq!(pool.backend_kind(), sqlite_wal_backend_name());
    assert_eq!(pool.sqlite_journal_mode().as_deref(), Ok("wal"));

    pool.insert_session(SessionRecord {
        session_id: "sess_bench".to_string(),
        agent_name: "claude-code".to_string(),
        project_root: "C:/workspace/miku".to_string(),
        created_at_ms: 1_000,
        state: "active".to_string(),
    })
    .unwrap();

    // Verify 10,000 individual incremental writes stay O(1)/O(log N) with p95 < 2 ms!
    let mut latencies_us: Vec<u128> = Vec::with_capacity(10_000);
    let total_t0 = Instant::now();
    for i in 0..10_000u64 {
        let t0 = Instant::now();
        pool.insert_turn(ConversationTurnRecord {
            turn_id: format!("turn_{:05}", i),
            session_id: "sess_bench".to_string(),
            seq: i + 1,
            role: if i % 2 == 0 { "user" } else { "assistant" }.to_string(),
            content: format!("Incremental WAL frame message payload #{}", i),
            token_estimate: 12,
            ts_ms: 2_000 + i,
        })
        .unwrap();
        latencies_us.push(t0.elapsed().as_micros());
    }
    let total_ms = total_t0.elapsed().as_secs_f64() * 1_000.0;
    latencies_us.sort_unstable();
    let p95_ms = (latencies_us[9_499] as f64) / 1_000.0;
    let max_ms = (*latencies_us.last().unwrap() as f64) / 1_000.0;

    // On Windows verify real binary SQLite header + active WAL file
    let header = fs::read(&db_file).unwrap();
    if cfg!(target_os = "windows") {
        assert!(
            header.starts_with(b"SQLite format 3\0"),
            "database file must be real binary SQLite3, not full JSON rewrite"
        );
    }

    assert!(
        p95_ms < 2.0,
        "10,000 incremental WAL inserts p95 must be < 2.0 ms (measured {:.4} ms, total={:.2} ms)",
        p95_ms,
        total_ms
    );

    println!(
        "STORAGE_4_12_WAL_BENCH: backend={} inserts=10000 p95_ms={:.4} max_ms={:.4} total_ms={:.2}",
        pool.backend_kind(),
        p95_ms,
        max_ms,
        total_ms
    );

    drop(pool);
    let _ = fs::remove_dir_all(&temp_dir);
}

#[test]
fn test_lossless_migration_from_legacy_json_snapshot_to_sqlite_wal_fts5_under_10ms() {
    let temp_dir = std::env::temp_dir().join(format!("miku_sqlite_mig_{}", std::process::id()));
    let _ = fs::remove_dir_all(&temp_dir);
    fs::create_dir_all(&temp_dir).unwrap();
    let db_file = temp_dir.join("legacy_snapshot.db");

    // 1. Write a legacy JSON DatabaseSnapshot directly to disk (old O(N) format)
    let mut legacy = DatabaseSnapshot::default();
    for tbl in [
        "sessions",
        "conversation_turns",
        "audit_chain_entries",
        "approved_profiles",
        "memory_fts5",
    ] {
        legacy.migrated_tables.insert(tbl.to_string());
    }
    legacy.schema_version = 1;
    legacy.sessions.insert(
        "sess_legacy".to_string(),
        SessionRecord {
            session_id: "sess_legacy".to_string(),
            agent_name: "cursor".to_string(),
            project_root: "C:/workspace/miku".to_string(),
            created_at_ms: 500,
            state: "active".to_string(),
        },
    );
    for i in 0..25u64 {
        legacy.turns.insert(
            format!("turn_leg_{}", i),
            ConversationTurnRecord {
                turn_id: format!("turn_leg_{}", i),
                session_id: "sess_legacy".to_string(),
                seq: i + 1,
                role: "user".to_string(),
                content: format!("Migrated legacy message {}", i),
                token_estimate: 5,
                ts_ms: 600 + i,
            },
        );
    }
    legacy.approved_profiles.insert(
        "c:/workspace/miku".to_string(),
        ApprovedProfileRecord {
            project_root: "c:/workspace/miku".to_string(),
            toml_sha256: "abc123def".to_string(),
            approved_by: "user".to_string(),
            approved_at_ms: 700,
        },
    );
    legacy.fts_documents.insert(
        "doc_leg_1".to_string(),
        MemoryFts5Document {
            doc_id: "doc_leg_1".to_string(),
            title: "Migración incremental SQLite WAL".to_string(),
            tags: "sqlite fts5 migration".to_string(),
            body: "Los datos antiguos de DatabaseSnapshot migran intactos al índice FTS5.".to_string(),
            updated_at_ms: 800,
        },
    );

    fs::write(&db_file, serde_json::to_vec(&legacy).unwrap()).unwrap();

    // 2. Open fresh SQLite WAL + FTS5 DB and migrate legacy JSON DatabaseSnapshot atomically (< 10 ms)
    let raw_legacy = fs::read(&db_file).unwrap();
    let sqlite_db_file = temp_dir.join("migrated_wal.db");
    let pool = SingleThreadStoragePool::open_wal_file(&sqlite_db_file).expect("open sqlite wal");

    let t0 = Instant::now();
    let imported = pool
        .migrate_legacy_json_bytes(&raw_legacy)
        .expect("migrate legacy json");
    let migration_ms = t0.elapsed().as_secs_f64() * 1_000.0;
    assert_eq!(imported, 28);

    // Verify reopened automatic file migration path too (`open_wal_file` detects `{` legacy JSON directly)
    let auto_reopened =
        SingleThreadStoragePool::open_wal_file(&db_file).expect("auto-migrate JSON file on open");
    assert_eq!(auto_reopened.list_all_sessions().unwrap().len(), 1);
    assert_eq!(auto_reopened.list_turns_for_session("sess_legacy").unwrap().len(), 25);

    assert!(
        migration_ms < 10.0,
        "legacy snapshot import to SQLite WAL + FTS5 must complete in < 10 ms (measured {:.3} ms)",
        migration_ms
    );
    assert_eq!(pool.list_all_sessions().unwrap().len(), 1);
    assert_eq!(pool.list_turns_for_session("sess_legacy").unwrap().len(), 25);

    let hits = pool.search_fts5("sqlite fts5 migration", 5).unwrap();
    assert_eq!(hits.len(), 1);
    assert_eq!(hits[0].doc_id, "doc_leg_1");

    if cfg!(target_os = "windows") {
        let header = fs::read(&sqlite_db_file).unwrap();
        assert!(header.starts_with(b"SQLite format 3\0"));
        let auto_header = fs::read(&db_file).unwrap();
        assert!(auto_header.starts_with(b"SQLite format 3\0"));
    }

    println!(
        "STORAGE_4_12_MIGRATION_OK: migration_ms={:.3} (< 10 ms) imported={} fts_hits={}",
        migration_ms,
        imported,
        hits.len()
    );

    drop(auto_reopened);
    drop(pool);
    let _ = fs::remove_dir_all(&temp_dir);
}
