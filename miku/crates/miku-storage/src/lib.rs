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

use std::cell::RefCell;
use std::collections::{BTreeMap, BTreeSet};
use std::fs::{self, OpenOptions};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::rc::Rc;

use miku_gatekeeper::{AuditEntry, GENESIS_AUDIT_HASH};
use miku_protocol::sha256_hex;
use serde::{Deserialize, Serialize};

pub const DEFAULT_BUSY_TIMEOUT_MS: u32 = 5_000;
pub const SCHEMA_VERSION_V1: u32 = 1;

pub const SQL_PRAGMA_INIT_WAL: &str = r##"
PRAGMA journal_mode = WAL;
PRAGMA synchronous = NORMAL;
PRAGMA foreign_keys = ON;
PRAGMA busy_timeout = 5000;
PRAGMA temp_store = MEMORY;
"##;

pub const SQL_MIGRATION_V1_DDL: &[(&str, &str)] = &[
    (
        "sessions",
        r##"CREATE TABLE IF NOT EXISTS sessions (
            session_id TEXT PRIMARY KEY NOT NULL,
            agent_name TEXT NOT NULL,
            project_root TEXT NOT NULL,
            created_at_ms INTEGER NOT NULL,
            state TEXT NOT NULL
        ) STRICT;"##,
    ),
    (
        "conversation_turns",
        r##"CREATE TABLE IF NOT EXISTS conversation_turns (
            turn_id TEXT PRIMARY KEY NOT NULL,
            session_id TEXT NOT NULL,
            seq INTEGER NOT NULL,
            role TEXT NOT NULL,
            content TEXT NOT NULL,
            token_estimate INTEGER NOT NULL,
            ts_ms INTEGER NOT NULL,
            FOREIGN KEY (session_id) REFERENCES sessions(session_id) ON DELETE CASCADE
        ) STRICT;"##,
    ),
    (
        "audit_chain_entries",
        r##"CREATE TABLE IF NOT EXISTS audit_chain_entries (
            seq INTEGER PRIMARY KEY NOT NULL,
            timestamp_ms INTEGER NOT NULL,
            agent TEXT NOT NULL,
            capability TEXT NOT NULL,
            target TEXT NOT NULL,
            verdict TEXT NOT NULL,
            reason TEXT NOT NULL,
            prev_hash TEXT NOT NULL,
            entry_hash TEXT UNIQUE NOT NULL
        ) STRICT;"##,
    ),
    (
        "approved_profiles",
        r##"CREATE TABLE IF NOT EXISTS approved_profiles (
            project_root TEXT PRIMARY KEY NOT NULL,
            toml_sha256 TEXT NOT NULL,
            approved_by TEXT NOT NULL,
            approved_at_ms INTEGER NOT NULL
        ) STRICT;"##,
    ),
    (
        "memory_fts5",
        r##"CREATE VIRTUAL TABLE IF NOT EXISTS memory_fts5 USING fts5(
            doc_id UNINDEXED,
            title,
            tags,
            body,
            updated_at_ms UNINDEXED,
            tokenize = 'unicode61 remove_diacritics 2'
        );"##,
    ),
];

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PragmaState {
    pub journal_mode: String,
    pub synchronous: String,
    pub foreign_keys: bool,
    pub busy_timeout_ms: u32,
    pub temp_store: String,
}

impl Default for PragmaState {
    fn default() -> Self {
        Self {
            journal_mode: "wal".to_string(),
            synchronous: "normal".to_string(),
            foreign_keys: true,
            busy_timeout_ms: DEFAULT_BUSY_TIMEOUT_MS,
            temp_store: "memory".to_string(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SessionRecord {
    pub session_id: String,
    pub agent_name: String,
    pub project_root: String,
    pub created_at_ms: u64,
    pub state: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ConversationTurnRecord {
    pub turn_id: String,
    pub session_id: String,
    pub seq: u64,
    pub role: String,
    pub content: String,
    pub token_estimate: u32,
    pub ts_ms: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ApprovedProfileRecord {
    pub project_root: String,
    pub toml_sha256: String,
    pub approved_by: String,
    pub approved_at_ms: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum ProfileApprovalStatus {
    Approved {
        toml_sha256: String,
        approved_by: String,
        approved_at_ms: u64,
    },
    UnapprovedFirstSeen {
        current_sha256: String,
    },
    InvalidatedHashMismatch {
        stored_sha256: String,
        current_sha256: String,
    },
}

impl ProfileApprovalStatus {
    pub fn is_approved(&self) -> bool {
        matches!(self, ProfileApprovalStatus::Approved { .. })
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MemoryFts5Document {
    pub doc_id: String,
    pub title: String,
    pub tags: String,
    pub body: String,
    pub updated_at_ms: u64,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Fts5SearchHit {
    pub doc_id: String,
    pub title: String,
    pub tags: String,
    pub bm25_score: f64,
    pub snippet: String,
    pub updated_at_ms: u64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum StorageError {
    InvalidPragma(String),
    MigrationNotInitialized(String),
    PrimaryKeyConflict { table: String, key: String },
    ForeignKeyViolation { table: String, parent_key: String },
    NotFound { table: String, key: String },
    AuditChainIntegrityError(String),
    ActiveTransactionAlreadyOpen,
    NoActiveTransaction,
    InvalidInput(String),
    IoError(String),
    CorruptedWalFrame(String),
}

impl std::fmt::Display for StorageError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            StorageError::InvalidPragma(msg) => write!(f, "invalid SQLite pragma: {}", msg),
            StorageError::MigrationNotInitialized(t) => {
                write!(f, "schema table '{}' is not migrated yet", t)
            }
            StorageError::PrimaryKeyConflict { table, key } => {
                write!(f, "UNIQUE/PRIMARY KEY constraint failed: {}.{}", table, key)
            }
            StorageError::ForeignKeyViolation { table, parent_key } => {
                write!(
                    f,
                    "FOREIGN KEY constraint failed on '{}' referencing '{}'",
                    table, parent_key
                )
            }
            StorageError::NotFound { table, key } => {
                write!(f, "record not found in '{}': '{}'", table, key)
            }
            StorageError::AuditChainIntegrityError(msg) => {
                write!(f, "audit chain SHA-256 integrity error: {}", msg)
            }
            StorageError::ActiveTransactionAlreadyOpen => {
                write!(f, "cannot start nested transaction on single-thread WAL connection")
            }
            StorageError::NoActiveTransaction => write!(f, "no active transaction to commit/rollback"),
            StorageError::InvalidInput(msg) => write!(f, "invalid storage input: {}", msg),
            StorageError::IoError(msg) => write!(f, "storage I/O error: {}", msg),
            StorageError::CorruptedWalFrame(msg) => write!(f, "corrupted WAL frame: {}", msg),
        }
    }
}

impl std::error::Error for StorageError {}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
struct DatabaseSnapshot {
    schema_version: u32,
    migrated_tables: BTreeSet<String>,
    sessions: BTreeMap<String, SessionRecord>,
    turns: BTreeMap<String, ConversationTurnRecord>,
    audit_entries: Vec<AuditEntry>,
    approved_profiles: BTreeMap<String, ApprovedProfileRecord>,
    fts_documents: BTreeMap<String, MemoryFts5Document>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct WalFrameRecord {
    frame_seq: u64,
    payload_sha256: String,
    snapshot: DatabaseSnapshot,
}

#[derive(Debug)]
struct StorageEngine {
    db_path: Option<PathBuf>,
    wal_path: Option<PathBuf>,
    pragmas: PragmaState,
    committed: DatabaseSnapshot,
    in_flight_tx: Option<DatabaseSnapshot>,
    wal_frame_seq: u64,
    migration_runs: u64,
}

impl StorageEngine {
    fn new_in_memory() -> Result<Self, StorageError> {
        let mut engine = Self {
            db_path: None,
            wal_path: None,
            pragmas: PragmaState {
                journal_mode: "delete".to_string(),
                synchronous: "full".to_string(),
                foreign_keys: false,
                busy_timeout_ms: 0,
                temp_store: "default".to_string(),
            },
            committed: DatabaseSnapshot::default(),
            in_flight_tx: None,
            wal_frame_seq: 0,
            migration_runs: 0,
        };
        engine.execute_pragma_sql(SQL_PRAGMA_INIT_WAL)?;
        Ok(engine)
    }

    fn open_file(path: &Path) -> Result<Self, StorageError> {
        if let Some(parent) = path.parent() {
            if !parent.as_os_str().is_empty() {
                fs::create_dir_all(parent).map_err(|e| StorageError::IoError(e.to_string()))?;
            }
        }
        let wal_path = PathBuf::from(format!("{}-wal", path.display()));
        let mut engine = Self::new_in_memory()?;
        engine.db_path = Some(path.to_path_buf());
        engine.wal_path = Some(wal_path.clone());

        if path.exists() {
            let mut raw = Vec::new();
            let mut file =
                fs::File::open(path).map_err(|e| StorageError::IoError(e.to_string()))?;
            file.read_to_end(&mut raw)
                .map_err(|e| StorageError::IoError(e.to_string()))?;
            if !raw.is_empty() {
                let snap: DatabaseSnapshot = serde_json::from_slice(&raw)
                    .map_err(|e| StorageError::CorruptedWalFrame(e.to_string()))?;
                engine.committed = snap;
            }
        }

        if wal_path.exists() {
            let raw =
                fs::read_to_string(&wal_path).map_err(|e| StorageError::IoError(e.to_string()))?;
            for line in raw.lines() {
                let trimmed = line.trim();
                if trimmed.is_empty() {
                    continue;
                }
                let frame: WalFrameRecord = serde_json::from_str(trimmed)
                    .map_err(|e| StorageError::CorruptedWalFrame(e.to_string()))?;
                let snapshot_bytes = serde_json::to_vec(&frame.snapshot)
                    .map_err(|e| StorageError::CorruptedWalFrame(e.to_string()))?;
                let computed = sha256_hex(&snapshot_bytes);
                if computed != frame.payload_sha256 {
                    return Err(StorageError::CorruptedWalFrame(format!(
                        "WAL frame {} SHA-256 mismatch: expected {}, got {}",
                        frame.frame_seq, frame.payload_sha256, computed
                    )));
                }
                engine.wal_frame_seq = frame.frame_seq;
                engine.committed = frame.snapshot;
            }
        }

        Ok(engine)
    }

    fn execute_pragma_sql(&mut self, sql_block: &str) -> Result<(), StorageError> {
        for raw_stmt in sql_block.split(';') {
            let stmt = raw_stmt.trim();
            if stmt.is_empty() {
                continue;
            }
            let lower = stmt.to_ascii_lowercase();
            let Some(rest) = lower.strip_prefix("pragma ") else {
                return Err(StorageError::InvalidPragma(stmt.to_string()));
            };
            let parts: Vec<&str> = rest.split('=').map(|s| s.trim()).collect();
            if parts.len() != 2 {
                return Err(StorageError::InvalidPragma(stmt.to_string()));
            }
            match (parts[0], parts[1]) {
                ("journal_mode", "wal") => self.pragmas.journal_mode = "wal".to_string(),
                ("synchronous", "normal") => self.pragmas.synchronous = "normal".to_string(),
                ("foreign_keys", "on") | ("foreign_keys", "1") => self.pragmas.foreign_keys = true,
                ("foreign_keys", "off") | ("foreign_keys", "0") => {
                    self.pragmas.foreign_keys = false
                }
                ("busy_timeout", val) => {
                    let parsed: u32 = val
                        .parse()
                        .map_err(|_| StorageError::InvalidPragma(stmt.to_string()))?;
                    self.pragmas.busy_timeout_ms = parsed;
                }
                ("temp_store", "memory") => self.pragmas.temp_store = "memory".to_string(),
                _ => return Err(StorageError::InvalidPragma(stmt.to_string())),
            }
        }
        Ok(())
    }

    fn run_migrations(&mut self) -> Result<u32, StorageError> {
        if self.pragmas.journal_mode != "wal" || !self.pragmas.foreign_keys {
            return Err(StorageError::InvalidPragma(
                "WAL mode and foreign_keys=ON must be active before migrations".to_string(),
            ));
        }
        let target = self.active_snapshot_mut();
        for (table_name, ddl) in SQL_MIGRATION_V1_DDL {
            if !ddl.contains("IF NOT EXISTS") {
                return Err(StorageError::InvalidInput(format!(
                    "DDL for {} must be idempotent (IF NOT EXISTS)",
                    table_name
                )));
            }
            target.migrated_tables.insert((*table_name).to_string());
        }
        target.schema_version = SCHEMA_VERSION_V1;
        self.migration_runs += 1;
        self.persist_wal_frame_if_needed()?;
        Ok(SCHEMA_VERSION_V1)
    }

    fn ensure_table_migrated(&self, table: &str) -> Result<(), StorageError> {
        let snap = self.active_snapshot();
        if !snap.migrated_tables.contains(table) {
            return Err(StorageError::MigrationNotInitialized(table.to_string()));
        }
        Ok(())
    }

    fn active_snapshot(&self) -> &DatabaseSnapshot {
        self.in_flight_tx.as_ref().unwrap_or(&self.committed)
    }

    fn active_snapshot_mut(&mut self) -> &mut DatabaseSnapshot {
        self.in_flight_tx.as_mut().unwrap_or(&mut self.committed)
    }

    fn persist_wal_frame_if_needed(&mut self) -> Result<(), StorageError> {
        if self.in_flight_tx.is_some() {
            return Ok(());
        }
        let Some(ref wal_path) = self.wal_path else {
            return Ok(());
        };
        self.wal_frame_seq += 1;
        let snapshot_bytes = serde_json::to_vec(&self.committed)
            .map_err(|e| StorageError::IoError(e.to_string()))?;
        let payload_sha256 = sha256_hex(&snapshot_bytes);
        let frame = WalFrameRecord {
            frame_seq: self.wal_frame_seq,
            payload_sha256,
            snapshot: self.committed.clone(),
        };
        let line =
            serde_json::to_string(&frame).map_err(|e| StorageError::IoError(e.to_string()))?;
        let mut file = OpenOptions::new()
            .create(true)
            .append(true)
            .open(wal_path)
            .map_err(|e| StorageError::IoError(e.to_string()))?;
        writeln!(file, "{}", line).map_err(|e| StorageError::IoError(e.to_string()))?;
        Ok(())
    }

    fn checkpoint_wal(&mut self) -> Result<(), StorageError> {
        if self.in_flight_tx.is_some() {
            return Err(StorageError::ActiveTransactionAlreadyOpen);
        }
        if let Some(ref db_path) = self.db_path {
            let bytes = serde_json::to_vec(&self.committed)
                .map_err(|e| StorageError::IoError(e.to_string()))?;
            fs::write(db_path, bytes).map_err(|e| StorageError::IoError(e.to_string()))?;
        }
        if let Some(ref wal_path) = self.wal_path {
            if wal_path.exists() {
                fs::write(wal_path, b"").map_err(|e| StorageError::IoError(e.to_string()))?;
            }
        }
        Ok(())
    }
}

#[derive(Clone)]
pub struct SingleThreadStoragePool {
    inner: Rc<RefCell<StorageEngine>>,
}

impl SingleThreadStoragePool {
    pub fn new_in_memory() -> Result<Self, StorageError> {
        let mut engine = StorageEngine::new_in_memory()?;
        engine.run_migrations()?;
        Ok(Self {
            inner: Rc::new(RefCell::new(engine)),
        })
    }

    pub fn open_wal_file(path: impl AsRef<Path>) -> Result<Self, StorageError> {
        let mut engine = StorageEngine::open_file(path.as_ref())?;
        engine.run_migrations()?;
        Ok(Self {
            inner: Rc::new(RefCell::new(engine)),
        })
    }

    pub fn pragma_state(&self) -> PragmaState {
        self.inner.borrow().pragmas.clone()
    }

    pub fn run_migrations(&self) -> Result<u32, StorageError> {
        self.inner.borrow_mut().run_migrations()
    }

    pub fn migration_run_count(&self) -> u64 {
        self.inner.borrow().migration_runs
    }

    pub fn migrated_table_names(&self) -> Vec<String> {
        self.inner
            .borrow()
            .active_snapshot()
            .migrated_tables
            .iter()
            .cloned()
            .collect()
    }

    pub fn wal_frame_seq(&self) -> u64 {
        self.inner.borrow().wal_frame_seq
    }

    pub fn checkpoint_wal(&self) -> Result<(), StorageError> {
        self.inner.borrow_mut().checkpoint_wal()
    }

    pub fn begin_transaction(&self) -> Result<TransactionGuard, StorageError> {
        {
            let mut eng = self.inner.borrow_mut();
            if eng.in_flight_tx.is_some() {
                return Err(StorageError::ActiveTransactionAlreadyOpen);
            }
            eng.in_flight_tx = Some(eng.committed.clone());
        }
        Ok(TransactionGuard {
            pool: self.clone(),
            committed: false,
        })
    }

    pub fn insert_session(&self, record: SessionRecord) -> Result<(), StorageError> {
        if record.session_id.trim().is_empty() || record.agent_name.trim().is_empty() {
            return Err(StorageError::InvalidInput(
                "session_id and agent_name must not be empty".to_string(),
            ));
        }
        let mut eng = self.inner.borrow_mut();
        eng.ensure_table_migrated("sessions")?;
        let snap = eng.active_snapshot_mut();
        if snap.sessions.contains_key(&record.session_id) {
            return Err(StorageError::PrimaryKeyConflict {
                table: "sessions".to_string(),
                key: record.session_id,
            });
        }
        snap.sessions.insert(record.session_id.clone(), record);
        eng.persist_wal_frame_if_needed()
    }

    pub fn get_session(&self, session_id: &str) -> Result<Option<SessionRecord>, StorageError> {
        let eng = self.inner.borrow();
        eng.ensure_table_migrated("sessions")?;
        Ok(eng.active_snapshot().sessions.get(session_id).cloned())
    }

    pub fn list_all_sessions(&self) -> Result<Vec<SessionRecord>, StorageError> {
        let eng = self.inner.borrow();
        eng.ensure_table_migrated("sessions")?;
        Ok(eng
            .active_snapshot()
            .sessions
            .values()
            .cloned()
            .collect())
    }

    pub fn delete_session(&self, session_id: &str) -> Result<usize, StorageError> {
        let mut eng = self.inner.borrow_mut();
        eng.ensure_table_migrated("sessions")?;
        eng.ensure_table_migrated("conversation_turns")?;
        let foreign_keys_on = eng.pragmas.foreign_keys;
        let snap = eng.active_snapshot_mut();
        if snap.sessions.remove(session_id).is_none() {
            return Err(StorageError::NotFound {
                table: "sessions".to_string(),
                key: session_id.to_string(),
            });
        }
        let mut cascaded = 0usize;
        if foreign_keys_on {
            let before = snap.turns.len();
            snap.turns.retain(|_, turn| turn.session_id != session_id);
            cascaded = before.saturating_sub(snap.turns.len());
        }
        eng.persist_wal_frame_if_needed()?;
        Ok(cascaded)
    }

    pub fn insert_turn(&self, turn: ConversationTurnRecord) -> Result<(), StorageError> {
        if turn.turn_id.trim().is_empty() {
            return Err(StorageError::InvalidInput(
                "turn_id must not be empty".to_string(),
            ));
        }
        let mut eng = self.inner.borrow_mut();
        eng.ensure_table_migrated("sessions")?;
        eng.ensure_table_migrated("conversation_turns")?;
        let fk_on = eng.pragmas.foreign_keys;
        let snap = eng.active_snapshot_mut();
        if fk_on && !snap.sessions.contains_key(&turn.session_id) {
            return Err(StorageError::ForeignKeyViolation {
                table: "conversation_turns".to_string(),
                parent_key: turn.session_id,
            });
        }
        if snap.turns.contains_key(&turn.turn_id) {
            return Err(StorageError::PrimaryKeyConflict {
                table: "conversation_turns".to_string(),
                key: turn.turn_id,
            });
        }
        snap.turns.insert(turn.turn_id.clone(), turn);
        eng.persist_wal_frame_if_needed()
    }

    pub fn list_turns_for_session(
        &self,
        session_id: &str,
    ) -> Result<Vec<ConversationTurnRecord>, StorageError> {
        let eng = self.inner.borrow();
        eng.ensure_table_migrated("conversation_turns")?;
        let mut list: Vec<ConversationTurnRecord> = eng
            .active_snapshot()
            .turns
            .values()
            .filter(|t| t.session_id == session_id)
            .cloned()
            .collect();
        list.sort_by_key(|t| (t.seq, t.turn_id.clone()));
        Ok(list)
    }

    pub fn append_audit_entry(&self, entry: AuditEntry) -> Result<(), StorageError> {
        let mut eng = self.inner.borrow_mut();
        eng.ensure_table_migrated("audit_chain_entries")?;
        let snap = eng.active_snapshot_mut();
        let expected_seq = (snap.audit_entries.len() as u64) + 1;
        if entry.seq != expected_seq {
            return Err(StorageError::AuditChainIntegrityError(format!(
                "expected seq {}, got {}",
                expected_seq, entry.seq
            )));
        }
        let expected_prev = snap
            .audit_entries
            .last()
            .map(|e| e.entry_hash.as_str())
            .unwrap_or(GENESIS_AUDIT_HASH);
        if entry.prev_hash != expected_prev {
            return Err(StorageError::AuditChainIntegrityError(format!(
                "prev_hash mismatch at seq {}: expected {}, got {}",
                entry.seq, expected_prev, entry.prev_hash
            )));
        }
        let recomputed = AuditEntry::compute_hash(
            &entry.prev_hash,
            entry.seq,
            entry.timestamp_ms,
            &entry.agent,
            &entry.capability,
            &entry.target,
            &entry.verdict,
            &entry.reason,
        );
        if recomputed != entry.entry_hash {
            return Err(StorageError::AuditChainIntegrityError(format!(
                "entry_hash mismatch at seq {}: expected {}, got {}",
                entry.seq, recomputed, entry.entry_hash
            )));
        }
        snap.audit_entries.push(entry);
        eng.persist_wal_frame_if_needed()
    }

    pub fn verify_persisted_audit_chain(&self) -> Result<usize, StorageError> {
        let eng = self.inner.borrow();
        eng.ensure_table_migrated("audit_chain_entries")?;
        let entries = &eng.active_snapshot().audit_entries;
        let mut expected_prev = GENESIS_AUDIT_HASH.to_string();
        for (idx, entry) in entries.iter().enumerate() {
            let expected_seq = (idx as u64) + 1;
            if entry.seq != expected_seq || entry.prev_hash != expected_prev {
                return Err(StorageError::AuditChainIntegrityError(format!(
                    "broken chain link at index {}",
                    idx
                )));
            }
            let recomputed = AuditEntry::compute_hash(
                &entry.prev_hash,
                entry.seq,
                entry.timestamp_ms,
                &entry.agent,
                &entry.capability,
                &entry.target,
                &entry.verdict,
                &entry.reason,
            );
            if recomputed != entry.entry_hash {
                return Err(StorageError::AuditChainIntegrityError(format!(
                    "tampered entry_hash at index {}",
                    idx
                )));
            }
            expected_prev = entry.entry_hash.clone();
        }
        Ok(entries.len())
    }

    pub fn evaluate_profile_approval(
        &self,
        project_root: &str,
        raw_miku_toml_bytes: &[u8],
    ) -> Result<ProfileApprovalStatus, StorageError> {
        let eng = self.inner.borrow();
        eng.ensure_table_migrated("approved_profiles")?;
        let canonical_key = normalize_project_root_key(project_root);
        let current_sha256 = sha256_hex(raw_miku_toml_bytes);
        match eng.active_snapshot().approved_profiles.get(&canonical_key) {
            None => Ok(ProfileApprovalStatus::UnapprovedFirstSeen { current_sha256 }),
            Some(record) => {
                if record.toml_sha256 == current_sha256 {
                    Ok(ProfileApprovalStatus::Approved {
                        toml_sha256: record.toml_sha256.clone(),
                        approved_by: record.approved_by.clone(),
                        approved_at_ms: record.approved_at_ms,
                    })
                } else {
                    Ok(ProfileApprovalStatus::InvalidatedHashMismatch {
                        stored_sha256: record.toml_sha256.clone(),
                        current_sha256,
                    })
                }
            }
        }
    }

    pub fn approve_profile_hash(
        &self,
        project_root: &str,
        raw_miku_toml_bytes: &[u8],
        approved_by: &str,
        approved_at_ms: u64,
    ) -> Result<ApprovedProfileRecord, StorageError> {
        if project_root.trim().is_empty() || approved_by.trim().is_empty() {
            return Err(StorageError::InvalidInput(
                "project_root and approved_by must not be empty".to_string(),
            ));
        }
        let mut eng = self.inner.borrow_mut();
        eng.ensure_table_migrated("approved_profiles")?;
        let canonical_key = normalize_project_root_key(project_root);
        let toml_sha256 = sha256_hex(raw_miku_toml_bytes);
        let rec = ApprovedProfileRecord {
            project_root: canonical_key.clone(),
            toml_sha256,
            approved_by: approved_by.to_string(),
            approved_at_ms,
        };
        eng.active_snapshot_mut()
            .approved_profiles
            .insert(canonical_key, rec.clone());
        eng.persist_wal_frame_if_needed()?;
        Ok(rec)
    }

    pub fn upsert_fts5_document(&self, doc: MemoryFts5Document) -> Result<(), StorageError> {
        if doc.doc_id.trim().is_empty() {
            return Err(StorageError::InvalidInput(
                "doc_id must not be empty".to_string(),
            ));
        }
        let mut eng = self.inner.borrow_mut();
        eng.ensure_table_migrated("memory_fts5")?;
        eng.active_snapshot_mut()
            .fts_documents
            .insert(doc.doc_id.clone(), doc);
        eng.persist_wal_frame_if_needed()
    }

    pub fn delete_fts5_document(&self, doc_id: &str) -> Result<bool, StorageError> {
        let mut eng = self.inner.borrow_mut();
        eng.ensure_table_migrated("memory_fts5")?;
        let removed = eng
            .active_snapshot_mut()
            .fts_documents
            .remove(doc_id)
            .is_some();
        if removed {
            eng.persist_wal_frame_if_needed()?;
        }
        Ok(removed)
    }

    pub fn clear_fts5_documents(&self) -> Result<usize, StorageError> {
        let mut eng = self.inner.borrow_mut();
        eng.ensure_table_migrated("memory_fts5")?;
        let count = eng.active_snapshot().fts_documents.len();
        if count > 0 {
            eng.active_snapshot_mut().fts_documents.clear();
            eng.persist_wal_frame_if_needed()?;
        }
        Ok(count)
    }

    pub fn fts5_document_count(&self) -> Result<usize, StorageError> {
        let eng = self.inner.borrow();
        eng.ensure_table_migrated("memory_fts5")?;
        Ok(eng.active_snapshot().fts_documents.len())
    }

    pub fn search_fts5(
        &self,
        raw_query: &str,
        limit: usize,
    ) -> Result<Vec<Fts5SearchHit>, StorageError> {
        self.search_fts5_filtered(raw_query, limit, |_| true)
    }

    pub fn search_fts5_filtered<F>(
        &self,
        raw_query: &str,
        limit: usize,
        mut accept_doc_id: F,
    ) -> Result<Vec<Fts5SearchHit>, StorageError>
    where
        F: FnMut(&str) -> bool,
    {
        let eng = self.inner.borrow();
        eng.ensure_table_migrated("memory_fts5")?;
        let query_terms = sanitize_fts5_query(raw_query);
        if query_terms.is_empty() || limit == 0 {
            return Ok(Vec::new());
        }

        let docs_map = &eng.active_snapshot().fts_documents;
        if docs_map.is_empty() {
            return Ok(Vec::new());
        }

        let n_docs = docs_map.len() as f64;
        let n_terms = query_terms.len();
        let k1 = 1.2_f64;
        let b = 0.75_f64;

        let mut scanned_docs: Vec<(&MemoryFts5Document, Vec<f64>, f64)> =
            Vec::with_capacity(docs_map.len());
        let mut df_counts: Vec<usize> = vec![0usize; n_terms];
        let mut total_weighted_len = 0.0_f64;
        let mut token_buf = String::with_capacity(64);
        let mut title_tfs = vec![0usize; n_terms];
        let mut tags_tfs = vec![0usize; n_terms];
        let mut body_tfs = vec![0usize; n_terms];

        for doc in docs_map.values() {
            let t_len = scan_lexical_field(&doc.title, &query_terms, &mut title_tfs, &mut token_buf);
            let g_len = scan_lexical_field(&doc.tags, &query_terms, &mut tags_tfs, &mut token_buf);
            let b_len = scan_lexical_field(&doc.body, &query_terms, &mut body_tfs, &mut token_buf);
            let weighted_len =
                ((t_len as f64) * 3.0 + (g_len as f64) * 2.0 + (b_len as f64)).max(1.0);
            total_weighted_len += weighted_len;

            let mut weighted_tfs = vec![0.0_f64; n_terms];
            let mut any_match = false;
            for idx in 0..n_terms {
                let wtf = (title_tfs[idx] as f64) * 3.0
                    + (tags_tfs[idx] as f64) * 2.0
                    + (body_tfs[idx] as f64) * 1.0;
                if wtf > 0.0 {
                    df_counts[idx] += 1;
                    weighted_tfs[idx] = wtf;
                    any_match = true;
                }
            }
            if any_match {
                scanned_docs.push((doc, weighted_tfs, weighted_len));
            }
        }

        let avgdl = (total_weighted_len / n_docs).max(1.0);

        let mut scored_docs: Vec<(&MemoryFts5Document, f64)> =
            Vec::with_capacity(scanned_docs.len());
        for (doc, weighted_tfs, doc_len) in scanned_docs {
            let mut score = 0.0_f64;
            let mut matched_terms = 0usize;

            for (idx, &weighted_tf) in weighted_tfs.iter().enumerate() {
                if weighted_tf > 0.0 {
                    matched_terms += 1;
                    let df = df_counts[idx] as f64;
                    let idf = ((n_docs - df + 0.5) / (df + 0.5) + 1.0).ln();
                    let denom = weighted_tf + k1 * (1.0 - b + b * (doc_len / avgdl));
                    score += idf * ((weighted_tf * (k1 + 1.0)) / denom);
                }
            }

            if matched_terms > 0 {
                // Boost documents that match more distinct query terms
                score *= 1.0 + (matched_terms as f64) * 0.25;
                scored_docs.push((doc, score));
            }
        }

        scored_docs.sort_by(|(doc_a, score_a), (doc_b, score_b)| {
            score_b
                .partial_cmp(score_a)
                .unwrap_or(std::cmp::Ordering::Equal)
                .then_with(|| doc_a.doc_id.cmp(&doc_b.doc_id))
        });

        let mut hits: Vec<Fts5SearchHit> = Vec::with_capacity(limit.min(scored_docs.len()));
        for (doc, score) in scored_docs {
            if !accept_doc_id(&doc.doc_id) {
                continue;
            }
            let snippet = build_highlighted_snippet(&doc.body, &query_terms);
            hits.push(Fts5SearchHit {
                doc_id: doc.doc_id.clone(),
                title: doc.title.clone(),
                tags: doc.tags.clone(),
                bm25_score: score,
                snippet,
                updated_at_ms: doc.updated_at_ms,
            });
            if hits.len() >= limit {
                break;
            }
        }
        Ok(hits)
    }
}

pub struct TransactionGuard {
    pool: SingleThreadStoragePool,
    committed: bool,
}

impl TransactionGuard {
    pub fn commit(mut self) -> Result<(), StorageError> {
        let mut eng = self.pool.inner.borrow_mut();
        let Some(staged) = eng.in_flight_tx.take() else {
            return Err(StorageError::NoActiveTransaction);
        };
        eng.committed = staged;
        self.committed = true;
        eng.persist_wal_frame_if_needed()
    }

    pub fn rollback(mut self) -> Result<(), StorageError> {
        let mut eng = self.pool.inner.borrow_mut();
        if eng.in_flight_tx.take().is_none() {
            return Err(StorageError::NoActiveTransaction);
        }
        self.committed = true;
        Ok(())
    }
}

impl Drop for TransactionGuard {
    fn drop(&mut self) {
        if !self.committed {
            if let Ok(mut eng) = self.pool.inner.try_borrow_mut() {
                let _ = eng.in_flight_tx.take();
            }
        }
    }
}

pub fn sanitize_fts5_query(raw: &str) -> Vec<String> {
    // Strip hostile SQL comment tails or statement separators before tokenizing
    let pre_cleaned = raw
        .split(';')
        .next()
        .unwrap_or("")
        .split("--")
        .next()
        .unwrap_or("")
        .split("/*")
        .next()
        .unwrap_or("");

    let reserved_fts_operators = ["and", "or", "not", "near", "drop", "table", "select", "delete"];
    let mut terms = Vec::new();
    for tok in tokenize_lexical(pre_cleaned) {
        if tok.len() < 2 || tok.len() > 64 {
            continue;
        }
        if reserved_fts_operators.contains(&tok.as_str()) {
            continue;
        }
        if !terms.contains(&tok) {
            terms.push(tok);
        }
    }
    terms
}

pub fn tokenize_lexical(input: &str) -> Vec<String> {
    let mut tokens = Vec::new();
    let mut current = String::new();

    for ch in input.chars() {
        let folded = fold_spanish_diacritic(ch);
        if folded.is_ascii_alphanumeric() || folded == '_' || folded == '-' {
            current.push(folded.to_ascii_lowercase());
        } else if !current.is_empty() {
            let trimmed = current.trim_matches(|c| c == '-' || c == '_').to_string();
            if !trimmed.is_empty() {
                tokens.push(trimmed);
            }
            current.clear();
        }
    }
    if !current.is_empty() {
        let trimmed = current.trim_matches(|c| c == '-' || c == '_').to_string();
        if !trimmed.is_empty() {
            tokens.push(trimmed);
        }
    }
    tokens
}

fn scan_lexical_field(
    input: &str,
    query_terms: &[String],
    term_counts: &mut [usize],
    token_buf: &mut String,
) -> usize {
    term_counts.fill(0);
    token_buf.clear();
    let mut total_tokens = 0usize;

    for ch in input.chars() {
        let folded = fold_spanish_diacritic(ch);
        if folded.is_ascii_alphanumeric() || folded == '_' || folded == '-' {
            token_buf.push(folded.to_ascii_lowercase());
        } else if !token_buf.is_empty() {
            let trimmed = token_buf.trim_matches(|c| c == '-' || c == '_');
            if !trimmed.is_empty() {
                total_tokens += 1;
                for (idx, term) in query_terms.iter().enumerate() {
                    if trimmed == term.as_str() {
                        term_counts[idx] += 1;
                    }
                }
            }
            token_buf.clear();
        }
    }
    if !token_buf.is_empty() {
        let trimmed = token_buf.trim_matches(|c| c == '-' || c == '_');
        if !trimmed.is_empty() {
            total_tokens += 1;
            for (idx, term) in query_terms.iter().enumerate() {
                if trimmed == term.as_str() {
                    term_counts[idx] += 1;
                }
            }
        }
        token_buf.clear();
    }
    total_tokens
}

fn fold_spanish_diacritic(ch: char) -> char {
    match ch {
        'á' | 'à' | 'ä' | 'â' | 'Á' | 'À' | 'Ä' | 'Â' => 'a',
        'é' | 'è' | 'ë' | 'ê' | 'É' | 'È' | 'Ë' | 'Ê' => 'e',
        'í' | 'ì' | 'ï' | 'î' | 'Í' | 'Ì' | 'Ï' | 'Î' => 'i',
        'ó' | 'ò' | 'ö' | 'ô' | 'Ó' | 'Ò' | 'Ö' | 'Ô' => 'o',
        'ú' | 'ù' | 'ü' | 'û' | 'Ú' | 'Ù' | 'Ü' | 'Û' => 'u',
        'ñ' | 'Ñ' => 'n',
        other => other,
    }
}

fn build_highlighted_snippet(body: &str, query_terms: &[String]) -> String {
    let mut words: Vec<String> = Vec::new();
    for raw_word in body.split_whitespace().take(28) {
        let clean_tokens = tokenize_lexical(raw_word);
        let matched = clean_tokens.iter().any(|t| query_terms.contains(t));
        if matched {
            words.push(format!("[{}]", raw_word));
        } else {
            words.push(raw_word.to_string());
        }
    }
    words.join(" ")
}

fn normalize_project_root_key(raw: &str) -> String {
    raw.trim()
        .replace('\\', "/")
        .trim_end_matches('/')
        .to_ascii_lowercase()
}

#[cfg(test)]
mod tests {
    use super::*;

    // LEDGER-1.5-1
    #[test]
    fn test_wal_mode_pragmas_and_idempotent_schema_migrations() {
        let pool = SingleThreadStoragePool::new_in_memory().expect("in-memory WAL pool");
        let pragmas = pool.pragma_state();
        assert_eq!(pragmas.journal_mode, "wal");
        assert_eq!(pragmas.synchronous, "normal");
        assert!(pragmas.foreign_keys);
        assert_eq!(pragmas.busy_timeout_ms, 5_000);
        assert_eq!(pragmas.temp_store, "memory");

        // Running migrations again is 100% idempotent
        let v_first = pool.run_migrations().expect("second migration run");
        let v_second = pool.run_migrations().expect("third migration run");
        assert_eq!(v_first, SCHEMA_VERSION_V1);
        assert_eq!(v_second, SCHEMA_VERSION_V1);
        assert_eq!(pool.migration_run_count(), 3);

        let tables = pool.migrated_table_names();
        assert_eq!(
            tables,
            vec![
                "approved_profiles".to_string(),
                "audit_chain_entries".to_string(),
                "conversation_turns".to_string(),
                "memory_fts5".to_string(),
                "sessions".to_string(),
            ]
        );
    }

    // LEDGER-1.5-2
    #[test]
    fn test_sessions_turns_and_audit_chain_transactional_crud_and_rollback() {
        let pool = SingleThreadStoragePool::new_in_memory().expect("pool init");

        // 1. Foreign key protection rejects orphan turn before session exists
        let orphan_turn = ConversationTurnRecord {
            turn_id: "turn_orphan".to_string(),
            session_id: "sess_missing".to_string(),
            seq: 1,
            role: "user".to_string(),
            content: "hola miku".to_string(),
            token_estimate: 4,
            ts_ms: 1_000,
        };
        let fk_err = pool
            .insert_turn(orphan_turn)
            .expect_err("must reject orphan turn");
        assert!(matches!(fk_err, StorageError::ForeignKeyViolation { .. }));

        // 2. Commit transaction with session + 2 turns + SHA-256 audit chain
        {
            let tx = pool.begin_transaction().expect("begin tx 1");
            pool.insert_session(SessionRecord {
                session_id: "sess_01".to_string(),
                agent_name: "claude_code".to_string(),
                project_root: "C:/workspace/miku".to_string(),
                created_at_ms: 1_000,
                state: "active".to_string(),
            })
            .expect("insert session");

            pool.insert_turn(ConversationTurnRecord {
                turn_id: "turn_01".to_string(),
                session_id: "sess_01".to_string(),
                seq: 1,
                role: "user".to_string(),
                content: "Ejecuta cargo test en miku-storage".to_string(),
                token_estimate: 8,
                ts_ms: 1_010,
            })
            .expect("insert turn 1");

            pool.insert_turn(ConversationTurnRecord {
                turn_id: "turn_02".to_string(),
                session_id: "sess_01".to_string(),
                seq: 2,
                role: "assistant".to_string(),
                content: "Todos los tests de miku-storage pasaron en verde".to_string(),
                token_estimate: 10,
                ts_ms: 1_020,
            })
            .expect("insert turn 2");

            let entry_1_hash = AuditEntry::compute_hash(
                GENESIS_AUDIT_HASH,
                1,
                1_030,
                "claude_code",
                "fs_read",
                "src/lib.rs",
                "allow",
                "allow_rule",
            );
            pool.append_audit_entry(AuditEntry {
                seq: 1,
                timestamp_ms: 1_030,
                agent: "claude_code".to_string(),
                capability: "fs_read".to_string(),
                target: "src/lib.rs".to_string(),
                verdict: "allow".to_string(),
                reason: "allow_rule".to_string(),
                prev_hash: GENESIS_AUDIT_HASH.to_string(),
                entry_hash: entry_1_hash,
            })
            .expect("append valid audit entry");

            tx.commit().expect("commit tx 1");
        }

        // 3. Uncommitted transaction rolls back automatically on Drop (RAII)
        {
            let _tx_aborted = pool.begin_transaction().expect("begin tx 2");
            pool.insert_session(SessionRecord {
                session_id: "sess_rolled_back".to_string(),
                agent_name: "cursor".to_string(),
                project_root: "C:/workspace/miku".to_string(),
                created_at_ms: 2_000,
                state: "active".to_string(),
            })
            .expect("staged session");
            // Drop without commit()
        }
        assert!(pool
            .get_session("sess_rolled_back")
            .expect("query rolled back")
            .is_none());

        // 4. Verify audit chain and ON DELETE CASCADE
        let verified_count = pool
            .verify_persisted_audit_chain()
            .expect("audit chain valid");
        assert_eq!(verified_count, 1);

        let turns_before = pool.list_turns_for_session("sess_01").expect("list turns");
        assert_eq!(turns_before.len(), 2);

        let cascaded_deleted = pool.delete_session("sess_01").expect("delete session");
        assert_eq!(cascaded_deleted, 2);
        let turns_after = pool
            .list_turns_for_session("sess_01")
            .expect("list turns after delete");
        assert!(turns_after.is_empty());
    }

    // LEDGER-1.5-3
    #[test]
    fn test_fts5_lexical_memory_search_ranking_and_query_sanitization() {
        let pool = SingleThreadStoragePool::new_in_memory().expect("pool init");

        pool.upsert_fts5_document(MemoryFts5Document {
            doc_id: "doc_gatekeeper".to_string(),
            title: "Arquitectura de Gatekeeper y cadena SHA-256".to_string(),
            tags: "seguridad gatekeeper sha256 toctou".to_string(),
            body: r##"El motor miku-gatekeeper aplica política deny-by-default con cadena SHA-256 y protección anti-TOCTOU."##
                .to_string(),
            updated_at_ms: 1_000,
        })
        .expect("insert doc 1");

        pool.upsert_fts5_document(MemoryFts5Document {
            doc_id: "doc_storage".to_string(),
            title: "Persistencia SQLite WAL y motor FTS5 BM25".to_string(),
            tags: "sqlite wal fts5 bm25 memoria".to_string(),
            body: r##"El crate miku-storage ejecuta SQLite en modo WAL sobre tokio current_thread con índice léxico FTS5 BM25 en menos de 2 ms."##
                .to_string(),
            updated_at_ms: 1_010,
        })
        .expect("insert doc 2");

        pool.upsert_fts5_document(MemoryFts5Document {
            doc_id: "doc_avatar".to_string(),
            title: "Física de resortes del avatar en Canvas HTML5".to_string(),
            tags: "avatar rig springs canvas".to_string(),
            body: r##"Los resortes amortiguados se duermen a 0 fps cuando convergen bajo sleep_epsilon."##
                .to_string(),
            updated_at_ms: 1_020,
        })
        .expect("insert doc 3");

        // Rank BM25 for Spanish/technical query
        let hits = pool
            .search_fts5("persistencia sqlite wal fts5", 5)
            .expect("fts5 search");
        assert!(!hits.is_empty());
        assert_eq!(hits[0].doc_id, "doc_storage");
        assert!(hits[0].bm25_score > 0.0);
        assert!(hits[0].snippet.contains("[SQLite]") || hits[0].snippet.contains("[WAL]"));

        // Property/Security: Hostile SQL/FTS5 injection strings never panic or corrupt tables
        let hostile_inputs = [
            r##""; DROP TABLE sessions; --"##,
            r##""unclosed quote * ^ NEAR/5 ( OR AND NOT"##,
            "sqlite wal; DELETE FROM memory_fts5 WHERE 1=1;",
            "\0\x01\x02   ",
            "",
        ];
        for hostile in hostile_inputs {
            let res = pool.search_fts5(hostile, 5);
            assert!(res.is_ok(), "hostile query must not fail: {}", hostile);
        }
        // Table remains intact after hostile queries
        let post_hits = pool.search_fts5("gatekeeper sha256", 5).expect("post check");
        assert_eq!(post_hits[0].doc_id, "doc_gatekeeper");
    }

    // LEDGER-1.5-4
    #[test]
    fn test_profile_hash_approval_store_invalidates_on_single_byte_change() {
        let temp_dir = std::env::temp_dir().join(format!("miku_storage_test_{}", std::process::id()));
        let _ = fs::remove_dir_all(&temp_dir);
        fs::create_dir_all(&temp_dir).expect("create temp dir");
        let db_file = temp_dir.join("miku_profile_test.db");

        let original_toml = br##"
name = "miku"
root = "."
[allow]
shell = ["cargo test", "cargo clippy"]
"##;

        {
            let pool = SingleThreadStoragePool::open_wal_file(&db_file).expect("open wal file");
            let initial_status = pool
                .evaluate_profile_approval(r"C:\Workspace\Miku\", original_toml)
                .expect("evaluate initial");
            assert!(matches!(
                initial_status,
                ProfileApprovalStatus::UnapprovedFirstSeen { .. }
            ));

            pool.approve_profile_hash(r"C:\Workspace\Miku", original_toml, "user_island", 5_000)
                .expect("approve profile");

            let approved_status = pool
                .evaluate_profile_approval("c:/workspace/miku", original_toml)
                .expect("evaluate approved");
            assert!(approved_status.is_approved());
        }

        // Reopen from WAL on disk and verify persistence + 1-byte tamper invalidation
        {
            let pool_reopened =
                SingleThreadStoragePool::open_wal_file(&db_file).expect("reopen wal file");
            let status_ok = pool_reopened
                .evaluate_profile_approval("C:/Workspace/Miku", original_toml)
                .expect("status after reopen");
            assert!(status_ok.is_approved());

            let mut modified_toml = original_toml.to_vec();
            let last_idx = modified_toml.len() - 2;
            modified_toml[last_idx] ^= 0x01; // Flip 1 bit / 1 byte

            let tampered_status = pool_reopened
                .evaluate_profile_approval("C:/Workspace/Miku", &modified_toml)
                .expect("evaluate tampered");
            assert!(matches!(
                tampered_status,
                ProfileApprovalStatus::InvalidatedHashMismatch { .. }
            ));
            assert!(!tampered_status.is_approved());
        }

        let _ = fs::remove_dir_all(&temp_dir);
    }
}
