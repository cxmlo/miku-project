use std::env;
use std::path::Path;

use miku_ui::{
    resolve_isolated_webview2_user_data_dir, EXPECTED_AVATAR_SVG_COUNT, FIVE_CYCLE_STRESS_REPS,
    MAX_10K_VERIFY_LATENCY_MS, MAX_AUDIT_APPEND_LATENCY_MS, MAX_AVATAR_HOT_RELOAD_LATENCY_MS,
    MAX_EXPANDED_WEBVIEW2_RAM_MB, MAX_EXPAND_VISIBLE_LATENCY_MS, MAX_GAMING_CPU_BUDGET_PCT,
    MAX_GATEKEEPER_INTERCEPT_LATENCY_MS, MAX_HOVER_RESPONSE_MS, MAX_IDLE_POST_TTL_RAM_MB,
    MAX_IPC_RTT_BUDGET_MS, MAX_KILL_SWITCH_LATENCY_MS, MAX_OVERVIEW_BUNDLE_BYTES,
    MAX_SOUNDWAVE_UPDATE_LATENCY_MS, MAX_VOICE_CANCEL_LATENCY_MS, MAX_WARM_WAKE_LATENCY_MS,
    REQUIRED_EX_STYLE,
};

use crate::bin_coordinator_runner::CoordinatorAggregatedStats;
use crate::bin_worker_probe::{
    GATEKEEPER_AUDIT_CYCLES_PER_WORKER, HOT_RELOAD_CYCLES_PER_WORKER,
    IDLE_SUSPENSION_CYCLES_PER_WORKER, IPC_BURST_PER_WORKER, KILL_SWITCH_AUDIT_CYCLES_PER_WORKER,
    MAX_BINARY_SIZE_MB, OVERVIEW_EVENTS_PER_WORKER, PERSISTENT_AUDIT_CYCLES_PER_WORKER,
    VOICE_AUDIT_CYCLES_PER_WORKER,
};

pub(crate) fn print_and_verify_coordinator_benchmark(
    stats: &CoordinatorAggregatedStats,
    repetitions: usize,
    enforce_release_budgets: bool,
) -> Result<(), String> {
    let CoordinatorAggregatedStats {
        bin_size_mb,
        ref sample,
        ex_style_ok_count,
        ma_noactivate_ok_count,
        focus_preserved_count,
        isolated_dir_ok_count,
        total_webview2_procs,
        total_idle_timers,
        total_mode_c_cycles,
        total_warm_reuses,
        total_ttl_evictions,
        total_blocked_navs,
        total_ipc_roundtrips,
        total_ipc_unauth_blocked,
        total_ipc_malformed_discarded,
        ipc_csp_ok_count,
        total_ipc_retained_bytes,
        overview_bundle_bytes,
        overview_layer_count,
        overview_accessory_slots,
        total_overview_events,
        overview_ring_len,
        max_overview_settled_fps,
        overview_text_safe_count,
        pack_spring_count,
        pack_moods_verified,
        total_pack_hot_reloads,
        total_pack_rollbacks,
        max_pack_settled_fps,
        total_gk_allowed,
        total_gk_blocks,
        total_gk_toctou,
        total_gk_replay,
        total_gk_ttl_expired,
        total_gk_audit_entries,
        gk_verified_count,
        total_audit_jsonl_entries,
        total_audit_rotations,
        audit_segments_per_worker,
        total_audit_viewer_rows,
        audit_jsonl_ok_count,
        total_voice_ptt,
        total_voice_wake,
        total_voice_power_blocks,
        total_voice_cancellations,
        total_voice_zero_fills,
        total_voice_residual_bytes,
        max_voice_idle_fps,
        voice_ok_count,
        total_ks_trips,
        total_ks_revoked,
        total_ks_voice_aborts,
        total_ks_rearms,
        total_ks_rejected_rearms,
        total_ks_blocked,
        ks_ok_count,
        total_idle_suspensions,
        total_idle_wakes,
        total_idle_deferred_gaming,
        total_idle_crit_alerts,
        total_idle_ww_frozen,
        max_idle_gaming_cpu_pct,
        idle_bank05_ok_count,
        verified_svgs,
        total_orphans,
        hover_p50,
        hover_p95,
        expand_p50,
        expand_p95,
        ov_expand_p50,
        ov_expand_p95,
        ipc_rtt_p50,
        ipc_rtt_p95,
        pack_reload_p50,
        pack_reload_p95,
        gk_intercept_p50,
        gk_intercept_p95,
        audit_append_p50,
        audit_append_p95,
        audit_verify_p50,
        audit_verify_p95,
        voice_wave_p50,
        voice_wave_p95,
        voice_cancel_p50,
        voice_cancel_p95,
        ks_trip_p50,
        ks_trip_p95,
        idle_wake_p50,
        idle_wake_p95,
        expanded_ram_p50,
        expanded_ram_p95,
        ram_p50,
        ram_p95,
    } = *stats;

    let local_app_data = env::var("LOCALAPPDATA")
        .unwrap_or_else(|_| r"C:\Users\Default\AppData\Local".to_string());
    let canonical_isolated_dir =
        resolve_isolated_webview2_user_data_dir(Some(Path::new(&local_app_data)))
            .map(|p| p.display().to_string())
            .unwrap_or_else(|_| "ERR".to_string());

    println!("================================================================================");
    println!("MIKU UI · STEP 2.1..2.10 BENCHMARK ({repetitions} REPS · IDLE SUSPENSION & BANK 0.5)");
    println!("================================================================================");
    println!(
        "Binary Size (release) : {:.2} MB (threshold < {:.1} MB)",
        bin_size_mb, MAX_BINARY_SIZE_MB
    );
    println!(
        "Isolated Data Folder  : {} (verified={}/{})",
        canonical_isolated_dir, isolated_dir_ok_count, repetitions
    );
    println!(
        "Monitors & Geometry   : monitors={} | primary_anchor=({}, {}, {}x{} @ {} DPI)",
        sample.monitors_detected,
        sample.primary_anchor_x,
        sample.primary_anchor_y,
        sample.primary_width,
        sample.primary_height,
        sample.primary_dpi
    );
    println!(
        "Win32 No-Focus Flags  : ex_style_ok={}/{} (0x{:08X}) | ma_noactivate={}/{} | focus_preserved={}/{}",
        ex_style_ok_count,
        repetitions,
        REQUIRED_EX_STYLE,
        ma_noactivate_ok_count,
        repetitions,
        focus_preserved_count,
        repetitions
    );
    println!(
        "Mode C Lifecycle (5x) : total_cycles={} | warm_reuses={} | ttl_evictions={} | blocked_navs={}",
        total_mode_c_cycles, total_warm_reuses, total_ttl_evictions, total_blocked_navs
    );
    println!(
        "IPC Bridge Audit      : roundtrips={} | origin_blocks={} | malformed_drops={} | csp_xss_ok={}/{} | retained_bytes={}",
        total_ipc_roundtrips,
        total_ipc_unauth_blocked,
        total_ipc_malformed_discarded,
        ipc_csp_ok_count,
        repetitions,
        total_ipc_retained_bytes
    );
    println!(
        "Overview Scene (2.4)  : bundle={} B (< {} B) | layers={} | accessories={} | events={} | ring_cap={} | settled_fps={:.2} | text_safe={}/{}",
        overview_bundle_bytes,
        MAX_OVERVIEW_BUNDLE_BYTES,
        overview_layer_count,
        overview_accessory_slots,
        total_overview_events,
        overview_ring_len,
        max_overview_settled_fps,
        overview_text_safe_count,
        repetitions
    );
    println!(
        "Avatar Pack (2.5)     : springs={} | moods={} | hot_reloads={} | rollbacks={} | pack_fps={:.2}",
        pack_spring_count,
        pack_moods_verified,
        total_pack_hot_reloads,
        total_pack_rollbacks,
        max_pack_settled_fps
    );
    println!(
        "Gatekeeper UI (2.6)   : allowed={} | blocks={} (toctou={} | replay={} | ttl={}) | sha256_entries={} | audit_ok={}/{}",
        total_gk_allowed,
        total_gk_blocks,
        total_gk_toctou,
        total_gk_replay,
        total_gk_ttl_expired,
        total_gk_audit_entries,
        gk_verified_count,
        repetitions
    );
    println!(
        "JSONL Audit (2.7)     : entries={} | rotations={} | segments/run={} | viewer_rows={} | jsonl_ok={}/{}",
        total_audit_jsonl_entries,
        total_audit_rotations,
        audit_segments_per_worker,
        total_audit_viewer_rows,
        audit_jsonl_ok_count,
        repetitions
    );
    println!(
        "Voice Loop (2.8)      : ptt={} | wake={} | battery_blocks={} | cancels={} | zero_fills={} | res_pcm_bytes={} | idle_fps={:.2} | voice_ok={}/{}",
        total_voice_ptt,
        total_voice_wake,
        total_voice_power_blocks,
        total_voice_cancellations,
        total_voice_zero_fills,
        total_voice_residual_bytes,
        max_voice_idle_fps,
        voice_ok_count,
        repetitions
    );
    println!(
        "Kill Switch (2.9)     : trips={} | revoked_tickets={} | voice_aborts={} | rearms={} | rejected_rearms={} | blocked_ops={} | ks_ok={}/{}",
        total_ks_trips,
        total_ks_revoked,
        total_ks_voice_aborts,
        total_ks_rearms,
        total_ks_rejected_rearms,
        total_ks_blocked,
        ks_ok_count,
        repetitions
    );
    println!(
        "Idle & Bank 0.5 (2.10): deep_suspensions={} | warm_wakes={} | deferred_gaming={} | crit_alerts={} | ww_frozen={} | gaming_cpu={:.2}% (< {:.1}%) | bank05_ok={}/{}",
        total_idle_suspensions,
        total_idle_wakes,
        total_idle_deferred_gaming,
        total_idle_crit_alerts,
        total_idle_ww_frozen,
        max_idle_gaming_cpu_pct,
        MAX_GAMING_CPU_BUDGET_PCT,
        idle_bank05_ok_count,
        repetitions
    );
    println!(
        "Zero-Idle-Cost & SVGs : residual_wv2_procs={} | idle_timers={} | verified_svgs={}",
        total_webview2_procs, total_idle_timers, verified_svgs
    );
    println!(
        "Hover Trigger Latency : p50 = {:.4} ms | p95 = {:.4} ms (threshold < {:.1} ms)",
        hover_p50, hover_p95, MAX_HOVER_RESPONSE_MS
    );
    println!(
        "WebView2 Expand Lat.  : p50 = {:.4} ms | p95 = {:.4} ms (BUD-02 threshold <= {:.1} ms)",
        expand_p50, expand_p95, MAX_EXPAND_VISIBLE_LATENCY_MS
    );
    println!(
        "Overview Expand Lat.  : p50 = {:.4} ms | p95 = {:.4} ms (BUD-02 threshold < {:.1} ms)",
        ov_expand_p50, ov_expand_p95, MAX_EXPAND_VISIBLE_LATENCY_MS
    );
    println!(
        "IPC Bridge RTT        : p50 = {:.4} ms | p95 = {:.4} ms (threshold < {:.1} ms)",
        ipc_rtt_p50, ipc_rtt_p95, MAX_IPC_RTT_BUDGET_MS
    );
    println!(
        "Avatar Hot-Reload Lat.: p50 = {:.4} ms | p95 = {:.4} ms (threshold < {:.1} ms)",
        pack_reload_p50, pack_reload_p95, MAX_AVATAR_HOT_RELOAD_LATENCY_MS
    );
    println!(
        "Gatekeeper Intercept  : p50 = {:.4} ms | p95 = {:.4} ms (threshold < {:.1} ms)",
        gk_intercept_p50, gk_intercept_p95, MAX_GATEKEEPER_INTERCEPT_LATENCY_MS
    );
    println!(
        "Audit JSONL Append    : p50 = {:.4} ms | p95 = {:.4} ms (target < {:.1} ms)",
        audit_append_p50, audit_append_p95, MAX_AUDIT_APPEND_LATENCY_MS
    );
    println!(
        "Audit Chain Verify    : p50 = {:.4} ms | p95 = {:.4} ms (threshold < {:.1} ms)",
        audit_verify_p50, audit_verify_p95, MAX_10K_VERIFY_LATENCY_MS
    );
    println!(
        "Voice Soundwave Lat.  : p50 = {:.4} ms | p95 = {:.4} ms (threshold < {:.1} ms)",
        voice_wave_p50, voice_wave_p95, MAX_SOUNDWAVE_UPDATE_LATENCY_MS
    );
    println!(
        "Voice Cancel Latency  : p50 = {:.4} ms | p95 = {:.4} ms (threshold < {:.1} ms)",
        voice_cancel_p50, voice_cancel_p95, MAX_VOICE_CANCEL_LATENCY_MS
    );
    println!(
        "Kill Switch Trip Lat. : p50 = {:.4} ms | p95 = {:.4} ms (threshold < {:.1} ms)",
        ks_trip_p50, ks_trip_p95, MAX_KILL_SWITCH_LATENCY_MS
    );
    println!(
        "Idle Warm-Wake Lat.   : p50 = {:.4} ms | p95 = {:.4} ms (threshold < {:.1} ms)",
        idle_wake_p50, idle_wake_p95, MAX_WARM_WAKE_LATENCY_MS
    );
    println!(
        "Expanded Scene RAM    : p50 = {:.2} MB | p95 = {:.2} MB (BUD-04 threshold <= {:.1} MB)",
        expanded_ram_p50, expanded_ram_p95, MAX_EXPANDED_WEBVIEW2_RAM_MB
    );
    println!(
        "Worker Private RAM    : p50 = {:.2} MB | p95 = {:.2} MB (BUD-01 post-TTL <= {:.1} MB)",
        ram_p50, ram_p95, MAX_IDLE_POST_TTL_RAM_MB
    );
    println!("Orphan Processes      : {}", total_orphans);

    let latency_limit = if enforce_release_budgets { MAX_HOVER_RESPONSE_MS } else { 100.0 };
    let expand_limit = if enforce_release_budgets { MAX_EXPAND_VISIBLE_LATENCY_MS } else { 500.0 };
    let ipc_limit = if enforce_release_budgets { MAX_IPC_RTT_BUDGET_MS } else { 50.0 };
    let reload_limit = if enforce_release_budgets { MAX_AVATAR_HOT_RELOAD_LATENCY_MS } else { 250.0 };
    let gk_limit = if enforce_release_budgets { MAX_GATEKEEPER_INTERCEPT_LATENCY_MS } else { 25.0 };
    let verify_limit = if enforce_release_budgets { MAX_10K_VERIFY_LATENCY_MS } else { 250.0 };
    let wave_limit = if enforce_release_budgets { MAX_SOUNDWAVE_UPDATE_LATENCY_MS } else { 100.0 };
    let cancel_limit = if enforce_release_budgets { MAX_VOICE_CANCEL_LATENCY_MS } else { 50.0 };
    let ks_limit = if enforce_release_budgets { MAX_KILL_SWITCH_LATENCY_MS } else { 100.0 };
    let wake_limit = if enforce_release_budgets { MAX_WARM_WAKE_LATENCY_MS } else { 100.0 };
    let bin_limit = if enforce_release_budgets { MAX_BINARY_SIZE_MB } else { 150.0 };
    let ram_limit = if enforce_release_budgets { MAX_IDLE_POST_TTL_RAM_MB } else { 120.0 };

    let verifier_ok = ex_style_ok_count == repetitions
        && ma_noactivate_ok_count == repetitions
        && focus_preserved_count == repetitions
        && isolated_dir_ok_count == repetitions
        && ipc_csp_ok_count == repetitions
        && overview_text_safe_count == repetitions
        && gk_verified_count == repetitions
        && audit_jsonl_ok_count == repetitions
        && voice_ok_count == repetitions
        && ks_ok_count == repetitions
        && idle_bank05_ok_count == repetitions
        && total_mode_c_cycles == repetitions * FIVE_CYCLE_STRESS_REPS
        && total_ttl_evictions == repetitions * FIVE_CYCLE_STRESS_REPS
        && total_ipc_roundtrips == repetitions * IPC_BURST_PER_WORKER * 2
        && total_ipc_unauth_blocked == repetitions * IPC_BURST_PER_WORKER
        && total_ipc_malformed_discarded == repetitions * IPC_BURST_PER_WORKER
        && total_ipc_retained_bytes == 0
        && overview_bundle_bytes > 0
        && overview_bundle_bytes <= MAX_OVERVIEW_BUNDLE_BYTES
        && overview_layer_count == 9
        && overview_accessory_slots == 14
        && total_overview_events == repetitions * OVERVIEW_EVENTS_PER_WORKER
        && max_overview_settled_fps == 0.0
        && pack_spring_count == 6
        && pack_moods_verified == 8
        && total_pack_hot_reloads == repetitions * HOT_RELOAD_CYCLES_PER_WORKER
        && total_pack_rollbacks == repetitions * 2
        && max_pack_settled_fps == 0.0
        && total_gk_allowed == repetitions * GATEKEEPER_AUDIT_CYCLES_PER_WORKER * 2
        && total_gk_blocks == repetitions * GATEKEEPER_AUDIT_CYCLES_PER_WORKER * 6
        && total_gk_toctou == repetitions * GATEKEEPER_AUDIT_CYCLES_PER_WORKER
        && total_gk_replay == repetitions * GATEKEEPER_AUDIT_CYCLES_PER_WORKER
        && total_gk_ttl_expired == repetitions * GATEKEEPER_AUDIT_CYCLES_PER_WORKER
        && total_gk_audit_entries == repetitions * GATEKEEPER_AUDIT_CYCLES_PER_WORKER * 11
        && total_audit_jsonl_entries == repetitions * PERSISTENT_AUDIT_CYCLES_PER_WORKER * 10
        && total_audit_rotations >= repetitions * 2
        && audit_segments_per_worker >= 3
        && total_audit_viewer_rows == repetitions * PERSISTENT_AUDIT_CYCLES_PER_WORKER * 10
        && total_voice_ptt == repetitions * VOICE_AUDIT_CYCLES_PER_WORKER
        && total_voice_wake == repetitions * VOICE_AUDIT_CYCLES_PER_WORKER
        && total_voice_power_blocks == repetitions * VOICE_AUDIT_CYCLES_PER_WORKER
        && total_voice_cancellations == repetitions * VOICE_AUDIT_CYCLES_PER_WORKER * 3
        && total_voice_zero_fills == repetitions * VOICE_AUDIT_CYCLES_PER_WORKER * 5
        && total_voice_residual_bytes == 0
        && max_voice_idle_fps == 0.0
        && total_ks_trips == repetitions * KILL_SWITCH_AUDIT_CYCLES_PER_WORKER
        && total_ks_revoked == repetitions * KILL_SWITCH_AUDIT_CYCLES_PER_WORKER * 3
        && total_ks_voice_aborts == repetitions * KILL_SWITCH_AUDIT_CYCLES_PER_WORKER
        && total_ks_rearms == repetitions * KILL_SWITCH_AUDIT_CYCLES_PER_WORKER
        && total_ks_rejected_rearms == repetitions * KILL_SWITCH_AUDIT_CYCLES_PER_WORKER
        && total_ks_blocked == repetitions * KILL_SWITCH_AUDIT_CYCLES_PER_WORKER
        && total_idle_suspensions == repetitions * IDLE_SUSPENSION_CYCLES_PER_WORKER
        && total_idle_wakes == repetitions * IDLE_SUSPENSION_CYCLES_PER_WORKER
        && total_idle_deferred_gaming == repetitions * 4
        && total_idle_crit_alerts == repetitions
        && max_idle_gaming_cpu_pct < MAX_GAMING_CPU_BUDGET_PCT
        && total_webview2_procs == 0
        && total_idle_timers == 0
        && verified_svgs == EXPECTED_AVATAR_SVG_COUNT
        && hover_p95 < latency_limit
        && expand_p95 <= expand_limit
        && ov_expand_p95 < expand_limit
        && ipc_rtt_p95 < ipc_limit
        && pack_reload_p95 < reload_limit
        && gk_intercept_p95 < gk_limit
        && audit_verify_p95 < verify_limit
        && voice_wave_p95 < wave_limit
        && voice_cancel_p95 < cancel_limit
        && ks_trip_p95 < ks_limit
        && idle_wake_p95 < wake_limit
        && expanded_ram_p95 <= MAX_EXPANDED_WEBVIEW2_RAM_MB
        && bin_size_mb < bin_limit
        && ram_p95 <= ram_limit
        && total_orphans == 0;

    if verifier_ok {
        println!(
            "VERIFIER: OK (focus={}/{}, cycles={}, ipc_rts={}, ov_bundle={} B, pack_reloads={}, gk_blocks={}, jsonl_entries={}, voice_ptt={}, ks_trips={}, idle_susp={}, idle_wakes={}, wake_p95={:.4} ms, post_ttl_ram_p95={:.2} MB, orphans=0)",
            focus_preserved_count,
            repetitions,
            total_mode_c_cycles,
            total_ipc_roundtrips,
            overview_bundle_bytes,
            total_pack_hot_reloads,
            total_gk_blocks,
            total_audit_jsonl_entries,
            total_voice_ptt,
            total_ks_trips,
            total_idle_suspensions,
            total_idle_wakes,
            idle_wake_p95,
            ram_p95
        );
        Ok(())
    } else {
        Err(format!(
            "VERIFIER FAILED: focus={}/{}, iso_dir={}/{}, cycles={}, ipc_rts={}, ov_bundle={}, pack_reloads={}, gk_blocks={}, jsonl_entries={}, voice_ptt={}, ks_trips={}, idle_susp={}, bank05_ok={}/{}, wake_p95={:.4}ms, ram_p95={:.2}MB, orphans={}",
            focus_preserved_count,
            repetitions,
            isolated_dir_ok_count,
            repetitions,
            total_mode_c_cycles,
            total_ipc_roundtrips,
            overview_bundle_bytes,
            total_pack_hot_reloads,
            total_gk_blocks,
            total_audit_jsonl_entries,
            total_voice_ptt,
            total_ks_trips,
            total_idle_suspensions,
            idle_bank05_ok_count,
            repetitions,
            idle_wake_p95,
            ram_p95,
            total_orphans
        ))
    }
}
