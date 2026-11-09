use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

use miku_gatekeeper::{Capability, DenyReason, GatekeeperEngine, PolicyEffect, ProjectProfile};
use miku_storage::SingleThreadStoragePool;

use crate::catalog::AgentCatalog;
use crate::custom_agents::{
    audit_custom_agent_soul, handle_island_custom_agent_ipc, normalize_os_path_for_agent_jail,
    CustomAgentDescriptor, CustomAgentError, CustomAgentOrchestrator,
    IslandCreateCustomAgentRequest, IslandCustomAgentIpcCommand, IslandCustomAgentIpcEnvelope,
    IslandCustomAgentIpcResponse, CUSTOM_AGENT_MANIFEST_FILENAME, DEFAULT_SOUL_FILENAME,
    RECOMMENDED_MAX_SOUL_TOKENS,
};
use crate::harness::LlmProvider;
use crate::secrets::{EncryptedMemoryBackend, SecretManager};
use crate::ToolRegistry;

static TEMP_COUNTER: AtomicU64 = AtomicU64::new(1);

struct TempDirGuard {
    path: PathBuf,
}

impl TempDirGuard {
    fn new(prefix: &str) -> Self {
        let id = TEMP_COUNTER.fetch_add(1, Ordering::SeqCst);
        let pid = std::process::id();
        let path = std::env::temp_dir().join(format!("miku_f06_{}_{}_{}", prefix, pid, id));
        let _ = fs::remove_dir_all(&path);
        fs::create_dir_all(&path).expect("create temp dir");
        Self { path }
    }
    fn path(&self) -> &Path {
        &self.path
    }
}

impl Drop for TempDirGuard {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.path);
    }
}

fn write_custom_agent_fixture(
    root: &Path,
    agent_id: &str,
    display_name: &str,
    provider: &str,
    model: &str,
    soul_rel: &str,
    allowed_tools: &[&str],
    credential_aliases: &[&str],
    soul_body: &str,
) -> PathBuf {
    let dir = root.join(agent_id);
    fs::create_dir_all(&dir).expect("create agent dir");
    let tools_toml = allowed_tools.iter().map(|t| format!("\"{}\"", t)).collect::<Vec<_>>().join(", ");
    let aliases_toml = credential_aliases.iter().map(|a| format!("\"{}\"", a)).collect::<Vec<_>>().join(", ");
    let toml_text = format!(
        "schema_version = 1\n\n[agent]\nid = \"{}\"\ndisplay_name = \"{}\"\ndescription = \"Custom specialized agent {}\"\nprovider = \"{}\"\nmodel = \"{}\"\nsoul_path = \"{}\"\nallowed_tools = [{}]\ncredential_aliases = [{}]\nmax_steps = 8\nmax_context_tokens = 4096\n",
        agent_id, display_name, agent_id, provider, model, soul_rel, tools_toml, aliases_toml
    );
    fs::write(dir.join(CUSTOM_AGENT_MANIFEST_FILENAME), toml_text).expect("write agent.toml");
    let soul_path = dir.join(soul_rel);
    if let Some(parent) = soul_path.parent() {
        let _ = fs::create_dir_all(parent);
    }
    fs::write(soul_path, soul_body).expect("write SOUL.md");
    dir
}

fn build_test_orchestrator(project_root: &str) -> (CustomAgentOrchestrator, GatekeeperEngine) {
    let profile = ProjectProfile::new("miku-f06-test", project_root, 60_000)
        .with_rule(Capability::FsRead, "src/**", PolicyEffect::Allow, false)
        .with_rule(Capability::FsRead, "secrets/**", PolicyEffect::Deny, true)
        .with_rule(Capability::FsWrite, "agents/**", PolicyEffect::Allow, false)
        .with_rule(Capability::FsWrite, "src/**", PolicyEffect::Ask, false)
        .with_rule(Capability::Network, "*", PolicyEffect::Allow, false);
    let gk_for_secrets = GatekeeperEngine::new(profile.clone()).expect("gk secrets");
    let gk_for_tools = GatekeeperEngine::new(profile).expect("gk tools");

    let secret_manager = SecretManager::new(
        Box::new(EncryptedMemoryBackend::new()),
        "f06-session-token".to_string(),
        gk_for_secrets,
    );
    let storage = SingleThreadStoragePool::new_in_memory().expect("in-memory storage");
    let orchestrator = CustomAgentOrchestrator::new(
        AgentCatalog::with_builtin_agents(),
        ToolRegistry::with_default_internal_tools(),
        storage,
        secret_manager,
    );
    (orchestrator, gk_for_tools)
}

// LEDGER-3.12-1
#[test]
fn test_agent_toml_manifest_validation_rejects_malformed_unknown_fields_duplicates_and_path_traversal_fail_closed() {
    let temp = TempDirGuard::new("manifest_validation");
    let jail_root = normalize_os_path_for_agent_jail(temp.path());
    let (mut orch, _gk) = build_test_orchestrator(&jail_root);

    // 1. Valid agent.toml + SOUL.md loads declaratively with 0 tool executions
    let valid_dir = write_custom_agent_fixture(
        temp.path(), "rust-auditor", "Rust Security Auditor", "anthropic", "claude-3-7-sonnet",
        DEFAULT_SOUL_FILENAME, &["fs_read", "memory_query"], &["alias-auditor"],
        "# Role\nYou audit Rust crates for memory safety and Gatekeeper invariants.",
    );
    let desc = orch.register_agent_from_dir(&valid_dir).expect("valid agent.toml must load");
    assert_eq!(desc.id, "rust-auditor");
    assert_eq!(desc.allowed_tools, vec!["fs_read", "memory_query"]);
    assert_eq!(orch.tool_invocations_executed(), 0);

    // 2. Duplicate agent ID is rejected fail-closed
    let dup_err = orch.register_agent_from_dir(&valid_dir).expect_err("duplicate id must fail");
    assert!(matches!(dup_err, CustomAgentError::DuplicateAgentId(ref id) if id == "rust-auditor"));

    // 3. Built-in ID collision ("claude-code") is rejected fail-closed
    let builtin_dup_dir = write_custom_agent_fixture(
        temp.path(), "claude-code", "Fake Builtin", "anthropic", "claude-3-7-sonnet",
        DEFAULT_SOUL_FILENAME, &["fs_read"], &[], "# Role\nCollision test.",
    );
    assert!(matches!(
        orch.register_agent_from_dir(&builtin_dup_dir),
        Err(CustomAgentError::DuplicateAgentId(ref id)) if id == "claude-code"
    ));

    // 4. Malformed TOML fails closed
    let base_reg = ToolRegistry::with_default_internal_tools();
    assert!(matches!(
        CustomAgentDescriptor::parse_and_validate("[agent\nid = unclosed", &valid_dir, &base_reg),
        Err(CustomAgentError::ManifestParseError(_))
    ));

    // 5. Unknown fields in agent.toml are rejected by #[serde(deny_unknown_fields)]
    let unknown_field_toml = "schema_version = 1\nunexpected_root_key = \"forbidden\"\n[agent]\nid = \"sec-agent\"\ndisplay_name = \"Sec\"\ndescription = \"Desc\"\nprovider = \"deep_seek\"\nmodel = \"deepseek-chat\"\nsoul_path = \"SOUL.md\"\nallowed_tools = [\"fs_read\"]\n";
    assert!(matches!(
        CustomAgentDescriptor::parse_and_validate(unknown_field_toml, &valid_dir, &base_reg),
        Err(CustomAgentError::ManifestParseError(_))
    ));

    // 6. Path traversal in soul_path ("../outside_soul.md") is rejected by CanonicalPathJail
    let traversal_toml = "schema_version = 1\n[agent]\nid = \"escape-agent\"\ndisplay_name = \"Escape\"\ndescription = \"Tries to read SOUL outside agent folder\"\nprovider = \"deep_seek\"\nmodel = \"deepseek-chat\"\nsoul_path = \"../outside_soul.md\"\nallowed_tools = [\"fs_read\"]\n";
    assert!(matches!(
        CustomAgentDescriptor::parse_and_validate(traversal_toml, &valid_dir, &base_reg),
        Err(CustomAgentError::PathJailViolation(_))
    ));

    // 7. Invalid soul_path extension ("SOUL.txt") and missing/empty SOUL.md fail closed
    let bad_ext_toml = traversal_toml.replace("../outside_soul.md", "SOUL.txt");
    assert!(matches!(
        CustomAgentDescriptor::parse_and_validate(&bad_ext_toml, &valid_dir, &base_reg),
        Err(CustomAgentError::InvalidSoulPath(_))
    ));

    let empty_soul_dir = write_custom_agent_fixture(
        temp.path(), "empty-soul", "Empty Soul", "open_ai", "gpt-4o",
        DEFAULT_SOUL_FILENAME, &["fs_read"], &[], "   \n  ",
    );
    assert!(matches!(
        CustomAgentDescriptor::load_from_dir(&empty_soul_dir, &base_reg),
        Err(CustomAgentError::EmptySoulFile(_))
    ));

    // 8. Unknown tool or raw secret key in credential_aliases fails closed
    let unknown_tool_toml = traversal_toml
        .replace("../outside_soul.md", "SOUL.md")
        .replace("\"fs_read\"", "\"non_existent_tool\"");
    assert!(matches!(
        CustomAgentDescriptor::parse_and_validate(&unknown_tool_toml, &valid_dir, &base_reg),
        Err(CustomAgentError::UnknownToolInManifest(_))
    ));

    let raw_key_alias_toml = "schema_version = 1\n[agent]\nid = \"raw-key-agent\"\ndisplay_name = \"Raw Key\"\ndescription = \"Puts sk- secret in alias list\"\nprovider = \"open_ai\"\nmodel = \"gpt-4o\"\nsoul_path = \"SOUL.md\"\nallowed_tools = [\"fs_read\"]\ncredential_aliases = [\"sk-proj-fake-key-value-12345\"]\n";
    assert!(matches!(
        CustomAgentDescriptor::parse_and_validate(raw_key_alias_toml, &valid_dir, &base_reg),
        Err(CustomAgentError::InvalidCredentialAlias(_))
    ));
}

// LEDGER-3.12-2
#[test]
fn test_strict_session_isolation_blocks_agent_a_from_reading_modifying_or_assembling_agent_b_private_history() {
    let temp = TempDirGuard::new("session_isolation");
    let jail_root = normalize_os_path_for_agent_jail(temp.path());
    let (mut orch, _gk) = build_test_orchestrator(&jail_root);

    let dir_a = write_custom_agent_fixture(
        temp.path(), "agent-alpha", "Agent Alpha", "deep_seek", "deepseek-chat",
        DEFAULT_SOUL_FILENAME, &["fs_read"], &["alias-alpha"], "# Alpha Soul\nYou are Agent Alpha.",
    );
    let dir_b = write_custom_agent_fixture(
        temp.path(), "agent-bravo", "Agent Bravo", "anthropic", "claude-3-7-sonnet",
        DEFAULT_SOUL_FILENAME, &["fs_read"], &["alias-bravo"], "# Bravo Soul\nYou are Agent Bravo.",
    );
    orch.register_agent_from_dir(&dir_a).expect("register alpha");
    orch.register_agent_from_dir(&dir_b).expect("register bravo");

    // Agent Bravo creates a private session and records private conversation turns
    orch.create_isolated_session("agent-bravo", "sess-bravo-secret-99", &jail_root, 1_000)
        .expect("create bravo session");
    orch.append_isolated_turn(
        "agent-bravo", "sess-bravo-secret-99", "turn-b-1", 1, "user",
        "CONFIDENTIAL_BRAVO_ARCHITECTURE_NOTE_XYZ", 1_010,
    ).expect("append bravo turn");

    // Agent Alpha creates its own session
    orch.create_isolated_session("agent-alpha", "sess-alpha-01", &jail_root, 1_020)
        .expect("create alpha session");
    orch.append_isolated_turn(
        "agent-alpha", "sess-alpha-01", "turn-a-1", 1, "user", "Public alpha task", 1_030,
    ).expect("append alpha turn");

    // 1. Agent Alpha CANNOT read Agent Bravo's session history even knowing `sess-bravo-secret-99`
    let read_err = orch
        .read_isolated_session_history("agent-alpha", "sess-bravo-secret-99")
        .expect_err("Agent Alpha must be blocked from reading Agent Bravo's session");
    assert!(matches!(
        read_err,
        CustomAgentError::CrossAgentSessionAccessDenied {
            ref requesting_agent, ref session_id, ref owner_agent,
        } if requesting_agent == "agent-alpha" && session_id == "sess-bravo-secret-99" && owner_agent == "agent-bravo"
    ));

    // 2. Agent Alpha CANNOT append turns to Agent Bravo's session
    let write_err = orch.append_isolated_turn(
        "agent-alpha", "sess-bravo-secret-99", "turn-injected", 2, "assistant", "Injected malicious turn", 1_040,
    ).expect_err("Agent Alpha must be blocked from modifying Agent Bravo's session");
    assert!(matches!(write_err, CustomAgentError::CrossAgentSessionAccessDenied { .. }));

    // 3. Agent Alpha CANNOT delete Agent Bravo's session
    let del_err = orch.delete_isolated_session("agent-alpha", "sess-bravo-secret-99")
        .expect_err("Agent Alpha must be blocked from deleting Agent Bravo's session");
    assert!(matches!(del_err, CustomAgentError::CrossAgentSessionAccessDenied { .. }));

    // 4. Agent Alpha CANNOT assemble a prompt against Agent Bravo's session
    let empty_chunks = BTreeMap::new();
    let prompt_err = orch.assemble_isolated_turn_prompt(
        "agent-alpha", "sess-bravo-secret-99", "Summarize history", &[], &empty_chunks, None,
    ).expect_err("Agent Alpha must be blocked from assembling prompt with Bravo's session");
    assert!(matches!(prompt_err, CustomAgentError::CrossAgentSessionAccessDenied { .. }));

    // 5. Verify Bravo's history is untouched and Alpha's prompt only contains Alpha's own turns
    let bravo_turns = orch.read_isolated_session_history("agent-bravo", "sess-bravo-secret-99")
        .expect("Bravo reads own session");
    assert_eq!(bravo_turns.len(), 1);
    assert_eq!(bravo_turns[0].content, "CONFIDENTIAL_BRAVO_ARCHITECTURE_NOTE_XYZ");

    let alpha_prompt = orch.assemble_isolated_turn_prompt(
        "agent-alpha", "sess-alpha-01", "Continue alpha work", &[], &empty_chunks, None,
    ).expect("Alpha assembles own session prompt");
    assert!(alpha_prompt.full_prompt.contains("Public alpha task"));
    assert!(!alpha_prompt.full_prompt.contains("CONFIDENTIAL_BRAVO_ARCHITECTURE_NOTE_XYZ"));
}

// LEDGER-3.12-3
#[test]
fn test_strict_credential_alias_isolation_blocks_cross_agent_secret_access_with_zero_secret_leak() {
    let temp = TempDirGuard::new("credential_isolation");
    let jail_root = normalize_os_path_for_agent_jail(temp.path());
    let (mut orch, _gk) = build_test_orchestrator(&jail_root);

    let dummy_secret_a = b"DUMMY_PORTABLE_CREDENTIAL_ALPHA_111";
    let dummy_secret_b = b"DUMMY_PORTABLE_CREDENTIAL_BRAVO_222";

    orch.secret_manager_mut()
        .set_secret("alias-only-alpha", "deep_seek", Some("deepseek-chat"), dummy_secret_a)
        .expect("set dummy secret A");
    orch.secret_manager_mut()
        .set_secret("alias-only-bravo", "anthropic", Some("claude-3-7-sonnet"), dummy_secret_b)
        .expect("set dummy secret B");

    let dir_a = write_custom_agent_fixture(
        temp.path(), "agent-alpha", "Agent Alpha", "deep_seek", "deepseek-chat",
        DEFAULT_SOUL_FILENAME, &["fs_read"], &["alias-only-alpha"], "# Alpha Soul\nAlpha directives.",
    );
    let dir_b = write_custom_agent_fixture(
        temp.path(), "agent-bravo", "Agent Bravo", "anthropic", "claude-3-7-sonnet",
        DEFAULT_SOUL_FILENAME, &["fs_read"], &["alias-only-bravo"], "# Bravo Soul\nBravo directives.",
    );
    orch.register_agent_from_dir(&dir_a).expect("register alpha");
    orch.register_agent_from_dir(&dir_b).expect("register bravo");

    // 1. Agent Alpha resolves its own assigned alias
    let resolved_a = orch
        .resolve_agent_credential_authorized("agent-alpha", "alias-only-alpha", "f06-session-token")
        .expect("Alpha resolves its own alias");
    assert_eq!(resolved_a.as_slice(), dummy_secret_a);
    let reads_after_valid = orch.secret_manager().authorized_read_attempts();
    assert_eq!(reads_after_valid, 1);

    // 2. Agent Alpha CANNOT resolve Agent Bravo's exclusive alias ("alias-only-bravo")
    let cross_err = orch
        .resolve_agent_credential_authorized("agent-alpha", "alias-only-bravo", "f06-session-token")
        .expect_err("Alpha must not read Bravo's credential alias");
    assert!(matches!(
        cross_err,
        CustomAgentError::UnauthorizedCredentialAlias { ref requesting_agent, ref alias }
        if requesting_agent == "agent-alpha" && alias == "alias-only-bravo"
    ));
    assert_eq!(orch.secret_manager().authorized_read_attempts(), reads_after_valid);
    assert!(!cross_err.to_string().contains("DUMMY_PORTABLE_CREDENTIAL_BRAVO_222"));

    // 3. Another agent attempting to register Bravo's exclusive alias in its manifest is rejected
    let dir_c = write_custom_agent_fixture(
        temp.path(), "agent-charlie", "Agent Charlie", "anthropic", "claude-3-7-sonnet",
        DEFAULT_SOUL_FILENAME, &["fs_read"], &["alias-only-bravo"],
        "# Charlie Soul\nAttempts to claim Bravo's alias.",
    );
    let claim_err = orch.register_agent_from_dir(&dir_c)
        .expect_err("claiming another agent's exclusive alias must fail");
    assert!(matches!(
        claim_err,
        CustomAgentError::CredentialAliasOwnedByAnotherAgent {
            ref alias, ref requesting_agent, ref owner_agent,
        } if alias == "alias-only-bravo" && requesting_agent == "agent-charlie" && owner_agent == "agent-bravo"
    ));
}

// LEDGER-3.12-4
#[test]
fn test_scoped_tools_and_gatekeeper_enforcement_require_both_manifest_declaration_and_gatekeeper_policy() {
    let temp = TempDirGuard::new("tools_gatekeeper");
    let jail_root = normalize_os_path_for_agent_jail(temp.path());
    let (mut orch, mut gk) = build_test_orchestrator(&jail_root);

    // Agent declares ONLY `fs_read` and `fs_write` (does NOT declare `shell_exec`)
    let dir_ro = write_custom_agent_fixture(
        temp.path(), "scoped-coder", "Scoped Coder", "deep_seek", "deepseek-coder",
        DEFAULT_SOUL_FILENAME, &["fs_read", "fs_write"], &[],
        "# Scoped Coder\nOnly reads and writes confined source files.",
    );
    orch.register_agent_from_dir(&dir_ro).expect("register scoped-coder");
    orch.create_isolated_session("scoped-coder", "sess-scoped-1", &jail_root, 2_000)
        .expect("create session");

    // 1. Undeclared tool (`shell_exec`) is rejected BEFORE Gatekeeper even if registered globally
    let undeclared_err = orch.invoke_agent_tool_authorized(
        &mut gk, "scoped-coder", "sess-scoped-1", "shell_exec", "cargo test", "{}", 2_010,
    ).expect_err("undeclared tool must be blocked");
    assert!(matches!(
        undeclared_err,
        CustomAgentError::UndeclaredToolForAgent { ref agent_id, ref tool_name }
        if agent_id == "scoped-coder" && tool_name == "shell_exec"
    ));
    assert_eq!(orch.tool_invocations_executed(), 0);

    // 2. Declared tool (`fs_read`) on a Gatekeeper-denied path (`secrets/keys.txt`) is blocked by Gatekeeper
    let gk_deny_err = orch.invoke_agent_tool_authorized(
        &mut gk, "scoped-coder", "sess-scoped-1", "fs_read", "secrets/keys.txt", "{}", 2_020,
    ).expect_err("Gatekeeper deny rule must block declared tool");
    assert!(matches!(
        gk_deny_err,
        CustomAgentError::GatekeeperDenied { reason: DenyReason::ExplicitDenyRule, .. }
    ));
    assert_eq!(orch.tool_invocations_executed(), 0);

    // 3. Declared tool (`fs_write`) on `src/lib.rs` requires Gatekeeper `Ask` confirmation
    let gk_ask_err = orch.invoke_agent_tool_authorized(
        &mut gk, "scoped-coder", "sess-scoped-1", "fs_write", "src/lib.rs", "{}", 2_030,
    ).expect_err("Gatekeeper Ask rule must require confirmation");
    assert!(matches!(gk_ask_err, CustomAgentError::GatekeeperAskRequired { .. }));
    assert_eq!(orch.tool_invocations_executed(), 0);

    // 4. Declared tool (`fs_read`) on Gatekeeper-allowed path (`src/lib.rs`) succeeds and audits
    let allowed_out = orch.invoke_agent_tool_authorized(
        &mut gk, "scoped-coder", "sess-scoped-1", "fs_read", "src/lib.rs", "{}", 2_040,
    ).expect("declared + Gatekeeper-allowed tool must succeed");
    assert_eq!(allowed_out.tool_name, "fs_read");
    assert_eq!(orch.tool_invocations_executed(), 1);
}

// LEDGER-3.12-5
#[test]
fn test_lazy_agent_loading_keeps_uninvoked_agents_at_zero_context_ram_until_first_invocation() {
    let temp = TempDirGuard::new("lazy_loading");
    let jail_root = normalize_os_path_for_agent_jail(temp.path());
    let (mut orch, mut gk) = build_test_orchestrator(&jail_root);

    for id in ["lazy-one", "lazy-two", "lazy-three"] {
        let dir = write_custom_agent_fixture(
            temp.path(), id, &format!("Agent {}", id), "deep_seek", "deepseek-chat",
            DEFAULT_SOUL_FILENAME, &["fs_read"], &[],
            &format!("# Soul for {}\nDetailed persona instructions for {}.", id, id),
        );
        orch.register_agent_from_dir(&dir).expect("register lazy agent");
    }

    // Listing and selecting an agent must NOT load any SOUL.md or StaticSystemPrefix into RAM
    let summaries = orch.list_island_summaries();
    assert_eq!(summaries.len(), 3);
    assert!(summaries.iter().all(|s| !s.context_loaded_in_ram));
    assert_eq!(orch.loaded_agent_count(), 0);

    orch.select_agent("lazy-two").expect("select lazy-two");
    assert_eq!(orch.selected_agent_id(), Some("lazy-two"));
    assert!(!orch.is_agent_context_loaded("lazy-two"));
    assert_eq!(orch.loaded_soul_bytes("lazy-two"), 0);
    assert_eq!(orch.loaded_agent_count(), 0);

    // Invoking `lazy-two` loads ONLY `lazy-two` into RAM; `lazy-one` and `lazy-three` stay at 0 bytes
    orch.create_isolated_session("lazy-two", "sess-lazy-2", &jail_root, 3_000)
        .expect("create session");
    let _ = orch.invoke_agent_tool_authorized(
        &mut gk, "lazy-two", "sess-lazy-2", "fs_read", "src/main.rs", "{}", 3_010,
    ).expect("invoke lazy-two");

    assert_eq!(orch.loaded_agent_count(), 1);
    assert!(orch.is_agent_context_loaded("lazy-two"));
    assert!(orch.loaded_soul_bytes("lazy-two") > 0);
    assert!(!orch.is_agent_context_loaded("lazy-one"));
    assert_eq!(orch.loaded_soul_bytes("lazy-one"), 0);
    assert!(!orch.is_agent_context_loaded("lazy-three"));
    assert_eq!(orch.loaded_soul_bytes("lazy-three"), 0);

    // Unloading `lazy-two` evicts its context from RAM back to 0
    assert!(orch.unload_agent_context("lazy-two"));
    assert_eq!(orch.loaded_agent_count(), 0);
    assert_eq!(orch.loaded_soul_bytes("lazy-two"), 0);
}

// LEDGER-3.12-6
#[test]
fn test_soul_md_isolation_and_soul_token_audit_warns_when_exceeding_recommended_threshold() {
    let temp = TempDirGuard::new("soul_audit");
    let jail_root = normalize_os_path_for_agent_jail(temp.path());
    let (mut orch, _gk) = build_test_orchestrator(&jail_root);
    let base_reg = ToolRegistry::with_default_internal_tools();

    let compact_dir = write_custom_agent_fixture(
        temp.path(), "compact-agent", "Compact Agent", "anthropic", "claude-3-7-sonnet",
        DEFAULT_SOUL_FILENAME, &["fs_read"], &[],
        "# Compact Soul\nMARKER_SOUL_COMPACT_ONLY: Keep patches minimal and tested.",
    );
    let oversized_body = format!(
        "# Oversized Soul\nMARKER_SOUL_OVERSIZED_ONLY\n{}",
        "verbose_directive_token ".repeat(1_200)
    );
    let oversized_dir = write_custom_agent_fixture(
        temp.path(), "oversized-agent", "Oversized Agent", "open_ai", "gpt-4o",
        DEFAULT_SOUL_FILENAME, &["fs_read"], &[], &oversized_body,
    );

    orch.register_agent_from_dir(&compact_dir).expect("register compact");
    orch.register_agent_from_dir(&oversized_dir).expect("register oversized");

    let ctx_compact = orch.ensure_agent_context_loaded("compact-agent").expect("load compact").clone();
    let ctx_oversized = orch.ensure_agent_context_loaded("oversized-agent").expect("load oversized").clone();

    // Verify SOUL.md content is strictly associated only with its own agent's StaticSystemPrefix
    assert!(ctx_compact.static_prefix.rendered_prefix.contains("MARKER_SOUL_COMPACT_ONLY"));
    assert!(!ctx_compact.static_prefix.rendered_prefix.contains("MARKER_SOUL_OVERSIZED_ONLY"));
    assert!(ctx_oversized.static_prefix.rendered_prefix.contains("MARKER_SOUL_OVERSIZED_ONLY"));
    assert!(!ctx_oversized.static_prefix.rendered_prefix.contains("MARKER_SOUL_COMPACT_ONLY"));

    // Verify token audit against RECOMMENDED_MAX_SOUL_TOKENS (1,024 tokens)
    let report_ok =
        audit_custom_agent_soul(&compact_dir, &base_reg, RECOMMENDED_MAX_SOUL_TOKENS).expect("audit compact");
    assert!(!report_ok.exceeds_recommended);
    assert!(report_ok.soul_tokens <= RECOMMENDED_MAX_SOUL_TOKENS);
    assert!(report_ok.warning_message.is_none());

    let report_warn =
        audit_custom_agent_soul(&oversized_dir, &base_reg, RECOMMENDED_MAX_SOUL_TOKENS).expect("audit oversized");
    assert!(report_warn.exceeds_recommended);
    assert!(report_warn.soul_tokens > RECOMMENDED_MAX_SOUL_TOKENS);
    assert!(report_warn.warning_message.as_deref().unwrap_or("").contains("oversized-agent"));
}

// LEDGER-3.12-7
#[test]
fn test_island_custom_agent_create_list_select_ipc_exposes_only_safe_metadata_and_enforces_sub_600_lines() {
    let temp = TempDirGuard::new("island_ipc");
    let agents_root = temp.path().join("agents");
    fs::create_dir_all(&agents_root).expect("create agents root");
    let jail_root = normalize_os_path_for_agent_jail(temp.path());
    let (mut orch, mut gk) = build_test_orchestrator(&jail_root);

    let secret_value = "TOP_SECRET_KEY_VALUE_DO_NOT_LEAK_777";
    orch.secret_manager_mut()
        .set_secret("alias-island-ui", "anthropic", Some("claude-3-7-sonnet"), secret_value.as_bytes())
        .expect("set secret");

    // 1. Create custom agent from Island via typed JSON-RPC 2.0 IPC
    let create_env = IslandCustomAgentIpcEnvelope {
        jsonrpc: "2.0".to_string(),
        v: 1,
        id: "ipc-create-1".to_string(),
        origin: "https://miku.local/index.html".to_string(),
        command: IslandCustomAgentIpcCommand::Create(IslandCreateCustomAgentRequest {
            id: "ui-specialist".to_string(),
            display_name: "UI Specialist".to_string(),
            description: "Designs native Win32 and Island components".to_string(),
            provider: LlmProvider::Anthropic,
            model: "claude-3-7-sonnet".to_string(),
            soul_markdown: "# UI Specialist\nPRIVATE_SOUL_DIRECTIVE_TEXT_999".to_string(),
            allowed_tools: vec!["fs_read".to_string()],
            credential_aliases: vec!["alias-island-ui".to_string()],
        }),
        timestamp_ms: 4_000,
    };
    let create_json = serde_json::to_string(&create_env).expect("serialize create");
    let resp_json = handle_island_custom_agent_ipc(&mut orch, &mut gk, &agents_root, "agents", &create_json)
        .expect("handle create IPC");

    // Response must NEVER contain the raw secret key or raw SOUL.md body
    assert!(!resp_json.contains(secret_value));
    assert!(!resp_json.contains("PRIVATE_SOUL_DIRECTIVE_TEXT_999"));

    // 2. Select custom agent from Island without triggering lazy RAM load
    let select_env = IslandCustomAgentIpcEnvelope {
        jsonrpc: "2.0".to_string(),
        v: 1,
        id: "ipc-select-2".to_string(),
        origin: "https://miku.local/index.html".to_string(),
        command: IslandCustomAgentIpcCommand::Select { agent_id: "ui-specialist".to_string() },
        timestamp_ms: 4_010,
    };
    let select_resp_raw = handle_island_custom_agent_ipc(
        &mut orch, &mut gk, &agents_root, "agents", &serde_json::to_string(&select_env).unwrap(),
    ).expect("handle select IPC");
    let decoded: IslandCustomAgentIpcResponse = serde_json::from_str(&select_resp_raw).expect("decode");
    assert_eq!(decoded.selected_agent_id.as_deref(), Some("ui-specialist"));
    assert_eq!(decoded.agents.len(), 1);
    assert_eq!(decoded.agents[0].status, "selected_idle");
    assert!(!decoded.agents[0].context_loaded_in_ram);

    // 3. Reject untrusted IPC origin
    let bad_origin = select_resp_raw.replace("https://miku.local/index.html", "https://evil.example");
    assert!(handle_island_custom_agent_ipc(&mut orch, &mut gk, &agents_root, "agents", &bad_origin).is_err());

    // 4. Verify all .rs files in miku-agent/src are strictly < 600 lines
    let src_dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
    assert_all_rs_files_under_600_lines(&src_dir);
}

fn assert_all_rs_files_under_600_lines(dir: &Path) {
    let entries = fs::read_dir(dir).expect("read src dir");
    for entry in entries.flatten() {
        let p = entry.path();
        if p.is_dir() {
            assert_all_rs_files_under_600_lines(&p);
        } else if p.extension().and_then(|e| e.to_str()) == Some("rs") {
            let content = fs::read_to_string(&p).expect("read rs file");
            let lines = content.lines().count();
            assert!(
                lines < 600,
                "File {} has {} lines, exceeding the 600-line modularity limit",
                p.display(),
                lines
            );
        }
    }
}
