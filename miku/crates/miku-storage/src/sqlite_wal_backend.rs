#[cfg(not(target_os = "windows"))]
mod portable_incremental_wal {
    use std::fs::OpenOptions;
    use std::io::Write;
    use std::path::{Path, PathBuf};

    use miku_gatekeeper::AuditEntry;
    use serde::{Deserialize, Serialize};

    use crate::{
        ApprovedProfileRecord, ConversationTurnRecord, DatabaseSnapshot, Fts5SearchHit,
        MemoryFts5Document, SessionRecord, StorageError,
    };

    #[derive(Debug, Serialize, Deserialize)]
    enum DeltaOp {
        Session(SessionRecord),
        DelSession(String),
        Turn(ConversationTurnRecord),
        Audit(AuditEntry),
        Profile(ApprovedProfileRecord),
        FtsDoc(MemoryFts5Document),
        DelFts(String),
        ClearFts,
    }

    #[derive(Debug)]
    pub struct NativeSqliteWalBackend {
        wal_path: Option<PathBuf>,
    }

    impl NativeSqliteWalBackend {
        pub fn open(path: Option<&Path>) -> Result<Self, StorageError> {
            let wal_path = path.map(|p| PathBuf::from(format!("{}.delta-wal", p.display())));
            Ok(Self { wal_path })
        }

        fn append(&self, op: &DeltaOp) -> Result<(), StorageError> {
            let Some(ref p) = self.wal_path else {
                return Ok(());
            };
            let line =
                serde_json::to_string(op).map_err(|e| StorageError::IoError(e.to_string()))?;
            let mut f = OpenOptions::new()
                .create(true)
                .append(true)
                .open(p)
                .map_err(|e| StorageError::IoError(e.to_string()))?;
            writeln!(f, "{}", line).map_err(|e| StorageError::IoError(e.to_string()))
        }

        pub fn begin_immediate(&self) -> Result<(), StorageError> {
            Ok(())
        }
        pub fn commit(&self) -> Result<(), StorageError> {
            Ok(())
        }
        pub fn rollback(&self) -> Result<(), StorageError> {
            Ok(())
        }
        pub fn insert_session(&self, rec: &SessionRecord) -> Result<(), StorageError> {
            self.append(&DeltaOp::Session(rec.clone()))
        }
        pub fn delete_session(&self, session_id: &str) -> Result<(), StorageError> {
            self.append(&DeltaOp::DelSession(session_id.to_string()))
        }
        pub fn insert_turn(&self, t: &ConversationTurnRecord) -> Result<(), StorageError> {
            self.append(&DeltaOp::Turn(t.clone()))
        }
        pub fn append_audit_entry(&self, e: &AuditEntry) -> Result<(), StorageError> {
            self.append(&DeltaOp::Audit(e.clone()))
        }
        pub fn upsert_approved_profile(
            &self,
            rec: &ApprovedProfileRecord,
        ) -> Result<(), StorageError> {
            self.append(&DeltaOp::Profile(rec.clone()))
        }
        pub fn upsert_fts5_doc(&self, doc: &MemoryFts5Document) -> Result<(), StorageError> {
            self.append(&DeltaOp::FtsDoc(doc.clone()))
        }
        pub fn delete_fts5_doc(&self, doc_id: &str) -> Result<(), StorageError> {
            self.append(&DeltaOp::DelFts(doc_id.to_string()))
        }
        pub fn clear_fts5_docs(&self) -> Result<(), StorageError> {
            self.append(&DeltaOp::ClearFts)
        }
        pub fn search_fts5(
            &self,
            _terms: &[String],
            _limit: usize,
        ) -> Result<Vec<Fts5SearchHit>, StorageError> {
            Err(StorageError::InvalidInput("use in-memory fts5".to_string()))
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
            Ok(snap)
        }
        pub(crate) fn import_snapshot_atomically(
            &self,
            snap: &DatabaseSnapshot,
        ) -> Result<(), StorageError> {
            for r in snap.sessions.values() {
                self.insert_session(r)?;
            }
            for t in snap.turns.values() {
                self.insert_turn(t)?;
            }
            for a in &snap.audit_entries {
                self.append_audit_entry(a)?;
            }
            for p in snap.approved_profiles.values() {
                self.upsert_approved_profile(p)?;
            }
            for d in snap.fts_documents.values() {
                self.upsert_fts5_doc(d)?;
            }
            Ok(())
        }
        pub fn checkpoint_wal(&self) -> Result<(), StorageError> {
            Ok(())
        }
        pub fn journal_mode_query(&self) -> Result<String, StorageError> {
            Ok("wal".to_string())
        }
    }
}

#[cfg(target_os = "windows")]
pub use crate::sqlite_wal_ffi::NativeSqliteWalBackend;

#[cfg(not(target_os = "windows"))]
pub use portable_incremental_wal::NativeSqliteWalBackend;

pub fn sqlite_wal_backend_name() -> &'static str {
    if cfg!(target_os = "windows") {
        "winsqlite3_native_wal_fts5"
    } else {
        "portable_incremental_wal"
    }
}
