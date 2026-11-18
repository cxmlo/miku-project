use miku_gatekeeper::Capability;
use serde::{Deserialize, Serialize};

use crate::PlatformError;

pub const DEFAULT_SHELL_TIMEOUT_MS: u64 = 15_000;
pub const MAX_KILL_SWITCH_ABORT_BUDGET_MS: f64 = 20.0;

/// Global immutable destructive shell patterns that CANNOT be overridden by `.miku.toml` `Allow` rules.
pub const IMMUTABLE_SHELL_DENY_PATTERNS: &[&str] = &[
    "rm -rf",
    "rm -fr",
    "del /f",
    "del /s",
    "erase /f",
    "rmdir /s",
    "rd /s",
    "format ",
    "diskpart",
    "bcdedit",
    "cipher /w",
    "reg delete",
    "vssadmin delete shadows",
    "shutdown ",
    "stop-computer",
    "restart-computer",
    "remove-item -recurse -force",
    "remove-item -force -recurse",
    "powershell -enc",
    "powershell -encodedcommand",
    "invoke-expression",
    "iex(",
    "iex ",
    "git reset --hard",
    "git push --force",
    "git push -f",
    "mkfs",
    ":(){:|:&};:",
];

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PcControlAction {
    FsRead {
        path: String,
    },
    FsWrite {
        path: String,
        content: String,
        create_dirs: bool,
    },
    ShellCommand {
        program: String,
        args: Vec<String>,
        cwd: Option<String>,
        timeout_ms: u64,
    },
    ClipboardRead,
    ClipboardWrite {
        text: String,
    },
    UiAutomationInvoke {
        window_title_pattern: String,
        automation_id_or_name: String,
        control_type: String,
    },
    UiAutomationFallbackPixelClick {
        window_title_pattern: String,
        x: i32,
        y: i32,
        ui_tree_unavailable_reason: String,
    },
    InputKeyCombo {
        keys: Vec<String>,
    },
    InputTypeText {
        text: String,
    },
    InputMouseClick {
        x: i32,
        y: i32,
    },
}

impl PcControlAction {
    pub fn capability(&self) -> Capability {
        match self {
            PcControlAction::FsRead { .. } => Capability::FsRead,
            PcControlAction::FsWrite { .. } => Capability::FsWrite,
            PcControlAction::ShellCommand { .. } => Capability::Shell,
            PcControlAction::ClipboardRead | PcControlAction::ClipboardWrite { .. } => {
                Capability::Clipboard
            }
            PcControlAction::UiAutomationInvoke { .. }
            | PcControlAction::UiAutomationFallbackPixelClick { .. } => Capability::UiAutomation,
            PcControlAction::InputKeyCombo { .. }
            | PcControlAction::InputTypeText { .. }
            | PcControlAction::InputMouseClick { .. } => Capability::Input,
        }
    }

    pub fn raw_target_and_args(&self) -> (String, String) {
        match self {
            PcControlAction::FsRead { path } => (path.clone(), "fs_read".to_string()),
            PcControlAction::FsWrite { path, content, .. } => {
                (path.clone(), format!("fs_write|bytes={}", content.len()))
            }
            PcControlAction::ShellCommand { program, args, .. } => {
                let full_cmd = if args.is_empty() {
                    program.clone()
                } else {
                    format!("{} {}", program, args.join(" "))
                };
                (full_cmd.clone(), full_cmd)
            }
            PcControlAction::ClipboardRead => {
                ("clipboard:read".to_string(), "clipboard:read".to_string())
            }
            PcControlAction::ClipboardWrite { text } => (
                "clipboard:write".to_string(),
                format!("clipboard:write|bytes={}", text.len()),
            ),
            PcControlAction::UiAutomationInvoke {
                window_title_pattern,
                automation_id_or_name,
                control_type,
            } => (
                format!("uia:{}:{}", window_title_pattern, automation_id_or_name),
                format!("uia_invoke|type={}", control_type),
            ),
            PcControlAction::UiAutomationFallbackPixelClick {
                window_title_pattern,
                x,
                y,
                ui_tree_unavailable_reason,
            } => (
                format!("uia_pixel_fallback:{}:{},{}", window_title_pattern, x, y),
                format!("reason={}", ui_tree_unavailable_reason),
            ),
            PcControlAction::InputKeyCombo { keys } => {
                let combo = keys.join("+");
                (format!("input:keys:{}", combo), combo)
            }
            PcControlAction::InputTypeText { text } => (
                "input:type_text".to_string(),
                format!("input:type_text|bytes={}", text.len()),
            ),
            PcControlAction::InputMouseClick { x, y } => (
                format!("input:mouse:{},{}", x, y),
                format!("click@{},{}", x, y),
            ),
        }
    }

    pub fn validate(&self) -> Result<(), PlatformError> {
        match self {
            PcControlAction::FsRead { path } | PcControlAction::FsWrite { path, .. } => {
                if path.trim().is_empty() {
                    return Err(PlatformError::RegistryBackendError(
                        "filesystem target path must not be empty".to_string(),
                    ));
                }
                Ok(())
            }
            PcControlAction::ShellCommand {
                program,
                timeout_ms,
                ..
            } => {
                if program.trim().is_empty() || program.contains('\0') {
                    return Err(PlatformError::RegistryBackendError(
                        "shell program must be a valid non-empty executable".to_string(),
                    ));
                }
                if *timeout_ms == 0 || *timeout_ms > 300_000 {
                    return Err(PlatformError::RegistryBackendError(
                        "shell timeout_ms must be between 1 and 300000".to_string(),
                    ));
                }
                Ok(())
            }
            PcControlAction::ClipboardRead | PcControlAction::ClipboardWrite { .. } => Ok(()),
            PcControlAction::UiAutomationInvoke {
                window_title_pattern,
                automation_id_or_name,
                control_type,
            } => {
                if window_title_pattern.trim().is_empty()
                    || automation_id_or_name.trim().is_empty()
                    || control_type.trim().is_empty()
                {
                    return Err(PlatformError::RegistryBackendError(
                        "UI Automation action requires non-empty window, element and control_type"
                            .to_string(),
                    ));
                }
                Ok(())
            }
            PcControlAction::UiAutomationFallbackPixelClick {
                window_title_pattern,
                ui_tree_unavailable_reason,
                ..
            } => {
                if window_title_pattern.trim().is_empty() {
                    return Err(PlatformError::RegistryBackendError(
                        "pixel fallback requires non-empty window title".to_string(),
                    ));
                }
                if ui_tree_unavailable_reason.trim().len() < 8 {
                    return Err(PlatformError::RegistryBackendError(
                        "UI Automation must take precedence: pixel fallback requires verified ui_tree_unavailable_reason"
                            .to_string(),
                    ));
                }
                Ok(())
            }
            PcControlAction::InputKeyCombo { keys } => {
                if keys.is_empty() {
                    return Err(PlatformError::RegistryBackendError(
                        "input key combo must contain at least one key".to_string(),
                    ));
                }
                Ok(())
            }
            PcControlAction::InputTypeText { text } => {
                if text.is_empty() || text.len() > 8192 {
                    return Err(PlatformError::RegistryBackendError(
                        "input text length must be 1..=8192 bytes".to_string(),
                    ));
                }
                Ok(())
            }
            PcControlAction::InputMouseClick { .. } => Ok(()),
        }
    }
}

/// Checks whether a shell invocation matches Miku's immutable destructive command denylist.
pub fn matches_immutable_shell_denylist(program: &str, args: &[String]) -> Option<&'static str> {
    let full = if args.is_empty() {
        program.trim().to_ascii_lowercase()
    } else {
        format!("{} {}", program.trim(), args.join(" ")).to_ascii_lowercase()
    };
    let collapsed = full.split_whitespace().collect::<Vec<_>>().join(" ");
    IMMUTABLE_SHELL_DENY_PATTERNS
        .iter()
        .copied()
        .find(|pattern| collapsed.contains(pattern) || full.contains(pattern))
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PcActionExecutionResult {
    pub capability: String,
    pub used_ui_automation_tree: bool,
    pub canonical_target: String,
    pub stdout_or_value: String,
    pub stderr: String,
    pub exit_code: Option<i32>,
    pub bytes_written: usize,
}
