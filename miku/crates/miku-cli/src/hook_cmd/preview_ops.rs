use std::fs;
use std::io::{self, Write};
use std::path::{Path, PathBuf};

use miku_gatekeeper::{
    CanonicalPathJail, Capability, GatekeeperEngine, GatekeeperVerdict, PolicyEffect,
    ProjectProfile,
};

use super::{
    compute_unified_diff, merge_miku_hook_settings_from_map, validate_and_parse_settings_json,
    HookConfirmationPrompter, HookInstallError, HookTargetAgent, MIKU_HOOK_ENTRY_ID,
};

#[derive(Debug, Clone)]
pub struct HookPreviewOutcome {
    pub agent: HookTargetAgent,
    pub settings_path: PathBuf,
    pub file_exists: bool,
    pub already_installed: bool,
    pub idempotent_noop: bool,
    pub backup_count: usize,
    pub latest_backup_path: Option<PathBuf>,
    pub unified_diff: String,
}

/// Prompter used by the Desktop Settings UI ONLY after the user explicitly
/// reviewed the unified diff preview and clicked Confirm.
pub struct NonInteractiveGuiHookPrompter {
    pub confirmed: bool,
}

impl HookConfirmationPrompter for NonInteractiveGuiHookPrompter {
    fn confirm_after_diff(
        &self,
        _unified_diff: &str,
        _backup_path: &Path,
    ) -> Result<bool, String> {
        Ok(self.confirmed)
    }
}

pub fn build_hook_gatekeeper_for_root(config_root: &Path) -> Result<GatekeeperEngine, String> {
    let root_str = config_root.to_string_lossy().replace('\\', "/");
    let profile = ProjectProfile::new("miku-cli-hook", &root_str, 60_000)
        .with_rule(Capability::FsRead, "**", PolicyEffect::Allow, false)
        .with_rule(Capability::FsWrite, "**", PolicyEffect::Allow, false);
    GatekeeperEngine::new(profile).map_err(|e| e.to_string())
}

pub fn discover_all_backups_for_settings(
    authorized_config_root: &Path,
    settings_path: &Path,
) -> Result<Vec<PathBuf>, HookInstallError> {
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
    Ok(candidates)
}

fn confine_json_target(
    authorized_config_root: &Path,
    settings_path: &Path,
) -> Result<(PathBuf, String), HookInstallError> {
    let root_str = authorized_config_root.to_string_lossy().replace('\\', "/");
    let jail = CanonicalPathJail::new(&root_str)
        .map_err(|e| HookInstallError::PathEscapeOrInvalid(e.to_string()))?;
    let target_str = settings_path.to_string_lossy().replace('\\', "/");
    let canonical = jail
        .resolve_and_confine(&target_str)
        .map_err(|e| HookInstallError::PathEscapeOrInvalid(e.to_string()))?;
    if !canonical.to_ascii_lowercase().ends_with(".json") {
        return Err(HookInstallError::PathEscapeOrInvalid(format!(
            "target path '{}' must end with '.json'",
            canonical
        )));
    }
    let buf = PathBuf::from(&canonical);
    if let Ok(meta) = fs::symlink_metadata(&buf) {
        if meta.file_type().is_symlink() {
            return Err(HookInstallError::SymlinkForbidden(buf.display().to_string()));
        }
    }
    let rel = jail.relative_to_root(&canonical);
    Ok((buf, rel))
}

fn evaluate_fs(
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

fn json_contains_miku_entry(root: &serde_json::Map<String, serde_json::Value>) -> bool {
    let has_coucou_style_group = root
        .get(MIKU_HOOK_ENTRY_ID)
        .and_then(|v| v.as_object())
        .map(|o| !o.is_empty())
        .unwrap_or(false);
    let has_mcp = root
        .get("mcpServers")
        .and_then(|v| v.as_object())
        .map(|o| o.contains_key(MIKU_HOOK_ENTRY_ID))
        .unwrap_or(false);
    let has_hook = root
        .get("hooks")
        .and_then(|v| v.as_object())
        .map(|o| {
            o.values().any(|arr| {
                arr.as_array()
                    .map(|items| {
                        items.iter().any(|item| {
                            item.get("id").and_then(|id| id.as_str()) == Some(MIKU_HOOK_ENTRY_ID)
                        })
                    })
                    .unwrap_or(false)
            })
        })
        .unwrap_or(false);
    has_coucou_style_group || has_mcp || has_hook
}

/// Computes a read-only dry-run unified diff for installing Miku's hook/MCP entry
/// without modifying any file or creating any backup.
pub fn preview_miku_hook(
    authorized_config_root: &Path,
    settings_path: &Path,
    agent: HookTargetAgent,
    gatekeeper: &mut GatekeeperEngine,
    now_ms: u64,
) -> Result<HookPreviewOutcome, HookInstallError> {
    let (confined_settings, rel_settings) =
        confine_json_target(authorized_config_root, settings_path)?;
    evaluate_fs(gatekeeper, Capability::FsRead, &rel_settings, now_ms)?;

    let (file_exists, original_text) = match fs::symlink_metadata(&confined_settings) {
        Ok(meta) => {
            if meta.file_type().is_symlink() {
                return Err(HookInstallError::SymlinkForbidden(
                    confined_settings.display().to_string(),
                ));
            }
            let bytes = fs::read(&confined_settings).map_err(|e| {
                HookInstallError::IoError(format!(
                    "failed to read {}: {}",
                    confined_settings.display(),
                    e
                ))
            })?;
            let txt =
                std::str::from_utf8(&bytes).map_err(|_| HookInstallError::InvalidJsonSyntax {
                    line: 1,
                    column: 1,
                    reason: "config JSON is not valid UTF-8".to_string(),
                })?;
            (true, txt.to_string())
        }
        Err(e) if e.kind() == io::ErrorKind::NotFound => (false, "{}\n".to_string()),
        Err(e) => {
            return Err(HookInstallError::IoError(format!(
                "failed to inspect config JSON: {}",
                e
            )));
        }
    };

    let root_map = validate_and_parse_settings_json(&original_text)?;
    let already_installed = file_exists && json_contains_miku_entry(&root_map);
    let (merged_text, idempotent_noop) =
        merge_miku_hook_settings_from_map(root_map, &original_text, agent)?;

    let file_label = confined_settings
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or("settings.json");
    let unified_diff = compute_unified_diff(
        &original_text,
        &merged_text,
        &format!("a/{}", file_label),
        &format!("b/{}", file_label),
    );
    let mut backups =
        discover_all_backups_for_settings(authorized_config_root, &confined_settings)
            .unwrap_or_default();
    let backup_count = backups.len();
    let latest_backup_path = backups.pop();

    Ok(HookPreviewOutcome {
        agent,
        settings_path: confined_settings,
        file_exists,
        already_installed,
        idempotent_noop,
        backup_count,
        latest_backup_path,
        unified_diff,
    })
}

/// Initializes an empty `{}` JSON file inside an already-existing confined harness directory
/// ONLY after explicit user confirmation when the config file does not exist yet.
pub fn seed_empty_settings_if_missing(
    authorized_config_root: &Path,
    settings_path: &Path,
    gatekeeper: &mut GatekeeperEngine,
    now_ms: u64,
) -> Result<bool, HookInstallError> {
    let (confined_settings, rel_settings) =
        confine_json_target(authorized_config_root, settings_path)?;
    evaluate_fs(gatekeeper, Capability::FsWrite, &rel_settings, now_ms)?;
    if confined_settings.exists() {
        return Ok(false);
    }
    let mut f = fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&confined_settings)
        .map_err(|e| HookInstallError::IoError(format!("failed to initialize config JSON: {}", e)))?;
    f.write_all(b"{}\n")
        .and_then(|_| f.sync_all())
        .map_err(|e| HookInstallError::IoError(format!("failed to flush config JSON: {}", e)))?;
    Ok(true)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::hook_cmd::{install_miku_hook, restore_miku_hook};

    #[test]
    fn antigravity_preview_seed_install_and_restore_preserves_foreign_mcps() {
        let root = std::env::temp_dir().join(format!("miku_antigravity_hook_{}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(&root).unwrap();
        let cfg_path = root.join("mcp_config.json");
        let mut gk = build_hook_gatekeeper_for_root(&root).unwrap();

        // 1. Dry-run preview works cleanly before mcp_config.json even exists on disk.
        let prev = preview_miku_hook(&root, &cfg_path, HookTargetAgent::Antigravity, &mut gk, 1_000)
            .expect("preview missing file");
        assert!(!prev.file_exists);
        assert!(!prev.already_installed);
        assert!(!prev.idempotent_noop);
        assert!(prev.unified_diff.contains("miku-orchestrator"));
        assert!(!cfg_path.exists());

        // 2. Write a pre-existing foreign MCP server to verify non-destructive merge + restore.
        let foreign = "{\n  \"mcpServers\": {\n    \"custom-db\": {\n      \"command\": \"uvx\",\n      \"args\": [\"mcp-db\"]\n    }\n  }\n}\n";
        fs::write(&cfg_path, foreign).unwrap();

        let prompter = NonInteractiveGuiHookPrompter { confirmed: true };
        let inst = install_miku_hook(
            &root,
            &cfg_path,
            HookTargetAgent::Antigravity,
            None,
            &prompter,
            &mut gk,
            2_000,
        )
        .expect("install antigravity mcp");
        assert!(inst.applied);
        assert!(inst.backup_path.is_file());

        let after_install = fs::read_to_string(&cfg_path).unwrap();
        assert!(after_install.contains("\"custom-db\""));
        assert!(after_install.contains(MIKU_HOOK_ENTRY_ID));

        let restored = restore_miku_hook(&root, &cfg_path, None, &mut gk, 3_000)
            .expect("restore backup");
        assert!(restored.restored_bytes > 0);
        let after_restore = fs::read_to_string(&cfg_path).unwrap();
        assert!(after_restore.contains("\"custom-db\""));
        assert!(!after_restore.contains(MIKU_HOOK_ENTRY_ID));

        let _ = fs::remove_dir_all(&root);
    }
}
