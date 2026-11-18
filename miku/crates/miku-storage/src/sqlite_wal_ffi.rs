#![cfg(target_os = "windows")]

use std::ffi::{c_char, c_int, c_void, CStr, CString};
use std::path::Path;
use std::ptr::{null, null_mut};

use miku_gatekeeper::AuditEntry;

use crate::{
    build_highlighted_snippet, ApprovedProfileRecord, ConversationTurnRecord, DatabaseSnapshot,
    Fts5SearchHit, MemoryFts5Document, SessionRecord, StorageError,
};

const SQLITE_OK: c_int = 0;
const SQLITE_ROW: c_int = 100;
const SQLITE_DONE: c_int = 101;
const SQLITE_TRANSIENT: isize = -1;
const SQLITE_OPEN_READWRITE: c_int = 0x0000_0002;
const SQLITE_OPEN_CREATE: c_int = 0x0000_0004;
const SQLITE_OPEN_FULLMUTEX: c_int = 0x0001_0000;
const SQLITE_CHECKPOINT_TRUNCATE: c_int = 3;

type FnOpenV2 =
    unsafe extern "C" fn(*const c_char, *mut *mut c_void, c_int, *const c_char) -> c_int;
type FnClose = unsafe extern "C" fn(*mut c_void) -> c_int;
type FnExec = unsafe extern "C" fn(
    *mut c_void,
    *const c_char,
    *mut c_void,
    *mut c_void,
    *mut *mut c_char,
) -> c_int;
type FnPrepareV2 = unsafe extern "C" fn(
    *mut c_void,
    *const c_char,
    c_int,
    *mut *mut c_void,
    *mut *const c_char,
) -> c_int;
type FnBindText = unsafe extern "C" fn(*mut c_void, c_int, *const c_char, c_int, isize) -> c_int;
type FnBindInt64 = unsafe extern "C" fn(*mut c_void, c_int, i64) -> c_int;
type FnStep = unsafe extern "C" fn(*mut c_void) -> c_int;
type FnReset = unsafe extern "C" fn(*mut c_void) -> c_int;
type FnClearBindings = unsafe extern "C" fn(*mut c_void) -> c_int;
type FnFinalize = unsafe extern "C" fn(*mut c_void) -> c_int;
type FnColText = unsafe extern "C" fn(*mut c_void, c_int) -> *const u8;
type FnColBytes = unsafe extern "C" fn(*mut c_void, c_int) -> c_int;
type FnColInt64 = unsafe extern "C" fn(*mut c_void, c_int) -> i64;
type FnColDouble = unsafe extern "C" fn(*mut c_void, c_int) -> f64;
type FnErrmsg = unsafe extern "C" fn(*mut c_void) -> *const c_char;
type FnFree = unsafe extern "C" fn(*mut c_void);
type FnWalCkptV2 =
    unsafe extern "C" fn(*mut c_void, *const c_char, c_int, *mut c_int, *mut c_int) -> c_int;

#[link(name = "kernel32")]
extern "system" {
    fn LoadLibraryW(lp_lib_file_name: *const u16) -> *mut c_void;
    fn GetProcAddress(h_module: *mut c_void, lp_proc_name: *const c_char) -> *mut c_void;
    fn FreeLibrary(h_lib_module: *mut c_void) -> i32;
}

struct Api {
    hmod: *mut c_void,
    open_v2: FnOpenV2,
    close: FnClose,
    exec: FnExec,
    prepare_v2: FnPrepareV2,
    bind_text: FnBindText,
    bind_int64: FnBindInt64,
    step: FnStep,
    reset: FnReset,
    clear_bindings: FnClearBindings,
    finalize: FnFinalize,
    col_text: FnColText,
    col_bytes: FnColBytes,
    col_int64: FnColInt64,
    col_double: FnColDouble,
    errmsg: FnErrmsg,
    free: FnFree,
    wal_ckpt_v2: FnWalCkptV2,
}

unsafe fn sym<T>(hmod: *mut c_void, name: &[u8]) -> Result<T, StorageError> {
    let ptr = GetProcAddress(hmod, name.as_ptr() as *const c_char);
    if ptr.is_null() {
        let label = String::from_utf8_lossy(name);
        return Err(StorageError::IoError(format!(
            "winsqlite3 missing symbol {}",
            label
        )));
    }
    Ok(std::mem::transmute_copy(&ptr))
}

impl Api {
    fn load() -> Result<Self, StorageError> {
        let dll: Vec<u16> = "winsqlite3.dll\0".encode_utf16().collect();
        let hmod = unsafe { LoadLibraryW(dll.as_ptr()) };
        if hmod.is_null() {
            return Err(StorageError::IoError(
                "LoadLibraryW(winsqlite3.dll) failed".to_string(),
            ));
        }
        unsafe {
            Ok(Self {
                hmod,
                open_v2: sym(hmod, b"sqlite3_open_v2\0")?,
                close: sym(hmod, b"sqlite3_close_v2\0")?,
                exec: sym(hmod, b"sqlite3_exec\0")?,
                prepare_v2: sym(hmod, b"sqlite3_prepare_v2\0")?,
                bind_text: sym(hmod, b"sqlite3_bind_text\0")?,
                bind_int64: sym(hmod, b"sqlite3_bind_int64\0")?,
                step: sym(hmod, b"sqlite3_step\0")?,
                reset: sym(hmod, b"sqlite3_reset\0")?,
                clear_bindings: sym(hmod, b"sqlite3_clear_bindings\0")?,
                finalize: sym(hmod, b"sqlite3_finalize\0")?,
                col_text: sym(hmod, b"sqlite3_column_text\0")?,
                col_bytes: sym(hmod, b"sqlite3_column_bytes\0")?,
                col_int64: sym(hmod, b"sqlite3_column_int64\0")?,
                col_double: sym(hmod, b"sqlite3_column_double\0")?,
                errmsg: sym(hmod, b"sqlite3_errmsg\0")?,
                free: sym(hmod, b"sqlite3_free\0")?,
                wal_ckpt_v2: sym(hmod, b"sqlite3_wal_checkpoint_v2\0")?,
            })
        }
    }
}

impl Drop for Api {
    fn drop(&mut self) {
        if !self.hmod.is_null() {
            unsafe {
                FreeLibrary(self.hmod);
            }
        }
    }
}

pub struct NativeSqliteWalBackend {
    api: Api,
    db: *mut c_void,
    stmt_ins_session: *mut c_void,
    stmt_del_session: *mut c_void,
    stmt_ins_turn: *mut c_void,
    stmt_ins_audit: *mut c_void,
    stmt_upsert_profile: *mut c_void,
    stmt_ins_fts: *mut c_void,
    stmt_del_fts: *mut c_void,
    stmt_search_fts: *mut c_void,
}

impl std::fmt::Debug for NativeSqliteWalBackend {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("NativeSqliteWalBackend").finish()
    }
}

impl NativeSqliteWalBackend {
    pub fn open(path: Option<&Path>) -> Result<Self, StorageError> {
        let api = Api::load()?;
        let uri_str = match path {
            Some(p) => p.to_string_lossy().to_string(),
            None => ":memory:".to_string(),
        };
        let c_path = CString::new(uri_str).map_err(|e| StorageError::IoError(e.to_string()))?;
        let mut db: *mut c_void = null_mut();
        let flags = SQLITE_OPEN_READWRITE | SQLITE_OPEN_CREATE | SQLITE_OPEN_FULLMUTEX;
        let rc = unsafe { (api.open_v2)(c_path.as_ptr(), &mut db, flags, null()) };
        if rc != SQLITE_OK || db.is_null() {
            if !db.is_null() {
                unsafe {
                    (api.close)(db);
                }
            }
            return Err(StorageError::IoError(format!(
                "sqlite3_open_v2 failed with rc={}",
                rc
            )));
        }

        let mut me = Self {
            api,
            db,
            stmt_ins_session: null_mut(),
            stmt_del_session: null_mut(),
            stmt_ins_turn: null_mut(),
            stmt_ins_audit: null_mut(),
            stmt_upsert_profile: null_mut(),
            stmt_ins_fts: null_mut(),
            stmt_del_fts: null_mut(),
            stmt_search_fts: null_mut(),
        };

        me.exec_batch(
            "PRAGMA journal_mode=WAL;
             PRAGMA synchronous=NORMAL;
             PRAGMA foreign_keys=ON;
             PRAGMA busy_timeout=5000;
             PRAGMA temp_store=MEMORY;",
        )?;
        me.create_schema_v1()?;
        me.prepare_statements()?;
        Ok(me)
    }

    pub fn exec_batch(&self, sql: &str) -> Result<(), StorageError> {
        let c_sql = CString::new(sql).map_err(|e| StorageError::IoError(e.to_string()))?;
        let mut err_ptr: *mut c_char = null_mut();
        let rc = unsafe {
            (self.api.exec)(self.db, c_sql.as_ptr(), null_mut(), null_mut(), &mut err_ptr)
        };
        if rc != SQLITE_OK {
            let msg = if !err_ptr.is_null() {
                let s = unsafe { CStr::from_ptr(err_ptr) }.to_string_lossy().to_string();
                unsafe {
                    (self.api.free)(err_ptr as *mut c_void);
                }
                s
            } else {
                self.last_err()
            };
            return Err(StorageError::IoError(format!(
                "SQLite exec error ({}): {}",
                rc, msg
            )));
        }
        Ok(())
    }

    fn last_err(&self) -> String {
        unsafe {
            let p = (self.api.errmsg)(self.db);
            if p.is_null() {
                "unknown sqlite error".to_string()
            } else {
                CStr::from_ptr(p).to_string_lossy().to_string()
            }
        }
    }

    fn prepare_one(&self, sql: &str) -> Result<*mut c_void, StorageError> {
        let c_sql = CString::new(sql).map_err(|e| StorageError::IoError(e.to_string()))?;
        let mut stmt: *mut c_void = null_mut();
        let rc = unsafe {
            (self.api.prepare_v2)(self.db, c_sql.as_ptr(), -1, &mut stmt, null_mut())
        };
        if rc != SQLITE_OK || stmt.is_null() {
            return Err(StorageError::IoError(format!(
                "prepare failed ({}): {}",
                rc,
                self.last_err()
            )));
        }
        Ok(stmt)
    }

    fn create_schema_v1(&self) -> Result<(), StorageError> {
        self.exec_batch(
            "BEGIN IMMEDIATE;
            CREATE TABLE IF NOT EXISTS sessions (
                session_id TEXT PRIMARY KEY NOT NULL,
                agent_name TEXT NOT NULL,
                project_root TEXT NOT NULL,
                created_at_ms INTEGER NOT NULL,
                state TEXT NOT NULL
            );
            CREATE TABLE IF NOT EXISTS conversation_turns (
                turn_id TEXT PRIMARY KEY NOT NULL,
                session_id TEXT NOT NULL,
                seq INTEGER NOT NULL,
                role TEXT NOT NULL,
                content TEXT NOT NULL,
                token_estimate INTEGER NOT NULL,
                ts_ms INTEGER NOT NULL,
                FOREIGN KEY (session_id) REFERENCES sessions(session_id) ON DELETE CASCADE
            );
            CREATE INDEX IF NOT EXISTS idx_turns_session_seq ON conversation_turns(session_id, seq);
            CREATE TABLE IF NOT EXISTS audit_chain_entries (
                seq INTEGER PRIMARY KEY NOT NULL,
                ts_ms INTEGER NOT NULL,
                agent TEXT NOT NULL,
                capability TEXT NOT NULL,
                target TEXT NOT NULL,
                verdict TEXT NOT NULL,
                reason TEXT NOT NULL,
                prev_hash TEXT NOT NULL,
                entry_hash TEXT NOT NULL
            );
            CREATE TABLE IF NOT EXISTS approved_profiles (
                project_root TEXT PRIMARY KEY NOT NULL,
                toml_sha256 TEXT NOT NULL,
                approved_by TEXT NOT NULL,
                approved_at_ms INTEGER NOT NULL
            );
            CREATE VIRTUAL TABLE IF NOT EXISTS memory_fts5 USING fts5(
                doc_id UNINDEXED,
                title,
                tags,
                body,
                updated_at_ms UNINDEXED,
                tokenize='unicode61 remove_diacritics 1'
            );
            COMMIT;",
        )
    }

    fn prepare_statements(&mut self) -> Result<(), StorageError> {
        self.stmt_ins_session = self.prepare_one(
            "INSERT INTO sessions(session_id, agent_name, project_root, created_at_ms, state) VALUES (?1, ?2, ?3, ?4, ?5);",
        )?;
        self.stmt_del_session = self.prepare_one("DELETE FROM sessions WHERE session_id = ?1;")?;
        self.stmt_ins_turn = self.prepare_one(
            "INSERT INTO conversation_turns(turn_id, session_id, seq, role, content, token_estimate, ts_ms) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7);",
        )?;
        self.stmt_ins_audit = self.prepare_one(
            "INSERT INTO audit_chain_entries(seq, ts_ms, agent, capability, target, verdict, reason, prev_hash, entry_hash) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9);",
        )?;
        self.stmt_upsert_profile = self.prepare_one(
            "INSERT OR REPLACE INTO approved_profiles(project_root, toml_sha256, approved_by, approved_at_ms) VALUES (?1, ?2, ?3, ?4);",
        )?;
        self.stmt_ins_fts = self.prepare_one(
            "INSERT INTO memory_fts5(doc_id, title, tags, body, updated_at_ms) VALUES (?1, ?2, ?3, ?4, ?5);",
        )?;
        self.stmt_del_fts = self.prepare_one("DELETE FROM memory_fts5 WHERE doc_id = ?1;")?;
        self.stmt_search_fts = self.prepare_one(
            "SELECT doc_id, title, tags, bm25(memory_fts5, 0.0, 3.0, 2.0, 1.0, 0.0) AS score, body, updated_at_ms FROM memory_fts5 WHERE memory_fts5 MATCH ?1 ORDER BY score ASC, doc_id ASC LIMIT ?2;",
        )?;
        Ok(())
    }

    fn bind_str(&self, stmt: *mut c_void, idx: c_int, val: &str) {
        unsafe {
            (self.api.bind_text)(
                stmt,
                idx,
                val.as_ptr() as *const c_char,
                val.len() as c_int,
                SQLITE_TRANSIENT,
            );
        }
    }

    fn step_done(&self, stmt: *mut c_void) -> Result<(), StorageError> {
        let rc = unsafe { (self.api.step)(stmt) };
        unsafe {
            (self.api.reset)(stmt);
            (self.api.clear_bindings)(stmt);
        }
        if rc != SQLITE_DONE && rc != SQLITE_ROW {
            return Err(StorageError::IoError(format!(
                "SQLite step error ({}): {}",
                rc,
                self.last_err()
            )));
        }
        Ok(())
    }

    fn col_str(&self, stmt: *mut c_void, col: c_int) -> String {
        unsafe {
            let p = (self.api.col_text)(stmt, col);
            let len = (self.api.col_bytes)(stmt, col);
            if p.is_null() || len <= 0 {
                return String::new();
            }
            let slice = std::slice::from_raw_parts(p, len as usize);
            String::from_utf8_lossy(slice).to_string()
        }
    }

    pub fn begin_immediate(&self) -> Result<(), StorageError> {
        self.exec_batch("BEGIN IMMEDIATE;")
    }

    pub fn commit(&self) -> Result<(), StorageError> {
        self.exec_batch("COMMIT;")
    }

    pub fn rollback(&self) -> Result<(), StorageError> {
        self.exec_batch("ROLLBACK;")
    }

    pub fn insert_session(&self, rec: &SessionRecord) -> Result<(), StorageError> {
        let s = self.stmt_ins_session;
        self.bind_str(s, 1, &rec.session_id);
        self.bind_str(s, 2, &rec.agent_name);
        self.bind_str(s, 3, &rec.project_root);
        unsafe { (self.api.bind_int64)(s, 4, rec.created_at_ms as i64) };
        self.bind_str(s, 5, &rec.state);
        self.step_done(s)
    }

    pub fn delete_session(&self, session_id: &str) -> Result<(), StorageError> {
        let s = self.stmt_del_session;
        self.bind_str(s, 1, session_id);
        self.step_done(s)
    }

    pub fn insert_turn(&self, t: &ConversationTurnRecord) -> Result<(), StorageError> {
        let s = self.stmt_ins_turn;
        self.bind_str(s, 1, &t.turn_id);
        self.bind_str(s, 2, &t.session_id);
        unsafe { (self.api.bind_int64)(s, 3, t.seq as i64) };
        self.bind_str(s, 4, &t.role);
        self.bind_str(s, 5, &t.content);
        unsafe { (self.api.bind_int64)(s, 6, t.token_estimate as i64) };
        unsafe { (self.api.bind_int64)(s, 7, t.ts_ms as i64) };
        self.step_done(s)
    }

    pub fn append_audit_entry(&self, e: &AuditEntry) -> Result<(), StorageError> {
        let s = self.stmt_ins_audit;
        unsafe {
            (self.api.bind_int64)(s, 1, e.seq as i64);
            (self.api.bind_int64)(s, 2, e.timestamp_ms as i64);
        }
        self.bind_str(s, 3, &e.agent);
        self.bind_str(s, 4, &e.capability);
        self.bind_str(s, 5, &e.target);
        self.bind_str(s, 6, &e.verdict);
        self.bind_str(s, 7, &e.reason);
        self.bind_str(s, 8, &e.prev_hash);
        self.bind_str(s, 9, &e.entry_hash);
        self.step_done(s)
    }

    pub fn upsert_approved_profile(&self, rec: &ApprovedProfileRecord) -> Result<(), StorageError> {
        let s = self.stmt_upsert_profile;
        self.bind_str(s, 1, &rec.project_root);
        self.bind_str(s, 2, &rec.toml_sha256);
        self.bind_str(s, 3, &rec.approved_by);
        unsafe { (self.api.bind_int64)(s, 4, rec.approved_at_ms as i64) };
        self.step_done(s)
    }

    pub fn upsert_fts5_doc(&self, doc: &MemoryFts5Document) -> Result<(), StorageError> {
        let del = self.stmt_del_fts;
        self.bind_str(del, 1, &doc.doc_id);
        self.step_done(del)?;

        let ins = self.stmt_ins_fts;
        self.bind_str(ins, 1, &doc.doc_id);
        self.bind_str(ins, 2, &doc.title);
        self.bind_str(ins, 3, &doc.tags);
        self.bind_str(ins, 4, &doc.body);
        unsafe { (self.api.bind_int64)(ins, 5, doc.updated_at_ms as i64) };
        self.step_done(ins)
    }

    pub fn delete_fts5_doc(&self, doc_id: &str) -> Result<(), StorageError> {
        let del = self.stmt_del_fts;
        self.bind_str(del, 1, doc_id);
        self.step_done(del)
    }

    pub fn clear_fts5_docs(&self) -> Result<(), StorageError> {
        self.exec_batch("DELETE FROM memory_fts5;")
    }

    pub fn search_fts5(
        &self,
        terms: &[String],
        limit: usize,
    ) -> Result<Vec<Fts5SearchHit>, StorageError> {
        if terms.is_empty() || limit == 0 {
            return Ok(Vec::new());
        }
        let match_expr = terms
            .iter()
            .map(|t| format!("\"{}\"", t.replace('"', "")))
            .collect::<Vec<_>>()
            .join(" OR ");
        let s = self.stmt_search_fts;
        self.bind_str(s, 1, &match_expr);
        unsafe { (self.api.bind_int64)(s, 2, limit as i64) };

        let mut hits = Vec::new();
        loop {
            let rc = unsafe { (self.api.step)(s) };
            if rc == SQLITE_ROW {
                let doc_id = self.col_str(s, 0);
                let title = self.col_str(s, 1);
                let tags = self.col_str(s, 2);
                let raw_bm25 = unsafe { (self.api.col_double)(s, 3) };
                let body = self.col_str(s, 4);
                let updated_at_ms = unsafe { (self.api.col_int64)(s, 5) } as u64;
                hits.push(Fts5SearchHit {
                    doc_id,
                    title,
                    tags,
                    bm25_score: (-raw_bm25).max(0.01),
                    snippet: build_highlighted_snippet(&body, terms),
                    updated_at_ms,
                });
            } else if rc == SQLITE_DONE {
                break;
            } else {
                unsafe {
                    (self.api.reset)(s);
                    (self.api.clear_bindings)(s);
                }
                return Err(StorageError::IoError(format!(
                    "FTS5 query failed ({}): {}",
                    rc,
                    self.last_err()
                )));
            }
        }
        unsafe {
            (self.api.reset)(s);
            (self.api.clear_bindings)(s);
        }
        Ok(hits)
    }

    pub(crate) fn load_full_snapshot(&self) -> Result<DatabaseSnapshot, StorageError> {
        let mut snap = DatabaseSnapshot::default();
        for tbl in [
            "sessions",
            "conversation_turns",
            "audit_chain_entries",
            "approved_profiles",
            "memory_fts5",
        ] {
            snap.migrated_tables.insert(tbl.to_string());
        }

        let stmt_sess = self.prepare_one(
            "SELECT session_id, agent_name, project_root, created_at_ms, state FROM sessions;",
        )?;
        loop {
            let rc = unsafe { (self.api.step)(stmt_sess) };
            if rc == SQLITE_ROW {
                let rec = SessionRecord {
                    session_id: self.col_str(stmt_sess, 0),
                    agent_name: self.col_str(stmt_sess, 1),
                    project_root: self.col_str(stmt_sess, 2),
                    created_at_ms: unsafe { (self.api.col_int64)(stmt_sess, 3) } as u64,
                    state: self.col_str(stmt_sess, 4),
                };
                snap.sessions.insert(rec.session_id.clone(), rec);
            } else {
                break;
            }
        }
        unsafe { (self.api.finalize)(stmt_sess) };

        let stmt_turns = self.prepare_one(
            "SELECT turn_id, session_id, seq, role, content, token_estimate, ts_ms FROM conversation_turns;",
        )?;
        loop {
            let rc = unsafe { (self.api.step)(stmt_turns) };
            if rc == SQLITE_ROW {
                let rec = ConversationTurnRecord {
                    turn_id: self.col_str(stmt_turns, 0),
                    session_id: self.col_str(stmt_turns, 1),
                    seq: unsafe { (self.api.col_int64)(stmt_turns, 2) } as u64,
                    role: self.col_str(stmt_turns, 3),
                    content: self.col_str(stmt_turns, 4),
                    token_estimate: unsafe { (self.api.col_int64)(stmt_turns, 5) } as u32,
                    ts_ms: unsafe { (self.api.col_int64)(stmt_turns, 6) } as u64,
                };
                snap.turns.insert(rec.turn_id.clone(), rec);
            } else {
                break;
            }
        }
        unsafe { (self.api.finalize)(stmt_turns) };

        let stmt_audit = self.prepare_one(
            "SELECT seq, ts_ms, agent, capability, target, verdict, reason, prev_hash, entry_hash FROM audit_chain_entries ORDER BY seq ASC;",
        )?;
        loop {
            let rc = unsafe { (self.api.step)(stmt_audit) };
            if rc == SQLITE_ROW {
                snap.audit_entries.push(AuditEntry {
                    seq: unsafe { (self.api.col_int64)(stmt_audit, 0) } as u64,
                    timestamp_ms: unsafe { (self.api.col_int64)(stmt_audit, 1) } as u64,
                    agent: self.col_str(stmt_audit, 2),
                    capability: self.col_str(stmt_audit, 3),
                    target: self.col_str(stmt_audit, 4),
                    verdict: self.col_str(stmt_audit, 5),
                    reason: self.col_str(stmt_audit, 6),
                    prev_hash: self.col_str(stmt_audit, 7),
                    entry_hash: self.col_str(stmt_audit, 8),
                });
            } else {
                break;
            }
        }
        unsafe { (self.api.finalize)(stmt_audit) };

        let stmt_prof = self.prepare_one(
            "SELECT project_root, toml_sha256, approved_by, approved_at_ms FROM approved_profiles;",
        )?;
        loop {
            let rc = unsafe { (self.api.step)(stmt_prof) };
            if rc == SQLITE_ROW {
                let rec = ApprovedProfileRecord {
                    project_root: self.col_str(stmt_prof, 0),
                    toml_sha256: self.col_str(stmt_prof, 1),
                    approved_by: self.col_str(stmt_prof, 2),
                    approved_at_ms: unsafe { (self.api.col_int64)(stmt_prof, 3) } as u64,
                };
                snap.approved_profiles.insert(rec.project_root.clone(), rec);
            } else {
                break;
            }
        }
        unsafe { (self.api.finalize)(stmt_prof) };

        let stmt_fts = self.prepare_one(
            "SELECT doc_id, title, tags, body, updated_at_ms FROM memory_fts5;",
        )?;
        loop {
            let rc = unsafe { (self.api.step)(stmt_fts) };
            if rc == SQLITE_ROW {
                let doc = MemoryFts5Document {
                    doc_id: self.col_str(stmt_fts, 0),
                    title: self.col_str(stmt_fts, 1),
                    tags: self.col_str(stmt_fts, 2),
                    body: self.col_str(stmt_fts, 3),
                    updated_at_ms: unsafe { (self.api.col_int64)(stmt_fts, 4) } as u64,
                };
                snap.fts_documents.insert(doc.doc_id.clone(), doc);
            } else {
                break;
            }
        }
        unsafe { (self.api.finalize)(stmt_fts) };

        Ok(snap)
    }

    pub(crate) fn import_snapshot_atomically(
        &self,
        snap: &DatabaseSnapshot,
    ) -> Result<(), StorageError> {
        self.begin_immediate()?;
        let res = (|| {
            for rec in snap.sessions.values() {
                self.insert_session(rec)?;
            }
            for turn in snap.turns.values() {
                self.insert_turn(turn)?;
            }
            for entry in &snap.audit_entries {
                self.append_audit_entry(entry)?;
            }
            for prof in snap.approved_profiles.values() {
                self.upsert_approved_profile(prof)?;
            }
            for doc in snap.fts_documents.values() {
                self.upsert_fts5_doc(doc)?;
            }
            Ok(())
        })();
        match res {
            Ok(()) => self.commit(),
            Err(e) => {
                let _ = self.rollback();
                Err(e)
            }
        }
    }

    pub fn checkpoint_wal(&self) -> Result<(), StorageError> {
        let mut log_frames: c_int = 0;
        let mut ckpt_frames: c_int = 0;
        let rc = unsafe {
            (self.api.wal_ckpt_v2)(
                self.db,
                null(),
                SQLITE_CHECKPOINT_TRUNCATE,
                &mut log_frames,
                &mut ckpt_frames,
            )
        };
        if rc != SQLITE_OK {
            return Err(StorageError::IoError(format!(
                "wal checkpoint failed ({}): {}",
                rc,
                self.last_err()
            )));
        }
        Ok(())
    }

    pub fn journal_mode_query(&self) -> Result<String, StorageError> {
        let stmt = self.prepare_one("PRAGMA journal_mode;")?;
        let rc = unsafe { (self.api.step)(stmt) };
        let mode = if rc == SQLITE_ROW {
            self.col_str(stmt, 0)
        } else {
            String::new()
        };
        unsafe { (self.api.finalize)(stmt) };
        Ok(mode)
    }
}

impl Drop for NativeSqliteWalBackend {
    fn drop(&mut self) {
        unsafe {
            for stmt in [
                self.stmt_ins_session,
                self.stmt_del_session,
                self.stmt_ins_turn,
                self.stmt_ins_audit,
                self.stmt_upsert_profile,
                self.stmt_ins_fts,
                self.stmt_del_fts,
                self.stmt_search_fts,
            ] {
                if !stmt.is_null() {
                    (self.api.finalize)(stmt);
                }
            }
            if !self.db.is_null() {
                (self.api.close)(self.db);
            }
        }
    }
}
