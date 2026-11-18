use std::env;
use std::fs;
use std::io::{self, IsTerminal};
use std::path::{Path, PathBuf};
use std::process::Command;

use serde::Serialize;

use super::operations::{
    apply_verified_update, install_local_binary, prompt_for_update_consent, uninstall_local_binary,
};
use super::{
    HttpsPowerShellFetcher, InstallLayout, LocalFileFetcher, ReleaseManifestV1,
};

const HKCU_RUN_SUBKEY: &str = r"HKCU\Software\Microsoft\Windows\CurrentVersion\Run";
const HKCU_RUN_VALUE: &str = "Miku";

/// Parses and executes `install`, `uninstall`, and `update` CLI subcommands.
pub fn execute_cli_installer_command(args: &[String]) -> Result<Option<String>, String> {
    let cmd = args.get(1).map(|s| s.as_str()).unwrap_or("");
    let json_mode = args.iter().any(|a| a == "--json");
    let dry_run = args.iter().any(|a| a == "--dry-run");

    match cmd {
        "install" => {
            let source = flag_value(args, "--source")
                .map(PathBuf::from)
                .or_else(|| env::current_exe().ok())
                .ok_or_else(|| "cannot resolve source executable path".to_string())?;
            let layout = resolve_cli_layout(args)?;
            let autostart = args.iter().any(|a| a == "--autostart");
            let report = install_local_binary(&source, &layout, autostart, dry_run)?;
            format_report(&report, json_mode, || {
                format!(
                    "MIKU_INSTALL_OK|exe={}|sha256={}|size={}|autostart={}|dry_run={}",
                    report.installed_exe.display(),
                    &report.sha256_hex[..12],
                    report.size_bytes,
                    report.autostart_registered,
                    report.dry_run
                )
            })
            .map(Some)
        }
        "uninstall" => {
            let layout = resolve_cli_layout(args)?;
            let purge_data = if args.iter().any(|a| a == "--purge-data") {
                Some(true)
            } else if args.iter().any(|a| a == "--keep-data") {
                Some(false)
            } else {
                None
            };
            let is_tty = io::stdin().is_terminal();
            let stdin = io::stdin();
            let mut stdout = io::stdout();
            let report = uninstall_local_binary(
                &layout,
                purge_data,
                dry_run,
                !args.iter().any(|a| a == "--dest-dir"),
                is_tty,
                stdin.lock(),
                &mut stdout,
            )?;
            format_report(&report, json_mode, || {
                format!(
                    "MIKU_UNINSTALL_OK|exe={}|staging={}|data_purged={}|dry_run={}",
                    report.removed_exe,
                    report.removed_staging,
                    report.data_purged,
                    report.dry_run
                )
            })
            .map(Some)
        }
        "update" => {
            let manifest_path = flag_value(args, "--manifest")
                .ok_or_else(|| "missing required flag --manifest <path>".to_string())?;
            let raw_manifest = fs::read_to_string(&manifest_path)
                .map_err(|e| format!("failed reading manifest '{}': {e}", manifest_path))?;
            let manifest: ReleaseManifestV1 = serde_json::from_str(&raw_manifest)
                .map_err(|e| format!("invalid manifest JSON: {e}"))?;
            manifest.validate().map_err(|e| e.to_string())?;

            let layout = resolve_cli_layout(args)?;
            let consent = if args.iter().any(|a| a == "--yes-consent") {
                true
            } else {
                let is_tty = io::stdin().is_terminal();
                let stdin = io::stdin();
                let mut stdout = io::stdout();
                prompt_for_update_consent(&manifest, is_tty, stdin.lock(), &mut stdout)?
            };

            let payload_override = flag_value(args, "--payload").map(PathBuf::from);
            let report = if payload_override.is_some()
                || manifest.download_url.to_ascii_lowercase().starts_with("local://")
            {
                let fetcher = LocalFileFetcher {
                    override_path: payload_override,
                };
                apply_verified_update(&manifest, &layout, &fetcher, consent)
            } else {
                apply_verified_update(&manifest, &layout, &HttpsPowerShellFetcher, consent)
            }
            .map_err(|e| e.to_string())?;

            format_report(&report, json_mode, || {
                format!(
                    "MIKU_UPDATE_OK|version={}|exe={}|sha256={}|backup={}",
                    report.target_version,
                    report.target_exe.display(),
                    &report.sha256_verified[..12],
                    report.previous_backup_created
                )
            })
            .map(Some)
        }
        _ => Ok(None),
    }
}

pub(crate) fn set_hkcu_run_entry(target_exe: &Path) -> Result<(), String> {
    let quoted = format!("\"{}\"", target_exe.display());
    let output = Command::new("reg.exe")
        .args([
            "add",
            HKCU_RUN_SUBKEY,
            "/v",
            HKCU_RUN_VALUE,
            "/t",
            "REG_SZ",
            "/d",
            &quoted,
            "/f",
        ])
        .output()
        .map_err(|e| format!("failed executing reg.exe: {e}"))?;
    if !output.status.success() {
        return Err(format!(
            "reg.exe add failed: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        ));
    }
    Ok(())
}

pub(crate) fn delete_hkcu_run_entry() -> bool {
    Command::new("reg.exe")
        .args(["delete", HKCU_RUN_SUBKEY, "/v", HKCU_RUN_VALUE, "/f"])
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false)
}

fn resolve_cli_layout(args: &[String]) -> Result<InstallLayout, String> {
    match (flag_value(args, "--dest-dir"), flag_value(args, "--data-dir")) {
        (Some(dest), Some(data)) => Ok(InstallLayout::from_roots(Path::new(&dest), Path::new(&data))),
        (Some(dest), None) => Ok(InstallLayout::from_roots(
            Path::new(&dest),
            &Path::new(&dest).join("data"),
        )),
        (None, Some(data)) => {
            let mut layout = InstallLayout::from_windows_localappdata()?;
            layout.data_dir = PathBuf::from(&data);
            layout.staging_dir = layout.data_dir.join("staging");
            Ok(layout)
        }
        (None, None) => InstallLayout::from_windows_localappdata(),
    }
}

fn flag_value(args: &[String], flag: &str) -> Option<String> {
    for i in 0..args.len() {
        if args[i] == flag && i + 1 < args.len() {
            return Some(args[i + 1].clone());
        }
    }
    None
}

fn format_report<T: Serialize, F: FnOnce() -> String>(
    report: &T,
    json_mode: bool,
    line_fn: F,
) -> Result<String, String> {
    if json_mode {
        serde_json::to_string_pretty(report)
            .map_err(|e| format!("failed serializing JSON report: {e}"))
    } else {
        Ok(line_fn())
    }
}
