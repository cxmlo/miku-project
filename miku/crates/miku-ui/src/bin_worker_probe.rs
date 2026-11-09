use std::env;
use std::path::Path;
use std::thread;
use std::time::Duration;

use miku_ui::{
    resolve_avatar_assets_dir, resolve_isolated_audit_log_dir,
    resolve_isolated_webview2_user_data_dir, run_avatar_pack_hot_reload_audit,
    run_five_cycle_webview2_ttl_recovery_audit, run_idle_suspension_bank05_audit,
    run_ipc_bridge_stress_audit, run_kill_switch_audit, run_native_pill_verification_cycle,
    run_overview_scene_audit, run_persistent_jsonl_audit_store_benchmark,
    run_ui_gatekeeper_interceptor_audit, run_voice_interaction_loop_audit, MonitorDescriptor,
    VirtualRect, EXPECTED_AVATAR_SVG_COUNT, IDLE_COMPACT_HEIGHT_PX, IDLE_COMPACT_WIDTH_PX,
    STANDARD_DPI, TEN_CYCLE_STABILITY_REPS,
};

pub(crate) const BENCHMARK_REPETITIONS: usize = 20;
pub(crate) const WORKER_HOLD_TIMEOUT_MS: u64 = 3_000;
pub(crate) const POST_KILL_SETTLE_MS: u64 = 150;
pub(crate) const MAX_BINARY_SIZE_MB: f64 = 15.0;
pub(crate) const IPC_BURST_PER_WORKER: usize = 10;
pub(crate) const OVERVIEW_EVENTS_PER_WORKER: usize = 40;
pub(crate) const HOT_RELOAD_CYCLES_PER_WORKER: usize = 10;
pub(crate) const GATEKEEPER_AUDIT_CYCLES_PER_WORKER: usize = 5;
pub(crate) const PERSISTENT_AUDIT_CYCLES_PER_WORKER: usize = 2;
pub(crate) const VOICE_AUDIT_CYCLES_PER_WORKER: usize = 5;
pub(crate) const KILL_SWITCH_AUDIT_CYCLES_PER_WORKER: usize = 5;
pub(crate) const IDLE_SUSPENSION_CYCLES_PER_WORKER: usize = TEN_CYCLE_STABILITY_REPS;

pub(crate) fn default_primary_monitor_from_anchor(
    anchor_x: i32,
    anchor_y: i32,
    dpi: u32,
) -> MonitorDescriptor {
    let width = (anchor_x * 2 + IDLE_COMPACT_WIDTH_PX as i32).max(1920);
    let height = 1080;
    let monitor_rect = VirtualRect::new(0, anchor_y, width, anchor_y + height).unwrap_or(
        VirtualRect {
            left: 0,
            top: 0,
            right: 1920,
            bottom: 1080,
        },
    );
    let work_area = VirtualRect::new(0, anchor_y, width, anchor_y + height - 40).unwrap_or(
        VirtualRect {
            left: 0,
            top: 0,
            right: 1920,
            bottom: 1040,
        },
    );
    MonitorDescriptor {
        device_name: "PRIMARY_DISPLAY".to_string(),
        is_primary: true,
        dpi: if dpi == 0 { STANDARD_DPI } else { dpi },
        monitor_rect,
        work_area,
    }
}

pub(crate) fn run_single_worker_iteration(rep: usize, hold_for_ram: bool) -> Result<(), String> {
    let avatar_dir = resolve_avatar_assets_dir();
    let pill = run_native_pill_verification_cycle(rep, &avatar_dir).map_err(|e| e.to_string())?;
    if pill.verified_svg_count != EXPECTED_AVATAR_SVG_COUNT {
        return Err(format!(
            "expected {} verified SVGs, found {}",
            EXPECTED_AVATAR_SVG_COUNT, pill.verified_svg_count
        ));
    }

    let local_app_data = env::var("LOCALAPPDATA")
        .unwrap_or_else(|_| r"C:\Users\Default\AppData\Local".to_string());
    let isolated_dir = resolve_isolated_webview2_user_data_dir(Some(Path::new(&local_app_data)))
        .map_err(|e| e.to_string())?;
    let isolated_audit_dir = resolve_isolated_audit_log_dir(Some(Path::new(&local_app_data)))
        .map_err(|e| e.to_string())?;
    let isolated_data_dir_ok = isolated_dir.ends_with(Path::new("miku").join("webview2"))
        && !isolated_dir.to_string_lossy().contains("EBWebView")
        && isolated_audit_dir.ends_with(Path::new("miku").join("audit"));

    let primary_monitor = default_primary_monitor_from_anchor(
        pill.primary_anchor.x,
        pill.primary_anchor.y,
        pill.primary_anchor.dpi,
    );

    let wv2_audit = run_five_cycle_webview2_ttl_recovery_audit(
        primary_monitor.clone(),
        Some(Path::new(&local_app_data)),
        2.51,
    )
    .map_err(|e| e.to_string())?;

    let ipc_audit =
        run_ipc_bridge_stress_audit(IPC_BURST_PER_WORKER).map_err(|e| e.to_string())?;

    let overview_audit =
        run_overview_scene_audit(primary_monitor, &avatar_dir, OVERVIEW_EVENTS_PER_WORKER)
            .map_err(|e| e.to_string())?;

    let hot_reload_audit =
        run_avatar_pack_hot_reload_audit(&avatar_dir, HOT_RELOAD_CYCLES_PER_WORKER)
            .map_err(|e| e.to_string())?;

    let gk_audit = run_ui_gatekeeper_interceptor_audit(GATEKEEPER_AUDIT_CYCLES_PER_WORKER)
        .map_err(|e| e.to_string())?;

    let gk_total_blocks = (gk_audit.denied_default_count
        + gk_audit.denied_rule_count
        + gk_audit.denied_jail_count
        + gk_audit.denied_toctou_count
        + gk_audit.denied_expired_count
        + gk_audit.denied_replay_count) as usize;

    let jsonl_audit =
        run_persistent_jsonl_audit_store_benchmark(PERSISTENT_AUDIT_CYCLES_PER_WORKER)
            .map_err(|e| e.to_string())?;

    let voice_audit = run_voice_interaction_loop_audit(&avatar_dir, VOICE_AUDIT_CYCLES_PER_WORKER)
        .map_err(|e| e.to_string())?;

    let ks_audit = run_kill_switch_audit(&avatar_dir, KILL_SWITCH_AUDIT_CYCLES_PER_WORKER)
        .map_err(|e| e.to_string())?;

    let idle_audit = run_idle_suspension_bank05_audit(
        default_primary_monitor_from_anchor(
            pill.primary_anchor.x,
            pill.primary_anchor.y,
            pill.primary_anchor.dpi,
        ),
        Some(Path::new(&local_app_data)),
        &avatar_dir,
        IDLE_SUSPENSION_CYCLES_PER_WORKER,
    )
    .map_err(|e| e.to_string())?;

    println!(
        "MIKU_UI_WORKER_OK\tmonitors={}\tanchor_x={}\tanchor_y={}\tw={}\th={}\tdpi={}\tex_style_ok={}\tma_ok={}\tfocus_ok={}\thover_ms={:.4}\twv2_procs={}\tidle_timers={}\tsvg_assets={}\tiso_dir_ok={}\tcycles={}\twarm_reuses={}\tttl_evictions={}\tblocked_navs={}\texpand_ms={:.4}\tipc_rts={}\tipc_max_ms={:.4}\tipc_unauth={}\tipc_malformed={}\tipc_csp_ok={}\tipc_retained={}\tov_bundle={}\tov_layers={}\tov_acc={}\tov_events={}\tov_ring={}\tov_fps={:.2}\tov_expand_ms={:.4}\tov_text_ok={}\texpanded_ram_mb={:.2}\tpack_springs={}\tpack_moods={}\tpack_reloads={}\tpack_rollbacks={}\tpack_reload_ms={:.4}\tpack_fps={:.2}\tgk_allow={}\tgk_blocks={}\tgk_toctou={}\tgk_replay={}\tgk_ttl={}\tgk_chain={}\tgk_ms={:.4}\tgk_ok={}\tjsonl_entries={}\tjsonl_rot={}\tjsonl_segs={}\tjsonl_rows={}\tjsonl_app_ms={:.4}\tjsonl_ver_ms={:.4}\tjsonl_ok={}\tv_ptt={}\tv_wake={}\tv_pblk={}\tv_cancel={}\tv_zero={}\tv_res={}\tv_wave_ms={:.4}\tv_can_ms={:.4}\tv_fps={:.2}\tv_ok={}\tks_trips={}\tks_rev={}\tks_vab={}\tks_rearm={}\tks_rej={}\tks_blk={}\tks_ms={:.4}\tks_ok={}\tidle_susp={}\tidle_wakes={}\tidle_def={}\tidle_crit={}\tidle_wwf={}\tidle_wake_ms={:.4}\tidle_gcpu={:.2}\tidle_b05_ok={}\tpost_ttl_ram_mb={:.2}",
        pill.monitor_count,
        pill.primary_anchor.x,
        pill.primary_anchor.y,
        IDLE_COMPACT_WIDTH_PX,
        IDLE_COMPACT_HEIGHT_PX,
        pill.primary_anchor.dpi,
        pill.ex_style_verified,
        pill.ma_noactivate_verified,
        pill.focus_preserved,
        pill.hover_latency_ms,
        pill.webview2_instances + wv2_audit.residual_webview2_pids + idle_audit.residual_webview2_pids,
        (pill.active_idle_timers + wv2_audit.residual_idle_timers + idle_audit.residual_idle_timers) as usize,
        pill.verified_svg_count,
        isolated_data_dir_ok,
        wv2_audit.cycles_completed,
        wv2_audit.warm_reuses,
        wv2_audit.ttl_evictions,
        wv2_audit.blocked_external_navs,
        wv2_audit.max_expand_latency_ms,
        ipc_audit.roundtrips_completed,
        ipc_audit.max_rtt_ms,
        ipc_audit.blocked_unauthorized_origins,
        ipc_audit.dropped_malformed_frames,
        ipc_audit.csp_enforced && ipc_audit.xss_neutralized,
        ipc_audit.retained_buffer_bytes_after_drain + idle_audit.residual_ipc_bytes,
        overview_audit.bundle_bytes,
        overview_audit.layer_count,
        overview_audit.accessory_slot_count,
        overview_audit.events_processed,
        overview_audit.bounded_feed_len,
        overview_audit.settled_fps,
        overview_audit.expand_transition_ms,
        overview_audit.text_content_only_verified,
        overview_audit.expanded_ram_mb,
        hot_reload_audit.spring_count,
        hot_reload_audit.moods_converged,
        hot_reload_audit.reload_commits,
        hot_reload_audit.reload_rollbacks,
        hot_reload_audit.max_reload_latency_ms,
        hot_reload_audit.settled_fps,
        gk_audit.allowed_count,
        gk_total_blocks,
        gk_audit.denied_toctou_count,
        gk_audit.denied_replay_count,
        gk_audit.denied_expired_count,
        gk_audit.audit_chain_entries,
        gk_audit.max_intercept_latency_ms,
        gk_audit.audit_chain_verified && gk_audit.tamper_simulation_detected,
        jsonl_audit.verified_entries,
        jsonl_audit.rotations_completed,
        jsonl_audit.segment_files_verified,
        jsonl_audit.viewer_rows_rendered,
        jsonl_audit.max_append_latency_ms,
        jsonl_audit.verify_latency_ms,
        jsonl_audit.viewer_xss_neutralized && jsonl_audit.tamper_detected_at_exact_line,
        voice_audit.ptt_sessions_completed,
        voice_audit.wake_word_sessions_completed,
        voice_audit.wake_word_power_blocks,
        voice_audit.cooperative_cancellations,
        voice_audit.pcm_zero_fills_verified,
        voice_audit.residual_pcm_bytes + idle_audit.residual_pcm_bytes,
        voice_audit.max_soundwave_latency_ms,
        voice_audit.max_cancel_latency_ms,
        voice_audit.idle_wave_fps,
        voice_audit.focus_preserved_and_xss_safe,
        ks_audit.trips_executed,
        ks_audit.total_revoked_tickets,
        ks_audit.voice_sessions_aborted,
        ks_audit.rearms_completed,
        ks_audit.rejected_rearm_attempts,
        ks_audit.blocked_actions_while_tripped,
        ks_audit.max_trip_latency_ms,
        ks_audit.audit_chain_verified
            && ks_audit.hotkey_fallback_verified
            && ks_audit.xss_and_focus_verified,
        idle_audit.deep_suspensions_completed,
        idle_audit.warm_wakes_completed,
        idle_audit.gaming_events_deferred,
        idle_audit.gaming_critical_alerts_delivered,
        idle_audit.wake_word_suspensions,
        idle_audit.max_wake_latency_ms,
        idle_audit.gaming_cpu_pct,
        idle_audit.focus_preserved_and_zero_drift
            && idle_audit.certification_matrix.all_budgets_certified,
        wv2_audit.max_post_ttl_ram_mb.max(idle_audit.max_post_ttl_ram_mb)
    );

    if hold_for_ram {
        println!("READY_FOR_RAM_SAMPLE");
        thread::sleep(Duration::from_millis(WORKER_HOLD_TIMEOUT_MS));
    }

    Ok(())
}
