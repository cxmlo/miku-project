#![allow(unused_imports)]
use crate::prelude::*;

// ============================================================================
// 11. Step 2.6: Strict Deny-by-Default UI/Core Gatekeeper Interceptor,
//     Destructive Confirmation Modal & Anti-TOCTOU Typed IPC Channel
//     (ADR-007 · F-03 · F-04)
// ============================================================================

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RiskSeverityLevel {
    Low,
    Medium,
    High,
    Destructive,
}

impl RiskSeverityLevel {
    pub fn as_str(&self) -> &'static str {
        match self {
            RiskSeverityLevel::Low => "low",
            RiskSeverityLevel::Medium => "medium",
            RiskSeverityLevel::High => "high",
            RiskSeverityLevel::Destructive => "destructive",
        }
    }

    pub fn classify(
        capability: Capability,
        canonical_target: &str,
        raw_args: &str,
        pc_only: bool,
    ) -> Self {
        let combined_lower = format!("{} {}", canonical_target, raw_args).to_ascii_lowercase();
        if pc_only
            || matches!(capability, Capability::Shell | Capability::Input)
            || combined_lower.contains("rm ")
            || combined_lower.contains("del ")
            || combined_lower.contains("reset --hard")
            || combined_lower.contains("format ")
            || combined_lower.contains("publish")
        {
            RiskSeverityLevel::Destructive
        } else if matches!(
            capability,
            Capability::FsWrite | Capability::UiAutomation | Capability::Clipboard
        ) {
            RiskSeverityLevel::High
        } else if matches!(
            capability,
            Capability::Network | Capability::Browser | Capability::Vision
        ) {
            RiskSeverityLevel::Medium
        } else {
            RiskSeverityLevel::Low
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ImpactSummaryCard {
    pub req_hash: String,
    pub agent: String,
    pub capability: Capability,
    pub canonical_target: String,
    pub exact_command_or_args: String,
    pub affected_files: Vec<String>,
    pub severity: RiskSeverityLevel,
    pub pc_only: bool,
    pub ttl_total_ms: u64,
    pub dom_renders: Vec<WebViewIpcPayloadV1>,
}

impl ImpactSummaryCard {
    pub fn build_sanitized(
        ticket: &PermissionTicket,
        affected_files: &[&str],
    ) -> Result<Self, UiError> {
        let severity = RiskSeverityLevel::classify(
            ticket.capability,
            &ticket.canonical_target,
            &ticket.canonical_args,
            ticket.pc_only,
        );
        let ttl_total_ms = ticket.expires_at_ms.saturating_sub(ticket.issued_at_ms);

        let safe_agent = escape_html_entities_for_text_content(&ticket.agent);
        let safe_target = escape_html_entities_for_text_content(&ticket.canonical_target);
        let safe_cmd = escape_html_entities_for_text_content(&ticket.canonical_args);

        let mut safe_files = Vec::with_capacity(affected_files.len());
        for f in affected_files {
            safe_files.push(escape_html_entities_for_text_content(f));
        }
        let files_joined = if safe_files.is_empty() {
            safe_target.clone()
        } else {
            safe_files.join(", ")
        };

        let title_render = sanitize_for_text_content(
            "miku-gk-modal-title",
            &format!(
                "[{}] {} solicita {}",
                severity.as_str().to_ascii_uppercase(),
                ticket.agent,
                ticket.capability.as_str()
            ),
        )?;
        let target_render = sanitize_for_text_content("miku-gk-modal-target", &ticket.canonical_target)?;
        let cmd_render = sanitize_for_text_content("miku-gk-modal-cmd", &ticket.canonical_args)?;
        let files_render = sanitize_for_text_content("miku-gk-modal-files", &files_joined)?;
        let hash_render = sanitize_for_text_content("miku-gk-modal-hash", &ticket.req_hash)?;
        let ttl_render = sanitize_for_text_content(
            "miku-gk-modal-ttl",
            &format!("{}s restantes", (ttl_total_ms / 1000).max(1)),
        )?;

        Ok(Self {
            req_hash: ticket.req_hash.clone(),
            agent: safe_agent,
            capability: ticket.capability,
            canonical_target: safe_target,
            exact_command_or_args: safe_cmd,
            affected_files: safe_files,
            severity,
            pc_only: ticket.pc_only,
            ttl_total_ms,
            dom_renders: vec![
                title_render,
                target_render,
                cmd_render,
                files_render,
                hash_render,
                ttl_render,
            ],
        })
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum ConfirmationModalState {
    Hidden,
    ActiveCountdown {
        ticket: PermissionTicket,
        impact: ImpactSummaryCard,
        remaining_ms: u64,
    },
    Resolved {
        req_hash: String,
        verdict: GatekeeperVerdict,
        resolved_at_ms: u64,
    },
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct GatekeeperInterceptOutcome {
    pub verdict: GatekeeperVerdict,
    pub modal_state: ConfirmationModalState,
    pub avatar_mood: AvatarMood,
    pub ipc_envelope: WebViewIpcEnvelopeV1,
    pub dom_renders: Vec<WebViewIpcPayloadV1>,
    pub elapsed_ms: f64,
}

pub struct UiGatekeeperInterceptor {
    pub(crate) engine: GatekeeperEngine,
    pub(crate) modal_state: ConfirmationModalState,
    pub(crate) avatar_mood: AvatarMood,
    pub(crate) total_interceptions: u64,
    pub(crate) allowed_count: u64,
    pub(crate) denied_default_count: u64,
    pub(crate) denied_rule_count: u64,
    pub(crate) denied_jail_count: u64,
    pub(crate) denied_toctou_count: u64,
    pub(crate) denied_expired_count: u64,
    pub(crate) denied_replay_count: u64,
    pub(crate) denied_user_count: u64,
    pub(crate) ask_opened_count: u64,
    pub(crate) max_intercept_latency_ms: f64,
}

impl UiGatekeeperInterceptor {
    pub fn new(profile: ProjectProfile) -> Result<Self, UiError> {
        let engine = GatekeeperEngine::new(profile)
            .map_err(|e| UiError::GatekeeperInterceptionError(e.to_string()))?;
        Ok(Self {
            engine,
            modal_state: ConfirmationModalState::Hidden,
            avatar_mood: AvatarMood::Idle,
            total_interceptions: 0,
            allowed_count: 0,
            denied_default_count: 0,
            denied_rule_count: 0,
            denied_jail_count: 0,
            denied_toctou_count: 0,
            denied_expired_count: 0,
            denied_replay_count: 0,
            denied_user_count: 0,
            ask_opened_count: 0,
            max_intercept_latency_ms: 0.0,
        })
    }

    pub fn default_production_profile(project_root: &str) -> ProjectProfile {
        ProjectProfile::new("miku-workspace", project_root, DEFAULT_CONFIRMATION_MODAL_TTL_MS)
            // Safe read-only rules inside project jail
            .with_rule(Capability::FsRead, "src/**", PolicyEffect::Allow, false)
            .with_rule(Capability::FsRead, "Cargo.toml", PolicyEffect::Allow, false)
            .with_rule(Capability::FsRead, "README.md", PolicyEffect::Allow, false)
            .with_rule(Capability::Shell, "cargo test*", PolicyEffect::Allow, false)
            .with_rule(Capability::Shell, "cargo clippy*", PolicyEffect::Allow, false)
            // Interactive Ask rules requiring user confirmation + Anti-TOCTOU req_hash
            .with_rule(Capability::FsWrite, "src/**", PolicyEffect::Ask, false)
            .with_rule(Capability::Shell, "cargo publish*", PolicyEffect::Ask, true)
            .with_rule(Capability::Shell, "git commit*", PolicyEffect::Ask, false)
            .with_rule(Capability::Network, "https://api.github.com/*", PolicyEffect::Ask, false)
            .with_rule(Capability::Browser, "https://github.com/*", PolicyEffect::Ask, false)
            // Explicit immovable Deny rules (Deny > Ask > Allow precedence)
            .with_rule(Capability::Shell, "*rm -rf*", PolicyEffect::Deny, true)
            .with_rule(Capability::Shell, "*git push --force*", PolicyEffect::Deny, true)
            .with_rule(Capability::Shell, "*format *", PolicyEffect::Deny, true)
            .with_rule(Capability::FsWrite, "src/secrets/**", PolicyEffect::Deny, true)
    }
}

pub fn default_ui_gatekeeper_profile(project_root: &str) -> ProjectProfile {
    UiGatekeeperInterceptor::default_production_profile(project_root)
}

impl UiGatekeeperInterceptor {

    pub fn engine(&self) -> &GatekeeperEngine {
        &self.engine
    }

    pub fn engine_mut(&mut self) -> &mut GatekeeperEngine {
        &mut self.engine
    }

    pub fn modal_state(&self) -> &ConfirmationModalState {
        &self.modal_state
    }

    pub fn avatar_mood(&self) -> AvatarMood {
        self.avatar_mood
    }

    pub fn total_interceptions(&self) -> u64 {
        self.total_interceptions
    }

    pub fn allowed_count(&self) -> u64 {
        self.allowed_count
    }

    pub fn denied_default_count(&self) -> u64 {
        self.denied_default_count
    }

    pub fn denied_rule_count(&self) -> u64 {
        self.denied_rule_count
    }

    pub fn denied_jail_count(&self) -> u64 {
        self.denied_jail_count
    }

    pub fn denied_toctou_count(&self) -> u64 {
        self.denied_toctou_count
    }

    pub fn denied_expired_count(&self) -> u64 {
        self.denied_expired_count
    }

    pub fn denied_replay_count(&self) -> u64 {
        self.denied_replay_count
    }

    pub fn denied_user_count(&self) -> u64 {
        self.denied_user_count
    }

    pub fn ask_opened_count(&self) -> u64 {
        self.ask_opened_count
    }

    pub fn max_intercept_latency_ms(&self) -> f64 {
        self.max_intercept_latency_ms
    }

    pub(crate) fn record_deny_reason(&mut self, reason: DenyReason) {
        match reason {
            DenyReason::DefaultDeny => self.denied_default_count += 1,
            DenyReason::ExplicitDenyRule => self.denied_rule_count += 1,
            DenyReason::PathJailViolation => self.denied_jail_count += 1,
            DenyReason::ToctouHashMismatch => self.denied_toctou_count += 1,
            DenyReason::TicketExpired => self.denied_expired_count += 1,
            DenyReason::TicketNotFoundOrAlreadyUsed => self.denied_replay_count += 1,
            DenyReason::UserDenied
            | DenyReason::PcOnlyFromMobileForbidden
            | DenyReason::BiometricRequiredOnMobile
            | DenyReason::KillSwitchActive => self.denied_user_count += 1,
        }
    }

    /// Intercepts a tool execution request (`CLI command`, `filesystem read/write`, `network call`)
    /// BEFORE execution, enforcing `deny-by-default`, `Deny > Ask > Allow`, and `CanonicalPathJail`.
    /// On `AskPending`, opens the visual confirmation modal with sanitized `textContent` impact summary
    /// and emits a typed `PermissionRequest` over `WebView2IpcBridge`.
    pub fn intercept_pre_execution(
        &mut self,
        bridge: &mut WebView2IpcBridge,
        request_id: &str,
        agent: &str,
        capability: Capability,
        raw_target: &str,
        raw_args: &str,
        affected_files: &[&str],
        now_ms: u64,
    ) -> Result<GatekeeperInterceptOutcome, UiError> {
        let t0 = Instant::now();
        self.total_interceptions += 1;

        let verdict =
            self.engine
                .evaluate_request(agent, capability, raw_target, raw_args, now_ms);

        let (ipc_envelope, dom_renders) = match &verdict {
            GatekeeperVerdict::Allow {
                canonical_target, ..
            } => {
                self.allowed_count += 1;
                self.modal_state = ConfirmationModalState::Hidden;
                self.avatar_mood = AvatarMood::Working;

                let status_render = sanitize_for_text_content(
                    "miku-overview-status",
                    &format!("ALLOW {} {}: {}", agent, capability.as_str(), canonical_target),
                )?;
                let env = WebViewIpcEnvelopeV1::new(
                    format!("gk-allow-{}", request_id),
                    Some(request_id.to_string()),
                    OVERVIEW_VIRTUAL_INDEX_URL,
                    WebViewIpcPayloadV1::MikuLink(MikuLinkMessageV1::AgentEvent {
                        agent: agent.to_string(),
                        kind: "gatekeeper_allowed".to_string(),
                        title: canonical_target.clone(),
                    }),
                );
                bridge.enqueue_outbound(env.clone())?;
                (env, vec![status_render])
            }
            GatekeeperVerdict::AskPending { ticket, .. } => {
                self.ask_opened_count += 1;
                let impact = ImpactSummaryCard::build_sanitized(ticket, affected_files)?;
                let remaining_ms = ticket.expires_at_ms.saturating_sub(now_ms);
                self.avatar_mood = if impact.severity == RiskSeverityLevel::Destructive {
                    AvatarMood::Surprised
                } else {
                    AvatarMood::Annoyed
                };
                let dom_renders = impact.dom_renders.clone();
                self.modal_state = ConfirmationModalState::ActiveCountdown {
                    ticket: ticket.clone(),
                    impact,
                    remaining_ms,
                };

                let env = WebViewIpcEnvelopeV1::new(
                    format!("gk-ask-{}", request_id),
                    Some(request_id.to_string()),
                    OVERVIEW_VIRTUAL_INDEX_URL,
                    WebViewIpcPayloadV1::MikuLink(MikuLinkMessageV1::PermissionRequest {
                        req_hash: ticket.req_hash.clone(),
                        agent: ticket.agent.clone(),
                        capability: ticket.capability.as_str().to_string(),
                        target: ticket.canonical_target.clone(),
                        ttl_sec: (remaining_ms / 1000).max(1) as u32,
                        pc_only: ticket.pc_only,
                    }),
                );
                bridge.enqueue_outbound(env.clone())?;
                (env, dom_renders)
            }
            GatekeeperVerdict::Deny { reason, detail, .. } => {
                self.record_deny_reason(*reason);
                self.modal_state = ConfirmationModalState::Hidden;
                self.avatar_mood = AvatarMood::Error;

                let status_render = sanitize_for_text_content(
                    "miku-overview-status",
                    &format!("BLOCKED [{}]: {}", reason.as_code(), detail),
                )?;
                let env = WebViewIpcEnvelopeV1::new(
                    format!("gk-deny-{}", request_id),
                    Some(request_id.to_string()),
                    OVERVIEW_VIRTUAL_INDEX_URL,
                    WebViewIpcPayloadV1::MikuLink(MikuLinkMessageV1::Error {
                        code: "gatekeeper_denied".to_string(),
                        supported: Some(vec![MIKU_PROTOCOL_VERSION_V1]),
                        detail: Some(format!("{}: {}", reason.as_code(), detail)),
                    }),
                );
                bridge.enqueue_outbound(env.clone())?;
                (env, vec![status_render])
            }
        };

        let elapsed_ms = t0.elapsed().as_secs_f64() * 1000.0;
        if elapsed_ms > self.max_intercept_latency_ms {
            self.max_intercept_latency_ms = elapsed_ms;
        }

        Ok(GatekeeperInterceptOutcome {
            verdict,
            modal_state: self.modal_state.clone(),
            avatar_mood: self.avatar_mood,
            ipc_envelope,
            dom_renders,
            elapsed_ms,
        })
    }
}
