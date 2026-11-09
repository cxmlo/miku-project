use std::collections::BTreeSet;
use std::env;
use std::fs;
use std::process::{Command, Stdio};
use std::thread;
use std::time::Duration;

use crate::bin_coordinator_report::print_and_verify_coordinator_benchmark;
use crate::bin_process_tree::{
    collect_descendant_tree, compute_percentile, kill_process_tree, query_windows_process_snapshot,
    ProcessIdentity,
};
use crate::bin_worker_probe::POST_KILL_SETTLE_MS;
use crate::bin_worker_telemetry::{parse_worker_line, WorkerUiMetrics};

pub(crate) struct CoordinatorAggregatedStats {
    pub(crate) bin_size_mb: f64,
    pub(crate) sample: WorkerUiMetrics,
    pub(crate) ex_style_ok_count: usize,
    pub(crate) ma_noactivate_ok_count: usize,
    pub(crate) focus_preserved_count: usize,
    pub(crate) isolated_dir_ok_count: usize,
    pub(crate) total_webview2_procs: usize,
    pub(crate) total_idle_timers: usize,
    pub(crate) total_mode_c_cycles: usize,
    pub(crate) total_warm_reuses: usize,
    pub(crate) total_ttl_evictions: usize,
    pub(crate) total_blocked_navs: usize,
    pub(crate) total_ipc_roundtrips: usize,
    pub(crate) total_ipc_unauth_blocked: usize,
    pub(crate) total_ipc_malformed_discarded: usize,
    pub(crate) ipc_csp_ok_count: usize,
    pub(crate) total_ipc_retained_bytes: usize,
    pub(crate) overview_bundle_bytes: usize,
    pub(crate) overview_layer_count: usize,
    pub(crate) overview_accessory_slots: usize,
    pub(crate) total_overview_events: usize,
    pub(crate) overview_ring_len: usize,
    pub(crate) max_overview_settled_fps: f64,
    pub(crate) overview_text_safe_count: usize,
    pub(crate) pack_spring_count: usize,
    pub(crate) pack_moods_verified: usize,
    pub(crate) total_pack_hot_reloads: usize,
    pub(crate) total_pack_rollbacks: usize,
    pub(crate) max_pack_settled_fps: f64,
    pub(crate) total_gk_allowed: usize,
    pub(crate) total_gk_blocks: usize,
    pub(crate) total_gk_toctou: usize,
    pub(crate) total_gk_replay: usize,
    pub(crate) total_gk_ttl_expired: usize,
    pub(crate) total_gk_audit_entries: usize,
    pub(crate) gk_verified_count: usize,
    pub(crate) total_audit_jsonl_entries: usize,
    pub(crate) total_audit_rotations: usize,
    pub(crate) audit_segments_per_worker: usize,
    pub(crate) total_audit_viewer_rows: usize,
    pub(crate) audit_jsonl_ok_count: usize,
    pub(crate) total_voice_ptt: usize,
    pub(crate) total_voice_wake: usize,
    pub(crate) total_voice_power_blocks: usize,
    pub(crate) total_voice_cancellations: usize,
    pub(crate) total_voice_zero_fills: usize,
    pub(crate) total_voice_residual_bytes: usize,
    pub(crate) max_voice_idle_fps: f64,
    pub(crate) voice_ok_count: usize,
    pub(crate) total_ks_trips: usize,
    pub(crate) total_ks_revoked: usize,
    pub(crate) total_ks_voice_aborts: usize,
    pub(crate) total_ks_rearms: usize,
    pub(crate) total_ks_rejected_rearms: usize,
    pub(crate) total_ks_blocked: usize,
    pub(crate) ks_ok_count: usize,
    pub(crate) total_idle_suspensions: usize,
    pub(crate) total_idle_wakes: usize,
    pub(crate) total_idle_deferred_gaming: usize,
    pub(crate) total_idle_crit_alerts: usize,
    pub(crate) total_idle_ww_frozen: usize,
    pub(crate) max_idle_gaming_cpu_pct: f64,
    pub(crate) idle_bank05_ok_count: usize,
    pub(crate) verified_svgs: usize,
    pub(crate) total_orphans: usize,
    pub(crate) hover_p50: f64,
    pub(crate) hover_p95: f64,
    pub(crate) expand_p50: f64,
    pub(crate) expand_p95: f64,
    pub(crate) ov_expand_p50: f64,
    pub(crate) ov_expand_p95: f64,
    pub(crate) ipc_rtt_p50: f64,
    pub(crate) ipc_rtt_p95: f64,
    pub(crate) pack_reload_p50: f64,
    pub(crate) pack_reload_p95: f64,
    pub(crate) gk_intercept_p50: f64,
    pub(crate) gk_intercept_p95: f64,
    pub(crate) audit_append_p50: f64,
    pub(crate) audit_append_p95: f64,
    pub(crate) audit_verify_p50: f64,
    pub(crate) audit_verify_p95: f64,
    pub(crate) voice_wave_p50: f64,
    pub(crate) voice_wave_p95: f64,
    pub(crate) voice_cancel_p50: f64,
    pub(crate) voice_cancel_p95: f64,
    pub(crate) ks_trip_p50: f64,
    pub(crate) ks_trip_p95: f64,
    pub(crate) idle_wake_p50: f64,
    pub(crate) idle_wake_p95: f64,
    pub(crate) expanded_ram_p50: f64,
    pub(crate) expanded_ram_p95: f64,
    pub(crate) ram_p50: f64,
    pub(crate) ram_p95: f64,
}

pub(crate) fn collect_coordinator_benchmark_runs(
    repetitions: usize,
) -> Result<CoordinatorAggregatedStats, String> {
    let exe_path = env::current_exe().map_err(|e| e.to_string())?;
    let bin_size_bytes = fs::metadata(&exe_path).map(|m| m.len()).unwrap_or(0);
    let bin_size_mb = (bin_size_bytes as f64) / (1024.0 * 1024.0);

    let mut hover_latencies_ms = Vec::with_capacity(repetitions);
    let mut expand_latencies_ms = Vec::with_capacity(repetitions);
    let mut overview_expand_latencies_ms = Vec::with_capacity(repetitions);
    let mut ipc_rtt_samples_ms = Vec::with_capacity(repetitions);
    let mut pack_reload_latencies_ms = Vec::with_capacity(repetitions);
    let mut gk_intercept_latencies_ms = Vec::with_capacity(repetitions);
    let mut audit_append_latencies_ms = Vec::with_capacity(repetitions);
    let mut audit_verify_latencies_ms = Vec::with_capacity(repetitions);
    let mut voice_wave_latencies_ms = Vec::with_capacity(repetitions);
    let mut voice_cancel_latencies_ms = Vec::with_capacity(repetitions);
    let mut ks_trip_latencies_ms = Vec::with_capacity(repetitions);
    let mut idle_wake_latencies_ms = Vec::with_capacity(repetitions);
    let mut expanded_ram_samples_mb = Vec::with_capacity(repetitions);
    let mut ram_samples_mb = Vec::with_capacity(repetitions);

    let mut ex_style_ok_count = 0usize;
    let mut ma_noactivate_ok_count = 0usize;
    let mut focus_preserved_count = 0usize;
    let mut isolated_dir_ok_count = 0usize;
    let mut total_webview2_procs = 0usize;
    let mut total_idle_timers = 0usize;
    let mut total_mode_c_cycles = 0usize;
    let mut total_warm_reuses = 0usize;
    let mut total_ttl_evictions = 0usize;
    let mut total_blocked_navs = 0usize;
    let mut total_ipc_roundtrips = 0usize;
    let mut total_ipc_unauth_blocked = 0usize;
    let mut total_ipc_malformed_discarded = 0usize;
    let mut ipc_csp_ok_count = 0usize;
    let mut total_ipc_retained_bytes = 0usize;
    let mut overview_bundle_bytes = 0usize;
    let mut overview_layer_count = 0usize;
    let mut overview_accessory_slots = 0usize;
    let mut total_overview_events = 0usize;
    let mut overview_ring_len = 0usize;
    let mut max_overview_settled_fps = 0.0f64;
    let mut overview_text_safe_count = 0usize;
    let mut pack_spring_count = 0usize;
    let mut pack_moods_verified = 0usize;
    let mut total_pack_hot_reloads = 0usize;
    let mut total_pack_rollbacks = 0usize;
    let mut max_pack_settled_fps = 0.0f64;
    let mut total_gk_allowed = 0usize;
    let mut total_gk_blocks = 0usize;
    let mut total_gk_toctou = 0usize;
    let mut total_gk_replay = 0usize;
    let mut total_gk_ttl_expired = 0usize;
    let mut total_gk_audit_entries = 0usize;
    let mut gk_verified_count = 0usize;
    let mut total_audit_jsonl_entries = 0usize;
    let mut total_audit_rotations = 0usize;
    let mut audit_segments_per_worker = 0usize;
    let mut total_audit_viewer_rows = 0usize;
    let mut audit_jsonl_ok_count = 0usize;
    let mut total_voice_ptt = 0usize;
    let mut total_voice_wake = 0usize;
    let mut total_voice_power_blocks = 0usize;
    let mut total_voice_cancellations = 0usize;
    let mut total_voice_zero_fills = 0usize;
    let mut total_voice_residual_bytes = 0usize;
    let mut max_voice_idle_fps = 0.0f64;
    let mut voice_ok_count = 0usize;
    let mut total_ks_trips = 0usize;
    let mut total_ks_revoked = 0usize;
    let mut total_ks_voice_aborts = 0usize;
    let mut total_ks_rearms = 0usize;
    let mut total_ks_rejected_rearms = 0usize;
    let mut total_ks_blocked = 0usize;
    let mut ks_ok_count = 0usize;
    let mut total_idle_suspensions = 0usize;
    let mut total_idle_wakes = 0usize;
    let mut total_idle_deferred_gaming = 0usize;
    let mut total_idle_crit_alerts = 0usize;
    let mut total_idle_ww_frozen = 0usize;
    let mut max_idle_gaming_cpu_pct = 0.0f64;
    let mut idle_bank05_ok_count = 0usize;
    let mut verified_svgs = 0usize;
    let mut total_orphans = 0usize;
    let mut last_metrics: Option<WorkerUiMetrics> = None;

    for rep in 0..repetitions {
        let hold_flag = rep == 0;
        let mut cmd = Command::new(&exe_path);
        cmd.arg("--worker");
        if hold_flag {
            cmd.arg("--hold-for-ram");
        }
        cmd.stdout(Stdio::piped()).stderr(Stdio::piped());

        let mut child = cmd.spawn().map_err(|e| e.to_string())?;
        let root_pid = child.id();

        let mut sampled_ram_mb = 0.0f64;
        let mut pre_kill_identities = BTreeSet::new();

        if hold_flag {
            thread::sleep(Duration::from_millis(120));
            let snap = query_windows_process_snapshot();
            let (priv_bytes, ids) = collect_descendant_tree(root_pid, &snap);
            if priv_bytes > 0 {
                sampled_ram_mb = (priv_bytes as f64) / (1024.0 * 1024.0);
            }
            pre_kill_identities = ids;
            kill_process_tree(root_pid, &mut child);
        }

        let out = child.wait_with_output().map_err(|e| e.to_string())?;
        let stdout_str = String::from_utf8_lossy(&out.stdout);
        let metrics = parse_worker_line(&stdout_str)?;

        if sampled_ram_mb <= 0.0 {
            sampled_ram_mb = metrics.post_ttl_ram_mb;
        }
        ram_samples_mb.push(sampled_ram_mb);
        expanded_ram_samples_mb.push(metrics.expanded_ram_mb);
        hover_latencies_ms.push(metrics.hover_trigger_latency_ms);
        expand_latencies_ms.push(metrics.max_expand_latency_ms);
        overview_expand_latencies_ms.push(metrics.overview_expand_latency_ms);
        ipc_rtt_samples_ms.push(metrics.ipc_max_rtt_ms);
        pack_reload_latencies_ms.push(metrics.pack_max_reload_ms);
        gk_intercept_latencies_ms.push(metrics.gk_max_intercept_ms);
        audit_append_latencies_ms.push(metrics.audit_append_ms);
        audit_verify_latencies_ms.push(metrics.audit_verify_ms);
        voice_wave_latencies_ms.push(metrics.voice_wave_ms);
        voice_cancel_latencies_ms.push(metrics.voice_cancel_ms);
        ks_trip_latencies_ms.push(metrics.ks_trip_ms);
        idle_wake_latencies_ms.push(metrics.idle_wake_ms);

        ex_style_ok_count += usize::from(metrics.ex_style_verified);
        ma_noactivate_ok_count += usize::from(metrics.ma_noactivate_verified);
        focus_preserved_count += usize::from(metrics.focus_preserved);
        isolated_dir_ok_count += usize::from(metrics.isolated_data_dir_ok);
        ipc_csp_ok_count += usize::from(metrics.ipc_csp_xss_verified);
        overview_text_safe_count += usize::from(metrics.overview_text_safe);
        gk_verified_count += usize::from(metrics.gk_audit_verified);
        audit_jsonl_ok_count += usize::from(metrics.audit_jsonl_ok);
        voice_ok_count += usize::from(metrics.voice_ok);
        ks_ok_count += usize::from(metrics.ks_ok);
        idle_bank05_ok_count += usize::from(metrics.idle_bank05_ok);

        total_webview2_procs += metrics.webview2_processes_spawned;
        total_idle_timers += metrics.active_idle_timers;
        total_mode_c_cycles += metrics.mode_c_cycles;
        total_warm_reuses += metrics.warm_reuses;
        total_ttl_evictions += metrics.ttl_evictions;
        total_blocked_navs += metrics.blocked_external_navs;
        total_ipc_roundtrips += metrics.ipc_roundtrips;
        total_ipc_unauth_blocked += metrics.ipc_unauthorized_blocked;
        total_ipc_malformed_discarded += metrics.ipc_malformed_discarded;
        total_ipc_retained_bytes += metrics.ipc_retained_bytes;
        overview_bundle_bytes = metrics.overview_bundle_bytes;
        overview_layer_count = metrics.overview_layer_count;
        overview_accessory_slots = metrics.overview_accessory_slots;
        total_overview_events += metrics.overview_events_processed;
        overview_ring_len = metrics.overview_ring_len;
        max_overview_settled_fps = max_overview_settled_fps.max(metrics.overview_settled_fps);
        pack_spring_count = metrics.pack_spring_count;
        pack_moods_verified = metrics.pack_moods_verified;
        total_pack_hot_reloads += metrics.pack_hot_reloads;
        total_pack_rollbacks += metrics.pack_rollbacks;
        max_pack_settled_fps = max_pack_settled_fps.max(metrics.pack_settled_fps);
        total_gk_allowed += metrics.gk_allowed;
        total_gk_blocks += metrics.gk_blocks;
        total_gk_toctou += metrics.gk_toctou_blocks;
        total_gk_replay += metrics.gk_replay_blocks;
        total_gk_ttl_expired += metrics.gk_expired_blocks;
        total_gk_audit_entries += metrics.gk_audit_entries;
        total_audit_jsonl_entries += metrics.audit_jsonl_entries;
        total_audit_rotations += metrics.audit_rotations;
        audit_segments_per_worker = metrics.audit_segments;
        total_audit_viewer_rows += metrics.audit_viewer_rows;
        total_voice_ptt += metrics.voice_ptt_sessions;
        total_voice_wake += metrics.voice_wake_sessions;
        total_voice_power_blocks += metrics.voice_power_blocks;
        total_voice_cancellations += metrics.voice_cancellations;
        total_voice_zero_fills += metrics.voice_zero_fills;
        total_voice_residual_bytes += metrics.voice_residual_bytes;
        max_voice_idle_fps = max_voice_idle_fps.max(metrics.voice_idle_fps);
        total_ks_trips += metrics.ks_trips;
        total_ks_revoked += metrics.ks_revoked;
        total_ks_voice_aborts += metrics.ks_voice_aborts;
        total_ks_rearms += metrics.ks_rearms;
        total_ks_rejected_rearms += metrics.ks_rejected_rearms;
        total_ks_blocked += metrics.ks_blocked;
        total_idle_suspensions += metrics.idle_suspensions;
        total_idle_wakes += metrics.idle_wakes;
        total_idle_deferred_gaming += metrics.idle_deferred_gaming;
        total_idle_crit_alerts += metrics.idle_crit_alerts;
        total_idle_ww_frozen += metrics.idle_ww_frozen;
        max_idle_gaming_cpu_pct = max_idle_gaming_cpu_pct.max(metrics.idle_gaming_cpu_pct);
        verified_svgs = metrics.verified_svg_assets;
        last_metrics = Some(metrics);

        if hold_flag && !pre_kill_identities.is_empty() {
            thread::sleep(Duration::from_millis(POST_KILL_SETTLE_MS));
            let post_snap = query_windows_process_snapshot();
            let post_identities: BTreeSet<ProcessIdentity> = post_snap
                .into_iter()
                .map(|r| ProcessIdentity {
                    pid: r.pid,
                    name_lower: r.name.to_ascii_lowercase(),
                    creation_date: r.creation_date,
                })
                .collect();
            total_orphans += pre_kill_identities.intersection(&post_identities).count();
        }
    }

    let sample = last_metrics.ok_or_else(|| "no worker samples collected".to_string())?;
    Ok(CoordinatorAggregatedStats {
        bin_size_mb,
        sample,
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
        hover_p50: compute_percentile(&hover_latencies_ms, 50.0),
        hover_p95: compute_percentile(&hover_latencies_ms, 95.0),
        expand_p50: compute_percentile(&expand_latencies_ms, 50.0),
        expand_p95: compute_percentile(&expand_latencies_ms, 95.0),
        ov_expand_p50: compute_percentile(&overview_expand_latencies_ms, 50.0),
        ov_expand_p95: compute_percentile(&overview_expand_latencies_ms, 95.0),
        ipc_rtt_p50: compute_percentile(&ipc_rtt_samples_ms, 50.0),
        ipc_rtt_p95: compute_percentile(&ipc_rtt_samples_ms, 95.0),
        pack_reload_p50: compute_percentile(&pack_reload_latencies_ms, 50.0),
        pack_reload_p95: compute_percentile(&pack_reload_latencies_ms, 95.0),
        gk_intercept_p50: compute_percentile(&gk_intercept_latencies_ms, 50.0),
        gk_intercept_p95: compute_percentile(&gk_intercept_latencies_ms, 95.0),
        audit_append_p50: compute_percentile(&audit_append_latencies_ms, 50.0),
        audit_append_p95: compute_percentile(&audit_append_latencies_ms, 95.0),
        audit_verify_p50: compute_percentile(&audit_verify_latencies_ms, 50.0),
        audit_verify_p95: compute_percentile(&audit_verify_latencies_ms, 95.0),
        voice_wave_p50: compute_percentile(&voice_wave_latencies_ms, 50.0),
        voice_wave_p95: compute_percentile(&voice_wave_latencies_ms, 95.0),
        voice_cancel_p50: compute_percentile(&voice_cancel_latencies_ms, 50.0),
        voice_cancel_p95: compute_percentile(&voice_cancel_latencies_ms, 95.0),
        ks_trip_p50: compute_percentile(&ks_trip_latencies_ms, 50.0),
        ks_trip_p95: compute_percentile(&ks_trip_latencies_ms, 95.0),
        idle_wake_p50: compute_percentile(&idle_wake_latencies_ms, 50.0),
        idle_wake_p95: compute_percentile(&idle_wake_latencies_ms, 95.0),
        expanded_ram_p50: compute_percentile(&expanded_ram_samples_mb, 50.0),
        expanded_ram_p95: compute_percentile(&expanded_ram_samples_mb, 95.0),
        ram_p50: compute_percentile(&ram_samples_mb, 50.0),
        ram_p95: compute_percentile(&ram_samples_mb, 95.0),
    })
}

pub(crate) fn run_coordinator_benchmark(
    repetitions: usize,
    enforce_release_budgets: bool,
) -> Result<(), String> {
    let stats = collect_coordinator_benchmark_runs(repetitions)?;
    print_and_verify_coordinator_benchmark(&stats, repetitions, enforce_release_budgets)
}
