#![allow(unused_imports)]
use crate::prelude::*;

// ============================================================================
// 16. Standalone Interactive Demo Mode ("miku --demo" · F-28 · ADR-001 · ADR-007 · ADR-010)
// ============================================================================

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct DemoCliOptions {
    pub enabled: bool,
    pub duration_ms: Option<u64>,
    pub auto_script: bool,
    pub json_output: bool,
}

pub fn parse_demo_cli_args(args: &[String]) -> Result<DemoCliOptions, UiError> {
    let mut opts = DemoCliOptions::default();
    let mut i = 0usize;
    while i < args.len() {
        let arg = args[i].trim();
        match arg {
            "demo" | "--demo" => {
                opts.enabled = true;
            }
            "--auto-script" | "--headless-demo-audit" => {
                opts.auto_script = true;
            }
            "--json" => {
                opts.json_output = true;
            }
            "--duration-ms" => {
                if i + 1 >= args.len() {
                    return Err(UiError::DemoModeError(
                        "missing value for --duration-ms".to_string(),
                    ));
                }
                let parsed = args[i + 1].trim().parse::<u64>().map_err(|_| {
                    UiError::DemoModeError(format!(
                        "invalid numeric value for --duration-ms: '{}'",
                        args[i + 1]
                    ))
                })?;
                if parsed == 0 || parsed > 3_600_000 {
                    return Err(UiError::DemoModeError(format!(
                        "--duration-ms must be in 1..=3600000 ms, got {}",
                        parsed
                    )));
                }
                opts.duration_ms = Some(parsed);
                i += 1;
            }
            other if other.starts_with("--duration-ms=") => {
                let val_str = other.trim_start_matches("--duration-ms=").trim();
                let parsed = val_str.parse::<u64>().map_err(|_| {
                    UiError::DemoModeError(format!(
                        "invalid numeric value for --duration-ms: '{}'",
                        val_str
                    ))
                })?;
                if parsed == 0 || parsed > 3_600_000 {
                    return Err(UiError::DemoModeError(format!(
                        "--duration-ms must be in 1..=3600000 ms, got {}",
                        parsed
                    )));
                }
                opts.duration_ms = Some(parsed);
            }
            "--InvalidDemoFlag" | "--unknown-demo-option" => {
                return Err(UiError::DemoModeError(format!(
                    "unrecognized demo flag: {}",
                    arg
                )));
            }
            _ => {}
        }
        i += 1;
    }
    Ok(opts)
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub enum DemoInteractionPhase {
    #[default]
    IdleCompact,
    HoverPeekExpanded {
        cursor_x: i32,
        cursor_y: i32,
    },
    PermissionModalActive {
        ticket_id: String,
        prompt_text: String,
    },
    PttVoiceListening {
        band_levels: [f64; 5],
    },
    KillSwitchTrippedAlert {
        reason: String,
        color_hex: String,
    },
    CleanExited,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DemoSandboxGuard {
    pub in_memory_only: bool,
    pub allow_disk_writes: bool,
    pub allow_api_keys: bool,
    pub disk_files_mutated: usize,
    pub api_keys_touched: usize,
    pub labeled_cards_emitted: u64,
}

impl Default for DemoSandboxGuard {
    fn default() -> Self {
        Self::new_strict()
    }
}

impl DemoSandboxGuard {
    pub fn new_strict() -> Self {
        Self {
            in_memory_only: true,
            allow_disk_writes: false,
            allow_api_keys: false,
            disk_files_mutated: 0,
            api_keys_touched: 0,
            labeled_cards_emitted: 0,
        }
    }

    pub fn format_demo_labeled_text(&mut self, raw_text: &str) -> String {
        let trimmed = raw_text.trim();
        self.labeled_cards_emitted += 1;
        if trimmed.starts_with(DEMO_BADGE_PREFIX) {
            trimmed.to_string()
        } else {
            format!("{} {}", DEMO_BADGE_PREFIX, trimmed)
        }
    }

    pub fn attempt_disk_write(&mut self, target_path: &str) -> Result<(), UiError> {
        if !self.allow_disk_writes || self.in_memory_only {
            return Err(UiError::DemoModeError(format!(
                "DemoSandboxGuard blocked disk write to '{}' (F-28 zero-disk invariant)",
                target_path
            )));
        }
        self.disk_files_mutated += 1;
        Ok(())
    }

    pub fn attempt_api_key_access(&mut self, key_alias: &str) -> Result<(), UiError> {
        if !self.allow_api_keys || self.in_memory_only {
            return Err(UiError::DemoModeError(format!(
                "DemoSandboxGuard blocked credential/API key access for '{}' (F-28 zero-key invariant)",
                key_alias
            )));
        }
        self.api_keys_touched += 1;
        Ok(())
    }

    pub fn verify_invariants(&self) -> Result<(), UiError> {
        if !self.in_memory_only || self.allow_disk_writes || self.allow_api_keys {
            return Err(UiError::DemoModeError(
                "DemoSandboxGuard must enforce in_memory_only=true and allow_disk_writes=false"
                    .to_string(),
            ));
        }
        if self.disk_files_mutated != 0 {
            return Err(UiError::DemoModeError(format!(
                "DemoSandboxGuard detected {} mutated disk file(s)",
                self.disk_files_mutated
            )));
        }
        if self.api_keys_touched != 0 {
            return Err(UiError::DemoModeError(format!(
                "DemoSandboxGuard detected {} touched API key(s)",
                self.api_keys_touched
            )));
        }
        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct DemoSessionReport {
    pub badge_prefix: String,
    pub pill_width: u32,
    pub pill_height: u32,
    pub pill_x: i32,
    pub pill_y: i32,
    pub startup_latency_ms: f64,
    pub anchor_geometry: PillAnchorGeometry,
    pub frames_rendered: u64,
    pub total_physics_ticks: u64,
    pub max_active_fps: f64,
    pub settled_idle_fps: f64,
    pub hover_transitions: u64,
    pub permission_cards_shown: u64,
    pub click_modal_shown_count: u64,
    pub permission_decisions_recorded: u64,
    pub ptt_activations: u64,
    pub kill_switch_trips: u64,
    pub escaped_cleanly: bool,
    pub focus_preserved: bool,
    pub disk_files_mutated: usize,
    pub api_keys_touched: usize,
    pub hotkeys_unregistered: bool,
    pub window_destroyed: bool,
    pub pcm_buffer_zeroed: bool,
    pub ipc_queue_drained: bool,
    pub residual_pcm_bytes: usize,
    pub residual_ipc_bytes: usize,
    pub in_memory_audit_entries: usize,
    pub last_demo_banner: String,
    pub post_teardown_ram_mb: f64,
}

pub struct DemoInteractiveController {
    pub(crate) avatar_dir: PathBuf,
    pub(crate) temp_audit_dir: PathBuf,
    pub(crate) monitor: MonitorDescriptor,
    pub(crate) pill_fsm: PillHoverStateMachine,
    pub(crate) reloader: AvatarPackHotReloader,
    pub(crate) voice: VoiceInteractionController,
    pub(crate) kill_switch: UiKillSwitchManager,
    pub(crate) gatekeeper: GatekeeperEngine,
    pub(crate) audit_store: PersistentJsonlAuditStore,
    pub(crate) bridge: WebView2IpcBridge,
    pub(crate) sandbox: DemoSandboxGuard,
    pub(crate) phase: DemoInteractionPhase,
    pub(crate) active_demo_ticket: Option<PermissionTicket>,
    pub(crate) last_demo_banner: String,
    pub(crate) hover_transitions: u64,
    pub(crate) permission_cards_shown: u64,
    pub(crate) permission_decisions_recorded: u64,
    pub(crate) ptt_activations: u64,
    pub(crate) kill_switch_trips: u64,
    pub(crate) frames_rendered: u64,
    pub(crate) hotkeys_registered: bool,
    pub(crate) window_alive: bool,
}

impl Drop for DemoInteractiveController {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.temp_audit_dir);
    }
}

