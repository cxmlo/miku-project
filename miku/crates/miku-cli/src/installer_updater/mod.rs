//! Lightweight per-user Windows installer, interactive uninstaller,
//! and explicit-consent SHA-256 verified self-updater (Step 5.4 / F-24).

pub mod cli_and_registry;
pub mod operations;

#[cfg(test)]
mod tests;

use std::env;
use std::fmt;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

use serde::{Deserialize, Serialize};

pub use cli_and_registry::execute_cli_installer_command;
pub use operations::{
    apply_verified_update, install_local_binary, prompt_for_update_consent, uninstall_local_binary,
};

/// Hard release binary budget (15 MiB) enforced on installer and updater payloads.
pub const MAX_INSTALLABLE_BINARY_BYTES: u64 = 15 * 1024 * 1024;

/// Signed / published release manifest metadata validated before downloading any update.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ReleaseManifestV1 {
    pub version: String,
    pub channel: String,
    pub artifact_name: String,
    pub download_url: String,
    pub sha256_hex: String,
    pub size_bytes: u64,
    #[serde(default)]
    pub release_notes_summary: String,
}

impl ReleaseManifestV1 {
    /// Validates structural safety of release manifest fields prior to prompting or fetching.
    pub fn validate(&self) -> Result<(), UpdateError> {
        if self.version.trim().is_empty() || self.version.len() > 64 {
            return Err(UpdateError::InvalidManifest(
                "version must be 1..=64 characters".to_string(),
            ));
        }
        if !is_safe_single_filename(&self.artifact_name) {
            return Err(UpdateError::InvalidManifest(format!(
                "unsafe artifact_name '{}'",
                self.artifact_name
            )));
        }
        if !is_valid_sha256_hex(&self.sha256_hex) {
            return Err(UpdateError::InvalidManifest(
                "sha256_hex must contain exactly 64 hex digits".to_string(),
            ));
        }
        if self.size_bytes == 0 || self.size_bytes > MAX_INSTALLABLE_BINARY_BYTES {
            return Err(UpdateError::InvalidManifest(format!(
                "size_bytes {} outside allowed range 1..={}",
                self.size_bytes, MAX_INSTALLABLE_BINARY_BYTES
            )));
        }
        let lower_url = self.download_url.to_ascii_lowercase();
        let allowed_scheme = lower_url.starts_with("https://") || lower_url.starts_with("local://");
        if !allowed_scheme || lower_url.contains("..") {
            return Err(UpdateError::InvalidManifest(format!(
                "disallowed download_url '{}'; expected https:// or local://",
                self.download_url
            )));
        }
        Ok(())
    }
}

/// Target directories and files for a per-user Windows installation.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct InstallLayout {
    pub install_dir: PathBuf,
    pub target_exe: PathBuf,
    pub previous_backup_exe: PathBuf,
    pub data_dir: PathBuf,
    pub staging_dir: PathBuf,
}

impl InstallLayout {
    /// Builds standard `%LOCALAPPDATA%`-rooted paths (`Programs\Miku` and `Miku`).
    pub fn from_windows_localappdata() -> Result<Self, String> {
        let local_app_data = env::var_os("LOCALAPPDATA")
            .map(PathBuf::from)
            .ok_or_else(|| "LOCALAPPDATA environment variable is not set".to_string())?;
        Ok(Self::from_roots(
            &local_app_data.join("Programs").join("Miku"),
            &local_app_data.join("Miku"),
        ))
    }

    /// Builds an explicit hermetic install layout from custom install and data roots.
    pub fn from_roots(install_dir: &Path, data_dir: &Path) -> Self {
        Self {
            install_dir: install_dir.to_path_buf(),
            target_exe: install_dir.join("miku.exe"),
            previous_backup_exe: install_dir.join("miku.exe.prev"),
            data_dir: data_dir.to_path_buf(),
            staging_dir: data_dir.join("staging"),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct InstallReport {
    pub installed_exe: PathBuf,
    pub sha256_hex: String,
    pub size_bytes: u64,
    pub autostart_registered: bool,
    pub dry_run: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct UninstallReport {
    pub removed_exe: bool,
    pub removed_backup: bool,
    pub removed_staging: bool,
    pub data_purged: bool,
    pub autostart_removed: bool,
    pub dry_run: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct UpdateReport {
    pub target_version: String,
    pub target_exe: PathBuf,
    pub sha256_verified: String,
    pub size_bytes: u64,
    pub previous_backup_created: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum UpdateError {
    InvalidManifest(String),
    ConsentDenied,
    FetchFailed(String),
    IntegrityMismatch {
        expected_sha256: String,
        actual_sha256: String,
        staging_cleaned: bool,
    },
    SizeMismatch {
        expected_bytes: u64,
        actual_bytes: u64,
        staging_cleaned: bool,
    },
    IoError(String),
}

impl fmt::Display for UpdateError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidManifest(msg) => write!(f, "invalid release manifest: {msg}"),
            Self::ConsentDenied => {
                write!(f, "update aborted: explicit user consent is required")
            }
            Self::FetchFailed(msg) => write!(f, "failed to download update payload: {msg}"),
            Self::IntegrityMismatch {
                expected_sha256,
                actual_sha256,
                staging_cleaned,
            } => write!(
                f,
                "SHA-256 mismatch (expected {expected_sha256}, got {actual_sha256}); staging_cleaned={staging_cleaned}"
            ),
            Self::SizeMismatch {
                expected_bytes,
                actual_bytes,
                staging_cleaned,
            } => write!(
                f,
                "payload size mismatch (expected {expected_bytes} B, got {actual_bytes} B); staging_cleaned={staging_cleaned}"
            ),
            Self::IoError(msg) => write!(f, "update I/O error: {msg}"),
        }
    }
}

impl std::error::Error for UpdateError {}

/// Abstraction for retrieving update payload bytes only after explicit user consent.
pub trait PayloadFetcher {
    fn fetch_bytes(&self, url: &str) -> Result<Vec<u8>, String>;
}

/// Offline / local-file fetcher for `--payload <path>` or `local://` manifests.
pub struct LocalFileFetcher {
    pub override_path: Option<PathBuf>,
}

impl PayloadFetcher for LocalFileFetcher {
    fn fetch_bytes(&self, url: &str) -> Result<Vec<u8>, String> {
        let path = if let Some(p) = &self.override_path {
            p.clone()
        } else if let Some(raw_local) = url.strip_prefix("local://") {
            PathBuf::from(raw_local)
        } else {
            return Err("HTTPS remote fetch requires HttpsPowerShellFetcher or --payload".to_string());
        };
        fs::read(&path).map_err(|e| format!("failed reading payload '{}': {e}", path.display()))
    }
}

/// TLS 1.2+ PowerShell `Invoke-WebRequest` fetcher used only after explicit user consent.
pub struct HttpsPowerShellFetcher;

impl PayloadFetcher for HttpsPowerShellFetcher {
    fn fetch_bytes(&self, url: &str) -> Result<Vec<u8>, String> {
        if !url.to_ascii_lowercase().starts_with("https://") {
            return Err(format!("refusing non-HTTPS update URL '{url}'"));
        }
        let temp_dl = env::temp_dir().join(format!("miku_dl_{}.bin", std::process::id()));
        let escaped_url = url.replace('\'', "''");
        let escaped_out = temp_dl.to_string_lossy().replace('\'', "''");
        let script = format!(
            "$ProgressPreference='SilentlyContinue'; [Net.ServicePointManager]::SecurityProtocol = [Net.SecurityProtocolType]::Tls12; Invoke-WebRequest -Uri '{escaped_url}' -OutFile '{escaped_out}' -UseBasicParsing"
        );
        let status = Command::new("powershell")
            .args(["-NoProfile", "-NonInteractive", "-Command", &script])
            .status()
            .map_err(|e| format!("failed spawning powershell downloader: {e}"))?;
        if !status.success() {
            let _ = fs::remove_file(&temp_dl);
            return Err(format!("HTTPS download exited with code {:?}", status.code()));
        }
        let bytes = fs::read(&temp_dl)
            .map_err(|e| format!("failed reading downloaded temp file: {e}"));
        let _ = fs::remove_file(&temp_dl);
        bytes
    }
}

fn is_safe_single_filename(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= 128
        && !name.contains('/')
        && !name.contains('\\')
        && !name.contains("..")
        && !name.contains(':')
        && !name.contains('\0')
}

fn is_valid_sha256_hex(hex_str: &str) -> bool {
    hex_str.len() == 64 && hex_str.bytes().all(|b| b.is_ascii_hexdigit())
}
