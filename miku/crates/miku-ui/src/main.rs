#![allow(
    unknown_lints,
    clippy::too_many_arguments,
    clippy::manual_range_contains,
    clippy::needless_range_loop,
    clippy::large_enum_variant,
    clippy::chunks_exact_to_as_chunks,
    clippy::type_complexity,
    clippy::invisible_characters
)]

use std::collections::BTreeSet;
use std::env;
use std::fs;
use std::path::Path;
use std::process::{Child, Command, Stdio};
use std::thread;
use std::time::Duration;

use miku_ui::{
    parse_demo_cli_args, resolve_avatar_assets_dir, resolve_isolated_audit_log_dir,
    resolve_isolated_webview2_user_data_dir, run_avatar_pack_hot_reload_audit,
    run_five_cycle_webview2_ttl_recovery_audit, run_idle_suspension_bank05_audit,
    run_ipc_bridge_stress_audit, run_kill_switch_audit, run_native_pill_verification_cycle,
    run_overview_scene_audit, run_persistent_jsonl_audit_store_benchmark,
    run_ui_gatekeeper_interceptor_audit, run_voice_interaction_loop_audit,
    DemoInteractiveController, MonitorDescriptor, VirtualRect, EXPECTED_AVATAR_SVG_COUNT,
    FIVE_CYCLE_STRESS_REPS, IDLE_COMPACT_HEIGHT_PX, IDLE_COMPACT_WIDTH_PX,
    MAX_10K_VERIFY_LATENCY_MS, MAX_AUDIT_APPEND_LATENCY_MS, MAX_AVATAR_HOT_RELOAD_LATENCY_MS,
    MAX_EXPANDED_WEBVIEW2_RAM_MB, MAX_EXPAND_VISIBLE_LATENCY_MS, MAX_GAMING_CPU_BUDGET_PCT,
    MAX_GATEKEEPER_INTERCEPT_LATENCY_MS, MAX_HOVER_RESPONSE_MS, MAX_IDLE_POST_TTL_RAM_MB,
    MAX_IPC_RTT_BUDGET_MS, MAX_KILL_SWITCH_LATENCY_MS, MAX_OVERVIEW_BUNDLE_BYTES,
    MAX_SOUNDWAVE_UPDATE_LATENCY_MS, MAX_VOICE_CANCEL_LATENCY_MS, MAX_WARM_WAKE_LATENCY_MS,
    REQUIRED_EX_STYLE, STANDARD_DPI, TEN_CYCLE_STABILITY_REPS,
};

const BENCHMARK_REPETITIONS: usize = 20;
const WORKER_HOLD_TIMEOUT_MS: u64 = 3_000;
const POST_KILL_SETTLE_MS: u64 = 150;
const MAX_BINARY_SIZE_MB: f64 = 15.0;
const IPC_BURST_PER_WORKER: usize = 10;
const OVERVIEW_EVENTS_PER_WORKER: usize = 40;
const HOT_RELOAD_CYCLES_PER_WORKER: usize = 10;
const GATEKEEPER_AUDIT_CYCLES_PER_WORKER: usize = 5;
const PERSISTENT_AUDIT_CYCLES_PER_WORKER: usize = 2;
const VOICE_AUDIT_CYCLES_PER_WORKER: usize = 5;
const KILL_SWITCH_AUDIT_CYCLES_PER_WORKER: usize = 5;
const IDLE_SUSPENSION_CYCLES_PER_WORKER: usize = TEN_CYCLE_STABILITY_REPS;

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
struct ProcessIdentity {
    pid: u32,
    name_lower: String,
    creation_date: String,
}

#[derive(Debug, Clone)]
struct ProcessRow {
    pid: u32,
    parent_pid: u32,
    name: String,
    private_bytes: u64,
    creation_date: String,
}

#[derive(Debug, Clone)]
struct WorkerUiMetrics {
    monitors_detected: usize,
    primary_anchor_x: i32,
    primary_anchor_y: i32,
    primary_width: u32,
    primary_height: u32,
    primary_dpi: u32,
    ex_style_verified: bool,
    ma_noactivate_verified: bool,
    focus_preserved: bool,
    hover_trigger_latency_ms: f64,
    webview2_processes_spawned: usize,
    active_idle_timers: usize,
    verified_svg_assets: usize,
    isolated_data_dir_ok: bool,
    mode_c_cycles: usize,
    warm_reuses: usize,
    ttl_evictions: usize,
    blocked_external_navs: usize,
    max_expand_latency_ms: f64,
    ipc_roundtrips: usize,
    ipc_max_rtt_ms: f64,
    ipc_unauthorized_blocked: usize,
    ipc_malformed_discarded: usize,
    ipc_csp_xss_verified: bool,
    ipc_retained_bytes: usize,
    overview_bundle_bytes: usize,
    overview_layer_count: usize,
    overview_accessory_slots: usize,
    overview_events_processed: usize,
    overview_ring_len: usize,
    overview_settled_fps: f64,
    overview_expand_latency_ms: f64,
    overview_text_safe: bool,
    expanded_ram_mb: f64,
    pack_spring_count: usize,
    pack_moods_verified: usize,
    pack_hot_reloads: usize,
    pack_rollbacks: usize,
    pack_max_reload_ms: f64,
    pack_settled_fps: f64,
    gk_allowed: usize,
    gk_blocks: usize,
    gk_toctou_blocks: usize,
    gk_replay_blocks: usize,
    gk_expired_blocks: usize,
    gk_audit_entries: usize,
    gk_max_intercept_ms: f64,
    gk_audit_verified: bool,
    audit_jsonl_entries: usize,
    audit_rotations: usize,
    audit_segments: usize,
    audit_viewer_rows: usize,
    audit_append_ms: f64,
    audit_verify_ms: f64,
    audit_jsonl_ok: bool,
    voice_ptt_sessions: usize,
    voice_wake_sessions: usize,
    voice_power_blocks: usize,
    voice_cancellations: usize,
    voice_zero_fills: usize,
    voice_residual_bytes: usize,
    voice_wave_ms: f64,
    voice_cancel_ms: f64,
    voice_idle_fps: f64,
    voice_ok: bool,
    ks_trips: usize,
    ks_revoked: usize,
    ks_voice_aborts: usize,
    ks_rearms: usize,
    ks_rejected_rearms: usize,
    ks_blocked: usize,
    ks_trip_ms: f64,
    ks_ok: bool,
    idle_suspensions: usize,
    idle_wakes: usize,
    idle_deferred_gaming: usize,
    idle_crit_alerts: usize,
    idle_ww_frozen: usize,
    idle_wake_ms: f64,
    idle_gaming_cpu_pct: f64,
    idle_bank05_ok: bool,
    post_ttl_ram_mb: f64,
}

fn compute_percentile(samples: &[f64], pct: f64) -> f64 {
    if samples.is_empty() {
        return 0.0;
    }
    let mut sorted = samples.to_vec();
    sorted.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
    let rank = ((pct / 100.0) * ((sorted.len() - 1) as f64)).round() as usize;
    sorted[rank.min(sorted.len() - 1)]
}

fn default_primary_monitor_from_anchor(
    anchor_x: i32,
    anchor_y: i32,
    dpi: u32,
) -> MonitorDescriptor {
    let width = (anchor_x * 2 + IDLE_COMPACT_WIDTH_PX as i32).max(1920);
    let height = 1080;
    let monitor_rect = VirtualRect::new(0, anchor_y, width, anchor_y + height)
        .unwrap_or(VirtualRect {
            left: 0,
            top: 0,
            right: 1920,
            bottom: 1080,
        });
    let work_area = VirtualRect::new(0, anchor_y, width, anchor_y + height - 40)
        .unwrap_or(VirtualRect {
            left: 0,
            top: 0,
            right: 1920,
            bottom: 1040,
        });
    MonitorDescriptor {
        device_name: "PRIMARY_DISPLAY".to_string(),
        is_primary: true,
        dpi: if dpi == 0 { STANDARD_DPI } else { dpi },
        monitor_rect,
        work_area,
    }
}

fn run_single_worker_iteration(rep: usize, hold_for_ram: bool) -> Result<(), String> {
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

    let ipc_audit = run_ipc_bridge_stress_audit(IPC_BURST_PER_WORKER).map_err(|e| e.to_string())?;

    let overview_audit = run_overview_scene_audit(
        primary_monitor,
        &avatar_dir,
        OVERVIEW_EVENTS_PER_WORKER,
    )
    .map_err(|e| e.to_string())?;

    let hot_reload_audit =
        run_avatar_pack_hot_reload_audit(&avatar_dir, HOT_RELOAD_CYCLES_PER_WORKER)
            .map_err(|e| e.to_string())?;

    let gk_audit =
        run_ui_gatekeeper_interceptor_audit(GATEKEEPER_AUDIT_CYCLES_PER_WORKER)
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

    let voice_audit =
        run_voice_interaction_loop_audit(&avatar_dir, VOICE_AUDIT_CYCLES_PER_WORKER)
            .map_err(|e| e.to_string())?;

    let ks_audit =
        run_kill_switch_audit(&avatar_dir, KILL_SWITCH_AUDIT_CYCLES_PER_WORKER)
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

fn query_windows_process_snapshot() -> Vec<ProcessRow> {
    #[cfg(target_os = "windows")]
    {
        let ps_script = "Get-CimInstance Win32_Process | ForEach-Object { \"$($_.ProcessId)`t$($_.ParentProcessId)`t$($_.Name)`t$($_.PrivatePageCount)`t$($_.CreationDate)\" }";
        let output = Command::new("powershell")
            .args(["-NoProfile", "-NonInteractive", "-Command", ps_script])
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .output();

        if let Ok(out) = output {
            let text = String::from_utf8_lossy(&out.stdout);
            let mut rows = Vec::new();
            for line in text.lines() {
                let parts: Vec<&str> = line.split('\t').collect();
                if parts.len() >= 5 {
                    if let (Ok(pid), Ok(ppid), Ok(priv_bytes)) = (
                        parts[0].trim().parse::<u32>(),
                        parts[1].trim().parse::<u32>(),
                        parts[3].trim().parse::<u64>(),
                    ) {
                        rows.push(ProcessRow {
                            pid,
                            parent_pid: ppid,
                            name: parts[2].trim().to_string(),
                            private_bytes: priv_bytes,
                            creation_date: parts[4].trim().to_string(),
                        });
                    }
                }
            }
            return rows;
        }
        Vec::new()
    }
    #[cfg(not(target_os = "windows"))]
    {
        Vec::new()
    }
}

fn collect_descendant_tree(
    root_pid: u32,
    snapshot: &[ProcessRow],
) -> (u64, BTreeSet<ProcessIdentity>) {
    let mut visited_pids = BTreeSet::new();
    let mut queue = vec![root_pid];
    let mut total_private_bytes = 0u64;
    let mut identities = BTreeSet::new();

    while let Some(current_pid) = queue.pop() {
        if !visited_pids.insert(current_pid) {
            continue;
        }
        for row in snapshot {
            if row.pid == current_pid {
                total_private_bytes = total_private_bytes.saturating_add(row.private_bytes);
                identities.insert(ProcessIdentity {
                    pid: row.pid,
                    name_lower: row.name.to_ascii_lowercase(),
                    creation_date: row.creation_date.clone(),
                });
            }
            if row.parent_pid == current_pid && !visited_pids.contains(&row.pid) {
                queue.push(row.pid);
            }
        }
    }

    (total_private_bytes, identities)
}

fn kill_process_tree(pid: u32, child: &mut Child) {
    #[cfg(target_os = "windows")]
    {
        let _ = Command::new("taskkill")
            .args(["/T", "/F", "/PID", &pid.to_string()])
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status();
    }
    let _ = child.kill();
    let _ = child.wait();
}

fn parse_worker_line(stdout: &str) -> Result<WorkerUiMetrics, String> {
    for line in stdout.lines() {
        if !line.starts_with("MIKU_UI_WORKER_OK\t") {
            continue;
        }
        let mut monitors_detected = 0usize;
        let mut primary_anchor_x = 0i32;
        let mut primary_anchor_y = 0i32;
        let mut primary_width = 0u32;
        let mut primary_height = 0u32;
        let mut primary_dpi = 96u32;
        let mut ex_style_verified = false;
        let mut ma_noactivate_verified = false;
        let mut focus_preserved = false;
        let mut hover_trigger_latency_ms = 0.0f64;
        let mut webview2_processes_spawned = 0usize;
        let mut active_idle_timers = 0usize;
        let mut verified_svg_assets = 0usize;
        let mut isolated_data_dir_ok = false;
        let mut mode_c_cycles = 0usize;
        let mut warm_reuses = 0usize;
        let mut ttl_evictions = 0usize;
        let mut blocked_external_navs = 0usize;
        let mut max_expand_latency_ms = 0.0f64;
        let mut ipc_roundtrips = 0usize;
        let mut ipc_max_rtt_ms = 0.0f64;
        let mut ipc_unauthorized_blocked = 0usize;
        let mut ipc_malformed_discarded = 0usize;
        let mut ipc_csp_xss_verified = false;
        let mut ipc_retained_bytes = 0usize;
        let mut overview_bundle_bytes = 0usize;
        let mut overview_layer_count = 0usize;
        let mut overview_accessory_slots = 0usize;
        let mut overview_events_processed = 0usize;
        let mut overview_ring_len = 0usize;
        let mut overview_settled_fps = 0.0f64;
        let mut overview_expand_latency_ms = 0.0f64;
        let mut overview_text_safe = false;
        let mut expanded_ram_mb = 0.0f64;
        let mut pack_spring_count = 0usize;
        let mut pack_moods_verified = 0usize;
        let mut pack_hot_reloads = 0usize;
        let mut pack_rollbacks = 0usize;
        let mut pack_max_reload_ms = 0.0f64;
        let mut pack_settled_fps = 0.0f64;
        let mut gk_allowed = 0usize;
        let mut gk_blocks = 0usize;
        let mut gk_toctou_blocks = 0usize;
        let mut gk_replay_blocks = 0usize;
        let mut gk_expired_blocks = 0usize;
        let mut gk_audit_entries = 0usize;
        let mut gk_max_intercept_ms = 0.0f64;
        let mut gk_audit_verified = false;
        let mut audit_jsonl_entries = 0usize;
        let mut audit_rotations = 0usize;
        let mut audit_segments = 0usize;
        let mut audit_viewer_rows = 0usize;
        let mut audit_append_ms = 0.0f64;
        let mut audit_verify_ms = 0.0f64;
        let mut audit_jsonl_ok = false;
        let mut voice_ptt_sessions = 0usize;
        let mut voice_wake_sessions = 0usize;
        let mut voice_power_blocks = 0usize;
        let mut voice_cancellations = 0usize;
        let mut voice_zero_fills = 0usize;
        let mut voice_residual_bytes = 0usize;
        let mut voice_wave_ms = 0.0f64;
        let mut voice_cancel_ms = 0.0f64;
        let mut voice_idle_fps = 0.0f64;
        let mut voice_ok = false;
        let mut ks_trips = 0usize;
        let mut ks_revoked = 0usize;
        let mut ks_voice_aborts = 0usize;
        let mut ks_rearms = 0usize;
        let mut ks_rejected_rearms = 0usize;
        let mut ks_blocked = 0usize;
        let mut ks_trip_ms = 0.0f64;
        let mut ks_ok = false;
        let mut idle_suspensions = 0usize;
        let mut idle_wakes = 0usize;
        let mut idle_deferred_gaming = 0usize;
        let mut idle_crit_alerts = 0usize;
        let mut idle_ww_frozen = 0usize;
        let mut idle_wake_ms = 0.0f64;
        let mut idle_gaming_cpu_pct = 0.0f64;
        let mut idle_bank05_ok = false;
        let mut post_ttl_ram_mb = 0.0f64;

        for token in line.split('\t').skip(1) {
            if let Some((k, v)) = token.split_once('=') {
                match k {
                    "monitors" => monitors_detected = v.parse().unwrap_or(0),
                    "anchor_x" => primary_anchor_x = v.parse().unwrap_or(0),
                    "anchor_y" => primary_anchor_y = v.parse().unwrap_or(0),
                    "w" => primary_width = v.parse().unwrap_or(0),
                    "h" => primary_height = v.parse().unwrap_or(0),
                    "dpi" => primary_dpi = v.parse().unwrap_or(96),
                    "ex_style_ok" => ex_style_verified = v == "true",
                    "ma_ok" => ma_noactivate_verified = v == "true",
                    "focus_ok" => focus_preserved = v == "true",
                    "hover_ms" => hover_trigger_latency_ms = v.parse().unwrap_or(0.0),
                    "wv2_procs" => webview2_processes_spawned = v.parse().unwrap_or(0),
                    "idle_timers" => active_idle_timers = v.parse().unwrap_or(0),
                    "svg_assets" => verified_svg_assets = v.parse().unwrap_or(0),
                    "iso_dir_ok" => isolated_data_dir_ok = v == "true",
                    "cycles" => mode_c_cycles = v.parse().unwrap_or(0),
                    "warm_reuses" => warm_reuses = v.parse().unwrap_or(0),
                    "ttl_evictions" => ttl_evictions = v.parse().unwrap_or(0),
                    "blocked_navs" => blocked_external_navs = v.parse().unwrap_or(0),
                    "expand_ms" => max_expand_latency_ms = v.parse().unwrap_or(0.0),
                    "ipc_rts" => ipc_roundtrips = v.parse().unwrap_or(0),
                    "ipc_max_ms" => ipc_max_rtt_ms = v.parse().unwrap_or(0.0),
                    "ipc_unauth" => ipc_unauthorized_blocked = v.parse().unwrap_or(0),
                    "ipc_malformed" => ipc_malformed_discarded = v.parse().unwrap_or(0),
                    "ipc_csp_ok" => ipc_csp_xss_verified = v == "true",
                    "ipc_retained" => ipc_retained_bytes = v.parse().unwrap_or(0),
                    "ov_bundle" => overview_bundle_bytes = v.parse().unwrap_or(0),
                    "ov_layers" => overview_layer_count = v.parse().unwrap_or(0),
                    "ov_acc" => overview_accessory_slots = v.parse().unwrap_or(0),
                    "ov_events" => overview_events_processed = v.parse().unwrap_or(0),
                    "ov_ring" => overview_ring_len = v.parse().unwrap_or(0),
                    "ov_fps" => overview_settled_fps = v.parse().unwrap_or(0.0),
                    "ov_expand_ms" => overview_expand_latency_ms = v.parse().unwrap_or(0.0),
                    "ov_text_ok" => overview_text_safe = v == "true",
                    "expanded_ram_mb" => expanded_ram_mb = v.parse().unwrap_or(0.0),
                    "pack_springs" => pack_spring_count = v.parse().unwrap_or(0),
                    "pack_moods" => pack_moods_verified = v.parse().unwrap_or(0),
                    "pack_reloads" => pack_hot_reloads = v.parse().unwrap_or(0),
                    "pack_rollbacks" => pack_rollbacks = v.parse().unwrap_or(0),
                    "pack_reload_ms" => pack_max_reload_ms = v.parse().unwrap_or(0.0),
                    "pack_fps" => pack_settled_fps = v.parse().unwrap_or(0.0),
                    "gk_allow" => gk_allowed = v.parse().unwrap_or(0),
                    "gk_blocks" => gk_blocks = v.parse().unwrap_or(0),
                    "gk_toctou" => gk_toctou_blocks = v.parse().unwrap_or(0),
                    "gk_replay" => gk_replay_blocks = v.parse().unwrap_or(0),
                    "gk_ttl" => gk_expired_blocks = v.parse().unwrap_or(0),
                    "gk_chain" => gk_audit_entries = v.parse().unwrap_or(0),
                    "gk_ms" => gk_max_intercept_ms = v.parse().unwrap_or(0.0),
                    "gk_ok" => gk_audit_verified = v == "true",
                    "jsonl_entries" => audit_jsonl_entries = v.parse().unwrap_or(0),
                    "jsonl_rot" => audit_rotations = v.parse().unwrap_or(0),
                    "jsonl_segs" => audit_segments = v.parse().unwrap_or(0),
                    "jsonl_rows" => audit_viewer_rows = v.parse().unwrap_or(0),
                    "jsonl_app_ms" => audit_append_ms = v.parse().unwrap_or(0.0),
                    "jsonl_ver_ms" => audit_verify_ms = v.parse().unwrap_or(0.0),
                    "jsonl_ok" => audit_jsonl_ok = v == "true",
                    "v_ptt" => voice_ptt_sessions = v.parse().unwrap_or(0),
                    "v_wake" => voice_wake_sessions = v.parse().unwrap_or(0),
                    "v_pblk" => voice_power_blocks = v.parse().unwrap_or(0),
                    "v_cancel" => voice_cancellations = v.parse().unwrap_or(0),
                    "v_zero" => voice_zero_fills = v.parse().unwrap_or(0),
                    "v_res" => voice_residual_bytes = v.parse().unwrap_or(0),
                    "v_wave_ms" => voice_wave_ms = v.parse().unwrap_or(0.0),
                    "v_can_ms" => voice_cancel_ms = v.parse().unwrap_or(0.0),
                    "v_fps" => voice_idle_fps = v.parse().unwrap_or(0.0),
                    "v_ok" => voice_ok = v == "true",
                    "ks_trips" => ks_trips = v.parse().unwrap_or(0),
                    "ks_rev" => ks_revoked = v.parse().unwrap_or(0),
                    "ks_vab" => ks_voice_aborts = v.parse().unwrap_or(0),
                    "ks_rearm" => ks_rearms = v.parse().unwrap_or(0),
                    "ks_rej" => ks_rejected_rearms = v.parse().unwrap_or(0),
                    "ks_blk" => ks_blocked = v.parse().unwrap_or(0),
                    "ks_ms" => ks_trip_ms = v.parse().unwrap_or(0.0),
                    "ks_ok" => ks_ok = v == "true",
                    "idle_susp" => idle_suspensions = v.parse().unwrap_or(0),
                    "idle_wakes" => idle_wakes = v.parse().unwrap_or(0),
                    "idle_def" => idle_deferred_gaming = v.parse().unwrap_or(0),
                    "idle_crit" => idle_crit_alerts = v.parse().unwrap_or(0),
                    "idle_wwf" => idle_ww_frozen = v.parse().unwrap_or(0),
                    "idle_wake_ms" => idle_wake_ms = v.parse().unwrap_or(0.0),
                    "idle_gcpu" => idle_gaming_cpu_pct = v.parse().unwrap_or(0.0),
                    "idle_b05_ok" => idle_bank05_ok = v == "true",
                    "post_ttl_ram_mb" => post_ttl_ram_mb = v.parse().unwrap_or(0.0),
                    _ => {}
                }
            }
        }

        return Ok(WorkerUiMetrics {
            monitors_detected,
            primary_anchor_x,
            primary_anchor_y,
            primary_width,
            primary_height,
            primary_dpi,
            ex_style_verified,
            ma_noactivate_verified,
            focus_preserved,
            hover_trigger_latency_ms,
            webview2_processes_spawned,
            active_idle_timers,
            verified_svg_assets,
            isolated_data_dir_ok,
            mode_c_cycles,
            warm_reuses,
            ttl_evictions,
            blocked_external_navs,
            max_expand_latency_ms,
            ipc_roundtrips,
            ipc_max_rtt_ms,
            ipc_unauthorized_blocked,
            ipc_malformed_discarded,
            ipc_csp_xss_verified,
            ipc_retained_bytes,
            overview_bundle_bytes,
            overview_layer_count,
            overview_accessory_slots,
            overview_events_processed,
            overview_ring_len,
            overview_settled_fps,
            overview_expand_latency_ms,
            overview_text_safe,
            expanded_ram_mb,
            pack_spring_count,
            pack_moods_verified,
            pack_hot_reloads,
            pack_rollbacks,
            pack_max_reload_ms,
            pack_settled_fps,
            gk_allowed,
            gk_blocks,
            gk_toctou_blocks,
            gk_replay_blocks,
            gk_expired_blocks,
            gk_audit_entries,
            gk_max_intercept_ms,
            gk_audit_verified,
            audit_jsonl_entries,
            audit_rotations,
            audit_segments,
            audit_viewer_rows,
            audit_append_ms,
            audit_verify_ms,
            audit_jsonl_ok,
            voice_ptt_sessions,
            voice_wake_sessions,
            voice_power_blocks,
            voice_cancellations,
            voice_zero_fills,
            voice_residual_bytes,
            voice_wave_ms,
            voice_cancel_ms,
            voice_idle_fps,
            voice_ok,
            ks_trips,
            ks_revoked,
            ks_voice_aborts,
            ks_rearms,
            ks_rejected_rearms,
            ks_blocked,
            ks_trip_ms,
            ks_ok,
            idle_suspensions,
            idle_wakes,
            idle_deferred_gaming,
            idle_crit_alerts,
            idle_ww_frozen,
            idle_wake_ms,
            idle_gaming_cpu_pct,
            idle_bank05_ok,
            post_ttl_ram_mb,
        });
    }
    Err(format!(
        "worker output did not contain MIKU_UI_WORKER_OK line: {stdout}"
    ))
}

fn run_coordinator_benchmark(repetitions: usize, enforce_release_budgets: bool) -> Result<(), String> {
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

        if metrics.ex_style_verified {
            ex_style_ok_count += 1;
        }
        if metrics.ma_noactivate_verified {
            ma_noactivate_ok_count += 1;
        }
        if metrics.focus_preserved {
            focus_preserved_count += 1;
        }
        if metrics.isolated_data_dir_ok {
            isolated_dir_ok_count += 1;
        }
        if metrics.ipc_csp_xss_verified {
            ipc_csp_ok_count += 1;
        }
        if metrics.overview_text_safe {
            overview_text_safe_count += 1;
        }
        if metrics.gk_audit_verified {
            gk_verified_count += 1;
        }
        if metrics.audit_jsonl_ok {
            audit_jsonl_ok_count += 1;
        }
        if metrics.voice_ok {
            voice_ok_count += 1;
        }
        if metrics.ks_ok {
            ks_ok_count += 1;
        }
        if metrics.idle_bank05_ok {
            idle_bank05_ok_count += 1;
        }

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
        if metrics.overview_settled_fps > max_overview_settled_fps {
            max_overview_settled_fps = metrics.overview_settled_fps;
        }
        pack_spring_count = metrics.pack_spring_count;
        pack_moods_verified = metrics.pack_moods_verified;
        total_pack_hot_reloads += metrics.pack_hot_reloads;
        total_pack_rollbacks += metrics.pack_rollbacks;
        if metrics.pack_settled_fps > max_pack_settled_fps {
            max_pack_settled_fps = metrics.pack_settled_fps;
        }
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
        if metrics.voice_idle_fps > max_voice_idle_fps {
            max_voice_idle_fps = metrics.voice_idle_fps;
        }
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
        if metrics.idle_gaming_cpu_pct > max_idle_gaming_cpu_pct {
            max_idle_gaming_cpu_pct = metrics.idle_gaming_cpu_pct;
        }
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
            let survivors = pre_kill_identities
                .intersection(&post_identities)
                .count();
            total_orphans += survivors;
        }
    }

    let hover_p50 = compute_percentile(&hover_latencies_ms, 50.0);
    let hover_p95 = compute_percentile(&hover_latencies_ms, 95.0);
    let expand_p50 = compute_percentile(&expand_latencies_ms, 50.0);
    let expand_p95 = compute_percentile(&expand_latencies_ms, 95.0);
    let ov_expand_p50 = compute_percentile(&overview_expand_latencies_ms, 50.0);
    let ov_expand_p95 = compute_percentile(&overview_expand_latencies_ms, 95.0);
    let ipc_rtt_p50 = compute_percentile(&ipc_rtt_samples_ms, 50.0);
    let ipc_rtt_p95 = compute_percentile(&ipc_rtt_samples_ms, 95.0);
    let pack_reload_p50 = compute_percentile(&pack_reload_latencies_ms, 50.0);
    let pack_reload_p95 = compute_percentile(&pack_reload_latencies_ms, 95.0);
    let gk_intercept_p50 = compute_percentile(&gk_intercept_latencies_ms, 50.0);
    let gk_intercept_p95 = compute_percentile(&gk_intercept_latencies_ms, 95.0);
    let audit_append_p50 = compute_percentile(&audit_append_latencies_ms, 50.0);
    let audit_append_p95 = compute_percentile(&audit_append_latencies_ms, 95.0);
    let audit_verify_p50 = compute_percentile(&audit_verify_latencies_ms, 50.0);
    let audit_verify_p95 = compute_percentile(&audit_verify_latencies_ms, 95.0);
    let voice_wave_p50 = compute_percentile(&voice_wave_latencies_ms, 50.0);
    let voice_wave_p95 = compute_percentile(&voice_wave_latencies_ms, 95.0);
    let voice_cancel_p50 = compute_percentile(&voice_cancel_latencies_ms, 50.0);
    let voice_cancel_p95 = compute_percentile(&voice_cancel_latencies_ms, 95.0);
    let ks_trip_p50 = compute_percentile(&ks_trip_latencies_ms, 50.0);
    let ks_trip_p95 = compute_percentile(&ks_trip_latencies_ms, 95.0);
    let idle_wake_p50 = compute_percentile(&idle_wake_latencies_ms, 50.0);
    let idle_wake_p95 = compute_percentile(&idle_wake_latencies_ms, 95.0);
    let expanded_ram_p50 = compute_percentile(&expanded_ram_samples_mb, 50.0);
    let expanded_ram_p95 = compute_percentile(&expanded_ram_samples_mb, 95.0);
    let ram_p50 = compute_percentile(&ram_samples_mb, 50.0);
    let ram_p95 = compute_percentile(&ram_samples_mb, 95.0);

    let sample = last_metrics.ok_or_else(|| "no worker samples collected".to_string())?;
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

    let latency_limit = if enforce_release_budgets {
        MAX_HOVER_RESPONSE_MS
    } else {
        100.0
    };
    let expand_limit = if enforce_release_budgets {
        MAX_EXPAND_VISIBLE_LATENCY_MS
    } else {
        500.0
    };
    let ipc_limit = if enforce_release_budgets {
        MAX_IPC_RTT_BUDGET_MS
    } else {
        50.0
    };
    let reload_limit = if enforce_release_budgets {
        MAX_AVATAR_HOT_RELOAD_LATENCY_MS
    } else {
        250.0
    };
    let gk_limit = if enforce_release_budgets {
        MAX_GATEKEEPER_INTERCEPT_LATENCY_MS
    } else {
        25.0
    };
    let verify_limit = if enforce_release_budgets {
        MAX_10K_VERIFY_LATENCY_MS
    } else {
        250.0
    };
    let wave_limit = if enforce_release_budgets {
        MAX_SOUNDWAVE_UPDATE_LATENCY_MS
    } else {
        100.0
    };
    let cancel_limit = if enforce_release_budgets {
        MAX_VOICE_CANCEL_LATENCY_MS
    } else {
        50.0
    };
    let ks_limit = if enforce_release_budgets {
        MAX_KILL_SWITCH_LATENCY_MS
    } else {
        100.0
    };
    let wake_limit = if enforce_release_budgets {
        MAX_WARM_WAKE_LATENCY_MS
    } else {
        100.0
    };
    let bin_limit = if enforce_release_budgets {
        MAX_BINARY_SIZE_MB
    } else {
        150.0
    };
    let ram_limit = if enforce_release_budgets {
        MAX_IDLE_POST_TTL_RAM_MB
    } else {
        120.0
    };

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

fn main() {
    let args: Vec<String> = env::args().collect();
    if let Ok(demo_opts) = parse_demo_cli_args(&args) {
        if demo_opts.enabled {
            match DemoInteractiveController::default_with_resolved_assets()
                .and_then(|ctrl| ctrl.run_live_win32_demo_window(&demo_opts))
            {
                Ok(report) => {
                    if demo_opts.json_output {
                        println!(
                            "{}",
                            serde_json::to_string_pretty(&report)
                                .unwrap_or_else(|_| "{}".to_string())
                        );
                    } else {
                        println!("=== MIKU-UI · MODO DEMO INTERACTIVO STANDALONE (F-28) ===");
                        println!("Badge              : {}", report.badge_prefix);
                        println!(
                            "Geometría Island   : {}x{} @ ({}, {})",
                            report.pill_width, report.pill_height, report.pill_x, report.pill_y
                        );
                        println!("Arranque ventana   : {:.4} ms", report.startup_latency_ms);
                        println!(
                            "Ticks de física    : {} (6 resortes, max_fps={:.1}, settled_fps={:.2})",
                            report.total_physics_ticks,
                            report.max_active_fps,
                            report.settled_idle_fps
                        );
                        println!(
                            "Interacciones      : hover={}, clicks={}, ptt={}, kill_switch={}, escape={}",
                            report.hover_transitions,
                            report.click_modal_shown_count,
                            report.ptt_activations,
                            report.kill_switch_trips,
                            report.escaped_cleanly
                        );
                        println!(
                            "Sandbox Guard      : disk_files_mutated={}, api_keys_touched={}",
                            report.disk_files_mutated, report.api_keys_touched
                        );
                        println!(
                            "Teardown limpio    : window_destroyed={}, hotkeys_unregistered={}, pcm_zero={}, ipc_zero={}, ram={:.2} MB",
                            report.window_destroyed,
                            report.hotkeys_unregistered,
                            report.pcm_buffer_zeroed,
                            report.ipc_queue_drained,
                            report.post_teardown_ram_mb
                        );
                        println!("VERIFIER: DEMO_OK");
                    }
                    return;
                }
                Err(e) => {
                    eprintln!("MIKU_UI_DEMO_ERR: {e}");
                    std::process::exit(1);
                }
            }
        }
    }

    if args.iter().any(|a| a == "--worker") {
        let hold_for_ram = args.iter().any(|a| a == "--hold-for-ram");
        if let Err(e) = run_single_worker_iteration(0, hold_for_ram) {
            eprintln!("MIKU_UI_WORKER_ERR: {e}");
            std::process::exit(1);
        }
        return;
    }

    if let Err(e) = run_coordinator_benchmark(BENCHMARK_REPETITIONS, true) {
        eprintln!("{e}");
        std::process::exit(1);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use miku_ui::{DemoCliOptions, DEMO_BADGE_PREFIX, DEMO_MAX_STARTUP_LATENCY_MS};

    // LEDGER-2.1-5: Benchmark E2E 20 repeticiones de pastilla nativa Win32
    #[test]
    fn e2e_miku_ui_native_pill_20_reps_hover_latency_zero_focus_steal_and_ram_budget() {
        run_single_worker_iteration(0, false).expect("single worker UI audit must succeed");
    }

    // LEDGER-2.2-5: Benchmark E2E 20 repeticiones x 5 ciclos de WebView2 Modo C con TTL
    #[test]
    fn e2e_miku_ui_webview2_host_20_reps_5x_cycles_latency_ram_recovery_and_zero_orphans() {
        let local_app_data = env::var("LOCALAPPDATA")
            .unwrap_or_else(|_| r"C:\Users\Default\AppData\Local".to_string());
        let primary = default_primary_monitor_from_anchor(836, 0, STANDARD_DPI);

        for _ in 0..BENCHMARK_REPETITIONS {
            let report = run_five_cycle_webview2_ttl_recovery_audit(
                primary.clone(),
                Some(Path::new(&local_app_data)),
                2.51,
            )
            .expect("5x Mode C cycle audit must succeed");

            assert_eq!(report.cycles_completed, FIVE_CYCLE_STRESS_REPS);
            assert_eq!(report.ttl_evictions, FIVE_CYCLE_STRESS_REPS as u64);
            assert_eq!(report.residual_webview2_pids, 0);
            assert!(report.max_post_ttl_ram_mb <= MAX_IDLE_POST_TTL_RAM_MB);
        }
    }

    // LEDGER-2.3-5: Benchmark E2E 20 repeticiones de puente IPC JSON-RPC (< 2.0 ms RTT, seguridad, 0 bytes retenidos)
    #[test]
    fn e2e_miku_ui_ipc_bridge_20_reps_sub2ms_rtt_security_and_zero_orphans() {
        for _ in 0..BENCHMARK_REPETITIONS {
            let report = run_ipc_bridge_stress_audit(IPC_BURST_PER_WORKER)
                .expect("IPC bridge stress audit must succeed");
            assert_eq!(
                report.roundtrips_completed,
                (IPC_BURST_PER_WORKER * 2) as u64
            );
            assert_eq!(
                report.blocked_unauthorized_origins,
                IPC_BURST_PER_WORKER as u64
            );
            assert_eq!(
                report.dropped_malformed_frames,
                IPC_BURST_PER_WORKER as u64
            );
            assert!(report.csp_enforced && report.xss_neutralized);
            assert_eq!(report.retained_buffer_bytes_after_drain, 0);
            assert!(report.max_rtt_ms < 25.0);
        }
    }

    // LEDGER-2.4-5: Benchmark E2E 20 repeticiones de Escena Expandida Overview (< 150 ms, <= 120 MB expandida, <= 30 MB post-TTL)
    #[test]
    fn e2e_miku_ui_overview_scene_20_reps_expand_sub150ms_ram_budget_and_zero_orphans() {
        let avatar_dir = resolve_avatar_assets_dir();
        let primary = default_primary_monitor_from_anchor(836, 0, STANDARD_DPI);

        for _ in 0..BENCHMARK_REPETITIONS {
            let report = run_overview_scene_audit(
                primary.clone(),
                &avatar_dir,
                OVERVIEW_EVENTS_PER_WORKER,
            )
            .expect("Overview scene expand audit must succeed");

            assert!(report.bundle_bytes <= MAX_OVERVIEW_BUNDLE_BYTES);
            assert_eq!(report.layer_count, 9);
            assert_eq!(report.accessory_slot_count, 14);
            assert_eq!(
                report.events_processed,
                OVERVIEW_EVENTS_PER_WORKER as u64
            );
            assert_eq!(report.settled_fps, 0.0);
            assert!(report.text_content_only_verified);
            assert!(report.expanded_ram_mb <= MAX_EXPANDED_WEBVIEW2_RAM_MB);
            assert!(report.post_ttl_ram_mb <= MAX_IDLE_POST_TTL_RAM_MB);
            assert_eq!(report.post_ttl_retained_bytes, 0);
        }
    }

    // LEDGER-2.5-5: Benchmark E2E 20 repeticiones de Avatar en Producción + Hot-Reload (< 50 ms, 0 fps en reposo y rollback seguro)
    #[test]
    fn e2e_miku_ui_avatar_pack_hot_reload_20_reps_sub50ms_zero_fps_idle_and_ram_budget() {
        let avatar_dir = resolve_avatar_assets_dir();

        for _ in 0..BENCHMARK_REPETITIONS {
            let report =
                run_avatar_pack_hot_reload_audit(&avatar_dir, HOT_RELOAD_CYCLES_PER_WORKER)
                    .expect("Avatar Pack hot-reload audit must succeed");

            assert_eq!(report.spring_count, 6);
            assert_eq!(report.moods_converged, 8);
            assert_eq!(report.verified_svg_count, EXPECTED_AVATAR_SVG_COUNT);
            assert_eq!(
                report.reload_commits,
                HOT_RELOAD_CYCLES_PER_WORKER as u64
            );
            assert_eq!(report.reload_rollbacks, 2);
            assert_eq!(report.settled_fps, 0.0);
            assert!(report.max_reload_latency_ms < 250.0);
        }
    }

    // LEDGER-2.6-5: Benchmark E2E 20 repeticiones de Gatekeeper UI/Core (< 1.0 ms intercepción, Anti-TOCTOU, TTL y cadena SHA-256)
    #[test]
    fn e2e_miku_ui_gatekeeper_interceptor_20_reps_sub1ms_latency_anti_toctou_and_ram_budget() {
        for _ in 0..BENCHMARK_REPETITIONS {
            let report =
                run_ui_gatekeeper_interceptor_audit(GATEKEEPER_AUDIT_CYCLES_PER_WORKER)
                    .expect("UI Gatekeeper interceptor audit must succeed");

            assert_eq!(
                report.allowed_count,
                (GATEKEEPER_AUDIT_CYCLES_PER_WORKER * 2) as u64
            );
            assert_eq!(
                report.denied_default_count,
                GATEKEEPER_AUDIT_CYCLES_PER_WORKER as u64
            );
            assert_eq!(
                report.denied_rule_count,
                GATEKEEPER_AUDIT_CYCLES_PER_WORKER as u64
            );
            assert_eq!(
                report.denied_jail_count,
                GATEKEEPER_AUDIT_CYCLES_PER_WORKER as u64
            );
            assert_eq!(
                report.denied_toctou_count,
                GATEKEEPER_AUDIT_CYCLES_PER_WORKER as u64
            );
            assert_eq!(
                report.denied_expired_count,
                GATEKEEPER_AUDIT_CYCLES_PER_WORKER as u64
            );
            assert_eq!(
                report.denied_replay_count,
                GATEKEEPER_AUDIT_CYCLES_PER_WORKER as u64
            );
            assert_eq!(
                report.audit_chain_entries,
                GATEKEEPER_AUDIT_CYCLES_PER_WORKER * 11
            );
            assert!(report.audit_chain_verified);
            assert!(report.tamper_simulation_detected);
            assert!(report.max_intercept_latency_ms < 25.0);
        }
    }

    // LEDGER-2.7-5: Benchmark E2E 20 repeticiones de Auditoría JSONL Persistente, Rotación Transaccional y Visor Seguro
    #[test]
    fn e2e_miku_ui_persistent_jsonl_audit_rotation_and_viewer_20_reps_benchmark() {
        for _ in 0..BENCHMARK_REPETITIONS {
            let report = run_persistent_jsonl_audit_store_benchmark(
                PERSISTENT_AUDIT_CYCLES_PER_WORKER,
            )
            .expect("Persistent JSONL audit store benchmark must succeed");

            assert_eq!(
                report.appended_entries,
                PERSISTENT_AUDIT_CYCLES_PER_WORKER * 10
            );
            assert_eq!(
                report.verified_entries,
                PERSISTENT_AUDIT_CYCLES_PER_WORKER * 10
            );
            assert!(report.rotations_completed >= 2);
            assert!(report.segment_files_verified >= 3);
            assert_eq!(
                report.viewer_rows_rendered,
                PERSISTENT_AUDIT_CYCLES_PER_WORKER * 10
            );
            assert!(report.viewer_xss_neutralized);
            assert!(report.tamper_detected_at_exact_line);
            assert!(report.verify_latency_ms < 250.0);
        }
    }

    // LEDGER-2.8-5: Benchmark E2E 20 repeticiones de Bucle de Voz PTT / Wake-Word, Onda Reactiva y Cancelación Cooperativa
    #[test]
    fn e2e_miku_ui_voice_interaction_ptt_wakeword_and_cancellation_20_reps_benchmark() {
        let avatar_dir = resolve_avatar_assets_dir();

        for _ in 0..BENCHMARK_REPETITIONS {
            let report =
                run_voice_interaction_loop_audit(&avatar_dir, VOICE_AUDIT_CYCLES_PER_WORKER)
                    .expect("Voice interaction loop benchmark must succeed");

            assert_eq!(
                report.ptt_sessions_completed,
                VOICE_AUDIT_CYCLES_PER_WORKER as u64
            );
            assert_eq!(
                report.wake_word_sessions_completed,
                VOICE_AUDIT_CYCLES_PER_WORKER as u64
            );
            assert_eq!(
                report.wake_word_power_blocks,
                VOICE_AUDIT_CYCLES_PER_WORKER as u64
            );
            assert_eq!(
                report.cooperative_cancellations,
                (VOICE_AUDIT_CYCLES_PER_WORKER * 3) as u64
            );
            assert_eq!(
                report.pcm_zero_fills_verified,
                (VOICE_AUDIT_CYCLES_PER_WORKER * 5) as u64
            );
            assert_eq!(report.residual_pcm_bytes, 0);
            assert_eq!(report.idle_wave_fps, 0.0);
            assert!(report.focus_preserved_and_xss_safe);
            assert!(report.max_soundwave_latency_ms < 50.0);
            assert!(report.max_cancel_latency_ms < 25.0);
        }
    }

    // LEDGER-2.9-5: Benchmark E2E 20 repeticiones de Kill Switch Global Win32 (< 20.0 ms reacción, revocación y rearme constante)
    #[test]
    fn e2e_miku_ui_kill_switch_reaction_latency_and_revocation_20_reps_benchmark() {
        let avatar_dir = resolve_avatar_assets_dir();

        for _ in 0..BENCHMARK_REPETITIONS {
            let report =
                run_kill_switch_audit(&avatar_dir, KILL_SWITCH_AUDIT_CYCLES_PER_WORKER)
                    .expect("Global Kill Switch benchmark must succeed");

            assert_eq!(
                report.trips_executed,
                KILL_SWITCH_AUDIT_CYCLES_PER_WORKER as u64
            );
            assert_eq!(
                report.total_revoked_tickets,
                (KILL_SWITCH_AUDIT_CYCLES_PER_WORKER * 3) as u64
            );
            assert_eq!(
                report.voice_sessions_aborted,
                KILL_SWITCH_AUDIT_CYCLES_PER_WORKER as u64
            );
            assert_eq!(
                report.pcm_zero_fills_verified,
                KILL_SWITCH_AUDIT_CYCLES_PER_WORKER as u64
            );
            assert_eq!(
                report.rearms_completed,
                KILL_SWITCH_AUDIT_CYCLES_PER_WORKER as u64
            );
            assert_eq!(
                report.rejected_rearm_attempts,
                KILL_SWITCH_AUDIT_CYCLES_PER_WORKER as u64
            );
            assert_eq!(
                report.blocked_actions_while_tripped,
                KILL_SWITCH_AUDIT_CYCLES_PER_WORKER as u64
            );
            assert!(report.audit_chain_verified);
            assert!(report.hotkey_fallback_verified);
            assert!(report.xss_and_focus_verified);
            assert!(report.max_trip_latency_ms < 100.0);
        }
    }

    // LEDGER-2.10-5: Benchmark E2E 20 repeticiones de Suspensión Total en Idle + Certificación Banco 0.5
    #[test]
    fn e2e_miku_ui_idle_suspension_and_bank_05_budget_certification_20_reps_benchmark() {
        let avatar_dir = resolve_avatar_assets_dir();
        let local_app_data = env::var("LOCALAPPDATA")
            .unwrap_or_else(|_| r"C:\Users\Default\AppData\Local".to_string());
        let primary = default_primary_monitor_from_anchor(836, 0, STANDARD_DPI);

        for _ in 0..BENCHMARK_REPETITIONS {
            let report = run_idle_suspension_bank05_audit(
                primary.clone(),
                Some(Path::new(&local_app_data)),
                &avatar_dir,
                IDLE_SUSPENSION_CYCLES_PER_WORKER,
            )
            .expect("Idle suspension and Bank 0.5 budget certification benchmark must succeed");

            assert_eq!(report.cycles_completed, IDLE_SUSPENSION_CYCLES_PER_WORKER);
            assert_eq!(
                report.deep_suspensions_completed,
                IDLE_SUSPENSION_CYCLES_PER_WORKER as u64
            );
            assert_eq!(
                report.warm_wakes_completed,
                IDLE_SUSPENSION_CYCLES_PER_WORKER as u64
            );
            assert_eq!(report.gaming_events_deferred, 4);
            assert_eq!(report.gaming_critical_alerts_delivered, 1);
            assert_eq!(report.residual_webview2_pids, 0);
            assert_eq!(report.residual_idle_timers, 0);
            assert_eq!(report.residual_ipc_bytes, 0);
            assert_eq!(report.residual_pcm_bytes, 0);
            assert_eq!(report.idle_settled_fps, 0.0);
            assert_eq!(report.idle_cpu_pct, 0.0);
            assert!(report.gaming_cpu_pct < MAX_GAMING_CPU_BUDGET_PCT);
            assert!(report.max_post_ttl_ram_mb <= MAX_IDLE_POST_TTL_RAM_MB);
            assert!(report.max_expanded_ram_mb <= MAX_EXPANDED_WEBVIEW2_RAM_MB);
            assert!(report.focus_preserved_and_zero_drift);
            assert!(report.certification_matrix.all_budgets_certified);
            assert!(report.max_wake_latency_ms < 50.0);
        }
    }

    // LEDGER-2.11-5: Benchmark E2E 20 repeticiones de Modo Demo Interactivo Standalone (F-28)
    #[test]
    fn e2e_miku_demo_mode_sandbox_zero_disk_touch_and_memory_stability_20_reps_benchmark() {
        let exe_path = env::current_exe().expect("resolve current_exe");
        let bin_size_mb = fs::metadata(&exe_path)
            .map(|m| (m.len() as f64) / (1024.0 * 1024.0))
            .unwrap_or(0.0);
        let max_bin_limit = if cfg!(debug_assertions) {
            150.0
        } else {
            MAX_BINARY_SIZE_MB
        };
        assert!(bin_size_mb < max_bin_limit);

        let demo_opts = DemoCliOptions {
            enabled: true,
            duration_ms: Some(120),
            auto_script: true,
            json_output: false,
        };

        let mut startup_latencies_ms = Vec::with_capacity(BENCHMARK_REPETITIONS);
        let mut ram_samples_mb = Vec::with_capacity(BENCHMARK_REPETITIONS);

        for rep in 0..BENCHMARK_REPETITIONS {
            let controller = DemoInteractiveController::default_with_resolved_assets()
                .expect("resolve demo assets");
            let report = controller
                .run_live_win32_demo_window(&demo_opts)
                .unwrap_or_else(|e| panic!("demo repetition {} failed: {}", rep + 1, e));

            assert_eq!(report.badge_prefix, DEMO_BADGE_PREFIX);
            assert_eq!(report.pill_width, IDLE_COMPACT_WIDTH_PX);
            assert_eq!(report.pill_height, IDLE_COMPACT_HEIGHT_PX);
            assert_eq!(report.disk_files_mutated, 0);
            assert_eq!(report.api_keys_touched, 0);
            assert!(report.hover_transitions >= 1);
            assert!(report.click_modal_shown_count >= 1);
            assert!(report.ptt_activations >= 1);
            assert!(report.kill_switch_trips >= 1);
            assert!(report.escaped_cleanly);
            assert!(report.focus_preserved);
            assert!(report.hotkeys_unregistered);
            assert!(report.window_destroyed);
            assert!(report.pcm_buffer_zeroed);
            assert!(report.ipc_queue_drained);
            assert_eq!(report.settled_idle_fps, 0.0);
            assert!(report.post_teardown_ram_mb <= MAX_IDLE_POST_TTL_RAM_MB);

            let startup_limit = if cfg!(debug_assertions) {
                250.0
            } else {
                DEMO_MAX_STARTUP_LATENCY_MS
            };
            assert!(
                report.startup_latency_ms < startup_limit,
                "startup_latency_ms {:.4} exceeded {:.1}",
                report.startup_latency_ms,
                startup_limit
            );

            startup_latencies_ms.push(report.startup_latency_ms);
            ram_samples_mb.push(report.post_teardown_ram_mb);
        }

        let startup_p50 = compute_percentile(&startup_latencies_ms, 50.0);
        let startup_p95 = compute_percentile(&startup_latencies_ms, 95.0);
        let ram_p95 = compute_percentile(&ram_samples_mb, 95.0);

        println!(
            "=== MIKU-UI · PASO 2.11 · MODO DEMO INTERACTIVO STANDALONE ({} REPS) ===",
            BENCHMARK_REPETITIONS
        );
        println!(
            "Demo Startup Latency  : p50 = {:.4} ms | p95 = {:.4} ms (threshold < {:.1} ms)",
            startup_p50, startup_p95, DEMO_MAX_STARTUP_LATENCY_MS
        );
        println!(
            "Sandbox Isolation     : disk_files_mutated = 0 | api_keys_touched = 0 | badge = {}",
            DEMO_BADGE_PREFIX
        );
        println!(
            "Post-Teardown RAM     : p95 = {:.2} MB (BUD-01 <= {:.1} MB)",
            ram_p95, MAX_IDLE_POST_TTL_RAM_MB
        );
        println!(
            "VERIFIER: OK (reps={}, startup_p95={:.4} ms, disk_mutated=0, api_keys=0, post_ram_p95={:.2} MB, orphans=0)",
            BENCHMARK_REPETITIONS, startup_p95, ram_p95
        );
    }
}


