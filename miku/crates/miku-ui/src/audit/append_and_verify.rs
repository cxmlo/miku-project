#![allow(unused_imports)]
use crate::prelude::*;

impl PersistentJsonlAuditStore {
    fn stage_entry_line(
        &mut self,
        batch_buf: &mut String,
        timestamp_ms: u64,
        agent: &str,
        capability: &str,
        target: &str,
        verdict: &str,
        reason: &str,
    ) -> Result<AuditEntry, UiError> {
        let seq = self.next_seq;
        let prev_hash = self.last_entry_hash.clone();
        let entry_hash = AuditEntry::compute_hash(
            &prev_hash,
            seq,
            timestamp_ms,
            agent,
            capability,
            target,
            verdict,
            reason,
        );

        let entry = AuditEntry {
            seq,
            timestamp_ms,
            agent: agent.to_string(),
            capability: capability.to_string(),
            target: target.to_string(),
            verdict: verdict.to_string(),
            reason: reason.to_string(),
            prev_hash,
            entry_hash: entry_hash.clone(),
        };

        let mut serialized = serde_json::to_string(&entry).map_err(|e| {
            UiError::AuditLogIoError(format!("failed to serialize AuditEntry: {}", e))
        })?;
        serialized.push('\n');
        let line_len = serialized.len();

        if self.current_file_bytes + batch_buf.len() > 0
            && self.current_file_bytes + batch_buf.len() + line_len > self.max_file_bytes
        {
            self.flush_staged_batch(batch_buf)?;
            self.rotate_files_transactionally()?;
        }

        batch_buf.push_str(&serialized);
        self.next_seq += 1;
        self.last_entry_hash = entry_hash;
        self.total_appended += 1;

        Ok(entry)
    }

    fn flush_staged_batch(&mut self, batch_buf: &mut String) -> Result<(), UiError> {
        if batch_buf.is_empty() {
            return Ok(());
        }

        if self.current_file_bytes == 0 && !self.active_file_path.exists() {
            fs::create_dir_all(&self.audit_dir).map_err(|e| {
                UiError::AuditLogIoError(format!(
                    "failed to create audit dir '{}': {}",
                    self.audit_dir.display(),
                    e
                ))
            })?;
        }

        let mut file = OpenOptions::new()
            .create(true)
            .append(true)
            .open(&self.active_file_path)
            .map_err(|e| {
                UiError::AuditLogIoError(format!(
                    "failed to open '{}' for append: {}",
                    self.active_file_path.display(),
                    e
                ))
            })?;
        let line_bytes = batch_buf.as_bytes();
        file.write_all(line_bytes).map_err(|e| {
            UiError::AuditLogIoError(format!(
                "failed to append JSONL line to '{}': {}",
                self.active_file_path.display(),
                e
            ))
        })?;
        file.flush().map_err(|e| {
            UiError::AuditLogIoError(format!(
                "failed to flush '{}': {}",
                self.active_file_path.display(),
                e
            ))
        })?;

        self.current_file_bytes += line_bytes.len();
        batch_buf.clear();
        Ok(())
    }

    /// Appends a new tamper-evident `AuditEntry` as a single JSONL line to `audit.jsonl`,
    /// performing transactional size rotation first if `current_file_bytes + line_len > max_file_bytes`.
    pub fn append_entry(
        &mut self,
        timestamp_ms: u64,
        agent: &str,
        capability: &str,
        target: &str,
        verdict: &str,
        reason: &str,
    ) -> Result<AuditEntry, UiError> {
        let t0 = Instant::now();
        let mut batch_buf = String::with_capacity(256);
        let entry = self.stage_entry_line(
            &mut batch_buf,
            timestamp_ms,
            agent,
            capability,
            target,
            verdict,
            reason,
        )?;
        self.flush_staged_batch(&mut batch_buf)?;

        let elapsed_ms = t0.elapsed().as_secs_f64() * 1000.0;
        if elapsed_ms > self.max_append_latency_ms {
            self.max_append_latency_ms = elapsed_ms;
        }

        Ok(entry)
    }

    /// Synchronizes any unpersisted entries from a `GatekeeperEngine`'s in-memory `TamperEvidentAuditLog`
    /// into this persistent JSONL store using a batched write.
    pub fn sync_from_gatekeeper_log(
        &mut self,
        log: &TamperEvidentAuditLog,
    ) -> Result<usize, UiError> {
        let t0 = Instant::now();
        let mut appended = 0usize;
        let mut batch_buf = String::new();
        for entry in log.entries() {
            if entry.seq >= self.next_seq {
                self.stage_entry_line(
                    &mut batch_buf,
                    entry.timestamp_ms,
                    &entry.agent,
                    &entry.capability,
                    &entry.target,
                    &entry.verdict,
                    &entry.reason,
                )?;
                appended += 1;
            }
        }
        if appended > 0 {
            self.flush_staged_batch(&mut batch_buf)?;
            let elapsed_ms = t0.elapsed().as_secs_f64() * 1000.0;
            if elapsed_ms > self.max_append_latency_ms {
                self.max_append_latency_ms = elapsed_ms;
            }
        }
        Ok(appended)
    }

    /// Atomically synchronizes unpersisted `GatekeeperEngine` log entries and appends a new
    /// tamper-evident `AuditEntry` in a single batched filesystem write.
    pub fn sync_gatekeeper_and_append_entry(
        &mut self,
        log: &TamperEvidentAuditLog,
        timestamp_ms: u64,
        agent: &str,
        capability: &str,
        target: &str,
        verdict: &str,
        reason: &str,
    ) -> Result<AuditEntry, UiError> {
        let t0 = Instant::now();
        let mut batch_buf = String::with_capacity(1024);
        for entry in log.entries() {
            if entry.seq >= self.next_seq {
                self.stage_entry_line(
                    &mut batch_buf,
                    entry.timestamp_ms,
                    &entry.agent,
                    &entry.capability,
                    &entry.target,
                    &entry.verdict,
                    &entry.reason,
                )?;
            }
        }
        let final_entry = self.stage_entry_line(
            &mut batch_buf,
            timestamp_ms,
            agent,
            capability,
            target,
            verdict,
            reason,
        )?;
        self.flush_staged_batch(&mut batch_buf)?;

        let elapsed_ms = t0.elapsed().as_secs_f64() * 1000.0;
        if elapsed_ms > self.max_append_latency_ms {
            self.max_append_latency_ms = elapsed_ms;
        }

        Ok(final_entry)
    }

    /// Fast verifier for a single JSONL buffer, pinpointing the exact 0-based `corrupted_line_index`
    /// on malformed JSON, sequence gap/swap, broken `prev_hash`, or altered `entry_hash`.
    pub fn verify_single_jsonl_str(
        file_name: &str,
        raw_jsonl: &str,
        initial_prev_hash: &str,
        initial_seq: u64,
    ) -> Result<JsonlChainVerificationReport, UiError> {
        use std::borrow::Cow;
        use std::fmt::Write;

        #[derive(Deserialize)]
        struct BorrowedAuditEntry<'a> {
            seq: u64,
            timestamp_ms: u64,
            #[serde(borrow)]
            agent: Cow<'a, str>,
            #[serde(borrow)]
            capability: Cow<'a, str>,
            #[serde(borrow)]
            target: Cow<'a, str>,
            #[serde(borrow)]
            verdict: Cow<'a, str>,
            #[serde(borrow)]
            reason: Cow<'a, str>,
            #[serde(borrow)]
            prev_hash: Cow<'a, str>,
            #[serde(borrow)]
            entry_hash: Cow<'a, str>,
        }

        let t0 = Instant::now();
        let mut expected_prev: Cow<'_, str> = Cow::Borrowed(initial_prev_hash);
        let mut expected_seq = initial_seq;
        let mut verified_entries = 0usize;
        let mut last_seq = 0u64;
        let mut canonical_buf = String::with_capacity(256);

        for (line_idx, raw_line) in raw_jsonl.lines().enumerate() {
            let trimmed = raw_line.trim();
            if trimmed.is_empty() {
                continue;
            }
            let entry: BorrowedAuditEntry<'_> = serde_json::from_str(trimmed).map_err(|e| {
                UiError::AuditLogTamperDetected(JsonlTamperDiagnostic {
                    file_name: file_name.to_string(),
                    corrupted_line_index: line_idx,
                    expected_seq,
                    found_seq: None,
                    reason: format!("malformed JSONL syntax: {}", e),
                })
            })?;

            if entry.seq != expected_seq {
                return Err(UiError::AuditLogTamperDetected(JsonlTamperDiagnostic {
                    file_name: file_name.to_string(),
                    corrupted_line_index: line_idx,
                    expected_seq,
                    found_seq: Some(entry.seq),
                    reason: format!(
                        "sequence discontinuity (deletion or reordering): expected seq {}, found {}",
                        expected_seq, entry.seq
                    ),
                }));
            }

            if entry.prev_hash.as_ref() != expected_prev.as_ref() {
                return Err(UiError::AuditLogTamperDetected(JsonlTamperDiagnostic {
                    file_name: file_name.to_string(),
                    corrupted_line_index: line_idx,
                    expected_seq,
                    found_seq: Some(entry.seq),
                    reason: format!(
                        "broken prev_hash link: expected {}, found {}",
                        expected_prev, entry.prev_hash
                    ),
                }));
            }

            canonical_buf.clear();
            let _ = write!(
                &mut canonical_buf,
                "MIKU_AUDIT_V1|prev={}|seq={}|ts={}|agent={}|cap={}|target={}|verdict={}|reason={}",
                entry.prev_hash,
                entry.seq,
                entry.timestamp_ms,
                entry.agent,
                entry.capability,
                entry.target,
                entry.verdict,
                entry.reason
            );
            let recomputed_bytes = miku_protocol::sha256_hex_bytes(canonical_buf.as_bytes());
            if recomputed_bytes.as_slice() != entry.entry_hash.as_bytes() {
                let recomputed_str = std::str::from_utf8(&recomputed_bytes).unwrap_or("");
                return Err(UiError::AuditLogTamperDetected(JsonlTamperDiagnostic {
                    file_name: file_name.to_string(),
                    corrupted_line_index: line_idx,
                    expected_seq,
                    found_seq: Some(entry.seq),
                    reason: format!(
                        "entry_hash cryptographic mismatch: recomputed {}, stored {}",
                        recomputed_str, entry.entry_hash
                    ),
                }));
            }

            expected_prev = entry.entry_hash;
            last_seq = entry.seq;
            expected_seq += 1;
            verified_entries += 1;
        }

        let elapsed_ms = t0.elapsed().as_secs_f64() * 1000.0;
        Ok(JsonlChainVerificationReport {
            verified_entries,
            verified_files: 1,
            first_seq: if verified_entries > 0 { initial_seq } else { 0 },
            last_seq,
            anchor_prev_hash: initial_prev_hash.to_string(),
            tip_entry_hash: expected_prev.into_owned(),
            elapsed_ms,
        })
    }

    /// Verifies the entire persisted chain across all rotated JSONL segments (`audit.N.jsonl -> ... -> audit.jsonl`).
    /// Pinpoints the exact file and 0-based line index if any entry is modified, deleted, or reordered.
    pub fn verify_persisted_chain(&self) -> Result<JsonlChainVerificationReport, UiError> {
        let t0 = Instant::now();
        let segments = self.ordered_segment_paths();
        if segments.is_empty() {
            return Ok(JsonlChainVerificationReport {
                verified_entries: 0,
                verified_files: 0,
                first_seq: 0,
                last_seq: 0,
                anchor_prev_hash: self.anchor_prev_hash.clone(),
                tip_entry_hash: self.anchor_prev_hash.clone(),
                elapsed_ms: t0.elapsed().as_secs_f64() * 1000.0,
            });
        }

        let mut expected_prev = self.anchor_prev_hash.clone();
        let mut expected_seq = self.anchor_first_seq;
        let mut total_entries = 0usize;
        let mut last_seq = 0u64;

        for (file_name, path) in &segments {
            let raw = fs::read_to_string(path).map_err(|e| {
                UiError::AuditLogIoError(format!("failed to read '{}': {}", file_name, e))
            })?;
            let sub = Self::verify_single_jsonl_str(
                file_name,
                &raw,
                &expected_prev,
                expected_seq,
            )?;
            if sub.verified_entries > 0 {
                expected_prev = sub.tip_entry_hash;
                last_seq = sub.last_seq;
                expected_seq = last_seq + 1;
                total_entries += sub.verified_entries;
            }
        }

        Ok(JsonlChainVerificationReport {
            verified_entries: total_entries,
            verified_files: segments.len(),
            first_seq: if total_entries > 0 {
                self.anchor_first_seq
            } else {
                0
            },
            last_seq,
            anchor_prev_hash: self.anchor_prev_hash.clone(),
            tip_entry_hash: expected_prev,
            elapsed_ms: t0.elapsed().as_secs_f64() * 1000.0,
        })
    }

    /// Alias for `verify_persisted_chain` verifying all retained active and rotated JSONL segments.
    pub fn verify_all_retained_files(&self) -> Result<JsonlChainVerificationReport, UiError> {
        self.verify_persisted_chain()
    }

    /// Loads all persisted `AuditEntry` records across rotated and active segments in chronological order.
    pub fn load_all_entries(&self) -> Result<Vec<AuditEntry>, UiError> {
        let segments = self.ordered_segment_paths();
        let mut entries = Vec::new();
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
                        expected_seq: (entries.len() as u64) + self.anchor_first_seq,
                        found_seq: None,
                        reason: format!("malformed JSONL entry: {}", e),
                    })
                })?;
                entries.push(entry);
            }
        }
        Ok(entries)
    }
}

pub(crate) struct FileCreationHelper;
impl FileCreationHelper {
    pub(crate) fn touch(path: &Path) -> Result<(), UiError> {
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).map_err(|e| {
                UiError::AuditLogIoError(format!(
                    "failed to create parent directory '{}': {}",
                    parent.display(),
                    e
                ))
            })?;
        }
        OpenOptions::new()
            .create(true)
            .write(true)
            .truncate(true)
            .open(path)
            .map(|_| ())
            .map_err(|e| {
                UiError::AuditLogIoError(format!(
                    "failed to initialize audit file '{}': {}",
                    path.display(),
                    e
                ))
            })
    }
}
