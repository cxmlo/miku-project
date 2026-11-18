use std::fs;
use std::io::{BufRead, Write};
use std::path::Path;

use miku_protocol::{constant_time_token_eq, sha256_hex};

use super::cli_and_registry::{delete_hkcu_run_entry, set_hkcu_run_entry};
use super::{
    InstallLayout, InstallReport, PayloadFetcher, ReleaseManifestV1, UninstallReport, UpdateError,
    UpdateReport, MAX_INSTALLABLE_BINARY_BYTES,
};

/// Installs `source_exe` into `layout.target_exe`, checking the `< 15 MB` budget and SHA-256 equality.
pub fn install_local_binary(
    source_exe: &Path,
    layout: &InstallLayout,
    autostart: bool,
    dry_run: bool,
) -> Result<InstallReport, String> {
    let bytes = fs::read(source_exe)
        .map_err(|e| format!("failed reading source binary '{}': {e}", source_exe.display()))?;
    let size_bytes = bytes.len() as u64;
    if size_bytes == 0 || size_bytes > MAX_INSTALLABLE_BINARY_BYTES {
        return Err(format!(
            "binary '{}' size {} B violates budget (1..={} B)",
            source_exe.display(),
            size_bytes,
            MAX_INSTALLABLE_BINARY_BYTES
        ));
    }
    let expected_sha = sha256_hex(&bytes);

    if dry_run {
        return Ok(InstallReport {
            installed_exe: layout.target_exe.clone(),
            sha256_hex: expected_sha,
            size_bytes,
            autostart_registered: autostart,
            dry_run: true,
        });
    }

    fs::create_dir_all(&layout.install_dir)
        .map_err(|e| format!("failed creating install dir: {e}"))?;
    fs::create_dir_all(&layout.data_dir)
        .map_err(|e| format!("failed creating data dir: {e}"))?;

    if source_exe != layout.target_exe {
        let staging_copy = layout.install_dir.join("miku.exe.install.tmp");
        fs::write(&staging_copy, &bytes)
            .map_err(|e| format!("failed writing staging install binary: {e}"))?;
        if layout.target_exe.exists() {
            let _ = fs::copy(&layout.target_exe, &layout.previous_backup_exe);
        }
        if let Err(e) = fs::rename(&staging_copy, &layout.target_exe) {
            let _ = fs::remove_file(&staging_copy);
            return Err(format!("failed finalizing installed binary: {e}"));
        }
    }

    let installed_bytes = fs::read(&layout.target_exe)
        .map_err(|e| format!("failed verifying installed binary: {e}"))?;
    let installed_sha = sha256_hex(&installed_bytes);
    if !constant_time_token_eq(&installed_sha, &expected_sha) {
        return Err("post-install SHA-256 verification failed".to_string());
    }

    let autostart_registered = if autostart {
        set_hkcu_run_entry(&layout.target_exe)?;
        true
    } else {
        false
    };

    Ok(InstallReport {
        installed_exe: layout.target_exe.clone(),
        sha256_hex: installed_sha,
        size_bytes,
        autostart_registered,
        dry_run: false,
    })
}

/// Uninstalls Miku binaries cleanly and removes `%LOCALAPPDATA%\Miku` only when confirmed.
pub fn uninstall_local_binary<R: BufRead, W: Write>(
    layout: &InstallLayout,
    purge_data: Option<bool>,
    dry_run: bool,
    remove_registry_autostart: bool,
    stdin_is_terminal: bool,
    mut reader: R,
    mut writer: W,
) -> Result<UninstallReport, String> {
    let should_purge = match purge_data {
        Some(flag) => flag,
        None => {
            if !stdin_is_terminal {
                false
            } else {
                write!(
                    writer,
                    "Delete Miku user data directory '{}' (vault, memory, logs)? [y/N]: ",
                    layout.data_dir.display()
                )
                .map_err(|e| format!("failed writing uninstall prompt: {e}"))?;
                let _ = writer.flush();
                let mut answer = String::new();
                reader
                    .read_line(&mut answer)
                    .map_err(|e| format!("failed reading uninstall confirmation: {e}"))?;
                is_affirmative_answer(&answer)
            }
        }
    };

    let exe_exists = layout.target_exe.exists();
    let backup_exists = layout.previous_backup_exe.exists();
    let staging_exists = layout.staging_dir.exists();
    let data_exists = layout.data_dir.exists();

    if dry_run {
        return Ok(UninstallReport {
            removed_exe: exe_exists,
            removed_backup: backup_exists,
            removed_staging: staging_exists,
            data_purged: should_purge && data_exists,
            autostart_removed: remove_registry_autostart,
            dry_run: true,
        });
    }

    let autostart_removed = if remove_registry_autostart {
        delete_hkcu_run_entry()
    } else {
        false
    };

    let removed_exe = remove_file_if_exists(&layout.target_exe)?;
    let removed_backup = remove_file_if_exists(&layout.previous_backup_exe)?;
    let removed_staging = if staging_exists {
        fs::remove_dir_all(&layout.staging_dir)
            .map_err(|e| format!("failed removing staging dir: {e}"))?;
        true
    } else {
        false
    };

    let data_purged = if should_purge && layout.data_dir.exists() {
        fs::remove_dir_all(&layout.data_dir)
            .map_err(|e| format!("failed removing user data dir: {e}"))?;
        true
    } else {
        false
    };

    Ok(UninstallReport {
        removed_exe,
        removed_backup,
        removed_staging,
        data_purged,
        autostart_removed,
        dry_run: false,
    })
}

/// Fail-closed updater: enforces explicit user consent BEFORE downloading, stages on disk,
/// verifies exact SHA-256 and byte size, and wipes staging immediately on any mismatch.
pub fn apply_verified_update(
    manifest: &ReleaseManifestV1,
    layout: &InstallLayout,
    fetcher: &dyn PayloadFetcher,
    consent_granted: bool,
) -> Result<UpdateReport, UpdateError> {
    manifest.validate()?;

    // Gate 1: Never download or touch disk without explicit user consent.
    if !consent_granted {
        return Err(UpdateError::ConsentDenied);
    }

    let payload_bytes = fetcher
        .fetch_bytes(&manifest.download_url)
        .map_err(UpdateError::FetchFailed)?;

    fs::create_dir_all(&layout.staging_dir)
        .map_err(|e| UpdateError::IoError(format!("create staging dir failed: {e}")))?;
    let staging_path = layout
        .staging_dir
        .join(format!("{}.staging", manifest.artifact_name));
    fs::write(&staging_path, &payload_bytes)
        .map_err(|e| UpdateError::IoError(format!("write staging file failed: {e}")))?;

    let actual_bytes = payload_bytes.len() as u64;
    if actual_bytes != manifest.size_bytes || actual_bytes > MAX_INSTALLABLE_BINARY_BYTES {
        let cleaned = cleanup_staging_file(&staging_path);
        return Err(UpdateError::SizeMismatch {
            expected_bytes: manifest.size_bytes,
            actual_bytes,
            staging_cleaned: cleaned,
        });
    }

    // Gate 2: Re-read staged artifact from disk and check SHA-256 in constant time.
    let staged_bytes = fs::read(&staging_path)
        .map_err(|e| UpdateError::IoError(format!("read staging file failed: {e}")))?;
    let actual_sha256 = sha256_hex(&staged_bytes);
    let expected_sha256 = manifest.sha256_hex.to_ascii_lowercase();
    if !constant_time_token_eq(&actual_sha256, &expected_sha256) {
        let cleaned = cleanup_staging_file(&staging_path);
        return Err(UpdateError::IntegrityMismatch {
            expected_sha256,
            actual_sha256,
            staging_cleaned: cleaned,
        });
    }

    // Gate 3: Safe atomic swap with previous rollback backup.
    fs::create_dir_all(&layout.install_dir)
        .map_err(|e| UpdateError::IoError(format!("create install dir failed: {e}")))?;
    let previous_backup_created = if layout.target_exe.exists() {
        fs::copy(&layout.target_exe, &layout.previous_backup_exe)
            .map_err(|e| UpdateError::IoError(format!("create backup failed: {e}")))?;
        true
    } else {
        false
    };

    if let Err(e) = fs::rename(&staging_path, &layout.target_exe) {
        fs::copy(&staging_path, &layout.target_exe).map_err(|copy_err| {
            let _ = cleanup_staging_file(&staging_path);
            UpdateError::IoError(format!("rename ({e}) & copy ({copy_err}) failed"))
        })?;
        let _ = cleanup_staging_file(&staging_path);
    }

    Ok(UpdateReport {
        target_version: manifest.version.clone(),
        target_exe: layout.target_exe.clone(),
        sha256_verified: actual_sha256,
        size_bytes: actual_bytes,
        previous_backup_created,
    })
}

/// Interactive consent prompt helper that fails closed when `stdin` is non-interactive.
pub fn prompt_for_update_consent<R: BufRead, W: Write>(
    manifest: &ReleaseManifestV1,
    stdin_is_terminal: bool,
    mut reader: R,
    mut writer: W,
) -> Result<bool, String> {
    if !stdin_is_terminal {
        return Ok(false);
    }
    writeln!(
        writer,
        "Miku Update Available: version={} size={} B sha256={}",
        manifest.version, manifest.size_bytes, manifest.sha256_hex
    )
    .map_err(|e| format!("failed writing consent prompt: {e}"))?;
    write!(writer, "Authorize download and SHA-256 verification? [y/N]: ")
        .map_err(|e| format!("failed writing consent prompt: {e}"))?;
    let _ = writer.flush();
    let mut line = String::new();
    reader
        .read_line(&mut line)
        .map_err(|e| format!("failed reading consent response: {e}"))?;
    Ok(is_affirmative_answer(&line))
}

fn is_affirmative_answer(raw: &str) -> bool {
    matches!(
        raw.trim().to_ascii_lowercase().as_str(),
        "y" | "yes" | "s" | "si" | "sí" | "purge"
    )
}

fn cleanup_staging_file(staging_path: &Path) -> bool {
    if !staging_path.exists() {
        return true;
    }
    fs::remove_file(staging_path).is_ok() && !staging_path.exists()
}

fn remove_file_if_exists(path: &Path) -> Result<bool, String> {
    if path.exists() {
        fs::remove_file(path)
            .map_err(|e| format!("failed removing '{}': {e}", path.display()))?;
        Ok(true)
    } else {
        Ok(false)
    }
}
