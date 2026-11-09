#[derive(Debug, Clone)]
pub(crate) struct WorkerUiMetrics {
    pub(crate) monitors_detected: usize,
    pub(crate) primary_anchor_x: i32,
    pub(crate) primary_anchor_y: i32,
    pub(crate) primary_width: u32,
    pub(crate) primary_height: u32,
    pub(crate) primary_dpi: u32,
    pub(crate) ex_style_verified: bool,
    pub(crate) ma_noactivate_verified: bool,
    pub(crate) focus_preserved: bool,
    pub(crate) hover_trigger_latency_ms: f64,
    pub(crate) webview2_processes_spawned: usize,
    pub(crate) active_idle_timers: usize,
    pub(crate) verified_svg_assets: usize,
    pub(crate) isolated_data_dir_ok: bool,
    pub(crate) mode_c_cycles: usize,
    pub(crate) warm_reuses: usize,
    pub(crate) ttl_evictions: usize,
    pub(crate) blocked_external_navs: usize,
    pub(crate) max_expand_latency_ms: f64,
    pub(crate) ipc_roundtrips: usize,
    pub(crate) ipc_max_rtt_ms: f64,
    pub(crate) ipc_unauthorized_blocked: usize,
    pub(crate) ipc_malformed_discarded: usize,
    pub(crate) ipc_csp_xss_verified: bool,
    pub(crate) ipc_retained_bytes: usize,
    pub(crate) overview_bundle_bytes: usize,
    pub(crate) overview_layer_count: usize,
    pub(crate) overview_accessory_slots: usize,
    pub(crate) overview_events_processed: usize,
    pub(crate) overview_ring_len: usize,
    pub(crate) overview_settled_fps: f64,
    pub(crate) overview_expand_latency_ms: f64,
    pub(crate) overview_text_safe: bool,
    pub(crate) expanded_ram_mb: f64,
    pub(crate) pack_spring_count: usize,
    pub(crate) pack_moods_verified: usize,
    pub(crate) pack_hot_reloads: usize,
    pub(crate) pack_rollbacks: usize,
    pub(crate) pack_max_reload_ms: f64,
    pub(crate) pack_settled_fps: f64,
    pub(crate) gk_allowed: usize,
    pub(crate) gk_blocks: usize,
    pub(crate) gk_toctou_blocks: usize,
    pub(crate) gk_replay_blocks: usize,
    pub(crate) gk_expired_blocks: usize,
    pub(crate) gk_audit_entries: usize,
    pub(crate) gk_max_intercept_ms: f64,
    pub(crate) gk_audit_verified: bool,
    pub(crate) audit_jsonl_entries: usize,
    pub(crate) audit_rotations: usize,
    pub(crate) audit_segments: usize,
    pub(crate) audit_viewer_rows: usize,
    pub(crate) audit_append_ms: f64,
    pub(crate) audit_verify_ms: f64,
    pub(crate) audit_jsonl_ok: bool,
    pub(crate) voice_ptt_sessions: usize,
    pub(crate) voice_wake_sessions: usize,
    pub(crate) voice_power_blocks: usize,
    pub(crate) voice_cancellations: usize,
    pub(crate) voice_zero_fills: usize,
    pub(crate) voice_residual_bytes: usize,
    pub(crate) voice_wave_ms: f64,
    pub(crate) voice_cancel_ms: f64,
    pub(crate) voice_idle_fps: f64,
    pub(crate) voice_ok: bool,
    pub(crate) ks_trips: usize,
    pub(crate) ks_revoked: usize,
    pub(crate) ks_voice_aborts: usize,
    pub(crate) ks_rearms: usize,
    pub(crate) ks_rejected_rearms: usize,
    pub(crate) ks_blocked: usize,
    pub(crate) ks_trip_ms: f64,
    pub(crate) ks_ok: bool,
    pub(crate) idle_suspensions: usize,
    pub(crate) idle_wakes: usize,
    pub(crate) idle_deferred_gaming: usize,
    pub(crate) idle_crit_alerts: usize,
    pub(crate) idle_ww_frozen: usize,
    pub(crate) idle_wake_ms: f64,
    pub(crate) idle_gaming_cpu_pct: f64,
    pub(crate) idle_bank05_ok: bool,
    pub(crate) post_ttl_ram_mb: f64,
}

pub(crate) fn parse_worker_line(stdout: &str) -> Result<WorkerUiMetrics, String> {
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
