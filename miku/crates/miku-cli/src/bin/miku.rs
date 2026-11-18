use std::env;
use std::fs;

use miku_cli::{
    execute_cli_demo_mode, execute_cli_hook_command, execute_cli_installer_command,
    execute_cli_secret_command, format_demo_session_report, maybe_run_ci_bench_and_exit,
    parse_ui_cli_args, query_live_windows_webview2_version, run_doctor_suite,
    run_live_island_ui_mode, run_resident_desktop_daemon, run_unified_smoke_summary,
    DoctorRunConfig, LiveGatewayConfig, LiveGatewayRunner, WebView2ProbeSource,
};

fn print_usage() {
    println!(
        "\
Miku Desktop Orchestrator\n\
  miku                 Modo residente de escritorio (Tray + Hotkeys + WebView2 + Scaffold UI)\n\
  miku serve           Daemon completo de escritorio (usa --headless para solo servidor HTTP/WS)\n\
  miku daemon | tray   Alias explicito del daemon de escritorio\n\
  miku ui              Ventana Dynamic Island conectada a 127.0.0.1:47400\n\
  miku demo            Modo demostracion en sandbox (sin tocar disco ni claves)\n\
  miku doctor [--json] Diagnostico completo de entorno y presupuestos\n\
  miku secret ...      Gestion de claves en Windows Credential Manager (DPAPI)\n\
  miku install ...     Registro e inicio automatico en Windows\n\
  miku --smoke [--json]Prueba rapida de arranque y cierre limpio"
    );
}

fn main() {
    let args: Vec<String> = env::args().collect();
    let cmd1 = args.get(1).map(|s| s.as_str());

    if matches!(cmd1, Some("--help") | Some("-h") | Some("help")) {
        print_usage();
        std::process::exit(0);
    }

    if cmd1 == Some("serve") && args.iter().any(|a| a == "--headless") {
        let ui_cfg = parse_ui_cli_args(&args);
        let cfg = LiveGatewayConfig {
            data_dir: ui_cfg.data_dir,
            port: ui_cfg.port,
            default_ui_dir: ui_cfg.default_ui_dir,
            user_override_dir: ui_cfg.user_override_dir,
            use_memory_secrets: ui_cfg.use_memory_secrets,
        };
        let data_disp = cfg.data_dir.display().to_string();
        match LiveGatewayRunner::start_with_lan(cfg, true) {
            Ok(runner) => {
                println!(
                    "MIKU_GATEWAY_READY|addr={}|url=http://{}/|data={}",
                    runner.local_addr, runner.local_addr, data_disp
                );
                if let Err(e) = runner.run_until_ctrl_c() {
                    eprintln!("MIKU_SERVE_ERR: {}", e);
                    std::process::exit(1);
                }
                std::process::exit(0);
            }
            Err(e) => {
                eprintln!("MIKU_SERVE_ERR: {}", e);
                std::process::exit(1);
            }
        }
    }

    if args.len() == 1 || matches!(cmd1, Some("serve") | Some("daemon") | Some("tray")) {
        let cfg = parse_ui_cli_args(&args);
        if let Err(e) = run_resident_desktop_daemon(cfg) {
            eprintln!("MIKU_DAEMON_ERR: {}", e);
            std::process::exit(1);
        }
        std::process::exit(0);
    }

    if cmd1 == Some("ui") {
        let cfg = parse_ui_cli_args(&args);
        if let Err(e) = run_live_island_ui_mode(cfg) {
            eprintln!("MIKU_UI_ERR: {}", e);
            std::process::exit(1);
        }
        std::process::exit(0);
    }

    match execute_cli_demo_mode(&args) {
        Ok(Some((opts, rep))) => {
            if opts.json_output {
                println!(
                    "{}",
                    serde_json::to_string_pretty(&rep).unwrap_or_else(|_| "{}".to_string())
                );
            } else {
                print!("{}", format_demo_session_report(&rep));
            }
            std::process::exit(0);
        }
        Ok(None) => {}
        Err(e) => {
            eprintln!("MIKU_DEMO_ERR: {}", e);
            std::process::exit(1);
        }
    }

    match execute_cli_secret_command(&args) {
        Ok(Some(out)) => {
            println!("{}", out);
            std::process::exit(0);
        }
        Ok(None) => {}
        Err(e) => {
            eprintln!("MIKU_SECRET_ERR: {}", e);
            std::process::exit(1);
        }
    }

    match execute_cli_hook_command(&args) {
        Ok(Some(out)) => {
            println!("{}", out);
            std::process::exit(0);
        }
        Ok(None) => {}
        Err(e) => {
            eprintln!("MIKU_HOOK_ERR: {}", e);
            std::process::exit(1);
        }
    }

    match execute_cli_installer_command(&args) {
        Ok(Some(out)) => {
            println!("{}", out);
            std::process::exit(0);
        }
        Ok(None) => {}
        Err(e) => {
            eprintln!("MIKU_INSTALL_ERR: {}", e);
            std::process::exit(1);
        }
    }

    if args.iter().any(|a| a == "doctor") {
        let json_mode = args.iter().any(|a| a == "--json");
        let live_wv2 = query_live_windows_webview2_version();
        let cfg = DoctorRunConfig {
            webview2_source: WebView2ProbeSource::SimulatedVersion(
                live_wv2.or_else(|| Some("131.0.2903.86".to_string())),
            ),
            ..DoctorRunConfig::default()
        };
        let report = run_doctor_suite(&cfg);
        if json_mode {
            println!(
                "{}",
                report
                    .to_json_pretty()
                    .unwrap_or_else(|_| "{}".to_string())
            );
        } else {
            print!("{}", report.render_ascii_table());
        }
        std::process::exit(report.exit_code);
    }

    maybe_run_ci_bench_and_exit(&args);

    let json_mode = args.iter().any(|a| a == "--json");
    let data_dir = env::temp_dir().join(format!("miku_unified_exe_{}", std::process::id()));
    match run_unified_smoke_summary(&data_dir) {
        Ok((boot, shut)) => {
            let _ = fs::remove_dir_all(&data_dir);
            if json_mode {
                println!(
                    "{}",
                    serde_json::to_string_pretty(&serde_json::json!({
                        "boot": boot,
                        "shutdown": shut,
                    }))
                    .unwrap_or_else(|_| "{}".to_string())
                );
            } else {
                println!(
                    "MIKU_UNIFIED_OK|crates={}|cold_ms={:.2}|pill_ms={:.2}|wal={}|ram_mb={:.2}|orphans={}",
                    boot.connected_subsystems.len(),
                    boot.cold_start_ms,
                    boot.pill_ready_ms,
                    boot.sqlite_journal_mode,
                    boot.process_private_mb,
                    shut.live_webview2_child_pids
                );
            }
        }
        Err(e) => {
            let _ = fs::remove_dir_all(&data_dir);
            eprintln!("MIKU_UNIFIED_ERR: {}", e);
            std::process::exit(1);
        }
    }
}
