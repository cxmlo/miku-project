#![allow(
    unknown_lints,
    clippy::too_many_arguments,
    clippy::manual_range_contains,
    clippy::needless_range_loop,
    clippy::large_enum_variant,
    clippy::chunks_exact_to_as_chunks,
    clippy::type_complexity,
    clippy::invisible_characters
)]

use std::collections::BTreeMap;
use std::process::Command;
use std::time::{Duration, Instant};

use miku_core::{build_current_thread_runtime, spawn_orchestrator};
use miku_gateway::{start_gateway_server, GatewayConfig, LoopbackWsClient};
use miku_protocol::{BaseContractBundle, IslandState, MikuLinkMessageV1};
use serde::{Deserialize, Serialize};

pub const HKCU_RUN_SUBKEY: &str = r"HKCU\Software\Microsoft\Windows\CurrentVersion\Run";
pub const MIKU_RUN_VALUE_NAME: &str = "MikuDesktopOrchestrator";
pub const REQUIRED_BACKGROUND_ARGS: &str = "--background --headless";
pub const WIN32_CREATE_NO_WINDOW: u32 = 0x0800_0000;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum PlatformError {
    InvalidExecutablePath(String),
    InvalidRegistryCommand(String),
    RegistryBackendError(String),
    DaemonStartupFailed(String),
}

impl std::fmt::Display for PlatformError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            PlatformError::InvalidExecutablePath(msg) => {
                write!(f, "invalid executable path for Windows Run key: {}", msg)
            }
            PlatformError::InvalidRegistryCommand(msg) => {
                write!(f, "invalid Windows Run registry command: {}", msg)
            }
            PlatformError::RegistryBackendError(msg) => {
                write!(f, "registry backend error: {}", msg)
            }
            PlatformError::DaemonStartupFailed(msg) => {
                write!(f, "headless daemon startup failed: {}", msg)
            }
        }
    }
}

impl std::error::Error for PlatformError {}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum AutostartState {
    Disabled,
    EnabledMatching {
        registered_command: String,
    },
    EnabledDrift {
        registered_command: String,
        expected_command: String,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ParsedRunCommand {
    pub executable_path: String,
    pub background: bool,
    pub headless: bool,
}

/// Normalizes a Windows absolute executable path to canonical backslashes (`\`),
/// validates drive letter + `.exe` suffix + absence of injection/control characters,
/// and wraps it in strict double quotes followed by `--background --headless`.
pub fn build_windows_run_command(raw_exe_path: &str) -> Result<String, PlatformError> {
    let trimmed = raw_exe_path.trim();
    if trimmed.is_empty() {
        return Err(PlatformError::InvalidExecutablePath(
            "executable path must not be empty".to_string(),
        ));
    }

    let forbidden_chars = ['\0', '\r', '\n', '"', '&', '|', ';', '>', '<', '`', '$'];
    for ch in trimmed.chars() {
        if forbidden_chars.contains(&ch) || ch.is_control() {
            return Err(PlatformError::InvalidExecutablePath(format!(
                "forbidden control or shell metacharacter '{}' in path",
                ch.escape_default()
            )));
        }
    }

    let normalized = trimmed.replace('/', r"\");

    if normalized.starts_with(r"\\") {
        return Err(PlatformError::InvalidExecutablePath(
            "UNC network paths are forbidden for localHKCU Run autostart".to_string(),
        ));
    }

    let bytes = normalized.as_bytes();
    let has_drive_prefix = bytes.len() >= 3
        && bytes[0].is_ascii_alphabetic()
        && bytes[1] == b':'
        && bytes[2] == b'\\';
    if !has_drive_prefix {
        return Err(PlatformError::InvalidExecutablePath(format!(
            "path '{}' must be an absolute Windows drive path (e.g. C:\\...)",
            normalized
        )));
    }

    if normalized.contains(r"\..\") || normalized.ends_with(r"\..") {
        return Err(PlatformError::InvalidExecutablePath(
            "relative parent traversal '..' is forbidden in Run executable path".to_string(),
        ));
    }

    if !normalized.to_ascii_lowercase().ends_with(".exe") {
        return Err(PlatformError::InvalidExecutablePath(format!(
            "executable path '{}' must end with .exe",
            normalized
        )));
    }

    Ok(format!("\"{}\" {}", normalized, REQUIRED_BACKGROUND_ARGS))
}

/// Parses and validates a command string stored in `HKCU\...\Run`.
pub fn parse_windows_run_command(raw_cmd: &str) -> Result<ParsedRunCommand, PlatformError> {
    let trimmed = raw_cmd.trim();
    if !trimmed.starts_with('"') {
        return Err(PlatformError::InvalidRegistryCommand(
            "Run command must start with a double-quoted executable path".to_string(),
        ));
    }

    let rest = &trimmed[1..];
    let closing_quote_idx = rest.find('"').ok_or_else(|| {
        PlatformError::InvalidRegistryCommand(
            "Run command is missing closing double quote for executable path".to_string(),
        )
    })?;

    let exe_path = &rest[..closing_quote_idx];
    let args_tail = rest[closing_quote_idx + 1..].trim();

    // Validate the extracted executable path using the same strict rules
    let _ = build_windows_run_command(exe_path)?;

    let mut background = false;
    let mut headless = false;
    for token in args_tail.split_whitespace() {
        match token {
            "--background" => background = true,
            "--headless" => headless = true,
            other => {
                return Err(PlatformError::InvalidRegistryCommand(format!(
                    "unexpected flag '{}' in Run command",
                    other
                )));
            }
        }
    }

    if !background || !headless {
        return Err(PlatformError::InvalidRegistryCommand(
            "Run command must include both --background and --headless flags".to_string(),
        ));
    }

    Ok(ParsedRunCommand {
        executable_path: exe_path.to_string(),
        background,
        headless,
    })
}

pub trait AutostartBackend {
    fn get_run_value(
        &mut self,
        subkey: &str,
        value_name: &str,
    ) -> Result<Option<String>, PlatformError>;

    fn set_run_value(
        &mut self,
        subkey: &str,
        value_name: &str,
        command: &str,
    ) -> Result<(), PlatformError>;

    fn delete_run_value(&mut self, subkey: &str, value_name: &str) -> Result<bool, PlatformError>;
}

/// Deterministic in-memory sandboxed registry backend used in all unit, integration,
/// and benchmark tests so the host machine's real `HKCU\...\Run` is never modified without consent.
#[derive(Debug, Default, Clone)]
pub struct SandboxedRegistryBackend {
    keys: BTreeMap<String, BTreeMap<String, String>>,
    pub reads_count: u64,
    pub writes_count: u64,
    pub deletes_count: u64,
}

impl SandboxedRegistryBackend {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn raw_value(&self, subkey: &str, value_name: &str) -> Option<&str> {
        self.keys
            .get(subkey)
            .and_then(|map| map.get(value_name))
            .map(|s| s.as_str())
    }
}

impl AutostartBackend for SandboxedRegistryBackend {
    fn get_run_value(
        &mut self,
        subkey: &str,
        value_name: &str,
    ) -> Result<Option<String>, PlatformError> {
        self.reads_count += 1;
        Ok(self
            .keys
            .get(subkey)
            .and_then(|m| m.get(value_name))
            .cloned())
    }

    fn set_run_value(
        &mut self,
        subkey: &str,
        value_name: &str,
        command: &str,
    ) -> Result<(), PlatformError> {
        self.writes_count += 1;
        let entry = self.keys.entry(subkey.to_string()).or_default();
        entry.insert(value_name.to_string(), command.to_string());
        Ok(())
    }

    fn delete_run_value(&mut self, subkey: &str, value_name: &str) -> Result<bool, PlatformError> {
        self.deletes_count += 1;
        if let Some(map) = self.keys.get_mut(subkey) {
            Ok(map.remove(value_name).is_some())
        } else {
            Ok(false)
        }
    }
}

pub struct AutostartManager<B: AutostartBackend> {
    backend: B,
    subkey: String,
    value_name: String,
}

impl<B: AutostartBackend> AutostartManager<B> {
    pub fn new(backend: B) -> Self {
        Self {
            backend,
            subkey: HKCU_RUN_SUBKEY.to_string(),
            value_name: MIKU_RUN_VALUE_NAME.to_string(),
        }
    }

    pub fn backend(&self) -> &B {
        &self.backend
    }

    pub fn subkey(&self) -> &str {
        &self.subkey
    }

    pub fn value_name(&self) -> &str {
        &self.value_name
    }

    pub fn enable(&mut self, raw_exe_path: &str) -> Result<String, PlatformError> {
        let cmd = build_windows_run_command(raw_exe_path)?;
        self.backend
            .set_run_value(&self.subkey, &self.value_name, &cmd)?;
        Ok(cmd)
    }

    pub fn disable(&mut self) -> Result<bool, PlatformError> {
        self.backend
            .delete_run_value(&self.subkey, &self.value_name)
    }

    pub fn status(&mut self, expected_raw_exe_path: &str) -> Result<AutostartState, PlatformError> {
        let expected_command = build_windows_run_command(expected_raw_exe_path)?;
        let current = self
            .backend
            .get_run_value(&self.subkey, &self.value_name)?;
        match current {
            None => Ok(AutostartState::Disabled),
            Some(registered_command) => {
                if registered_command == expected_command {
                    Ok(AutostartState::EnabledMatching { registered_command })
                } else {
                    Ok(AutostartState::EnabledDrift {
                        registered_command,
                        expected_command,
                    })
                }
            }
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LaunchConfig {
    pub background: bool,
    pub headless: bool,
    pub worker: bool,
    pub hold_for_ram: bool,
    pub rep: usize,
    pub win32_creation_flags: u32,
}

impl LaunchConfig {
    pub fn from_args<I, S>(args: I) -> Self
    where
        I: IntoIterator<Item = S>,
        S: AsRef<str>,
    {
        let collected: Vec<String> = args.into_iter().map(|s| s.as_ref().to_string()).collect();
        let background = collected.iter().any(|a| a == "--background");
        let headless = collected.iter().any(|a| a == "--headless");
        let worker = collected.iter().any(|a| a == "--worker");
        let hold_for_ram = collected.iter().any(|a| a == "--hold-for-ram");
        let rep = collected
            .iter()
            .position(|a| a == "--rep")
            .and_then(|idx| collected.get(idx + 1))
            .and_then(|v| v.parse::<usize>().ok())
            .unwrap_or(1);

        let win32_creation_flags = if background || headless {
            WIN32_CREATE_NO_WINDOW
        } else {
            0
        };

        Self {
            background,
            headless,
            worker,
            hold_for_ram,
            rep,
            win32_creation_flags,
        }
    }

    pub fn is_silent_background(&self) -> bool {
        self.background && self.headless && self.win32_creation_flags == WIN32_CREATE_NO_WINDOW
    }
}

/// Applies `CREATE_NO_WINDOW` (`0x08000000`) on Windows when spawning a background/headless process
/// so no console window flashes or steals focus on startup.
pub fn configure_headless_command(cmd: &mut Command, launch_cfg: &LaunchConfig) {
    if launch_cfg.background {
        cmd.arg("--background");
    }
    if launch_cfg.headless {
        cmd.arg("--headless");
    }
    #[cfg(target_os = "windows")]
    {
        use std::os::windows::process::CommandExt;
        if launch_cfg.win32_creation_flags != 0 {
            cmd.creation_flags(launch_cfg.win32_creation_flags);
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ColdStartCycleReport {
    pub ws_ready_ms: f64,
    pub total_cycle_ms: f64,
    pub bound_port: u16,
    pub handshake_verified: bool,
    pub ping_pong_verified: bool,
    pub sandboxed_autostart_verified: bool,
    pub residual_active_connections: usize,
}

/// Executes a complete headless startup cycle on `tokio::current_thread`:
/// 1. Verifies sandboxed autostart lifecycle (`HKCU\...\Run` in memory).
/// 2. Spawns `miku-core` + `miku-gateway` on `127.0.0.1:0`.
/// 3. Connects a loopback WebSocket client and completes `hello -> hello_ack` + `ping -> pong`.
/// 4. Shuts down cleanly and verifies `0` residual connections.
pub fn run_headless_daemon_cycle(exe_path_for_sandbox: &str) -> Result<ColdStartCycleReport, PlatformError> {
    let t0 = Instant::now();

    // 1. Verify sandboxed autostart registration & drift detection in memory
    let mut manager = AutostartManager::new(SandboxedRegistryBackend::new());
    let registered = manager.enable(exe_path_for_sandbox)?;
    let parsed = parse_windows_run_command(&registered)?;
    let state_after_enable = manager.status(exe_path_for_sandbox)?;
    let state_matches = matches!(state_after_enable, AutostartState::EnabledMatching { .. });
    let removed = manager.disable()?;
    let state_after_disable = manager.status(exe_path_for_sandbox)?;
    let sandboxed_autostart_verified =
        parsed.background && parsed.headless && state_matches && removed && state_after_disable == AutostartState::Disabled;

    // 2. Initialize single-threaded runtime, miku-core, and miku-gateway
    let rt = build_current_thread_runtime().map_err(PlatformError::DaemonStartupFailed)?;

    rt.block_on(async move {
        let bundle = BaseContractBundle::load_embedded_defaults()
            .map_err(|e| PlatformError::DaemonStartupFailed(e.to_string()))?;
        let session_token = "miku_platform_coldstart_tok_v1".to_string();
        let (orch_handle, orch_join) = spawn_orchestrator(bundle, session_token, 128);

        let (gw_handle, gw_join) =
            start_gateway_server(GatewayConfig::loopback(0), orch_handle.clone())
                .await
                .map_err(|e| PlatformError::DaemonStartupFailed(e.to_string()))?;

        let bound_port = gw_handle.local_addr.port();

        // 3. Connect loopback WS client and verify hello -> hello_ack
        let mut client = LoopbackWsClient::connect(gw_handle.local_addr)
            .await
            .map_err(|e| PlatformError::DaemonStartupFailed(e.to_string()))?;

        let hello = r##"{"v":1,"id":"plat-h-1","msg":{"type":"hello","data":{"client":"miku_autostart_probe","token":"miku_platform_coldstart_tok_v1","capabilities":["agent_events"]}}}"##;
        client
            .send_text(hello)
            .await
            .map_err(|e| PlatformError::DaemonStartupFailed(e.to_string()))?;
        let ack = client
            .recv_envelope()
            .await
            .map_err(|e| PlatformError::DaemonStartupFailed(e.to_string()))?;

        let handshake_verified = ack.reply_to.as_deref() == Some("plat-h-1")
            && matches!(
                ack.msg,
                MikuLinkMessageV1::HelloAck {
                    island_state: IslandState::Idle,
                    ..
                }
            );

        let ws_ready_ms = t0.elapsed().as_secs_f64() * 1_000.0;

        // 4. Verify ping -> pong over WS
        let ping = r##"{"v":1,"id":"plat-p-1","msg":{"type":"ping","data":{"ts":1762400999}}}"##;
        client
            .send_text(ping)
            .await
            .map_err(|e| PlatformError::DaemonStartupFailed(e.to_string()))?;
        let pong = client
            .recv_envelope()
            .await
            .map_err(|e| PlatformError::DaemonStartupFailed(e.to_string()))?;
        let ping_pong_verified = matches!(pong.msg, MikuLinkMessageV1::Pong { ts: 1762400999 });

        client
            .close_cleanly()
            .await
            .map_err(|e| PlatformError::DaemonStartupFailed(e.to_string()))?;

        // Give the server task a brief yield to drop the SessionGuard
        tokio::time::sleep(Duration::from_millis(1)).await;
        let snap = gw_handle.metrics();

        gw_handle.trigger_shutdown();
        let _ = gw_join.await;
        orch_handle.trigger_system_shutdown_signal();
        let _ = orch_join.await;

        let total_cycle_ms = t0.elapsed().as_secs_f64() * 1_000.0;

        Ok(ColdStartCycleReport {
            ws_ready_ms,
            total_cycle_ms,
            bound_port,
            handshake_verified,
            ping_pong_verified,
            sandboxed_autostart_verified,
            residual_active_connections: snap.active_connections,
        })
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    // LEDGER-1.7-1
    #[test]
    fn test_windows_run_command_quoting_escapes_spaces_backslashes_and_rejects_injection() {
        let path_with_spaces = r"C:\Program Files\Miku Desktop Orchestrator\miku.exe";
        let cmd = build_windows_run_command(path_with_spaces).expect("valid Windows path");
        assert_eq!(
            cmd,
            r#""C:\Program Files\Miku Desktop Orchestrator\miku.exe" --background --headless"#
        );

        // Mixed slashes are normalized to canonical Windows backslashes
        let mixed_slashes = "C:/Users/Camila Miranda/AppData/Local/Miku/miku.exe";
        let cmd_mixed = build_windows_run_command(mixed_slashes).expect("mixed slash path");
        assert_eq!(
            cmd_mixed,
            r#""C:\Users\Camila Miranda\AppData\Local\Miku\miku.exe" --background --headless"#
        );

        let parsed = parse_windows_run_command(&cmd_mixed).expect("parse valid Run command");
        assert_eq!(
            parsed.executable_path,
            r"C:\Users\Camila Miranda\AppData\Local\Miku\miku.exe"
        );
        assert!(parsed.background);
        assert!(parsed.headless);

        // Reject relative paths, non-.exe paths, UNC paths, traversal, and command injection
        assert!(build_windows_run_command(r"relative\path\miku.exe").is_err());
        assert!(build_windows_run_command(r"C:\Miku\miku.bat").is_err());
        assert!(build_windows_run_command(r"\\evil-server\share\miku.exe").is_err());
        assert!(build_windows_run_command(r"C:\Miku\..\System32\miku.exe").is_err());
        assert!(build_windows_run_command(r#"C:\Miku\miku.exe" & calc.exe"#).is_err());
        assert!(build_windows_run_command("C:\\Miku\\miku.exe\n--evil").is_err());
    }

    // LEDGER-1.7-2
    #[test]
    fn test_sandboxed_registry_backend_lifecycle_enable_status_disable_idempotency() {
        let mut manager = AutostartManager::new(SandboxedRegistryBackend::new());
        let exe_v1 = r"C:\Users\cmiranda\AppData\Local\Programs\Miku\miku.exe";
        let exe_v2 = r"D:\PortableApps\Miku\miku.exe";

        assert_eq!(manager.subkey(), HKCU_RUN_SUBKEY);
        assert_eq!(manager.value_name(), MIKU_RUN_VALUE_NAME);
        assert_eq!(
            manager.status(exe_v1).expect("initial status"),
            AutostartState::Disabled
        );

        // Enable v1
        let written_cmd = manager.enable(exe_v1).expect("enable v1");
        assert_eq!(
            manager.backend().raw_value(HKCU_RUN_SUBKEY, MIKU_RUN_VALUE_NAME),
            Some(written_cmd.as_str())
        );
        assert_eq!(
            manager.status(exe_v1).expect("matching status"),
            AutostartState::EnabledMatching {
                registered_command: written_cmd.clone(),
            }
        );

        // Detect drift when querying against a moved binary path (exe_v2)
        let drift_status = manager.status(exe_v2).expect("drift status");
        assert!(matches!(drift_status, AutostartState::EnabledDrift { .. }));

        // Disable and verify idempotency on second disable
        assert!(manager.disable().expect("first disable returns true"));
        assert!(!manager.disable().expect("second disable returns false"));
        assert_eq!(
            manager.status(exe_v1).expect("status after disable"),
            AutostartState::Disabled
        );
        assert_eq!(manager.backend().writes_count, 1);
        assert_eq!(manager.backend().deletes_count, 2);
    }

    // LEDGER-1.7-3
    #[test]
    fn test_cli_launch_mode_parser_enforces_headless_background_flags_and_no_window_creation_flags()
    {
        let silent_cfg = LaunchConfig::from_args([
            "miku-platform.exe",
            "--worker",
            "--background",
            "--headless",
            "--rep",
            "7",
            "--hold-for-ram",
        ]);
        assert!(silent_cfg.background);
        assert!(silent_cfg.headless);
        assert!(silent_cfg.worker);
        assert!(silent_cfg.hold_for_ram);
        assert_eq!(silent_cfg.rep, 7);
        assert_eq!(silent_cfg.win32_creation_flags, WIN32_CREATE_NO_WINDOW);
        assert!(silent_cfg.is_silent_background());

        let interactive_cfg = LaunchConfig::from_args(["miku-platform.exe"]);
        assert!(!interactive_cfg.background);
        assert!(!interactive_cfg.headless);
        assert_eq!(interactive_cfg.win32_creation_flags, 0);
        assert!(!interactive_cfg.is_silent_background());
    }

    // LEDGER-1.7-4
    #[test]
    fn test_cold_start_headless_daemon_reaches_websocket_ready_state_and_shuts_down_cleanly() {
        let report = run_headless_daemon_cycle(r"C:\Program Files\Miku\miku.exe")
            .expect("headless daemon cycle must succeed");
        assert!(report.bound_port > 0);
        assert!(report.handshake_verified);
        assert!(report.ping_pong_verified);
        assert!(report.sandboxed_autostart_verified);
        assert_eq!(report.residual_active_connections, 0);
    }
}
