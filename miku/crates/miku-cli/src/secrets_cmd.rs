use std::io::IsTerminal;

#[cfg(any(test, not(target_os = "windows")))]
use miku_agent::secrets::EncryptedMemoryBackend;
use miku_agent::secrets::{SecretManager, SecretStoreBackend, SecretSummary, Zeroizing};
#[cfg(target_os = "windows")]
use miku_agent::secrets::WindowsCredentialBackend;
use miku_gatekeeper::{Capability, GatekeeperEngine, PolicyEffect, ProjectProfile};

fn build_cli_gatekeeper() -> GatekeeperEngine {
    let profile = ProjectProfile::new("miku-cli-secrets", "C:/Users/cmiranda/dev/miku", 60_000)
        .with_rule(Capability::Network, "*", PolicyEffect::Allow, false);
    GatekeeperEngine::new(profile).expect("GatekeeperEngine must build")
}

fn build_default_backend() -> Box<dyn SecretStoreBackend> {
    #[cfg(target_os = "windows")]
    {
        Box::new(WindowsCredentialBackend::new())
    }
    #[cfg(not(target_os = "windows"))]
    {
        Box::new(EncryptedMemoryBackend::new())
    }
}

/// Abstract prompter trait to allow test simulation of terminal secret input.
pub trait SecretPrompter {
    fn is_terminal(&self) -> bool;
    fn prompt_secret(&self, prompt_text: &str) -> Result<Zeroizing<Vec<u8>>, String>;
}

/// Default prompter reading hidden input from an interactive terminal.
#[derive(Debug, Default)]
pub struct TerminalSecretPrompter;

impl SecretPrompter for TerminalSecretPrompter {
    fn is_terminal(&self) -> bool {
        std::io::stdin().is_terminal()
    }

    fn prompt_secret(&self, prompt_text: &str) -> Result<Zeroizing<Vec<u8>>, String> {
        if !self.is_terminal() {
            return Err(
                "secret set requires an interactive terminal for secure non-echo input; secrets cannot be provided via non-interactive pipes or command arguments"
                    .to_string(),
            );
        }

        #[cfg(target_os = "windows")]
        {
            read_password_windows(prompt_text)
        }
        #[cfg(not(target_os = "windows"))]
        {
            read_password_unix(prompt_text)
        }
    }
}

pub const CONSOLE_ENABLE_ECHO_INPUT: u32 = 0x0004;

/// Low-level console operations for testing and enforcing fail-closed non-echo input.
pub trait ConsoleDriver {
    fn get_console_mode(&self) -> Result<u32, String>;
    fn set_console_mode(&self, mode: u32) -> Result<(), String>;
    fn read_line_raw(&self, buf: &mut String) -> Result<usize, String>;
}

struct ZeroizingLineBuffer(String);

impl Drop for ZeroizingLineBuffer {
    fn drop(&mut self) {
        unsafe {
            let bytes = self.0.as_mut_vec();
            for b in bytes.iter_mut() {
                std::ptr::write_volatile(b, 0);
            }
            std::sync::atomic::compiler_fence(std::sync::atomic::Ordering::SeqCst);
            bytes.clear();
        }
    }
}

pub fn capture_secret_with_driver<D: ConsoleDriver>(
    driver: &D,
    _prompt: &str,
) -> Result<Zeroizing<Vec<u8>>, String> {
    let original_mode = driver.get_console_mode()?;
    let no_echo_mode = original_mode & !CONSOLE_ENABLE_ECHO_INPUT;

    // Must fail-closed BEFORE reading if echo cannot be disabled
    driver.set_console_mode(no_echo_mode)?;

    struct ConsoleModeGuard<'a, D: ConsoleDriver> {
        driver: &'a D,
        mode: u32,
    }
    impl<'a, D: ConsoleDriver> Drop for ConsoleModeGuard<'a, D> {
        fn drop(&mut self) {
            let _ = self.driver.set_console_mode(self.mode);
        }
    }
    let _guard = ConsoleModeGuard {
        driver,
        mode: original_mode,
    };

    if (driver.get_console_mode()? & CONSOLE_ENABLE_ECHO_INPUT) != 0 {
        return Err(
            "console echo remained active after set_console_mode; refusing to read secret"
                .to_string(),
        );
    }

    let mut line = ZeroizingLineBuffer(String::new());
    driver.read_line_raw(&mut line.0)?;

    let trimmed = line.0.trim_end_matches(['\r', '\n']);
    if trimmed.is_empty() {
        return Err("secret cannot be empty".to_string());
    }

    let mut secret_buf = Zeroizing::new(Vec::with_capacity(trimmed.len()));
    secret_buf.extend_from_slice(trimmed.as_bytes());
    Ok(secret_buf)
}

pub fn capture_secret_unix_fallback<D: ConsoleDriver>(
    driver: &D,
    prompt: &str,
) -> Result<Zeroizing<Vec<u8>>, String> {
    // Reject reading if echo cannot be suppressed; never read_line with active echo
    capture_secret_with_driver(driver, prompt)
}

#[cfg(target_os = "windows")]
fn read_password_windows(prompt: &str) -> Result<Zeroizing<Vec<u8>>, String> {
    use std::io::{self, Write};

    const STD_INPUT_HANDLE: u32 = 0xFFFFFFF6;

    #[link(name = "kernel32")]
    extern "system" {
        fn GetStdHandle(nStdHandle: u32) -> *mut std::ffi::c_void;
        fn GetConsoleMode(hConsoleHandle: *mut std::ffi::c_void, lpMode: *mut u32) -> i32;
        fn SetConsoleMode(hConsoleHandle: *mut std::ffi::c_void, dwMode: u32) -> i32;
    }

    struct Win32ConsoleDriver {
        handle: *mut std::ffi::c_void,
    }

    impl ConsoleDriver for Win32ConsoleDriver {
        fn get_console_mode(&self) -> Result<u32, String> {
            let mut mode: u32 = 0;
            if unsafe { GetConsoleMode(self.handle, &mut mode) } == 0 {
                return Err("stdin is not an interactive console; cannot prompt securely".to_string());
            }
            Ok(mode)
        }

        fn set_console_mode(&self, mode: u32) -> Result<(), String> {
            if unsafe { SetConsoleMode(self.handle, mode) } == 0 {
                return Err(
                    "SetConsoleMode failed to disable console echo; aborting secret capture"
                        .to_string(),
                );
            }
            Ok(())
        }

        fn read_line_raw(&self, buf: &mut String) -> Result<usize, String> {
            let res = io::stdin()
                .read_line(buf)
                .map_err(|e| format!("failed to read secret from console: {}", e));
            eprintln!();
            res
        }
    }

    let handle = unsafe { GetStdHandle(STD_INPUT_HANDLE) };
    if handle.is_null() || handle as isize == -1 {
        return Err("invalid console handle; interactive terminal required".to_string());
    }

    eprint!("{}", prompt);
    let _ = io::stderr().flush();

    let driver = Win32ConsoleDriver { handle };
    capture_secret_with_driver(&driver, prompt)
}

#[cfg(not(target_os = "windows"))]
fn read_password_unix(_prompt: &str) -> Result<Zeroizing<Vec<u8>>, String> {
    Err(
        "secure non-echo terminal input is unavailable on this platform; refusing to read secret with active echo"
            .to_string(),
    )
}

/// Executes secret management CLI commands without exposing secret keys:
/// `miku-cli secret list [--json]`
/// `miku-cli secret get <alias>`
/// `miku-cli secret delete <alias>`
/// `miku-cli secret set <alias> --provider <provider> [--model <model>]`
pub fn execute_cli_secret_command(args: &[String]) -> Result<Option<String>, String> {
    let prompter = TerminalSecretPrompter;
    execute_cli_secret_command_with_prompter_and_backend(args, &prompter, None)
}

fn render_summary_table(summaries: &[SecretSummary]) -> String {
    let mut out = String::from(
        "+----------------------+-----------------+------------+\n| ALIAS                | PROVIDER        | STATUS     |\n+----------------------+-----------------+------------+\n",
    );
    if summaries.is_empty() {
        out.push_str("| (no secrets found)   | -               | -          |\n");
    } else {
        for s in summaries {
            out.push_str(&format!("| {:<20} | {:<15} | {:<10} |\n", s.alias, s.provider, s.status));
        }
    }
    out.push_str("+----------------------+-----------------+------------+");
    out
}

/// Internal execution helper supporting custom prompters and test backends.
pub fn execute_cli_secret_command_with_prompter_and_backend(
    args: &[String],
    prompter: &dyn SecretPrompter,
    test_backend: Option<Box<dyn SecretStoreBackend>>,
) -> Result<Option<String>, String> {
    let mut iter = args.iter().peekable();
    let _ = iter.next();

    let mut is_secret_cmd = false;
    let mut remaining = Vec::new();

    for arg in iter {
        if !is_secret_cmd && (arg == "secret" || arg == "secrets") {
            is_secret_cmd = true;
            continue;
        }
        if is_secret_cmd {
            remaining.push(arg.as_str());
        }
    }

    if !is_secret_cmd {
        return Ok(None);
    }

    if remaining.contains(&"--value") {
        return Err(
            "passing secrets via command-line arguments (--value) is strictly prohibited to prevent leakage in process lists, shell history, and logs; enter secret interactively via terminal"
                .to_string(),
        );
    }

    let subcmd = remaining.first().copied().unwrap_or("list");
    let gatekeeper = build_cli_gatekeeper();
    let backend = test_backend.unwrap_or_else(build_default_backend);
    let mut manager = SecretManager::new(backend, "cli-session-token".to_string(), gatekeeper);

    match subcmd {
        "list" => {
            let summaries: Vec<SecretSummary> = manager.list_summaries();
            if remaining.contains(&"--json") {
                serde_json::to_string_pretty(&summaries)
                    .map(Some)
                    .map_err(|e| format!("failed to format JSON: {}", e))
            } else {
                Ok(Some(render_summary_table(&summaries)))
            }
        }
        "get" => {
            let alias = remaining
                .get(1)
                .ok_or_else(|| "missing alias argument: miku secret get <alias>".to_string())?;
            let meta = manager
                .get_metadata(alias)
                .ok_or_else(|| format!("secret alias '{}' not found", alias))?;
            serde_json::to_string_pretty(&SecretSummary {
                alias: meta.alias.clone(),
                provider: meta.provider.clone(),
                status: meta.status.as_str().to_string(),
            })
            .map(Some)
            .map_err(|e| e.to_string())
        }
        "delete" => {
            let alias = remaining
                .get(1)
                .ok_or_else(|| "missing alias argument: miku secret delete <alias>".to_string())?;
            if manager.delete_secret(alias).map_err(|e| e.to_string())? {
                Ok(Some(format!("secret alias '{}' deleted successfully", alias)))
            } else {
                Err(format!("secret alias '{}' not found", alias))
            }
        }
        "set" => {
            let alias = remaining
                .get(1)
                .filter(|a| !a.starts_with("--"))
                .ok_or_else(|| "missing alias: miku secret set <alias> --provider <provider>".to_string())?;

            let mut provider = None;
            let mut model = None;
            let json_mode = remaining.contains(&"--json");

            let mut i = 2;
            while i < remaining.len() {
                match remaining[i] {
                    "--provider" if i + 1 < remaining.len() => {
                        provider = Some(remaining[i + 1]);
                        i += 2;
                        continue;
                    }
                    "--model" if i + 1 < remaining.len() => {
                        model = Some(remaining[i + 1]);
                        i += 2;
                        continue;
                    }
                    _ => {}
                }
                i += 1;
            }

            let prov = provider.unwrap_or("custom");
            let prompt_msg = format!("Enter API secret for alias '{}': ", alias);
            let secret_buf = prompter.prompt_secret(&prompt_msg)?;

            manager
                .set_secret(alias, prov, model, secret_buf.as_slice())
                .map_err(|e| e.to_string())?;

            let summary = SecretSummary {
                alias: alias.to_string(),
                provider: prov.to_string(),
                status: "configured".to_string(),
            };

            if json_mode {
                serde_json::to_string_pretty(&summary)
                    .map(Some)
                    .map_err(|e| format!("failed to format JSON: {}", e))
            } else {
                Ok(Some(render_summary_table(&[summary])))
            }
        }
        other => Err(format!(
            "unknown secret subcommand '{}'. Available: list, get, set, delete",
            other
        )),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicU32, AtomicUsize, Ordering};

    fn to_args(items: &[&str]) -> Vec<String> {
        items.iter().map(|s| (*s).to_string()).collect()
    }

    #[test]
    fn test_cli_secret_subcommand_dispatch_and_zero_secret_leak() {
        let list_args = to_args(&["miku-cli", "secret", "list", "--json"]);
        let out = execute_cli_secret_command(&list_args).expect("execute secret list");
        assert!(!out.unwrap().contains("sk-"));

        let unknown_args = to_args(&["miku-cli", "secret", "invalid_cmd"]);
        assert!(execute_cli_secret_command(&unknown_args).is_err());
    }

    #[test]
    fn test_cli_secret_rejects_value_flag_and_never_leaks_arg_value() {
        let sensitive_val = "SECRET_VALUE_TO_NOT_LEAK_98765";
        let args = to_args(&[
            "miku-cli", "secret", "set", "test-alias", "--provider", "anthropic", "--value",
            sensitive_val,
        ]);
        let res = execute_cli_secret_command(&args);
        assert!(res.is_err(), "Passing --value via CLI arguments must be rejected");
        let err = res.err().unwrap();
        assert!(!err.contains(sensitive_val), "Error output must NEVER echo or leak the secret value");
        assert!(
            err.to_lowercase().contains("prohibited")
                || err.to_lowercase().contains("rejected")
                || err.to_lowercase().contains("interactive"),
            "Error message must clearly explain that passing secrets as arguments is prohibited"
        );
    }

    #[test]
    fn test_cli_secret_set_requires_interactive_terminal_when_non_interactive() {
        let args = to_args(&["miku-cli", "secret", "set", "test-alias", "--provider", "deepseek"]);
        let res = execute_cli_secret_command(&args);
        assert!(res.is_err(), "Calling secret set in non-interactive mode must fail safely");
        assert!(res.err().unwrap().to_lowercase().contains("interactive"));
    }

    #[test]
    fn test_cli_secret_set_interactive_success_renders_only_metadata() {
        struct MockPrompter {
            secret: Vec<u8>,
        }
        impl SecretPrompter for MockPrompter {
            fn is_terminal(&self) -> bool {
                true
            }
            fn prompt_secret(&self, _prompt: &str) -> Result<Zeroizing<Vec<u8>>, String> {
                let mut z = Zeroizing::new(Vec::with_capacity(self.secret.len()));
                z.extend_from_slice(&self.secret);
                Ok(z)
            }
        }

        let sensitive_token = "MOCK_INTERACTIVE_SECRET_KEY_443322";
        let prompter = MockPrompter {
            secret: sensitive_token.as_bytes().to_vec(),
        };
        let args = to_args(&[
            "miku-cli", "secret", "set", "mock-alias", "--provider", "openai", "--json",
        ]);
        let test_backend = Box::new(EncryptedMemoryBackend::new());
        let output = execute_cli_secret_command_with_prompter_and_backend(
            &args,
            &prompter,
            Some(test_backend),
        )
        .expect("Interactive secret set must succeed")
        .expect("Response must be Some");

        assert!(!output.contains(sensitive_token), "CLI response must never leak or echo the entered secret");
        let parsed: SecretSummary = serde_json::from_str(&output).expect("Must parse as SecretSummary");
        assert_eq!(parsed.alias, "mock-alias");
        assert_eq!(parsed.provider, "openai");
        assert_eq!(parsed.status, "configured");
    }

    #[test]
    fn test_console_capture_fails_closed_when_set_console_mode_fails_and_never_reads_secret() {
        struct FailingSetModeDriver {
            mode: AtomicU32,
            fail_set_mode: bool,
            read_calls: AtomicUsize,
            restore_calls: AtomicUsize,
            input: String,
        }

        impl ConsoleDriver for FailingSetModeDriver {
            fn get_console_mode(&self) -> Result<u32, String> {
                Ok(self.mode.load(Ordering::SeqCst))
            }

            fn set_console_mode(&self, mode: u32) -> Result<(), String> {
                if self.fail_set_mode && (mode & CONSOLE_ENABLE_ECHO_INPUT) == 0 {
                    return Err("SetConsoleMode failed with system error 5".to_string());
                }
                if (mode & CONSOLE_ENABLE_ECHO_INPUT) != 0 {
                    self.restore_calls.fetch_add(1, Ordering::SeqCst);
                }
                self.mode.store(mode, Ordering::SeqCst);
                Ok(())
            }

            fn read_line_raw(&self, buf: &mut String) -> Result<usize, String> {
                self.read_calls.fetch_add(1, Ordering::SeqCst);
                buf.push_str(&self.input);
                Ok(self.input.len())
            }
        }

        let failing_driver = FailingSetModeDriver {
            mode: AtomicU32::new(0x0007),
            fail_set_mode: true,
            read_calls: AtomicUsize::new(0),
            restore_calls: AtomicUsize::new(0),
            input: "SUPER_SECRET_NEVER_READ_IF_ECHO_ACTIVE\r\n".to_string(),
        };

        assert!(
            capture_secret_with_driver(&failing_driver, "Enter secret: ").is_err(),
            "capture_secret_with_driver debe fallar cerrado cuando SetConsoleMode falla"
        );
        assert_eq!(
            failing_driver.read_calls.load(Ordering::SeqCst),
            0,
            "NUNCA debe leer la línea de entrada si no se pudo desactivar el eco"
        );

        let working_driver = FailingSetModeDriver {
            mode: AtomicU32::new(0x0007),
            fail_set_mode: false,
            read_calls: AtomicUsize::new(0),
            restore_calls: AtomicUsize::new(0),
            input: "VALID_HIDDEN_SECRET\r\n".to_string(),
        };
        assert!(capture_secret_with_driver(&working_driver, "Enter secret: ").is_ok());
        assert_eq!(working_driver.read_calls.load(Ordering::SeqCst), 1);
        assert_eq!(
            working_driver.mode.load(Ordering::SeqCst),
            0x0007,
            "El modo original de la consola debe restaurarse siempre mediante RAII"
        );
        assert_eq!(working_driver.restore_calls.load(Ordering::SeqCst), 1);
    }

    #[test]
    fn test_unix_capture_without_echo_suppression_fails_closed_and_never_reads_with_active_echo() {
        struct EchoActiveUnixDriver {
            read_calls: AtomicUsize,
        }

        impl ConsoleDriver for EchoActiveUnixDriver {
            fn get_console_mode(&self) -> Result<u32, String> {
                Ok(CONSOLE_ENABLE_ECHO_INPUT)
            }

            fn set_console_mode(&self, _mode: u32) -> Result<(), String> {
                Err("termios echo suppression unavailable".to_string())
            }

            fn read_line_raw(&self, buf: &mut String) -> Result<usize, String> {
                self.read_calls.fetch_add(1, Ordering::SeqCst);
                buf.push_str("LEAKED_UNIX_SECRET\n");
                Ok(19)
            }
        }

        let unix_driver = EchoActiveUnixDriver {
            read_calls: AtomicUsize::new(0),
        };
        assert!(
            capture_secret_unix_fallback(&unix_driver, "Enter secret: ").is_err(),
            "read_password_unix no debe usar read_line con eco activo; debe fallar cerrado si no logra ocultar el eco"
        );
        assert_eq!(
            unix_driver.read_calls.load(Ordering::SeqCst),
            0,
            "read_line_raw no debe invocarse nunca con el eco activo"
        );
    }
}
