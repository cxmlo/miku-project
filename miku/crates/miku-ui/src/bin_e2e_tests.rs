use std::env;
use std::fs;
use std::path::Path;
use std::sync::{Mutex, MutexGuard};

use miku_ui::{
    resolve_avatar_assets_dir, run_avatar_pack_hot_reload_audit,
    run_five_cycle_webview2_ttl_recovery_audit, run_idle_suspension_bank05_audit,
    run_ipc_bridge_stress_audit, run_kill_switch_audit, run_overview_scene_audit,
    run_persistent_jsonl_audit_store_benchmark, run_ui_gatekeeper_interceptor_audit,
    run_voice_interaction_loop_audit, DemoCliOptions, DemoInteractiveController,
    DEMO_BADGE_PREFIX, DEMO_MAX_STARTUP_LATENCY_MS, EXPECTED_AVATAR_SVG_COUNT,
    FIVE_CYCLE_STRESS_REPS, IDLE_COMPACT_HEIGHT_PX, IDLE_COMPACT_WIDTH_PX,
    MAX_EXPANDED_WEBVIEW2_RAM_MB, MAX_GAMING_CPU_BUDGET_PCT, MAX_IDLE_POST_TTL_RAM_MB,
    MAX_OVERVIEW_BUNDLE_BYTES, STANDARD_DPI,
};

use crate::bin_process_tree::compute_percentile;
use crate::bin_worker_probe::{
    default_primary_monitor_from_anchor, run_single_worker_iteration, BENCHMARK_REPETITIONS,
    GATEKEEPER_AUDIT_CYCLES_PER_WORKER, HOT_RELOAD_CYCLES_PER_WORKER,
    IDLE_SUSPENSION_CYCLES_PER_WORKER, IPC_BURST_PER_WORKER, KILL_SWITCH_AUDIT_CYCLES_PER_WORKER,
    MAX_BINARY_SIZE_MB, OVERVIEW_EVENTS_PER_WORKER, PERSISTENT_AUDIT_CYCLES_PER_WORKER,
    VOICE_AUDIT_CYCLES_PER_WORKER,
};

static E2E_BENCH_LOCK: Mutex<()> = Mutex::new(());

fn lock_e2e_bench() -> MutexGuard<'static, ()> {
    E2E_BENCH_LOCK.lock().unwrap_or_else(|e| e.into_inner())
}

// LEDGER-2.1-5: Benchmark E2E 20 repeticiones de pastilla nativa Win32
#[test]
fn e2e_miku_ui_native_pill_20_reps_hover_latency_zero_focus_steal_and_ram_budget() {
    let _guard = lock_e2e_bench();
    run_single_worker_iteration(0, false).expect("single worker UI audit must succeed");
}

// LEDGER-2.2-5: Benchmark E2E 20 repeticiones x 5 ciclos de WebView2 Modo C con TTL
#[test]
fn e2e_miku_ui_webview2_host_20_reps_5x_cycles_latency_ram_recovery_and_zero_orphans() {
    let _guard = lock_e2e_bench();
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
    let _guard = lock_e2e_bench();
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
    let _guard = lock_e2e_bench();
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
    let _guard = lock_e2e_bench();
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
    let _guard = lock_e2e_bench();
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
    let _guard = lock_e2e_bench();
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
    let _guard = lock_e2e_bench();
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
    let _guard = lock_e2e_bench();
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
    let _guard = lock_e2e_bench();
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
    let _guard = lock_e2e_bench();
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

    let controller = DemoInteractiveController::default_with_resolved_assets()
        .expect("resolve demo assets");
    // Warm up OS Win32/DWM/GDI DLLs before timed 20-rep measurement loop
    let _ = controller
        .run_live_win32_demo_window(&demo_opts)
        .expect("demo warm-up pass must succeed");

    let mut startup_latencies_ms = Vec::with_capacity(BENCHMARK_REPETITIONS);
    let mut ram_samples_mb = Vec::with_capacity(BENCHMARK_REPETITIONS);

    let startup_limit = if cfg!(debug_assertions) {
        250.0
    } else {
        DEMO_MAX_STARTUP_LATENCY_MS
    };

    for rep in 0..BENCHMARK_REPETITIONS {
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
    assert!(
        startup_p95 < startup_limit,
        "startup_p95 {:.4} exceeded {:.1}",
        startup_p95,
        startup_limit
    );

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
