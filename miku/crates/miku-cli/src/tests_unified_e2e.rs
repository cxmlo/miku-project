use std::fs;
use std::time::{SystemTime, UNIX_EPOCH};

use miku_gatekeeper::Capability;
use miku_protocol::PermissionDecisionKind;

use crate::unified_runtime::MikuUnifiedRuntime;

fn temp_unified_dir(tag: &str) -> std::path::PathBuf {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    let dir = std::env::temp_dir().join(format!("miku_4_14_{}_{}_{}", tag, std::process::id(), nanos));
    let _ = fs::create_dir_all(&dir);
    dir
}

// LEDGER-4.14-1
#[test]
fn test_4_14_unified_binary_cold_start_connects_all_crates_within_budgets() {
    let _wv2_guard = crate::live_ui_host::CLI_LIVE_WV2_LOCK
        .lock()
        .unwrap_or_else(|e| e.into_inner());
    let temp = temp_unified_dir("boot");
    let rt = MikuUnifiedRuntime::boot_cold(&temp, "miku-4-14-tok-boot").expect("unified boot");
    let rep = rt.boot_report().clone();

    assert!(rep.connected_subsystems.len() >= 12);
    assert!(rep.cold_start_ms < 1_000.0, "cold_start_ms={}", rep.cold_start_ms);
    assert!(rep.pill_ready_ms < 100.0, "pill_ready_ms={}", rep.pill_ready_ms);
    assert_eq!(rep.sqlite_journal_mode.to_ascii_lowercase(), "wal");
    assert!(rep.autostart_cmd_valid);
    let _ = rt.subsystem_health_counts();

    let shut = rt.shutdown_cleanly().expect("clean shutdown");
    assert_eq!(shut.live_webview2_child_pids, 0);
    assert_eq!(shut.webview2_active_instances, 0);
    let _ = fs::remove_dir_all(&temp);
}

// LEDGER-4.14-2
#[test]
fn test_4_14_unified_e2e_ptt_and_text_prompts_permission_cards_and_fts5_persistence() {
    let _wv2_guard = crate::live_ui_host::CLI_LIVE_WV2_LOCK
        .lock()
        .unwrap_or_else(|e| e.into_inner());
    let temp = temp_unified_dir("e2e_flow");
    let mut rt = MikuUnifiedRuntime::boot_cold(&temp, "miku-4-14-tok-flow").expect("unified boot");

    let text_rec = rt
        .submit_prompt("sess-unified-1", "text", "analiza crates de storage y gateway")
        .expect("text prompt");
    assert!(text_rec.fts_hits_found >= 1);
    assert!(text_rec.ui_safe_render_ok);

    let ptt_rec = rt
        .submit_prompt("sess-unified-1", "ptt", "ejecuta cargo test ahora")
        .expect("ptt prompt");
    assert!(ptt_rec.fts_hits_found >= 1);
    assert!(ptt_rec.ui_safe_render_ok);

    let allowed = rt
        .request_and_decide_action(
            Capability::Shell,
            "cargo test --workspace",
            PermissionDecisionKind::Allow,
        )
        .expect("allow card");
    assert_eq!(allowed.initial_verdict, "ask");
    assert!(allowed.executed);

    let denied = rt
        .request_and_decide_action(Capability::Shell, "git push origin main", PermissionDecisionKind::Deny)
        .expect("deny card");
    assert_eq!(denied.initial_verdict, "deny");
    assert!(!denied.executed);

    let expand_ms = rt.expand_island_overview(5_000).expect("expand island");
    assert!(expand_ms < 150.0);

    let shut = rt.shutdown_cleanly().expect("shutdown");
    assert_eq!(shut.live_webview2_child_pids, 0);
    let _ = fs::remove_dir_all(&temp);
}

// LEDGER-4.14-3
#[test]
fn test_4_14_unified_kill_switch_stops_voice_ducking_tunnel_and_gateway_under_100ms() {
    let _wv2_guard = crate::live_ui_host::CLI_LIVE_WV2_LOCK
        .lock()
        .unwrap_or_else(|e| e.into_inner());
    let temp = temp_unified_dir("kill_switch");
    let mut rt = MikuUnifiedRuntime::boot_cold(&temp, "miku-4-14-tok-ks").expect("unified boot");

    let ks = rt
        .trip_global_kill_switch(12_000, "e2e-4-14-kill-switch")
        .expect("trip kill switch");
    assert!(ks.kill_latency_ms < 100.0, "kill_latency_ms={}", ks.kill_latency_ms);
    assert_eq!(ks.pcm_residual_non_zero, 0);
    assert!(ks.audio_ducking_restored);
    assert!(ks.tunnel_away_disabled);
    assert!(ks.avatar_mood_is_alert);
    assert!(ks.gateway_still_clean_shutdown);

    let shut = rt.shutdown_cleanly().expect("shutdown");
    assert_eq!(shut.live_webview2_child_pids, 0);
    assert_eq!(shut.webview2_active_instances, 0);
    let _ = fs::remove_dir_all(&temp);
}
