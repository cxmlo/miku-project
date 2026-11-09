use std::fs;
use std::path::Path;
use std::time::Instant;

use miku_agent::{
    audit_custom_agent_soul, SoulTokenAuditReport, ToolRegistry, CUSTOM_AGENT_MANIFEST_FILENAME,
    RECOMMENDED_MAX_SOUL_TOKENS,
};

use crate::{DiagnosticProbeResult, ProbeStatus};

/// Audits all custom agent directories (`<agents_root>/<agent_id>/agent.toml` + `SOUL.md`)
/// for `miku doctor` (`F-06`):
/// - Returns `ProbeStatus::Fail` if any `agent.toml` or `SOUL.md` is malformed or violates `CanonicalPathJail`.
/// - Returns `ProbeStatus::Warn` if all manifests are valid, but one or more `SOUL.md` files
///   exceed `recommended_max_soul_tokens` (`RECOMMENDED_MAX_SOUL_TOKENS = 1_024` tokens).
/// - Returns `ProbeStatus::Pass` if all custom agents are valid and within the token budget (or if `agents_root` has 0 agents).
pub fn probe_custom_agents_directory(
    agents_root: &Path,
    recommended_max_soul_tokens: u32,
) -> (DiagnosticProbeResult, Vec<SoulTokenAuditReport>) {
    let t0 = Instant::now();
    let threshold = if recommended_max_soul_tokens == 0 {
        RECOMMENDED_MAX_SOUL_TOKENS
    } else {
        recommended_max_soul_tokens
    };

    if !agents_root.exists() {
        let latency_ms = t0.elapsed().as_secs_f64() * 1_000.0;
        return (
            DiagnosticProbeResult {
                probe_id: "custom_agents_soul".to_string(),
                title: "Custom Agents (agent.toml & SOUL.md Token Budget)".to_string(),
                status: ProbeStatus::Pass,
                latency_ms,
                summary: format!(
                    "0 custom agents configured (threshold <= {} SOUL.md tokens)",
                    threshold
                ),
                remediation: None,
            },
            Vec::new(),
        );
    }

    let entries = match fs::read_dir(agents_root) {
        Ok(e) => e,
        Err(err) => {
            let latency_ms = t0.elapsed().as_secs_f64() * 1_000.0;
            return (
                DiagnosticProbeResult {
                    probe_id: "custom_agents_soul".to_string(),
                    title: "Custom Agents (agent.toml & SOUL.md Token Budget)".to_string(),
                    status: ProbeStatus::Fail,
                    latency_ms,
                    summary: format!(
                        "Failed to read custom agents directory '{}': {}",
                        agents_root.display(),
                        err
                    ),
                    remediation: Some(
                        "Verify filesystem permissions on the custom agents directory.".to_string(),
                    ),
                },
                Vec::new(),
            );
        }
    };

    let base_registry = ToolRegistry::with_default_internal_tools();
    let mut reports = Vec::new();
    let mut errors = Vec::new();
    let mut warnings = Vec::new();

    let mut candidate_dirs = Vec::new();
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() && path.join(CUSTOM_AGENT_MANIFEST_FILENAME).exists() {
            candidate_dirs.push(path);
        }
    }
    candidate_dirs.sort();

    for dir in candidate_dirs {
        match audit_custom_agent_soul(&dir, &base_registry, threshold) {
            Ok(report) => {
                if let Some(ref warn_msg) = report.warning_message {
                    warnings.push(warn_msg.clone());
                }
                reports.push(report);
            }
            Err(err) => {
                let folder_name = dir
                    .file_name()
                    .and_then(|n| n.to_str())
                    .unwrap_or("unknown");
                errors.push(format!("{}: {}", folder_name, err));
            }
        }
    }

    let latency_ms = t0.elapsed().as_secs_f64() * 1_000.0;

    if !errors.is_empty() {
        return (
            DiagnosticProbeResult {
                probe_id: "custom_agents_soul".to_string(),
                title: "Custom Agents (agent.toml & SOUL.md Token Budget)".to_string(),
                status: ProbeStatus::Fail,
                latency_ms,
                summary: format!(
                    "{} invalid custom agent manifest(s): {}",
                    errors.len(),
                    errors.join("; ")
                ),
                remediation: Some(
                    "Fix agent.toml schema errors, ensure soul_path stays inside the agent directory, and declare only valid ToolRegistry tools.".to_string(),
                ),
            },
            reports,
        );
    }

    if !warnings.is_empty() {
        return (
            DiagnosticProbeResult {
                probe_id: "custom_agents_soul".to_string(),
                title: "Custom Agents (agent.toml & SOUL.md Token Budget)".to_string(),
                status: ProbeStatus::Warn,
                latency_ms,
                summary: format!(
                    "{}/{} custom agent(s) exceed recommended SOUL.md budget ({} tokens): {}",
                    warnings.len(),
                    reports.len(),
                    threshold,
                    warnings.join("; ")
                ),
                remediation: Some(format!(
                    "Keep each SOUL.md <= {} tokens and move long reference material into miku-memory Markdown notes.",
                    threshold
                )),
            },
            reports,
        );
    }

    (
        DiagnosticProbeResult {
            probe_id: "custom_agents_soul".to_string(),
            title: "Custom Agents (agent.toml & SOUL.md Token Budget)".to_string(),
            status: ProbeStatus::Pass,
            latency_ms,
            summary: format!(
                "{} custom agent(s) validated; all SOUL.md prompts <= {} tokens",
                reports.len(),
                threshold
            ),
            remediation: None,
        },
        reports,
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicU64, Ordering};

    static COUNTER: AtomicU64 = AtomicU64::new(1);

    #[test]
    fn probe_custom_agents_directory_emits_warn_when_soul_exceeds_token_threshold_and_fail_on_invalid_manifest() {
        let id = COUNTER.fetch_add(1, Ordering::SeqCst);
        let root = std::env::temp_dir().join(format!(
            "miku_cli_f06_probe_{}_{}",
            std::process::id(),
            id
        ));
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(&root).expect("create root");

        // 1. Valid compact agent -> ProbeStatus::Pass
        let ok_dir = root.join("agent-compact");
        fs::create_dir_all(&ok_dir).unwrap();
        fs::write(
            ok_dir.join("agent.toml"),
            r#"schema_version = 1
[agent]
id = "agent-compact"
display_name = "Compact"
description = "Compact custom agent"
provider = "deepseek"
model = "deepseek-chat"
soul_path = "SOUL.md"
allowed_tools = ["fs_read"]
credential_aliases = ["alias-compact"]
"#,
        )
        .unwrap();
        fs::write(ok_dir.join("SOUL.md"), "# Compact\nShort persona.").unwrap();

        let (res_pass, reports_pass) =
            probe_custom_agents_directory(&root, RECOMMENDED_MAX_SOUL_TOKENS);
        assert_eq!(res_pass.status, ProbeStatus::Pass);
        assert_eq!(reports_pass.len(), 1);

        // 2. Oversized SOUL.md (> 1024 tokens) -> ProbeStatus::Warn
        let big_dir = root.join("agent-bloated");
        fs::create_dir_all(&big_dir).unwrap();
        fs::write(
            big_dir.join("agent.toml"),
            r#"schema_version = 1
[agent]
id = "agent-bloated"
display_name = "Bloated"
description = "Oversized SOUL.md agent"
provider = "anthropic"
model = "claude-3-7-sonnet"
soul_path = "SOUL.md"
allowed_tools = ["fs_read"]
"#,
        )
        .unwrap();
        let big_soul = format!("# Bloated\n{}", "long_directive_word ".repeat(1_150));
        fs::write(big_dir.join("SOUL.md"), big_soul).unwrap();

        let (res_warn, reports_warn) =
            probe_custom_agents_directory(&root, RECOMMENDED_MAX_SOUL_TOKENS);
        assert_eq!(res_warn.status, ProbeStatus::Warn);
        assert_eq!(reports_warn.len(), 2);
        assert!(res_warn.summary.contains("agent-bloated"));
        assert!(res_warn.remediation.is_some());

        // 3. Invalid agent.toml (path traversal in soul_path) -> ProbeStatus::Fail
        let bad_dir = root.join("agent-broken");
        fs::create_dir_all(&bad_dir).unwrap();
        fs::write(
            bad_dir.join("agent.toml"),
            r#"schema_version = 1
[agent]
id = "agent-broken"
display_name = "Broken"
description = "Escapes folder"
provider = "openai"
model = "gpt-4o"
soul_path = "../outside.md"
allowed_tools = ["fs_read"]
"#,
        )
        .unwrap();

        let (res_fail, _) = probe_custom_agents_directory(&root, RECOMMENDED_MAX_SOUL_TOKENS);
        assert_eq!(res_fail.status, ProbeStatus::Fail);

        let _ = fs::remove_dir_all(&root);
    }
}
