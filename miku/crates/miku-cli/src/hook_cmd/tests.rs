use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Mutex, MutexGuard};

use miku_gatekeeper::{Capability, GatekeeperEngine, PolicyEffect, ProjectProfile};
use miku_protocol::sha256_hex;

use super::{
    execute_cli_hook_command_with_prompter, install_miku_hook, restore_miku_hook, HookAuditStep,
    HookConfirmationPrompter, HookInstallError, HookTargetAgent, HOOK_OP_BUDGET_MS,
    MIKU_HOOK_ENTRY_ID,
};

static TEST_COUNTER: AtomicU64 = AtomicU64::new(1);
static HOOK_TEST_IO_LOCK: Mutex<()> = Mutex::new(());

fn acquire_hook_test_lock() -> MutexGuard<'static, ()> {
    HOOK_TEST_IO_LOCK
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

fn create_temp_config_dir(tag: &str) -> PathBuf {
    let seq = TEST_COUNTER.fetch_add(1, Ordering::SeqCst);
    let dir = std::env::temp_dir().join(format!(
        "miku_cli_step313_{}_{}_{}",
        tag,
        std::process::id(),
        seq
    ));
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(&dir).expect("create temp config dir");
    let warmup_path = dir.join(".warmup.bak");
    let _ = fs::write(&warmup_path, b"{}");
    let _ = fs::read(&warmup_path);
    let _ = fs::remove_file(&warmup_path);
    dir
}

fn build_allow_gatekeeper(root: &Path) -> GatekeeperEngine {
    let root_str = root.to_string_lossy().replace('\\', "/");
    let profile = ProjectProfile::new("miku-hook-test", &root_str, 60_000)
        .with_rule(Capability::FsRead, "**", PolicyEffect::Allow, false)
        .with_rule(Capability::FsWrite, "**", PolicyEffect::Allow, false);
    GatekeeperEngine::new(profile).expect("build allow GatekeeperEngine")
}

struct FixedPrompter {
    approve: bool,
}

impl HookConfirmationPrompter for FixedPrompter {
    fn confirm_after_diff(&self, _unified_diff: &str, _backup_path: &Path) -> Result<bool, String> {
        Ok(self.approve)
    }
}

// LEDGER-3.13-1
#[test]
fn test_merge_miku_hook_preserves_all_existing_user_keys_and_values() {
    let _io_guard = acquire_hook_test_lock();
    let dir = create_temp_config_dir("preserve_keys");
    let mut gk = build_allow_gatekeeper(&dir);
    let claude_path = dir.join("claude_settings.json");
    let initial_claude = r#"{
  "alwaysThinkingEnabled": true,
  "env": { "CLAUDE_CODE_MAX_OUTPUT_TOKENS": "16384", "CUSTOM_USER_FLAG": "keep-me" },
  "model": "claude-3-7-sonnet-20250219",
  "permissions": { "allow": ["Bash(cargo test:*)"], "deny": ["Read(.env)"] },
  "userCustomObject": { "nestedArray": [1, 2, 3], "theme": "tokyo-night" }
}
"#;
    fs::write(&claude_path, initial_claude).expect("write initial claude settings");

    let prompter = FixedPrompter { approve: true };
    let outcome = install_miku_hook(
        &dir, &claude_path, HookTargetAgent::ClaudeCode, None, &prompter, &mut gk, 1_700_000_001_000,
    )
    .expect("install_miku_hook on claude_settings.json must succeed");
    assert!(outcome.applied);
    assert!(!outcome.idempotent_noop);

    let updated_raw = fs::read_to_string(&claude_path).expect("read updated claude settings");
    let parsed: serde_json::Value = serde_json::from_str(&updated_raw).expect("valid JSON");
    assert_eq!(parsed["alwaysThinkingEnabled"], serde_json::json!(true));
    assert_eq!(parsed["env"]["CLAUDE_CODE_MAX_OUTPUT_TOKENS"], serde_json::json!("16384"));
    assert_eq!(parsed["env"]["CUSTOM_USER_FLAG"], serde_json::json!("keep-me"));
    assert_eq!(parsed["model"], serde_json::json!("claude-3-7-sonnet-20250219"));
    assert_eq!(parsed["permissions"]["allow"], serde_json::json!(["Bash(cargo test:*)"]));
    assert_eq!(parsed["permissions"]["deny"], serde_json::json!(["Read(.env)"]));
    assert_eq!(parsed["userCustomObject"]["nestedArray"], serde_json::json!([1, 2, 3]));
    assert_eq!(parsed["userCustomObject"]["theme"], serde_json::json!("tokyo-night"));
    assert!(parsed["hooks"]["PreToolUse"].is_array());
    assert!(parsed["hooks"]["PostToolUse"].is_array());
    assert!(parsed["hooks"]["Notification"].is_array());

    let cursor_path = dir.join("cursor_settings.json");
    let initial_cursor = r#"{
  "cursor.aipreview.enabled": true,
  "editor.fontSize": 14,
  "workbench.colorTheme": "Default Dark Modern"
}
"#;
    fs::write(&cursor_path, initial_cursor).expect("write initial cursor settings");
    let cursor_outcome = install_miku_hook(
        &dir, &cursor_path, HookTargetAgent::Cursor, None, &prompter, &mut gk, 1_700_000_002_000,
    )
    .expect("install_miku_hook on cursor_settings.json must succeed");
    assert!(cursor_outcome.applied);

    let cursor_parsed: serde_json::Value =
        serde_json::from_str(&fs::read_to_string(&cursor_path).unwrap()).unwrap();
    assert_eq!(cursor_parsed["cursor.aipreview.enabled"], serde_json::json!(true));
    assert_eq!(cursor_parsed["editor.fontSize"], serde_json::json!(14));
    assert_eq!(cursor_parsed["workbench.colorTheme"], serde_json::json!("Default Dark Modern"));
    assert_eq!(cursor_parsed["mcpServers"][MIKU_HOOK_ENTRY_ID]["id"], serde_json::json!(MIKU_HOOK_ENTRY_ID));

    let _ = fs::remove_dir_all(&dir);
}

// LEDGER-3.13-2
#[test]
fn test_repeated_hook_install_is_idempotent_and_never_duplicates_hooks() {
    let _io_guard = acquire_hook_test_lock();
    let dir = create_temp_config_dir("idempotent");
    let mut gk = build_allow_gatekeeper(&dir);
    let settings_path = dir.join("settings.json");
    let initial = r#"{
  "model": "claude-3-7-sonnet",
  "hooks": {
    "PreToolUse": [
      { "id": "user-custom-linter", "matcher": "Edit", "type": "command", "command": "cargo check" }
    ]
  }
}
"#;
    fs::write(&settings_path, initial).expect("write initial settings");

    let prompter = FixedPrompter { approve: true };
    let first = install_miku_hook(
        &dir, &settings_path, HookTargetAgent::ClaudeCode, None, &prompter, &mut gk, 1_700_000_010_000,
    )
    .expect("first install must succeed");
    assert!(first.applied);
    assert!(!first.idempotent_noop);
    let bytes_after_first = fs::read(&settings_path).expect("read after first install");

    let second = install_miku_hook(
        &dir, &settings_path, HookTargetAgent::ClaudeCode, None, &prompter, &mut gk, 1_700_000_020_000,
    )
    .expect("second install must succeed idempotently");
    assert!(second.applied);
    assert!(second.idempotent_noop, "second install must detect idempotent state");

    let bytes_after_second = fs::read(&settings_path).expect("read after second install");
    assert_eq!(bytes_after_first, bytes_after_second, "idempotent install must produce identical bytes");

    let parsed: serde_json::Value = serde_json::from_slice(&bytes_after_second).unwrap();
    for event_key in ["PreToolUse", "PostToolUse", "Notification"] {
        let arr = parsed["hooks"][event_key].as_array().expect("event hook array");
        let miku_count = arr
            .iter()
            .filter(|item| item.get("id").and_then(|v| v.as_str()) == Some(MIKU_HOOK_ENTRY_ID))
            .count();
        assert_eq!(miku_count, 1, "event '{}' must have exactly 1 Miku entry", event_key);
    }

    let _ = fs::remove_dir_all(&dir);
}

// LEDGER-3.13-3
#[test]
fn test_backup_created_and_validated_before_write_and_diff_shown_before_confirmation() {
    let _io_guard = acquire_hook_test_lock();
    let dir = create_temp_config_dir("backup_and_diff_order");
    let mut gk = build_allow_gatekeeper(&dir);
    let settings_path = dir.join("settings.json");

    let original_content = "{\n  \"theme\": \"dark\",\n  \"telemetry\": false\n}\n";
    fs::write(&settings_path, original_content).expect("write original settings");
    let _ = fs::read(&settings_path);
    let original_bytes = original_content.as_bytes().to_vec();

    struct OrderVerifyingPrompter {
        settings_path: PathBuf,
        expected_original_bytes: Vec<u8>,
        verified_at_prompt_time: AtomicBool,
    }

    impl HookConfirmationPrompter for OrderVerifyingPrompter {
        fn confirm_after_diff(&self, unified_diff: &str, backup_path: &Path) -> Result<bool, String> {
            assert!(unified_diff.contains("--- a/settings.json"));
            assert!(unified_diff.contains("+++ b/settings.json"));
            assert!(unified_diff.contains("@@"));
            assert!(unified_diff.contains(MIKU_HOOK_ENTRY_ID));
            let bak_bytes = fs::read(backup_path)
                .expect("backup .bak must exist and be readable before confirmation/write");
            assert_eq!(bak_bytes, self.expected_original_bytes);

            let active_bytes = fs::read(&self.settings_path).expect("read active file at prompt time");
            assert_eq!(
                active_bytes, self.expected_original_bytes,
                "settings.json must NOT be modified before confirmation completes"
            );
            self.verified_at_prompt_time.store(true, Ordering::SeqCst);
            Ok(true)
        }
    }

    let spy_prompter = OrderVerifyingPrompter {
        settings_path: settings_path.clone(),
        expected_original_bytes: original_bytes,
        verified_at_prompt_time: AtomicBool::new(false),
    };

    let outcome = install_miku_hook(
        &dir, &settings_path, HookTargetAgent::ClaudeCode, None, &spy_prompter, &mut gk, 1_700_000_030_000,
    )
    .expect("install with spy prompter must succeed");

    println!(
        "HOOK_INSTALL_TIMINGS_MS: total={:.3} (jail_gk={:.3}, read_parse={:.3}, merge_diff={:.3}, backup_write_verify={:.3}, prompt={:.3}, active_write={:.3})",
        outcome.phase_timings.total_elapsed_ms,
        outcome.phase_timings.jail_and_gatekeeper_ms,
        outcome.phase_timings.read_and_parse_ms,
        outcome.phase_timings.merge_and_diff_ms,
        outcome.phase_timings.backup_write_verify_ms,
        outcome.phase_timings.confirmation_prompt_ms,
        outcome.phase_timings.active_write_ms,
    );

    assert!(spy_prompter.verified_at_prompt_time.load(Ordering::SeqCst));
    assert!(outcome.applied);
    assert!(
        outcome.elapsed_ms < HOOK_OP_BUDGET_MS,
        "hook install took {:.3} ms, exceeding {:.1} ms budget (breakdown={:?})",
        outcome.elapsed_ms,
        HOOK_OP_BUDGET_MS,
        outcome.phase_timings
    );
    assert_eq!(
        outcome.audit_trail,
        vec![
            HookAuditStep::ValidatedPathAndGatekeeper,
            HookAuditStep::ParsedExistingSettings,
            HookAuditStep::MergedMikuEntryOnly,
            HookAuditStep::CreatedAndValidatedBackup,
            HookAuditStep::RenderedUnifiedDiff,
            HookAuditStep::RequestedUserConfirmation,
            HookAuditStep::WroteMergedSettings,
        ]
    );

    let _ = fs::remove_dir_all(&dir);
}

// LEDGER-3.13-4
#[test]
fn test_user_cancellation_leaves_active_settings_file_completely_unmodified() {
    let _io_guard = acquire_hook_test_lock();
    let dir = create_temp_config_dir("cancel_no_write");
    let mut gk = build_allow_gatekeeper(&dir);
    let settings_path = dir.join("settings.json");

    let original = "{\n  \"userSetting\": \"untouched-on-cancel\",\n  \"count\": 42\n}\n";
    fs::write(&settings_path, original).expect("write original settings");
    let original_bytes = fs::read(&settings_path).unwrap();

    let cancel_prompter = FixedPrompter { approve: false };
    let outcome = install_miku_hook(
        &dir, &settings_path, HookTargetAgent::ClaudeCode, None, &cancel_prompter, &mut gk, 1_700_000_040_000,
    )
    .expect("cancelled install returns clean outcome without writing active file");

    assert!(!outcome.applied, "cancelled install must report applied=false");
    assert!(outcome.audit_trail.contains(&HookAuditStep::CancelledWithoutModifyingActiveFile));
    assert!(!outcome.audit_trail.contains(&HookAuditStep::WroteMergedSettings));

    let after_cancel_bytes = fs::read(&settings_path).expect("read active file after cancel");
    assert_eq!(
        after_cancel_bytes, original_bytes,
        "active settings.json must remain byte-for-byte untouched when user cancels"
    );

    let _ = fs::remove_dir_all(&dir);
}

// LEDGER-3.13-5
#[test]
fn test_hook_restore_recovers_backup_byte_for_byte_and_completes_under_20ms() {
    let _io_guard = acquire_hook_test_lock();
    let dir = create_temp_config_dir("restore_byte_for_byte");
    let mut gk = build_allow_gatekeeper(&dir);
    let settings_path = dir.join("settings.json");

    let raw_custom_bytes =
        b"{\r\n    \"customFormatting\":   true,\r\n    \"hooks\": {\r\n        \"PreToolUse\": []\r\n    }\r\n}\r\n";
    fs::write(&settings_path, raw_custom_bytes).expect("write custom formatted bytes");
    let expected_sha256 = sha256_hex(raw_custom_bytes);

    let prompter = FixedPrompter { approve: true };
    let install_out = install_miku_hook(
        &dir, &settings_path, HookTargetAgent::ClaudeCode, None, &prompter, &mut gk, 1_700_000_050_000,
    )
    .expect("install must succeed");
    assert!(install_out.applied);

    let modified_bytes = fs::read(&settings_path).expect("read modified settings");
    assert_ne!(modified_bytes.as_slice(), raw_custom_bytes);

    let restore_out = restore_miku_hook(
        &dir, &settings_path, Some(&install_out.backup_path), &mut gk, 1_700_000_055_000,
    )
    .expect("restore_miku_hook must succeed");

    println!("HOOK_RESTORE_TIMINGS_MS: total={:.3}", restore_out.elapsed_ms);

    assert_eq!(restore_out.restored_bytes, raw_custom_bytes.len());
    assert!(
        restore_out.elapsed_ms < HOOK_OP_BUDGET_MS,
        "hook restore took {:.3} ms, exceeding {:.1} ms budget",
        restore_out.elapsed_ms,
        HOOK_OP_BUDGET_MS
    );

    let restored_bytes = fs::read(&settings_path).expect("read restored settings");
    assert_eq!(restored_bytes.as_slice(), raw_custom_bytes);
    assert_eq!(sha256_hex(&restored_bytes), expected_sha256);

    let cli_install_args: Vec<String> = [
        "miku-cli", "hook", "install", "--agent", "claude-code",
        "--config-root", &dir.to_string_lossy(),
        "--settings", &settings_path.to_string_lossy(),
    ]
    .iter()
    .map(|s| (*s).to_string())
    .collect();
    let cli_install_text = execute_cli_hook_command_with_prompter(&cli_install_args, &prompter)
        .expect("CLI hook install must succeed")
        .expect("CLI hook install output");
    assert!(cli_install_text.contains("--- a/settings.json"));

    let cli_restore_args: Vec<String> = [
        "miku-cli", "hook", "restore",
        "--config-root", &dir.to_string_lossy(),
        "--settings", &settings_path.to_string_lossy(),
    ]
    .iter()
    .map(|s| (*s).to_string())
    .collect();
    let cli_restore_text = execute_cli_hook_command_with_prompter(&cli_restore_args, &prompter)
        .expect("CLI hook restore must succeed")
        .expect("CLI hook restore output");
    assert!(cli_restore_text.contains("restored"));
    assert_eq!(fs::read(&settings_path).unwrap().as_slice(), raw_custom_bytes);

    let _ = fs::remove_dir_all(&dir);
}

// LEDGER-3.13-6
#[test]
fn test_invalid_json_invalid_path_or_corrupt_backup_fails_safely_with_exact_line_and_zero_secret_leak() {
    let _io_guard = acquire_hook_test_lock();
    let dir = create_temp_config_dir("invalid_inputs");
    let mut gk = build_allow_gatekeeper(&dir);
    let prompter = FixedPrompter { approve: true };

    let broken_settings = dir.join("broken_settings.json");
    let sensitive_token = "sk-live-SUPER-SECRET-TOKEN-99887766";
    let broken_json = format!(
        "{{\n  \"model\": \"claude-3-7-sonnet\",\n  \"apiKey\": \"{}\",\n  \"hooks\": {{\n    \"PreToolUse\": [\n      INVALID_JSON_TOKEN_AT_LINE_6\n    ]\n  }}\n}}\n",
        sensitive_token
    );
    fs::write(&broken_settings, &broken_json).expect("write broken json");
    let broken_original_bytes = fs::read(&broken_settings).unwrap();

    let err = install_miku_hook(
        &dir, &broken_settings, HookTargetAgent::ClaudeCode, None, &prompter, &mut gk, 1_700_000_060_000,
    )
    .expect_err("invalid settings.json must fail before touching files");

    match &err {
        HookInstallError::InvalidJsonSyntax { line, .. } => assert_eq!(*line, 6),
        other => panic!("expected InvalidJsonSyntax with line 6, got: {:?}", other),
    }
    assert!(!err.to_string().contains(sensitive_token));
    assert_eq!(fs::read(&broken_settings).unwrap(), broken_original_bytes);

    let traversal_path = dir.join("..").join("escaped_settings.json");
    let traversal_err = install_miku_hook(
        &dir, &traversal_path, HookTargetAgent::ClaudeCode, None, &prompter, &mut gk, 1_700_000_061_000,
    )
    .expect_err("path traversal must be rejected by CanonicalPathJail");
    assert!(matches!(traversal_err, HookInstallError::PathEscapeOrInvalid(_)));

    let valid_settings = dir.join("valid_settings.json");
    let valid_content = "{\n  \"status\": \"healthy-active-file\"\n}\n";
    fs::write(&valid_settings, valid_content).unwrap();

    let corrupt_bak = dir.join("valid_settings.json.corrupt.bak");
    fs::write(&corrupt_bak, "{\n  \"status\": \n  BROKEN_AT_LINE_3\n}\n").unwrap();

    let restore_err = restore_miku_hook(
        &dir, &valid_settings, Some(&corrupt_bak), &mut gk, 1_700_000_062_000,
    )
    .expect_err("corrupt backup must be rejected before overwriting active settings.json");
    match restore_err {
        HookInstallError::InvalidBackup { line, .. } => assert_eq!(line, Some(3)),
        other => panic!("expected InvalidBackup at line 3, got: {:?}", other),
    }
    assert_eq!(fs::read_to_string(&valid_settings).unwrap(), valid_content);

    let root_str = dir.to_string_lossy().replace('\\', "/");
    let deny_profile = ProjectProfile::new("miku-hook-deny", &root_str, 60_000)
        .with_rule(Capability::FsRead, "**", PolicyEffect::Allow, false)
        .with_rule(Capability::FsWrite, "**", PolicyEffect::Deny, false);
    let mut deny_gk = GatekeeperEngine::new(deny_profile).unwrap();
    let gk_err = install_miku_hook(
        &dir, &valid_settings, HookTargetAgent::ClaudeCode, None, &prompter, &mut deny_gk, 1_700_000_063_000,
    )
    .expect_err("Gatekeeper FsWrite deny must block hook install");
    assert!(matches!(gk_err, HookInstallError::GatekeeperDenied(_)));
    assert_eq!(fs::read_to_string(&valid_settings).unwrap(), valid_content);

    let _ = fs::remove_dir_all(&dir);
}

// LEDGER-3.13-7
#[test]
fn test_only_miku_entry_is_modified_while_foreign_hooks_and_mcp_servers_remain_identical_and_sub_600_lines() {
    let _io_guard = acquire_hook_test_lock();
    let dir = create_temp_config_dir("foreign_hooks_untouched");
    let mut gk = build_allow_gatekeeper(&dir);
    let prompter = FixedPrompter { approve: true };
    let settings_path = dir.join("settings.json");

    let initial = serde_json::json!({
        "hooks": {
            "PreToolUse": [{ "id": "foreign-security-hook", "matcher": "Bash", "type": "command", "command": "custom-audit --strict" }],
            "PostToolUse": [{ "id": "foreign-formatter", "matcher": "Write", "type": "command", "command": "rustfmt --edition 2021" }],
            "Stop": [{ "id": "foreign-stop-notifier", "type": "command", "command": "notify-done" }]
        },
        "mcpServers": {
            "foreign-github-mcp": { "command": "npx", "args": ["-y", "@modelcontextprotocol/server-github"] }
        }
    });
    fs::write(&settings_path, format!("{}\n", serde_json::to_string_pretty(&initial).unwrap())).unwrap();

    install_miku_hook(&dir, &settings_path, HookTargetAgent::ClaudeCode, None, &prompter, &mut gk, 1_700_000_070_000)
        .expect("claude-code hook install");
    install_miku_hook(&dir, &settings_path, HookTargetAgent::Cursor, None, &prompter, &mut gk, 1_700_000_071_000)
        .expect("cursor hook install");

    let final_val: serde_json::Value =
        serde_json::from_str(&fs::read_to_string(&settings_path).unwrap()).unwrap();
    assert_eq!(final_val["hooks"]["PreToolUse"][0], initial["hooks"]["PreToolUse"][0]);
    assert_eq!(final_val["hooks"]["PostToolUse"][0], initial["hooks"]["PostToolUse"][0]);
    assert_eq!(final_val["hooks"]["Stop"], initial["hooks"]["Stop"]);
    assert_eq!(final_val["mcpServers"]["foreign-github-mcp"], initial["mcpServers"]["foreign-github-mcp"]);
    assert_eq!(final_val["hooks"]["PreToolUse"][1]["id"], serde_json::json!(MIKU_HOOK_ENTRY_ID));
    assert_eq!(final_val["mcpServers"][MIKU_HOOK_ENTRY_ID]["id"], serde_json::json!(MIKU_HOOK_ENTRY_ID));

    let _ = fs::remove_dir_all(&dir);

    let src_dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
    let mut stack = vec![src_dir];
    while let Some(curr) = stack.pop() {
        for entry in fs::read_dir(&curr).expect("read miku-cli src dir").flatten() {
            let p = entry.path();
            if p.is_dir() {
                stack.push(p);
            } else if p.extension().and_then(|e| e.to_str()) == Some("rs") {
                let line_count = fs::read_to_string(&p).expect("read rs file").lines().count();
                assert!(line_count < 600, "file '{}' has {} lines (>= 600)", p.display(), line_count);
            }
        }
    }
}
