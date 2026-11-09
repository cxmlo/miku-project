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

use serde::{Deserialize, Serialize};

pub mod custom_agents_probe;
pub mod doctor_config_and_budgets;
pub mod doctor_probes;
pub mod doctor_runner;
pub mod license_audit;
pub mod secrets_cmd;

pub use custom_agents_probe::*;
pub use doctor_config_and_budgets::*;
pub use doctor_probes::*;
pub use doctor_runner::*;
pub use secrets_cmd::*;

pub const WEBVIEW2_RUNTIME_GUID: &str = "{F3017226-FE2A-4295-8BDF-00C3A9A7E4C5}";
pub const DOCTOR_SCHEMA_VERSION_V0: &str = "miku.doctor.v0";

pub const BUDGET_MAX_PRIVATE_RAM_MB: f64 = 30.0;
pub const BUDGET_MAX_RELEASE_BINARY_MB: f64 = 15.0;
pub const BUDGET_MAX_COLD_START_WS_MS: f64 = 100.0;
pub const BUDGET_MAX_IDLE_CPU_PCT: f64 = 0.0;
pub const BUDGET_MIN_FREE_DISK_MB: f64 = 100.0;

pub const DEFAULT_SAMPLE_MIKU_TOML: &str = r#"
[project]
name = "miku-desktop-orchestrator"
root = "C:/Users/cxmluwu/antigravity/Prompt-Genie-2026-11-05-5baa8"
default_ttl_ms = 30000

[browser]
driver = "playwright"
profile_isolation = true

[allow]
fs_read = ["src/**", "crates/**", "README.md"]
shell = ["cargo test*", "cargo check*"]

[ask]
fs_write = ["src/**", "crates/**"]
shell = ["git push*", "npm publish*"]
browser_domains = ["*.github.com", "localhost"]

[deny]
fs_read = [".env*", "secrets/**"]
fs_write = [".git/**", ".miku.toml"]
process_kill = ["*"]
"#;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "UPPERCASE")]
pub enum ProbeStatus {
    Pass,
    Warn,
    Fail,
}

impl ProbeStatus {
    pub fn as_badge(&self) -> &'static str {
        match self {
            ProbeStatus::Pass => "[PASS]",
            ProbeStatus::Warn => "[WARN]",
            ProbeStatus::Fail => "[FAIL]",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct DiagnosticProbeResult {
    pub probe_id: String,
    pub title: String,
    pub status: ProbeStatus,
    pub latency_ms: f64,
    pub summary: String,
    pub remediation: Option<String>,
}

#[cfg(test)]
mod doctor_tests;
