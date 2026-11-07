#![allow(unused_imports)]
use crate::prelude::*;

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct AuditViewerFilter {
    pub agent_exact: Option<String>,
    pub verdict_exact: Option<String>,
    pub query_substring: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AuditViewerRow {
    pub seq: u64,
    pub timestamp_ms: u64,
    pub agent_cell: WebViewIpcPayloadV1,
    pub capability_cell: WebViewIpcPayloadV1,
    pub target_cell: WebViewIpcPayloadV1,
    pub verdict_cell: WebViewIpcPayloadV1,
    pub reason_cell: WebViewIpcPayloadV1,
    pub hash_badge_cell: WebViewIpcPayloadV1,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AuditViewerPage {
    pub page_index: usize,
    pub page_size: usize,
    pub total_matching_entries: usize,
    pub total_pages: usize,
    pub chain_integrity_verified: bool,
    pub integrity_status_badge: WebViewIpcPayloadV1,
    pub rows: Vec<AuditViewerRow>,
}

pub struct SafeWebViewAuditViewer;

impl SafeWebViewAuditViewer {
    /// Renders a paginated, filtered view of the persistent audit log where every single cell
    /// is wrapped in `WebViewIpcPayloadV1::DomSafeTextRender` (`textContent`-only with HTML entity
    /// escaping), neutralizing any XSS payload stored in audited file paths or shell arguments.
    pub fn render_page(
        store: &PersistentJsonlAuditStore,
        filter: &AuditViewerFilter,
        page_index: usize,
        requested_page_size: usize,
    ) -> Result<AuditViewerPage, UiError> {
        let page_size = requested_page_size.clamp(1, MAX_AUDIT_VIEWER_PAGE_SIZE);

        let (chain_integrity_verified, integrity_status_badge, entries) =
            match store.verify_persisted_chain() {
                Ok(report) => {
                    let badge = sanitize_for_text_content(
                        "miku-audit-integrity-badge",
                        &format!(
                            "CHAIN_VERIFIED_OK (entries={} files={} tip={})",
                            report.verified_entries,
                            report.verified_files,
                            &report.tip_entry_hash[..12.min(report.tip_entry_hash.len())]
                        ),
                    )?;
                    let loaded = store.load_all_entries().unwrap_or_default();
                    (true, badge, loaded)
                }
                Err(UiError::AuditLogTamperDetected(diag)) => {
                    let badge = sanitize_for_text_content(
                        "miku-audit-integrity-badge",
                        &format!(
                            "TAMPER_DETECTED_LINE_{} [file={}]: {}",
                            diag.corrupted_line_index, diag.file_name, diag.reason
                        ),
                    )?;
                    let loaded = store.load_all_entries().unwrap_or_default();
                    (false, badge, loaded)
                }
                Err(other) => return Err(other),
            };

        let query_lower = filter
            .query_substring
            .as_ref()
            .map(|q| q.trim().to_ascii_lowercase())
            .filter(|q| !q.is_empty());

        let matching: Vec<&AuditEntry> = entries
            .iter()
            .filter(|e| {
                if let Some(ref ag) = filter.agent_exact {
                    if !e.agent.eq_ignore_ascii_case(ag.trim()) {
                        return false;
                    }
                }
                if let Some(ref vd) = filter.verdict_exact {
                    if !e.verdict.eq_ignore_ascii_case(vd.trim()) {
                        return false;
                    }
                }
                if let Some(ref q) = query_lower {
                    let hit = e.agent.to_ascii_lowercase().contains(q)
                        || e.capability.to_ascii_lowercase().contains(q)
                        || e.target.to_ascii_lowercase().contains(q)
                        || e.verdict.to_ascii_lowercase().contains(q)
                        || e.reason.to_ascii_lowercase().contains(q);
                    if !hit {
                        return false;
                    }
                }
                true
            })
            .collect();

        let total_matching_entries = matching.len();
        let total_pages = if total_matching_entries == 0 {
            1
        } else {
            total_matching_entries.div_ceil(page_size)
        };

        let start = page_index.saturating_mul(page_size);
        let slice = if start < total_matching_entries {
            let end = (start + page_size).min(total_matching_entries);
            &matching[start..end]
        } else {
            &[]
        };

        let mut rows = Vec::with_capacity(slice.len());
        for (row_offset, entry) in slice.iter().enumerate() {
            let short_prev = &entry.prev_hash[..8.min(entry.prev_hash.len())];
            let short_curr = &entry.entry_hash[..8.min(entry.entry_hash.len())];
            rows.push(AuditViewerRow {
                seq: entry.seq,
                timestamp_ms: entry.timestamp_ms,
                agent_cell: sanitize_for_text_content(
                    &format!("miku-audit-agent-{}", row_offset),
                    &entry.agent,
                )?,
                capability_cell: sanitize_for_text_content(
                    &format!("miku-audit-cap-{}", row_offset),
                    &entry.capability,
                )?,
                target_cell: sanitize_for_text_content(
                    &format!("miku-audit-target-{}", row_offset),
                    &entry.target,
                )?,
                verdict_cell: sanitize_for_text_content(
                    &format!("miku-audit-verdict-{}", row_offset),
                    &entry.verdict,
                )?,
                reason_cell: sanitize_for_text_content(
                    &format!("miku-audit-reason-{}", row_offset),
                    &entry.reason,
                )?,
                hash_badge_cell: sanitize_for_text_content(
                    &format!("miku-audit-hash-{}", row_offset),
                    &format!("{} -> {}", short_prev, short_curr),
                )?,
            });
        }

        Ok(AuditViewerPage {
            page_index,
            page_size,
            total_matching_entries,
            total_pages,
            chain_integrity_verified,
            integrity_status_badge,
            rows,
        })
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PersistentJsonlAuditBenchmarkReport {
    pub appended_entries: usize,
    pub verified_entries: usize,
    pub rotations_completed: u64,
    pub segment_files_verified: usize,
    pub viewer_rows_rendered: usize,
    pub viewer_xss_neutralized: bool,
    pub tamper_detected_at_exact_line: bool,
    pub max_append_latency_ms: f64,
    pub verify_latency_ms: f64,
}

pub fn run_persistent_jsonl_audit_store_benchmark(
    cycles: usize,
) -> Result<PersistentJsonlAuditBenchmarkReport, UiError> {
    let unique_dir = create_unique_isolated_audit_temp_dir("miku_audit_step27_bench", cycles)?;

    let result = (|| -> Result<PersistentJsonlAuditBenchmarkReport, UiError> {
        // Use a compact 2,400-byte threshold so `cycles * 10` entries deterministically rotate
        // across multiple segment files (`audit.2.jsonl -> audit.1.jsonl -> audit.jsonl`).
        let mut store = PersistentJsonlAuditStore::open(&unique_dir, 2_400, 4)?;
        let total_to_append = cycles.max(1) * 10;

        for i in 0..total_to_append {
            let ts = 50_000 + (i as u64) * 25;
            let (agent, cap, target, verdict, reason) = match i % 5 {
                0 => (
                    "claude_code",
                    "fs_read",
                    "src/lib.rs",
                    "allow",
                    "ALLOW_RULE_MATCH",
                ),
                1 => (
                    "cursor",
                    "network",
                    "https://evil.example.com/<script>alert(1)</script>",
                    "deny",
                    "DEFAULT_DENY_UNLISTED",
                ),
                2 => (
                    "claude_code",
                    "shell",
                    "git commit -m \"<img src=x onerror=alert(1)>\"",
                    "allow",
                    "INTERACTIVE_USER_APPROVED",
                ),
                3 => (
                    "antigravity",
                    "fs_write",
                    r"..\..\Windows\System32\drivers\etc\hosts",
                    "deny",
                    "PATH_JAIL_VIOLATION",
                ),
                _ => (
                    "claude_code",
                    "shell",
                    "rm -rf /",
                    "deny",
                    "EXPLICIT_DENY_RULE",
                ),
            };
            store.append_entry(ts, agent, cap, target, verdict, reason)?;
        }

        let verify_report = store.verify_persisted_chain()?;
        if verify_report.verified_entries != total_to_append {
            return Err(UiError::AuditLogIoError(format!(
                "expected {} verified entries across rotated segments, got {}",
                total_to_append, verify_report.verified_entries
            )));
        }

        let page = SafeWebViewAuditViewer::render_page(
            &store,
            &AuditViewerFilter::default(),
            0,
            MAX_AUDIT_VIEWER_PAGE_SIZE,
        )?;
        let mut viewer_xss_neutralized = page.chain_integrity_verified && !page.rows.is_empty();
        for row in &page.rows {
            if let WebViewIpcPayloadV1::DomSafeTextRender {
                safe_text,
                csp_enforced,
                ..
            } = &row.target_cell
            {
                if !csp_enforced || safe_text.contains("<script>") || safe_text.contains("<img") {
                    viewer_xss_neutralized = false;
                }
            } else {
                viewer_xss_neutralized = false;
            }
        }

        // Verify tamper detection pinpoints exact line index on the active file
        let active_raw = fs::read_to_string(store.active_file_path())
            .map_err(|e| UiError::AuditLogIoError(e.to_string()))?;
        let mut lines: Vec<String> = active_raw.lines().map(|s| s.to_string()).collect();
        let mut tamper_detected_at_exact_line = false;
        if let Some(first_line) = lines.get_mut(0) {
            let first_entry: AuditEntry = serde_json::from_str(first_line)
                .map_err(|e| UiError::AuditLogIoError(e.to_string()))?;
            let init_prev = first_entry.prev_hash.clone();
            let init_seq = first_entry.seq;
            *first_line = first_line.replace("\"allow\"", "\"forged_allow\"").replace("\"deny\"", "\"forged_allow\"");
            let corrupted_raw = lines.join("\n");
            if let Err(UiError::AuditLogTamperDetected(diag)) =
                PersistentJsonlAuditStore::verify_single_jsonl_str(
                    ACTIVE_AUDIT_JSONL_FILENAME,
                    &corrupted_raw,
                    &init_prev,
                    init_seq,
                )
            {
                tamper_detected_at_exact_line = diag.corrupted_line_index == 0;
            }
        }

        Ok(PersistentJsonlAuditBenchmarkReport {
            appended_entries: total_to_append,
            verified_entries: verify_report.verified_entries,
            rotations_completed: store.rotations_completed(),
            segment_files_verified: verify_report.verified_files,
            viewer_rows_rendered: page.rows.len(),
            viewer_xss_neutralized,
            tamper_detected_at_exact_line,
            max_append_latency_ms: store.max_append_latency_ms(),
            verify_latency_ms: verify_report.elapsed_ms,
        })
    })();

    let _ = fs::remove_dir_all(&unique_dir);
    result
}

