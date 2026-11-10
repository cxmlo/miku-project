use std::fs;
use std::io::{self, IsTerminal, Write};
use std::path::{Path, PathBuf};
use std::time::{Instant, SystemTime, UNIX_EPOCH};

use miku_gatekeeper::{
    CanonicalPathJail, Capability, GatekeeperEngine, GatekeeperVerdict, PolicyEffect,
    ProjectProfile,
};

pub mod merge_and_diff;
pub use merge_and_diff::*;

#[cfg(test)]
mod tests;

pub const MIKU_HOOK_ENTRY_ID: &str = "miku-orchestrator";
pub const HOOK_OP_BUDGET_MS: f64 = 20.0;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HookTargetAgent {
    ClaudeCode,
    Cursor,
}

impl HookTargetAgent {
    pub fn as_str(&self) -> &'static str {
        match self {
            HookTargetAgent::ClaudeCode => "claude-code",
            HookTargetAgent::Cursor => "cursor",
        }
    }

    pub fn parse(raw: &str) -> Result<Self, HookInstallError> {
        match raw.trim().to_ascii_lowercase().as_str() {
            "claude-code" | "claude_code" | "claude" => Ok(HookTargetAgent::ClaudeCode),
            "cursor" | "cursor-ide" => Ok(HookTargetAgent::Cursor),
            other => Err(HookInstallError::InvalidAgent(format!(
                "unsupported hook agent '{}'; expected 'claude-code' or 'cursor'",
                other
            ))),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HookAuditStep {
    ValidatedPathAndGatekeeper,
    ParsedExistingSettings,
    MergedMikuEntryOnly,
    CreatedAndValidatedBackup,
    RenderedUnifiedDiff,
    RequestedUserConfirmation,
    WroteMergedSettings,
    CancelledWithoutModifyingActiveFile,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum HookInstallError {
    InvalidAgent(String),
    InvalidJsonSyntax { line: usize, column: usize, reason: String },
    InvalidJsonRootType { line: usize, reason: String },
    PathEscapeOrInvalid(String),
    SymlinkForbidden(String),
    GatekeeperDenied(String),
    BackupFailed(String),
    InvalidBackup { line: Option<usize>, reason: String },
    ConfirmationRejected(String),
    IoError(String),
}

impl std::fmt::Display for HookInstallError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            HookInstallError::InvalidAgent(msg) => write!(f, "invalid agent target: {}", msg),
            HookInstallError::InvalidJsonSyntax { line, column, reason } => write!(
                f, "invalid settings.json syntax at line {}, column {}: {}", line, column, reason
            ),
            HookInstallError::InvalidJsonRootType { line, reason } => {
                write!(f, "invalid settings.json structure at line {}: {}", line, reason)
            }
            HookInstallError::PathEscapeOrInvalid(msg) => write!(f, "invalid or unauthorized config path: {}", msg),
            HookInstallError::SymlinkForbidden(msg) => write!(f, "symbolic link or reparse point forbidden: {}", msg),
            HookInstallError::GatekeeperDenied(msg) => write!(f, "Gatekeeper denied configuration access: {}", msg),
            HookInstallError::BackupFailed(msg) => write!(f, "failed to create or verify .bak backup: {}", msg),
            HookInstallError::InvalidBackup { line: Some(ln), reason } => {
                write!(f, "invalid backup file at line {}: {}", ln, reason)
            }
            HookInstallError::InvalidBackup { line: None, reason } => write!(f, "invalid backup file: {}", reason),
            HookInstallError::ConfirmationRejected(msg) => write!(f, "hook installation confirmation rejected: {}", msg),
            HookInstallError::IoError(msg) => write!(f, "I/O error: {}", msg),
        }
    }
}

impl std::error::Error for HookInstallError {}

/// Trait for displaying the unified diff and requesting explicit user confirmation before writing.
pub trait HookConfirmationPrompter {
    fn confirm_after_diff(&self, unified_diff: &str, backup_path: &Path) -> Result<bool, String>;
}

/// Interactive terminal prompter that prints the unified diff and requires explicit `y`/`yes`.
#[derive(Debug, Default)]
pub struct TerminalHookPrompter;

impl HookConfirmationPrompter for TerminalHookPrompter {
    fn confirm_after_diff(&self, unified_diff: &str, backup_path: &Path) -> Result<bool, String> {
        if !io::stdin().is_terminal() {
            return Err("miku hook install requires an interactive terminal for explicit diff confirmation; automatic non-interactive confirmation is forbidden".to_string());
        }
        println!("{}\nBackup created & verified: {}", unified_diff, backup_path.display());
        print!("Apply Miku hook changes to settings.json? [y/N]: ");
        let _ = io::stdout().flush();
        let mut answer = String::new();
        io::stdin().read_line(&mut answer).map_err(|e| format!("failed to read confirmation: {}", e))?;
        let normalized = answer.trim().to_ascii_lowercase();
        Ok(normalized == "y" || normalized == "yes")
    }
}

#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct HookPhaseTimings {
    pub jail_and_gatekeeper_ms: f64,
    pub read_and_parse_ms: f64,
    pub merge_and_diff_ms: f64,
    pub backup_write_verify_ms: f64,
    pub confirmation_prompt_ms: f64,
    pub active_write_ms: f64,
    pub total_elapsed_ms: f64,
}

#[derive(Debug, Clone)]
pub struct HookInstallOutcome {
    pub agent: HookTargetAgent,
    pub settings_path: PathBuf,
    pub backup_path: PathBuf,
    pub unified_diff: String,
    pub applied: bool,
    pub idempotent_noop: bool,
    pub elapsed_ms: f64,
    pub phase_timings: HookPhaseTimings,
    pub audit_trail: Vec<HookAuditStep>,
}

#[derive(Debug, Clone)]
pub struct HookRestoreOutcome {
    pub settings_path: PathBuf,
    pub backup_path: PathBuf,
    pub restored_bytes: usize,
    pub elapsed_ms: f64,
}

fn ensure_not_symlink(path: &Path) -> Result<(), HookInstallError> {
    if let Ok(meta) = fs::symlink_metadata(path) {
        if meta.file_type().is_symlink() {
            return Err(HookInstallError::SymlinkForbidden(path.display().to_string()));
        }
    }
    Ok(())
}

fn open_existing_non_symlink_rw(path: &Path) -> Result<(fs::File, Vec<u8>), HookInstallError> {
    use std::io::Read;
    let meta = fs::symlink_metadata(path).map_err(|e| {
        if e.kind() == io::ErrorKind::NotFound {
            HookInstallError::PathEscapeOrInvalid(format!("settings file '{}' does not exist", path.display()))
        } else {
            HookInstallError::IoError(format!("failed to inspect settings.json: {}", e))
        }
    })?;
    if meta.file_type().is_symlink() {
        return Err(HookInstallError::SymlinkForbidden(path.display().to_string()));
    }
    let mut file = fs::OpenOptions::new().read(true).write(true).open(path)
        .map_err(|e| HookInstallError::IoError(format!("failed to open settings.json: {}", e)))?;
    let mut bytes = Vec::with_capacity(meta.len() as usize);
    file.read_to_end(&mut bytes)
        .map_err(|e| HookInstallError::IoError(format!("failed to read settings.json: {}", e)))?;
    Ok((file, bytes))
}

fn write_and_verify_file_bytes(path: &Path, expected_bytes: &[u8]) -> Result<(), String> {
    use std::io::{Read, Seek, SeekFrom};
    let mut file = fs::OpenOptions::new().read(true).write(true).create(true).truncate(true).open(path)
        .map_err(|e| format!("failed to open '{}': {}", path.display(), e))?;
    file.write_all(expected_bytes).map_err(|e| format!("failed to write '{}': {}", path.display(), e))?;
    file.seek(SeekFrom::Start(0)).map_err(|e| format!("failed to seek '{}': {}", path.display(), e))?;
    let mut verified = Vec::with_capacity(expected_bytes.len());
    file.read_to_end(&mut verified).map_err(|e| format!("failed to read back '{}': {}", path.display(), e))?;
    if verified.as_slice() != expected_bytes {
        return Err(format!("byte verification mismatch for '{}'", path.display()));
    }
    Ok(())
}

fn build_phase_timings(
    t0: Instant, t_gk: Instant, t_parse: Instant, t_diff: Instant, t_bak: Instant, t_prompt: Instant, t_end: Instant, wrote_active: bool,
) -> HookPhaseTimings {
    HookPhaseTimings {
        jail_and_gatekeeper_ms: (t_gk - t0).as_secs_f64() * 1_000.0,
        read_and_parse_ms: (t_parse - t_gk).as_secs_f64() * 1_000.0,
        merge_and_diff_ms: (t_diff - t_parse).as_secs_f64() * 1_000.0,
        backup_write_verify_ms: (t_bak - t_diff).as_secs_f64() * 1_000.0,
        confirmation_prompt_ms: (t_prompt - t_bak).as_secs_f64() * 1_000.0,
        active_write_ms: if wrote_active { (t_end - t_prompt).as_secs_f64() * 1_000.0 } else { 0.0 },
        total_elapsed_ms: (t_end - t0).as_secs_f64() * 1_000.0,
    }
}

fn build_config_jail(authorized_config_root: &Path) -> Result<CanonicalPathJail, HookInstallError> {
    let root_str = authorized_config_root.to_string_lossy().replace('\\', "/");
    CanonicalPathJail::new(&root_str).map_err(|e| HookInstallError::PathEscapeOrInvalid(e.to_string()))
}

fn validate_confined_path_with_jail(
    jail: &CanonicalPathJail, target_path: &Path, expected_ext: &str, check_symlink_now: bool,
) -> Result<(PathBuf, String), HookInstallError> {
    let target_str = target_path.to_string_lossy().replace('\\', "/");
    let canonical = jail
        .resolve_and_confine(&target_str)
        .map_err(|e| HookInstallError::PathEscapeOrInvalid(e.to_string()))?;

    if !canonical.to_ascii_lowercase().ends_with(expected_ext) {
        return Err(HookInstallError::PathEscapeOrInvalid(format!(
            "target path '{}' must end with '{}'",
            canonical, expected_ext
        )));
    }

    let confined_buf = PathBuf::from(&canonical);
    if check_symlink_now {
        ensure_not_symlink(&confined_buf)?;
    }

    let rel = jail.relative_to_root(&canonical);
    Ok((confined_buf, rel))
}

fn evaluate_gatekeeper_fs(
    gatekeeper: &mut GatekeeperEngine,
    capability: Capability,
    rel_target: &str,
    now_ms: u64,
) -> Result<(), HookInstallError> {
    match gatekeeper.evaluate_request("miku-cli-hook", capability, rel_target, "", now_ms) {
        GatekeeperVerdict::Allow { .. } => Ok(()),
        GatekeeperVerdict::Deny { reason, detail, .. } => Err(HookInstallError::GatekeeperDenied(
            format!("{}: {}", reason.as_code(), detail),
        )),
        GatekeeperVerdict::AskPending { ticket, .. } => Err(HookInstallError::GatekeeperDenied(
            format!("confirmation_pending: {}", ticket.req_hash),
        )),
    }
}

fn default_dated_backup_path(settings_path: &Path, timestamp_ms: u64) -> PathBuf {
    let file_name = settings_path
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or("settings.json");
    let bak_name = format!("{}.{}.bak", file_name, timestamp_ms);
    match settings_path.parent() {
        Some(parent) => parent.join(bak_name),
        None => PathBuf::from(bak_name),
    }
}

fn discover_latest_backup_for_settings(
    authorized_config_root: &Path,
    settings_path: &Path,
) -> Result<PathBuf, HookInstallError> {
    let file_name = settings_path
        .file_name()
        .and_then(|n| n.to_str())
        .ok_or_else(|| HookInstallError::InvalidBackup {
            line: None,
            reason: "cannot determine settings filename".to_string(),
        })?;
    let prefix = format!("{}.", file_name);
    let search_dir = settings_path.parent().unwrap_or(authorized_config_root);

    let mut candidates = Vec::new();
    let entries = fs::read_dir(search_dir).map_err(|e| HookInstallError::InvalidBackup {
        line: None,
        reason: format!("failed to scan backup directory: {}", e),
    })?;

    for entry in entries.flatten() {
        let p = entry.path();
        if let Some(name) = p.file_name().and_then(|n| n.to_str()) {
            if name.starts_with(&prefix) && name.ends_with(".bak") {
                candidates.push(p);
            }
        }
    }
    candidates.sort();
    candidates.pop().ok_or_else(|| HookInstallError::InvalidBackup {
        line: None,
        reason: format!("no .bak backup found for '{}'", file_name),
    })
}

/// Safely installs or updates ONLY Miku's hook entry in `settings_path`.
pub fn install_miku_hook(
    authorized_config_root: &Path,
    settings_path: &Path,
    agent: HookTargetAgent,
    custom_backup_path: Option<&Path>,
    prompter: &dyn HookConfirmationPrompter,
    gatekeeper: &mut GatekeeperEngine,
    now_ms: u64,
) -> Result<HookInstallOutcome, HookInstallError> {
    let t0 = Instant::now();
    let mut audit_trail = Vec::with_capacity(8);

    let jail = build_config_jail(authorized_config_root)?;
    let (confined_settings, rel_settings) =
        validate_confined_path_with_jail(&jail, settings_path, ".json", false)?;
    let raw_bak = custom_backup_path
        .map(PathBuf::from)
        .unwrap_or_else(|| default_dated_backup_path(&confined_settings, now_ms));
    let (confined_backup, rel_backup) =
        validate_confined_path_with_jail(&jail, &raw_bak, ".bak", custom_backup_path.is_some())?;

    evaluate_gatekeeper_fs(gatekeeper, Capability::FsRead, &rel_settings, now_ms)?;
    evaluate_gatekeeper_fs(gatekeeper, Capability::FsWrite, &rel_backup, now_ms)?;
    evaluate_gatekeeper_fs(gatekeeper, Capability::FsWrite, &rel_settings, now_ms)?;
    audit_trail.push(HookAuditStep::ValidatedPathAndGatekeeper);
    let t_after_jail_gk = Instant::now();

    let (mut settings_file, original_bytes) = open_existing_non_symlink_rw(&confined_settings)?;
    let original_text = std::str::from_utf8(&original_bytes)
        .map_err(|_| HookInstallError::InvalidJsonSyntax {
            line: 1,
            column: 1,
            reason: "settings.json is not valid UTF-8".to_string(),
        })?;

    let root_map = validate_and_parse_settings_json(original_text)?;
    audit_trail.push(HookAuditStep::ParsedExistingSettings);
    let t_after_read_parse = Instant::now();

    let (merged_text, idempotent_noop) =
        merge_miku_hook_settings_from_map(root_map, original_text, agent)?;
    audit_trail.push(HookAuditStep::MergedMikuEntryOnly);
    let unified_diff =
        compute_unified_diff(original_text, &merged_text, "a/settings.json", "b/settings.json");
    let t_after_merge_diff = Instant::now();

    write_and_verify_file_bytes(&confined_backup, &original_bytes)
        .map_err(HookInstallError::BackupFailed)?;
    audit_trail.push(HookAuditStep::CreatedAndValidatedBackup);
    audit_trail.push(HookAuditStep::RenderedUnifiedDiff);
    audit_trail.push(HookAuditStep::RequestedUserConfirmation);
    let t_after_backup = Instant::now();

    let confirmed = prompter
        .confirm_after_diff(&unified_diff, &confined_backup)
        .map_err(HookInstallError::ConfirmationRejected)?;
    let t_after_prompt = Instant::now();

    if !confirmed {
        audit_trail.push(HookAuditStep::CancelledWithoutModifyingActiveFile);
        let phase_timings = build_phase_timings(
            t0, t_after_jail_gk, t_after_read_parse, t_after_merge_diff, t_after_backup, t_after_prompt, t_after_prompt, false,
        );
        return Ok(HookInstallOutcome {
            agent,
            settings_path: confined_settings,
            backup_path: confined_backup,
            unified_diff,
            applied: false,
            idempotent_noop,
            elapsed_ms: phase_timings.total_elapsed_ms,
            phase_timings,
            audit_trail,
        });
    }

    if !idempotent_noop {
        use std::io::{Seek, SeekFrom};
        settings_file
            .set_len(0)
            .and_then(|_| settings_file.seek(SeekFrom::Start(0)))
            .and_then(|_| settings_file.write_all(merged_text.as_bytes()))
            .map_err(|e| {
                HookInstallError::IoError(format!("failed to write merged settings.json: {}", e))
            })?;
    }
    drop(settings_file);
    audit_trail.push(HookAuditStep::WroteMergedSettings);
    let t_end = Instant::now();
    let phase_timings = build_phase_timings(
        t0, t_after_jail_gk, t_after_read_parse, t_after_merge_diff, t_after_backup, t_after_prompt, t_end, true,
    );

    Ok(HookInstallOutcome {
        agent,
        settings_path: confined_settings,
        backup_path: confined_backup,
        unified_diff,
        applied: true,
        idempotent_noop,
        elapsed_ms: phase_timings.total_elapsed_ms,
        phase_timings,
        audit_trail,
    })
}

/// Restores `settings_path` byte-for-byte from a validated `.bak` backup file.
pub fn restore_miku_hook(
    authorized_config_root: &Path,
    settings_path: &Path,
    backup_path: Option<&Path>,
    gatekeeper: &mut GatekeeperEngine,
    now_ms: u64,
) -> Result<HookRestoreOutcome, HookInstallError> {
    let t0 = Instant::now();
    let jail = build_config_jail(authorized_config_root)?;
    let (confined_settings, rel_settings) =
        validate_confined_path_with_jail(&jail, settings_path, ".json", true)?;
    let resolved_bak = match backup_path {
        Some(p) => p.to_path_buf(),
        None => discover_latest_backup_for_settings(authorized_config_root, &confined_settings)?,
    };
    let (confined_backup, rel_backup) =
        validate_confined_path_with_jail(&jail, &resolved_bak, ".bak", false)?;

    evaluate_gatekeeper_fs(gatekeeper, Capability::FsRead, &rel_backup, now_ms)?;
    evaluate_gatekeeper_fs(gatekeeper, Capability::FsWrite, &rel_settings, now_ms)?;

    let bak_meta = fs::symlink_metadata(&confined_backup).map_err(|e| {
        if e.kind() == io::ErrorKind::NotFound {
            HookInstallError::InvalidBackup {
                line: None,
                reason: format!("backup file '{}' does not exist", confined_backup.display()),
            }
        } else {
            HookInstallError::InvalidBackup {
                line: None,
                reason: format!("failed to inspect backup file: {}", e),
            }
        }
    })?;
    if bak_meta.file_type().is_symlink() {
        return Err(HookInstallError::SymlinkForbidden(confined_backup.display().to_string()));
    }

    let backup_bytes = fs::read(&confined_backup).map_err(|e| HookInstallError::InvalidBackup {
        line: None,
        reason: format!("failed to read backup file: {}", e),
    })?;
    let backup_text =
        std::str::from_utf8(&backup_bytes).map_err(|_| HookInstallError::InvalidBackup {
            line: Some(1),
            reason: "backup file is not valid UTF-8".to_string(),
        })?;

    validate_and_parse_settings_json(backup_text).map_err(|err| match err {
        HookInstallError::InvalidJsonSyntax { line, reason, .. }
        | HookInstallError::InvalidJsonRootType { line, reason } => HookInstallError::InvalidBackup {
            line: Some(line),
            reason,
        },
        other => HookInstallError::InvalidBackup {
            line: None,
            reason: other.to_string(),
        },
    })?;

    write_and_verify_file_bytes(&confined_settings, &backup_bytes)
        .map_err(HookInstallError::IoError)?;

    Ok(HookRestoreOutcome {
        settings_path: confined_settings,
        backup_path: confined_backup,
        restored_bytes: backup_bytes.len(),
        elapsed_ms: t0.elapsed().as_secs_f64() * 1_000.0,
    })
}

fn current_epoch_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(1_700_000_000_000)
}

fn build_cli_hook_gatekeeper(config_root: &Path) -> Result<GatekeeperEngine, String> {
    let root_str = config_root.to_string_lossy().replace('\\', "/");
    let profile = ProjectProfile::new("miku-cli-hook", &root_str, 60_000)
        .with_rule(Capability::FsRead, "**", PolicyEffect::Allow, false)
        .with_rule(Capability::FsWrite, "**", PolicyEffect::Allow, false);
    GatekeeperEngine::new(profile).map_err(|e| e.to_string())
}

pub fn execute_cli_hook_command(args: &[String]) -> Result<Option<String>, String> {
    let prompter = TerminalHookPrompter;
    execute_cli_hook_command_with_prompter(args, &prompter)
}

pub fn execute_cli_hook_command_with_prompter(
    args: &[String],
    prompter: &dyn HookConfirmationPrompter,
) -> Result<Option<String>, String> {
    let mut is_hook_cmd = false;
    let mut remaining = Vec::new();
    for arg in args.iter().skip(1) {
        if !is_hook_cmd && (arg == "hook" || arg == "hooks") {
            is_hook_cmd = true;
            continue;
        }
        if is_hook_cmd {
            remaining.push(arg.as_str());
        }
    }
    if !is_hook_cmd {
        return Ok(None);
    }

    let subcmd = remaining.first().copied().ok_or_else(|| {
        "missing hook subcommand: expected 'miku hook install' or 'miku hook restore'".to_string()
    })?;

    let mut agent_raw = "claude-code";
    let mut settings_opt: Option<&str> = None;
    let mut config_root_opt: Option<&str> = None;
    let mut backup_opt: Option<&str> = None;
    let mut i = 1;
    while i + 1 < remaining.len() {
        match remaining[i] {
            "--agent" => { agent_raw = remaining[i + 1]; i += 2; }
            "--settings" => { settings_opt = Some(remaining[i + 1]); i += 2; }
            "--config-root" => { config_root_opt = Some(remaining[i + 1]); i += 2; }
            "--backup" => { backup_opt = Some(remaining[i + 1]); i += 2; }
            _ => { i += 1; }
        }
    }

    let settings_str = settings_opt
        .ok_or_else(|| "missing --settings <path/to/settings.json> argument".to_string())?;
    let settings_path = PathBuf::from(settings_str);
    let config_root = match config_root_opt {
        Some(r) => PathBuf::from(r),
        None => settings_path
            .parent()
            .map(Path::to_path_buf)
            .ok_or_else(|| "cannot infer --config-root from settings path".to_string())?,
    };

    let mut gatekeeper = build_cli_hook_gatekeeper(&config_root)?;
    let now_ms = current_epoch_ms();

    match subcmd {
        "install" => {
            let agent = HookTargetAgent::parse(agent_raw).map_err(|e| e.to_string())?;
            let out = install_miku_hook(
                &config_root, &settings_path, agent, backup_opt.map(Path::new), prompter, &mut gatekeeper, now_ms,
            )
            .map_err(|e| e.to_string())?;
            let status_line = if out.applied {
                format!(
                    "MIKU_HOOK_INSTALLED|agent={}|settings={}|backup={}|idempotent={}|elapsed_ms={:.3}",
                    out.agent.as_str(), out.settings_path.display(), out.backup_path.display(), out.idempotent_noop, out.elapsed_ms
                )
            } else {
                format!(
                    "MIKU_HOOK_CANCELLED|agent={}|settings={}|backup={}|elapsed_ms={:.3}",
                    out.agent.as_str(), out.settings_path.display(), out.backup_path.display(), out.elapsed_ms
                )
            };
            Ok(Some(format!("{}\n{}", out.unified_diff, status_line)))
        }
        "restore" => {
            let out = restore_miku_hook(
                &config_root, &settings_path, backup_opt.map(Path::new), &mut gatekeeper, now_ms,
            )
            .map_err(|e| e.to_string())?;
            Ok(Some(format!(
                "MIKU_HOOK_RESTORED|status=restored|settings={}|backup={}|bytes={}|elapsed_ms={:.3}",
                out.settings_path.display(), out.backup_path.display(), out.restored_bytes, out.elapsed_ms
            )))
        }
        other => Err(format!("unknown hook subcommand '{}'; available: install, restore", other)),
    }
}
