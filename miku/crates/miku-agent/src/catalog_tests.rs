use std::collections::HashMap;

use miku_gatekeeper::{Capability, GatekeeperEngine, PolicyEffect, ProjectProfile};

use crate::adapters::{AntigravityBridge, ClaudeBridge, CursorBridge, GitHubBridge};
use crate::bridge::{
    AgentBridge, AgentBridgeError, AgentBridgeEvent, AgentBridgeResponse, AgentBridgeStatus,
};
use crate::catalog::{
    AgentCard, AgentCatalog, AgentManifest, AgentTransportKind, CatalogError,
};

/// Helper to build a test Gatekeeper with default deny and standard rules.
fn create_test_gatekeeper() -> GatekeeperEngine {
    let profile = ProjectProfile::new("catalog-test", "C:/Users/cmiranda/dev/miku", 60_000)
        .with_rule(Capability::FsRead, "src/**", PolicyEffect::Allow, false)
        .with_rule(Capability::Shell, "git status*", PolicyEffect::Allow, false)
        .with_rule(Capability::Shell, "*rm -rf*", PolicyEffect::Deny, true)
        .with_rule(Capability::Shell, "*format*", PolicyEffect::Deny, true);
    GatekeeperEngine::new(profile).expect("Gatekeeper engine should build")
}

#[test]
fn test_agent_catalog_declarative_schema_validation_and_rejection_of_invalid_manifests() {
    // LEDGER-3.5-1: Declarative catalog validation and rejection of invalid manifests
    let mut catalog = AgentCatalog::with_builtin_agents();

    // Verify 4 built-in agents are registered
    assert!(catalog.get("claude-code").is_some());
    assert!(catalog.get("cursor").is_some());
    assert!(catalog.get("antigravity").is_some());
    assert!(catalog.get("github").is_some());

    let claude = catalog.get("claude-code").unwrap();
    assert_eq!(claude.display_name, "Claude Code");
    assert_eq!(claude.transport, AgentTransportKind::StdioHook);
    assert!(claude.is_builtin);

    let cursor = catalog.get("cursor").unwrap();
    assert_eq!(cursor.display_name, "Cursor IDE");
    assert_eq!(cursor.transport, AgentTransportKind::McpHost);

    let antigravity = catalog.get("antigravity").unwrap();
    assert_eq!(antigravity.display_name, "Google Antigravity");
    assert_eq!(antigravity.transport, AgentTransportKind::PassiveLog);

    let github = catalog.get("github").unwrap();
    assert_eq!(github.display_name, "GitHub Integration");
    assert_eq!(github.transport, AgentTransportKind::WebhookRest);

    // Test rejection: Duplicate ID
    let dup_res = catalog.register(AgentManifest {
        id: "cursor".to_string(),
        display_name: "Cursor Duplicate".to_string(),
        description: "duplicate".to_string(),
        vendor: "Anysphere".to_string(),
        version: "0.4.0".to_string(),
        transport: AgentTransportKind::McpHost,
        capabilities: vec![],
        icon_slug: "cursor".to_string(),
        is_builtin: false,
    });
    assert!(matches!(dup_res, Err(CatalogError::DuplicateAgentId(id)) if id == "cursor"));

    // Test rejection: Empty ID
    let empty_id_res = catalog.register(AgentManifest {
        id: "".to_string(),
        display_name: "Empty ID Agent".to_string(),
        description: "no id".to_string(),
        vendor: "Vendor".to_string(),
        version: "1.0.0".to_string(),
        transport: AgentTransportKind::StdioHook,
        capabilities: vec![],
        icon_slug: "icon".to_string(),
        is_builtin: false,
    });
    assert!(matches!(empty_id_res, Err(CatalogError::InvalidManifest(msg)) if msg.contains("empty")));

    // Test rejection: Empty display name
    let empty_name_res = catalog.register(AgentManifest {
        id: "custom-agent".to_string(),
        display_name: "".to_string(),
        description: "desc".to_string(),
        vendor: "Vendor".to_string(),
        version: "1.0.0".to_string(),
        transport: AgentTransportKind::StdioHook,
        capabilities: vec![],
        icon_slug: "icon".to_string(),
        is_builtin: false,
    });
    assert!(matches!(empty_name_res, Err(CatalogError::InvalidManifest(msg)) if msg.contains("display_name")));

    // Test parsing custom manifests from TOML
    let toml_snippet = r#"
[[agents]]
id = "cline"
display_name = "Cline VSCode Extension"
description = "Autonomous coding agent for VSCode"
vendor = "Cline"
version = "3.2.0"
transport = "mcp_host"
capabilities = ["fs_read", "fs_write", "shell"]
icon_slug = "cline"
is_builtin = false
"#;
    let count = catalog.load_from_toml(toml_snippet).expect("TOML load should succeed");
    assert_eq!(count, 1);
    assert!(catalog.get("cline").is_some());

    // Test unified card generation without UI duplication
    let mut statuses = HashMap::new();
    statuses.insert("claude-code".to_string(), AgentBridgeStatus::Active);
    statuses.insert("cursor".to_string(), AgentBridgeStatus::Idle);
    statuses.insert(
        "antigravity".to_string(),
        AgentBridgeStatus::Unsupported {
            reason: "Passive observation only".to_string(),
        },
    );
    statuses.insert("github".to_string(), AgentBridgeStatus::Active);

    let cards = catalog.render_cards(&statuses);
    assert_eq!(cards.len(), 5); // 4 builtins + 1 custom
    let claude_card = cards.iter().find(|c| c.agent_id == "claude-code").unwrap();
    assert_eq!(claude_card.mood_indicator, "working");
    let antigravity_card = cards.iter().find(|c| c.agent_id == "antigravity").unwrap();
    assert_eq!(antigravity_card.mood_indicator, "unsupported");
}

#[test]
fn test_agent_bridge_polymorphic_contract_and_unified_island_card_rendering() {
    // LEDGER-3.5-2: Polymorphic AgentBridge contract and standardized Island card rendering
    let catalog = AgentCatalog::with_builtin_agents();

    let claude_manifest = catalog.get("claude-code").unwrap().clone();
    let cursor_manifest = catalog.get("cursor").unwrap().clone();
    let antigravity_manifest = catalog.get("antigravity").unwrap().clone();
    let github_manifest = catalog.get("github").unwrap().clone();

    let mut bridges: Vec<Box<dyn AgentBridge>> = vec![
        Box::new(ClaudeBridge::new(claude_manifest, create_test_gatekeeper())),
        Box::new(CursorBridge::new(cursor_manifest, create_test_gatekeeper())),
        Box::new(AntigravityBridge::new(antigravity_manifest)),
        Box::new(GitHubBridge::new(github_manifest, "https://api.github.com")),
    ];

    let expected_ids = ["claude-code", "cursor", "antigravity", "github"];

    for (i, bridge) in bridges.iter_mut().enumerate() {
        assert_eq!(bridge.agent_id(), expected_ids[i]);
        assert_eq!(bridge.manifest().id, expected_ids[i]);

        let card: AgentCard = bridge.render_card();
        assert_eq!(card.agent_id, expected_ids[i]);
        assert!(!card.display_name.is_empty());
        assert!(!card.icon_slug.is_empty());
        assert!(!card.summary.is_empty());
        assert!(
            ["idle", "working", "thinking", "error", "offline", "unsupported"]
                .contains(&card.mood_indicator.as_str()),
            "Card mood indicator '{}' must be standardized",
            card.mood_indicator
        );

        // Heartbeat response
        let hb_res = bridge.handle_event(&AgentBridgeEvent::Heartbeat);
        assert!(hb_res.is_ok());
    }
}

#[test]
fn test_cursor_bridge_mcp_capability_contract_and_gatekeeper_enforcement() {
    // LEDGER-3.5-3: Cursor adapter MCP contract enforcement and Gatekeeper deny-by-default
    let catalog = AgentCatalog::with_builtin_agents();
    let gatekeeper = create_test_gatekeeper();
    let manifest = catalog.get("cursor").unwrap().clone();

    let mut cursor_bridge = CursorBridge::new(manifest, gatekeeper);

    // Initial status is Idle
    assert_eq!(cursor_bridge.status(), AgentBridgeStatus::Idle);

    // 1. Allowed Tool Execution (fs_read on allowed pattern)
    let allow_event = AgentBridgeEvent::ToolCall {
        tool_name: "fs_read".to_string(),
        capability: Capability::FsRead,
        target: "src/lib.rs".to_string(),
        payload_json: r#"{"encoding":"utf-8"}"#.to_string(),
    };
    let allow_res = cursor_bridge.handle_event(&allow_event).expect("Allowed call must succeed");
    match allow_res {
        AgentBridgeResponse::ToolExecuted { output } => {
            assert!(output.contains("dispatched"));
        }
        other => panic!("Expected ToolExecuted, got {:?}", other),
    }

    // Status transitions to Active during activity
    assert_eq!(cursor_bridge.status(), AgentBridgeStatus::Active);

    // 2. Denied Tool Execution (destructive shell execution blocked by Gatekeeper)
    let deny_event = AgentBridgeEvent::ToolCall {
        tool_name: "shell_exec".to_string(),
        capability: Capability::Shell,
        target: "rm -rf *".to_string(),
        payload_json: r#"{"args":[]}"#.to_string(),
    };
    let deny_res = cursor_bridge.handle_event(&deny_event);
    assert!(
        matches!(deny_res, Err(AgentBridgeError::GatekeeperDenied(ref reason)) if reason.contains("deny_rule") || reason.contains("rm -rf")),
        "Destructive shell command must be denied by Gatekeeper: {:?}",
        deny_res
    );

    // 3. Unlisted Capability Execution (Network blocked by default deny)
    let unlisted_event = AgentBridgeEvent::ToolCall {
        tool_name: "net_fetch".to_string(),
        capability: Capability::Network,
        target: "https://telemetry.evil.com/leak".to_string(),
        payload_json: r#"{}"#.to_string(),
    };
    let unlisted_res = cursor_bridge.handle_event(&unlisted_event);
    assert!(
        matches!(unlisted_res, Err(AgentBridgeError::GatekeeperDenied(_))),
        "Unlisted capability must be rejected: {:?}",
        unlisted_res
    );
}

#[test]
fn test_antigravity_bridge_spike_06_conformance_reports_typed_unsupported_without_faking() {
    // LEDGER-3.5-4: Antigravity adapter conservative stance (reports typed Unsupported)
    let catalog = AgentCatalog::with_builtin_agents();
    let manifest = catalog.get("antigravity").unwrap().clone();

    let mut antigravity_bridge = AntigravityBridge::new(manifest);

    // Initial status reflects conservative stance: no confirmed protocol in repo
    let initial_status = antigravity_bridge.status();
    match initial_status {
        AgentBridgeStatus::Unsupported { reason } => {
            assert!(
                reason.to_lowercase().contains("no confirmed")
                    || reason.to_lowercase().contains("passive")
                    || reason.to_lowercase().contains("spike 0.6"),
                "Reason must clearly reflect conservative rationale: {}",
                reason
            );
        }
        other => panic!("Expected Unsupported status, got {:?}", other),
    }

    // Card rendering truthfully reflects unsupported status
    let card = antigravity_bridge.render_card();
    assert_eq!(card.mood_indicator, "unsupported");
    assert!(card.summary.contains("Passive") || card.summary.contains("unsupported"));

    // Attempting direct tool dispatch returns typed Unsupported error without faking connection
    let tool_event = AgentBridgeEvent::ToolCall {
        tool_name: "fs_write".to_string(),
        capability: Capability::FsWrite,
        target: r"..\..\Windows\System32\cmd.exe".to_string(),
        payload_json: r#"{"bytes":128}"#.to_string(),
    };
    let call_res = antigravity_bridge.handle_event(&tool_event);
    assert!(
        matches!(call_res, Err(AgentBridgeError::Unsupported(ref reason)) if reason.contains("unsupported") || reason.contains("passive")),
        "Direct IPC tool dispatch must return typed Unsupported error: {:?}",
        call_res
    );
}

#[test]
fn test_github_bridge_etag_caching_304_not_modified_and_zero_secret_leak() {
    // LEDGER-3.5-5: GitHub adapter ETag caching (304 Not Modified) and zero secret leak
    let catalog = AgentCatalog::with_builtin_agents();
    let manifest = catalog.get("github").unwrap().clone();

    let secret_token = "ghp_superSecretTokenXYZ987654321";
    let mut github_bridge = GitHubBridge::with_auth_token(
        manifest,
        "https://api.github.com",
        Some(secret_token.to_string()),
    );

    // Initial status is Idle
    assert_eq!(github_bridge.status(), AgentBridgeStatus::Idle);

    // 1. First sync with ETag "etag-v1" and payload
    let event1 = AgentBridgeEvent::StatusSync {
        etag: Some(r#""etag-v1""#.to_string()),
        payload: Some(r#"{"open_prs":3,"pending_reviews":1}"#.to_string()),
    };
    let res1 = github_bridge.handle_event(&event1).expect("First sync should succeed");
    assert!(matches!(res1, AgentBridgeResponse::Acknowledged { .. }));
    assert_eq!(github_bridge.status(), AgentBridgeStatus::Active);

    // 2. Subsequent sync with matching ETag: must return NotModified without redundant parsing
    let event2 = AgentBridgeEvent::StatusSync {
        etag: Some(r#""etag-v1""#.to_string()),
        payload: Some(r#"{"dummy":"payload should not be parsed"}"#.to_string()),
    };
    let res2 = github_bridge.handle_event(&event2).expect("Conditional sync must succeed");
    match res2 {
        AgentBridgeResponse::NotModified { etag } => {
            assert_eq!(etag, r#""etag-v1""#);
        }
        other => panic!("Expected NotModified, got {:?}", other),
    }

    // 3. Sync with updated ETag "etag-v2": updates state
    let event3 = AgentBridgeEvent::StatusSync {
        etag: Some(r#""etag-v2""#.to_string()),
        payload: Some(r#"{"open_prs":4,"pending_reviews":0}"#.to_string()),
    };
    let res3 = github_bridge.handle_event(&event3).expect("Updated sync should succeed");
    assert!(matches!(res3, AgentBridgeResponse::Acknowledged { .. }));

    // 4. Security verification: Zero Secret Leak in card, status, summary, and error outputs
    let card = github_bridge.render_card();
    assert!(!card.summary.contains(secret_token), "Secret token must never leak in card summary");
    let serialized_card = serde_json::to_string(&card).unwrap();
    assert!(!serialized_card.contains(secret_token), "Secret token must never leak in card JSON");

    let err_res = github_bridge.handle_event(&AgentBridgeEvent::ToolCall {
        tool_name: "invalid_tool".to_string(),
        capability: Capability::Shell,
        target: "".to_string(),
        payload_json: format!(r#"{{"auth":"{}"}}"#, secret_token),
    });
    if let Err(err) = err_res {
        let err_str = format!("{:?}", err);
        assert!(!err_str.contains(secret_token), "Secret token must never leak in error messages: {}", err_str);
    }
}
