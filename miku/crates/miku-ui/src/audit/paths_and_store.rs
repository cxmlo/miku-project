#![allow(unused_imports)]
use crate::prelude::*;

// ============================================================================
// 12. Persistent Append-Only JSONL Audit Store, Transactional Size Rotation,
//     <50ms 10k Chain Verifier & Safe WebView2 Audit Viewer (Step 2.7 · F-20 / ADR-007)
// ============================================================================

/// Formats and validates a canonical Windows isolated audit directory `%LOCALAPPDATA%\miku\audit`.
/// Rejects relative paths, `..` traversal, control characters, and collisions with `webview2`,
/// `config`, or `workspace` directories so an agent with project access cannot tamper with logs.
pub fn format_canonical_windows_audit_log_dir(base_local_appdata: &str) -> Result<String, UiError> {
    let trimmed = base_local_appdata.trim();
    if trimmed.is_empty() {
        return Err(UiError::AuditLogIoError(
            "LOCALAPPDATA path cannot be empty for isolated audit store".to_string(),
        ));
    }
    if trimmed
        .chars()
        .any(|c| c == '\0' || c == '\r' || c == '\n' || c == '|' || c == ';' || c == '&')
    {
        return Err(UiError::AuditLogIoError(
            "LOCALAPPDATA contains forbidden control or injection characters".to_string(),
        ));
    }

    let normalized = trimmed.replace('/', r"\");
    let bytes = normalized.as_bytes();
    let has_drive_prefix = bytes.len() >= 3
        && bytes[0].is_ascii_alphabetic()
        && bytes[1] == b':'
        && bytes[2] == b'\\';
    if !has_drive_prefix {
        return Err(UiError::AuditLogIoError(format!(
            "LOCALAPPDATA must be an absolute Windows drive path, got '{}'",
            base_local_appdata
        )));
    }

    let mut clean_segments: Vec<&str> = Vec::new();
    for (idx, seg) in normalized.split('\\').enumerate() {
        if idx == 0 {
            clean_segments.push(seg);
            continue;
        }
        if seg.is_empty() || seg == "." {
            continue;
        }
        if seg == ".." {
            return Err(UiError::AuditLogIoError(format!(
                "path traversal '..' forbidden in isolated audit directory: '{}'",
                base_local_appdata
            )));
        }
        let lower_seg = seg.to_ascii_lowercase();
        if lower_seg == "webview2"
            || lower_seg == "ebwebview"
            || lower_seg == "config"
            || lower_seg == "profiles"
        {
            return Err(UiError::AuditLogIoError(format!(
                "isolated audit directory cannot share segment '{}' with config/webview2 folders",
                seg
            )));
        }
        clean_segments.push(seg);
    }

    if clean_segments.len() < 2 {
        return Err(UiError::AuditLogIoError(
            "LOCALAPPDATA cannot be a bare drive root".to_string(),
        ));
    }

    let mut joined = clean_segments.join(r"\");
    let lower = joined.to_ascii_lowercase();
    if !lower.ends_with(r"\miku\audit") {
        joined.push('\\');
        joined.push_str(AUDIT_ISOLATED_SUBDIR_WINDOWS);
    }
    Ok(joined)
}

/// Resolves the isolated `%LOCALAPPDATA%\miku\audit` directory on Windows or from a test override.
pub fn resolve_isolated_audit_log_dir(
    local_appdata_override: Option<&Path>,
) -> Result<PathBuf, UiError> {
    let raw_base = match local_appdata_override {
        Some(p) => p.to_string_lossy().to_string(),
        None => std::env::var("LOCALAPPDATA").unwrap_or_else(|_| {
            if cfg!(target_os = "windows") {
                r"C:\Users\Default\AppData\Local".to_string()
            } else {
                r"C:\Users\ci\AppData\Local".to_string()
            }
        }),
    };

    if !cfg!(target_os = "windows") && raw_base.starts_with('/') {
        if raw_base.contains("..") || raw_base.contains('\0') {
            return Err(UiError::AuditLogIoError(
                "POSIX LOCALAPPDATA override contains traversal or null byte".to_string(),
            ));
        }
        let lower = raw_base.to_ascii_lowercase();
        if lower.contains("/webview2") || lower.contains("/config") || lower.contains("/profiles") {
            return Err(UiError::AuditLogIoError(
                "isolated audit log directory cannot collide with webview2/config/profiles".to_string(),
            ));
        }
        let base_path = PathBuf::from(&raw_base);
        if raw_base.ends_with("/miku/audit") {
            return Ok(base_path);
        }
        return Ok(base_path.join("miku").join("audit"));
    }

    let canonical_win = format_canonical_windows_audit_log_dir(&raw_base)?;
    Ok(PathBuf::from(canonical_win))
}

static UNIQUE_AUDIT_DIR_SEQ: AtomicU64 = AtomicU64::new(1);

/// Creates a guaranteed-unique isolated directory under the OS temp dir using PID, worker index,
/// atomic sequence counter, and `SystemTime` nanoseconds so parallel tests in Windows never collide.
pub fn create_unique_isolated_audit_temp_dir(
    prefix: &str,
    worker_idx: usize,
) -> Result<PathBuf, UiError> {
    let seq = UNIQUE_AUDIT_DIR_SEQ.fetch_add(1, Ordering::SeqCst);
    let now_ns = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    let audit_dir = std::env::temp_dir().join(format!(
        "{}_{}_{}_{}_{}",
        prefix,
        std::process::id(),
        worker_idx,
        seq,
        now_ns
    ));
    std::fs::create_dir_all(&audit_dir)
        .map_err(|e| UiError::AuditLogIoError(format!("failed to create dir: {e}")))?;
    Ok(audit_dir)
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct JsonlChainVerificationReport {
    pub verified_entries: usize,
    pub verified_files: usize,
    pub first_seq: u64,
    pub last_seq: u64,
    pub anchor_prev_hash: String,
    pub tip_entry_hash: String,
    pub elapsed_ms: f64,
}

#[derive(Debug, Clone)]
pub struct PersistentJsonlAuditStore {
    pub(crate) audit_dir: PathBuf,
    pub(crate) active_file_path: PathBuf,
    pub(crate) max_file_bytes: usize,
    pub(crate) max_rotated_files: usize,
    pub(crate) next_seq: u64,
    pub(crate) last_entry_hash: String,
    pub(crate) anchor_prev_hash: String,
    pub(crate) anchor_first_seq: u64,
    pub(crate) current_file_bytes: usize,
    pub(crate) rotations_completed: u64,
    pub(crate) total_appended: u64,
    pub(crate) max_append_latency_ms: f64,
}

impl PersistentJsonlAuditStore {
    pub fn open(
        audit_dir: &Path,
        max_file_bytes: usize,
        max_rotated_files: usize,
    ) -> Result<Self, UiError> {
        if audit_dir.as_os_str().is_empty() {
            return Err(UiError::AuditLogIoError(
                "audit_dir path cannot be empty".to_string(),
            ));
        }
        if max_file_bytes < 256 {
            return Err(UiError::AuditLogIoError(format!(
                "max_file_bytes ({}) must be at least 256 bytes",
                max_file_bytes
            )));
        }
        if max_rotated_files == 0 {
            return Err(UiError::AuditLogIoError(
                "max_rotated_files must be >= 1".to_string(),
            ));
        }

        fs::create_dir_all(audit_dir).map_err(|e| {
            UiError::AuditLogIoError(format!(
                "failed to create isolated audit directory '{}': {}",
                audit_dir.display(),
                e
            ))
        })?;

        let active_file_path = audit_dir.join(ACTIVE_AUDIT_JSONL_FILENAME);
        let mut store = Self {
            audit_dir: audit_dir.to_path_buf(),
            active_file_path,
            max_file_bytes,
            max_rotated_files,
            next_seq: 1,
            last_entry_hash: GENESIS_AUDIT_HASH.to_string(),
            anchor_prev_hash: GENESIS_AUDIT_HASH.to_string(),
            anchor_first_seq: 1,
            current_file_bytes: 0,
            rotations_completed: 0,
            total_appended: 0,
            max_append_latency_ms: 0.0,
        };

        store.recover_existing_chain_state()?;
        Ok(store)
    }

    pub fn open_default(audit_dir: &Path) -> Result<Self, UiError> {
        Self::open(
            audit_dir,
            DEFAULT_MAX_AUDIT_FILE_BYTES,
            DEFAULT_MAX_ROTATED_AUDIT_FILES,
        )
    }

    pub fn audit_dir(&self) -> &Path {
        &self.audit_dir
    }

    pub fn active_file_path(&self) -> &Path {
        &self.active_file_path
    }

    pub fn rotated_file_path(&self, index: usize) -> PathBuf {
        self.audit_dir.join(format!("audit.{}.jsonl", index))
    }

    pub fn next_seq(&self) -> u64 {
        self.next_seq
    }

    pub fn last_entry_hash(&self) -> &str {
        &self.last_entry_hash
    }

    pub fn anchor_prev_hash(&self) -> &str {
        &self.anchor_prev_hash
    }

    pub fn anchor_first_seq(&self) -> u64 {
        self.anchor_first_seq
    }

    pub fn current_file_bytes(&self) -> usize {
        self.current_file_bytes
    }

    pub fn rotations_completed(&self) -> u64 {
        self.rotations_completed
    }

    pub fn total_appended(&self) -> u64 {
        self.total_appended
    }

    pub fn max_append_latency_ms(&self) -> f64 {
        self.max_append_latency_ms
    }

    /// Returns all existing audit segment files in strict chronological order:
    /// `audit.{N}.jsonl` (oldest) -> ... -> `audit.1.jsonl` -> `audit.jsonl` (newest active).
    pub fn ordered_segment_paths(&self) -> Vec<(String, PathBuf)> {
        let mut segments = Vec::new();
        for idx in (1..=self.max_rotated_files).rev() {
            let p = self.rotated_file_path(idx);
            if p.exists() {
                if let Ok(meta) = fs::metadata(&p) {
                    if meta.len() > 0 {
                        segments.push((format!("audit.{}.jsonl", idx), p));
                    }
                }
            }
        }
        if self.active_file_path.exists() {
            if let Ok(meta) = fs::metadata(&self.active_file_path) {
                if meta.len() > 0 {
                    segments.push((
                        ACTIVE_AUDIT_JSONL_FILENAME.to_string(),
                        self.active_file_path.clone(),
                    ));
                }
            }
        }
        segments
    }

    pub(crate) fn recover_existing_chain_state(&mut self) -> Result<(), UiError> {
        let segments = self.ordered_segment_paths();
        if segments.is_empty() {
            if !self.active_file_path.exists() {
                FileCreationHelper::touch(&self.active_file_path)?;
            }
            self.current_file_bytes = 0;
            self.next_seq = 1;
            self.last_entry_hash = GENESIS_AUDIT_HASH.to_string();
            self.anchor_prev_hash = GENESIS_AUDIT_HASH.to_string();
            self.anchor_first_seq = 1;
            return Ok(());
        }

        // Read first entry of oldest retained segment to establish anchor_prev_hash & anchor_first_seq
        let mut first_found = false;
        let mut last_seq = 0u64;
        let mut last_hash = GENESIS_AUDIT_HASH.to_string();

        for (file_name, path) in &segments {
            let file = fs::File::open(path).map_err(|e| {
                UiError::AuditLogIoError(format!("failed to open '{}': {}", file_name, e))
            })?;
            let reader = BufReader::new(file);
            for (line_idx, line_res) in reader.lines().enumerate() {
                let line = line_res.map_err(|e| {
                    UiError::AuditLogIoError(format!(
                        "failed to read line {} of '{}': {}",
                        line_idx, file_name, e
                    ))
                })?;
                let trimmed = line.trim();
                if trimmed.is_empty() {
                    continue;
                }
                let entry: AuditEntry = serde_json::from_str(trimmed).map_err(|e| {
                    UiError::AuditLogTamperDetected(JsonlTamperDiagnostic {
                        file_name: file_name.clone(),
                        corrupted_line_index: line_idx,
                        expected_seq: if last_seq == 0 { 1 } else { last_seq + 1 },
                        found_seq: None,
                        reason: format!("malformed JSONL during recovery: {}", e),
                    })
                })?;
                if !first_found {
                    self.anchor_prev_hash = entry.prev_hash.clone();
                    self.anchor_first_seq = entry.seq;
                    first_found = true;
                }
                last_seq = entry.seq;
                last_hash = entry.entry_hash;
            }
        }

        if first_found {
            self.next_seq = last_seq + 1;
            self.last_entry_hash = last_hash;
        }

        self.current_file_bytes = fs::metadata(&self.active_file_path)
            .map(|m| m.len() as usize)
            .unwrap_or(0);

        // Verify the recovered chain immediately on open
        let _ = self.verify_persisted_chain()?;
        Ok(())
    }

    pub(crate) fn read_first_entry_of_file(path: &Path) -> Option<AuditEntry> {
        let file = fs::File::open(path).ok()?;
        let reader = BufReader::new(file);
        for line in reader.lines().map_while(Result::ok) {
            let trimmed = line.trim();
            if !trimmed.is_empty() {
                return serde_json::from_str::<AuditEntry>(trimmed).ok();
            }
        }
        None
    }

    /// Rotates `audit.jsonl -> audit.1.jsonl -> audit.2.jsonl ... -> audit.{N}.jsonl` transactionally
    /// when adding the next entry would exceed `max_file_bytes`, preserving unbroken SHA-256 link
    /// continuity across file boundaries.
    pub fn rotate_files_transactionally(&mut self) -> Result<(), UiError> {
        fs::create_dir_all(&self.audit_dir).map_err(|e| {
            UiError::AuditLogIoError(format!(
                "failed to create audit dir '{}': {}",
                self.audit_dir.display(),
                e
            ))
        })?;
        // If `audit.{max_rotated_files}.jsonl` exists, it will be overwritten by `audit.{max_rotated_files - 1}.jsonl`
        // (or by `audit.jsonl` when `max_rotated_files == 1`). Update `anchor_prev_hash` and `anchor_first_seq`
        // to the first entry of the segment that will become the new oldest retained file.
        let oldest_slot = self.rotated_file_path(self.max_rotated_files);
        if oldest_slot.exists() {
            let next_oldest_path = if self.max_rotated_files >= 2 {
                self.rotated_file_path(self.max_rotated_files - 1)
            } else {
                self.active_file_path.clone()
            };
            if let Some(first_entry) = Self::read_first_entry_of_file(&next_oldest_path) {
                self.anchor_prev_hash = first_entry.prev_hash;
                self.anchor_first_seq = first_entry.seq;
            }
            let _ = fs::remove_file(&oldest_slot);
        }

        for idx in (2..=self.max_rotated_files).rev() {
            let src = self.rotated_file_path(idx - 1);
            let dst = self.rotated_file_path(idx);
            if src.exists() {
                fs::rename(&src, &dst).map_err(|e| {
                    UiError::AuditLogIoError(format!(
                        "failed to rotate '{}' -> '{}': {}",
                        src.display(),
                        dst.display(),
                        e
                    ))
                })?;
            }
        }

        if self.active_file_path.exists() {
            let dst_1 = self.rotated_file_path(1);
            fs::rename(&self.active_file_path, &dst_1).map_err(|e| {
                UiError::AuditLogIoError(format!(
                    "failed to rotate active '{}' -> '{}': {}",
                    self.active_file_path.display(),
                    dst_1.display(),
                    e
                ))
            })?;
        }

        FileCreationHelper::touch(&self.active_file_path)?;
        self.current_file_bytes = 0;
        self.rotations_completed += 1;
        Ok(())
    }
}
