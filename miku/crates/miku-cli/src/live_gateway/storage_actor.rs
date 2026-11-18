use std::path::Path;
use std::sync::mpsc;
use std::thread::JoinHandle as ThreadJoinHandle;
use std::time::{SystemTime, UNIX_EPOCH};

use miku_storage::{
    ConversationTurnRecord, Fts5SearchHit, MemoryFts5Document, SessionRecord,
    SingleThreadStoragePool,
};
use serde::{Deserialize, Serialize};

pub(super) fn now_epoch_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

fn default_true() -> bool {
    true
}

fn default_tools_mode() -> String {
    "auto".to_string()
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AgentProfileDto {
    pub id: String,
    pub name: String,
    pub role: String,
    pub model: String,
    #[serde(default)]
    pub provider: String,
    pub c: String,
    pub outfit: String,
    #[serde(default)]
    pub prompt: String,
    #[serde(default = "default_true")]
    pub active: bool,
    #[serde(default)]
    pub isl: u8,
    #[serde(default = "default_tools_mode")]
    pub tools_mode: String,
    #[serde(default)]
    pub predefined: bool,
    #[serde(default)]
    pub harness_id: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct McpServerEntry {
    pub name: String,
    pub command: String,
    #[serde(default)]
    pub args: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub(super) struct SecretAliasIndexEntry {
    pub alias: String,
    pub provider: String,
    pub model: Option<String>,
}

enum StorageCmd {
    RecordTurn {
        session_id: String,
        role: String,
        content: String,
        seq: u64,
        ts_ms: u64,
        reply: mpsc::Sender<Result<usize, String>>,
    },
    SearchFts5 {
        query: String,
        limit: usize,
        reply: mpsc::Sender<Result<Vec<Fts5SearchHit>, String>>,
    },
    ListRecentTurns {
        session_id: String,
        limit: usize,
        reply: mpsc::Sender<Result<Vec<ConversationTurnRecord>, String>>,
    },
    Checkpoint {
        reply: mpsc::Sender<Result<(), String>>,
    },
    Shutdown,
}

pub(super) struct StorageActorHandle {
    tx: mpsc::Sender<StorageCmd>,
    join: Option<ThreadJoinHandle<()>>,
    initial_max_seq: u64,
}

fn compute_initial_max_seq(pool: &SingleThreadStoragePool) -> u64 {
    let mut max_seq: u64 = 0;
    if let Ok(sessions) = pool.list_all_sessions() {
        for s in sessions {
            if let Ok(turns) = pool.list_turns_for_session(&s.session_id) {
                for t in turns {
                    max_seq = max_seq.max(t.seq);
                }
            }
        }
    }
    max_seq
}

impl StorageActorHandle {
    pub(super) fn spawn(data_dir: &Path) -> Result<Self, String> {
        let db_path = data_dir.join("miku_live.db");
        let root_str = data_dir.to_string_lossy().to_string();
        let (tx, rx) = mpsc::channel::<StorageCmd>();
        let (init_tx, init_rx) = mpsc::channel::<Result<u64, String>>();

        let join = std::thread::Builder::new()
            .name("miku-storage-wal".to_string())
            .spawn(move || {
                let pool = match SingleThreadStoragePool::open_wal_file(&db_path) {
                    Ok(p) => {
                        let max_seq = compute_initial_max_seq(&p);
                        let _ = init_tx.send(Ok(max_seq));
                        p
                    }
                    Err(e) => {
                        let _ = init_tx.send(Err(e.to_string()));
                        return;
                    }
                };

                while let Ok(cmd) = rx.recv() {
                    match cmd {
                        StorageCmd::RecordTurn {
                            session_id,
                            role,
                            content,
                            seq,
                            ts_ms,
                            reply,
                        } => {
                            let res = record_turn_in_pool(
                                &pool,
                                &root_str,
                                &session_id,
                                &role,
                                &content,
                                seq,
                                ts_ms,
                            );
                            let _ = reply.send(res);
                        }
                        StorageCmd::SearchFts5 {
                            query,
                            limit,
                            reply,
                        } => {
                            let trimmed = query.trim();
                            let res = if trimmed.is_empty() {
                                Ok(Vec::new())
                            } else {
                                pool.search_fts5(trimmed, limit).map_err(|e| e.to_string())
                            };
                            let _ = reply.send(res);
                        }
                        StorageCmd::ListRecentTurns {
                            session_id,
                            limit,
                            reply,
                        } => {
                            let res = pool
                                .list_turns_for_session(&session_id)
                                .map(|mut turns| {
                                    turns.retain(|t| t.role == "user" || t.role == "assistant");
                                    if turns.len() > limit {
                                        turns.drain(..turns.len() - limit);
                                    }
                                    turns
                                })
                                .map_err(|e| e.to_string());
                            let _ = reply.send(res);
                        }
                        StorageCmd::Checkpoint { reply } => {
                            let _ = reply.send(pool.checkpoint_wal().map_err(|e| e.to_string()));
                        }
                        StorageCmd::Shutdown => {
                            let _ = pool.checkpoint_wal();
                            break;
                        }
                    }
                }
            })
            .map_err(|e| format!("spawn storage actor: {}", e))?;

        let initial_max_seq = init_rx
            .recv()
            .map_err(|_| "storage actor exited before initialization".to_string())??;

        Ok(Self {
            tx,
            join: Some(join),
            initial_max_seq,
        })
    }

    pub(super) fn initial_max_seq(&self) -> u64 {
        self.initial_max_seq
    }

    pub(super) fn record_turn(
        &self,
        session_id: &str,
        role: &str,
        content: &str,
        seq: u64,
        ts_ms: u64,
    ) -> Result<usize, String> {
        let (reply, rx) = mpsc::channel();
        self.tx
            .send(StorageCmd::RecordTurn {
                session_id: session_id.to_string(),
                role: role.to_string(),
                content: content.to_string(),
                seq,
                ts_ms,
                reply,
            })
            .map_err(|e| format!("storage actor send: {}", e))?;
        rx.recv().map_err(|e| format!("storage actor recv: {}", e))?
    }

    pub(super) fn search_fts5(
        &self,
        query: &str,
        limit: usize,
    ) -> Result<Vec<Fts5SearchHit>, String> {
        let (reply, rx) = mpsc::channel();
        self.tx
            .send(StorageCmd::SearchFts5 {
                query: query.to_string(),
                limit,
                reply,
            })
            .map_err(|e| format!("storage actor send: {}", e))?;
        rx.recv().map_err(|e| format!("storage actor recv: {}", e))?
    }

    pub(super) fn list_recent_turns(
        &self,
        session_id: &str,
        limit: usize,
    ) -> Result<Vec<ConversationTurnRecord>, String> {
        let (reply, rx) = mpsc::channel();
        self.tx
            .send(StorageCmd::ListRecentTurns {
                session_id: session_id.to_string(),
                limit,
                reply,
            })
            .map_err(|e| format!("storage actor send: {}", e))?;
        rx.recv().map_err(|e| format!("storage actor recv: {}", e))?
    }

    pub(super) fn checkpoint(&self) -> Result<(), String> {
        let (reply, rx) = mpsc::channel();
        let _ = self.tx.send(StorageCmd::Checkpoint { reply });
        rx.recv().unwrap_or(Ok(()))
    }
}

impl Drop for StorageActorHandle {
    fn drop(&mut self) {
        let _ = self.tx.send(StorageCmd::Shutdown);
        if let Some(j) = self.join.take() {
            let _ = j.join();
        }
    }
}

fn record_turn_in_pool(
    pool: &SingleThreadStoragePool,
    root_str: &str,
    session_id: &str,
    role: &str,
    content: &str,
    seq: u64,
    ts_ms: u64,
) -> Result<usize, String> {
    if pool
        .get_session(session_id)
        .map_err(|e| e.to_string())?
        .is_none()
    {
        pool.insert_session(SessionRecord {
            session_id: session_id.to_string(),
            agent_name: "miku".to_string(),
            project_root: root_str.to_string(),
            created_at_ms: ts_ms,
            state: "active".to_string(),
        })
        .map_err(|e| e.to_string())?;
    }

    let turn_id = format!("turn-{}-{}", session_id, seq);
    pool.insert_turn(ConversationTurnRecord {
        turn_id: turn_id.clone(),
        session_id: session_id.to_string(),
        seq,
        role: role.to_string(),
        content: content.to_string(),
        token_estimate: (content.len() / 4).max(1) as u32,
        ts_ms,
    })
    .map_err(|e| e.to_string())?;

    pool.upsert_fts5_document(MemoryFts5Document {
        doc_id: turn_id,
        title: format!("{} #{}", role, seq),
        tags: format!("live {}", role),
        body: content.to_string(),
        updated_at_ms: ts_ms,
    })
    .map_err(|e| e.to_string())?;

    pool.fts5_document_count().map_err(|e| e.to_string())
}
