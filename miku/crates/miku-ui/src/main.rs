#![allow(
    clippy::collapsible_if,
    clippy::double_must_use,
    clippy::manual_is_multiple_of,
    clippy::manual_range_contains,
    clippy::needless_range_loop,
    clippy::too_many_arguments
)]

mod bin_coordinator_report;
mod bin_coordinator_runner;
mod bin_process_tree;
mod bin_worker_probe;
mod bin_worker_telemetry;

#[cfg(test)]
#[path = "bin_e2e_tests.rs"]
mod tests;

use std::env;

use miku_ui::{parse_demo_cli_args, DemoInteractiveController};

use crate::bin_coordinator_runner::run_coordinator_benchmark;
use crate::bin_worker_probe::{run_single_worker_iteration, BENCHMARK_REPETITIONS};

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
