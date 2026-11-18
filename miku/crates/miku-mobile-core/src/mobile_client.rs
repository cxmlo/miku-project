use miku_gatekeeper::{Capability, GatekeeperEngine, GatekeeperVerdict};
use miku_protocol::{
    constant_time_token_eq, MikuLinkEnvelopeV1, MikuLinkMessageV1, PermissionDecisionKind,
    MIKU_PROTOCOL_VERSION_V1,
};
use serde::{Deserialize, Serialize};

/// Canonical permission request card displayed on the mobile UI.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DisplayedPermissionCard {
    pub req_hash: String,
    pub agent: String,
    pub capability: String,
    pub target: String,
    pub args: String,
    pub ttl_sec: u32,
    pub received_at_ms: u64,
    pub pc_only: bool,
}

/// Hardware-backed biometric verification proof bound to a specific `req_hash`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct BiometricProof {
    pub verified: bool,
    pub auth_method: String,
    pub bound_req_hash: String,
    pub verified_at_ms: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MobileDecisionInput {
    pub message_id: String,
    pub card: DisplayedPermissionCard,
    pub requested_decision: PermissionDecisionKind,
    pub biometric: Option<BiometricProof>,
    pub now_ms: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MobileDecisionEnvelopeResult {
    pub final_decision: PermissionDecisionKind,
    pub auto_expired_to_deny: bool,
    pub envelope_json: String,
    pub verified_req_hash: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MobileAgentFeedEntry {
    pub agent: String,
    pub kind: String,
    pub title: String,
    pub received_at_ms: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MobileChatQueueItem {
    pub client_msg_id: String,
    pub session_id: String,
    pub text: String,
    pub queued_offline: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ParityMatrixArea {
    pub area: &'static str,
    pub island_desktop: &'static str,
    pub android_mobile: &'static str,
    pub parity_rule: &'static str,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum MobileCoreError {
    PcOnlyApprovalForbiddenOnMobile { capability: String },
    ReqHashMismatch { expected: String, actual: String },
    BiometricRequiredForAllow,
    BiometricBoundHashMismatch,
    ProviderApiKeyStorageForbidden { indicator: String },
    EmptyField(&'static str),
    InvalidJson(String),
}

impl std::fmt::Display for MobileCoreError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::PcOnlyApprovalForbiddenOnMobile { capability } => write!(
                f,
                "pc_only capability '{}' can only be approved from the desktop Island",
                capability
            ),
            Self::ReqHashMismatch { expected, actual } => write!(
                f,
                "displayed parameters do not match signed req_hash (expected {}, got {})",
                expected, actual
            ),
            Self::BiometricRequiredForAllow => {
                write!(f, "Allow on mobile requires verified hardware biometrics")
            }
            Self::BiometricBoundHashMismatch => {
                write!(f, "biometric proof bound_req_hash does not match card req_hash")
            }
            Self::ProviderApiKeyStorageForbidden { indicator } => write!(
                f,
                "mobile client must never store provider API keys (detected '{}')",
                indicator
            ),
            Self::EmptyField(field) => write!(f, "field '{}' cannot be empty", field),
            Self::InvalidJson(msg) => write!(f, "invalid JSON payload: {}", msg),
        }
    }
}

impl std::error::Error for MobileCoreError {}

/// Computes the exact `MIKU_REQ_HASH_V1` digest matching `miku-gatekeeper`.
pub fn compute_req_hash_v1(
    agent: &str,
    capability: &str,
    canonical_target: &str,
    canonical_args: &str,
) -> String {
    if let Some(parsed_cap) = Capability::parse(capability) {
        return GatekeeperEngine::compute_request_hash(
            agent,
            parsed_cap,
            canonical_target,
            canonical_args,
        );
    }
    let raw = format!(
        "MIKU_REQ_HASH_V1|agent={}|cap={}|target={}|args={}",
        agent.trim(),
        capability.trim(),
        canonical_target,
        canonical_args.trim()
    );
    format!("sha256:{}", miku_protocol::sha256_hex(raw.as_bytes()))
}

/// Ensures mobile state/config payloads never contain LLM provider API keys.
pub fn validate_mobile_config_no_provider_keys(raw_payload: &str) -> Result<(), MobileCoreError> {
    let lower = raw_payload.to_ascii_lowercase();
    let forbidden_markers = [
        "gsk_",
        "sk-ant-",
        "sk-proj-",
        "sk-live-",
        "anthropic_api_key",
        "openai_api_key",
        "groq_api_key",
        "provider_api_key",
        "gemini_api_key",
    ];
    for marker in forbidden_markers {
        if lower.contains(marker) {
            return Err(MobileCoreError::ProviderApiKeyStorageForbidden {
                indicator: marker.to_string(),
            });
        }
    }
    Ok(())
}

/// Stateful mobile client kernel shared across Rust tests and Android JNI/UniFFI.
#[derive(Debug, Clone)]
pub struct MobileCoreClient {
    pub session_id: String,
    pub device_id: String,
    pub connected: bool,
    pub agent_feed: Vec<MobileAgentFeedEntry>,
    pub outbound_chat_queue: Vec<MobileChatQueueItem>,
}

impl MobileCoreClient {
    pub fn new(session_id: impl Into<String>, device_id: impl Into<String>) -> Self {
        Self {
            session_id: session_id.into(),
            device_id: device_id.into(),
            connected: false,
            agent_feed: Vec::new(),
            outbound_chat_queue: Vec::new(),
        }
    }

    pub fn set_connected(&mut self, connected: bool) {
        self.connected = connected;
    }

    pub fn push_agent_event(&mut self, event: MobileAgentFeedEntry) {
        self.agent_feed.push(event);
        if self.agent_feed.len() > 200 {
            let overflow = self.agent_feed.len() - 200;
            self.agent_feed.drain(0..overflow);
        }
    }

    /// Queues a chat prompt locally when offline or produces an immediate `Command` envelope.
    pub fn enqueue_or_dispatch_prompt(
        &mut self,
        client_msg_id: &str,
        prompt_text: &str,
    ) -> Result<Option<String>, MobileCoreError> {
        if prompt_text.trim().is_empty() {
            return Err(MobileCoreError::EmptyField("prompt_text"));
        }
        validate_mobile_config_no_provider_keys(prompt_text)?;

        if !self.connected {
            self.outbound_chat_queue.push(MobileChatQueueItem {
                client_msg_id: client_msg_id.to_string(),
                session_id: self.session_id.clone(),
                text: prompt_text.to_string(),
                queued_offline: true,
            });
            return Ok(None);
        }

        let env = MikuLinkEnvelopeV1 {
            v: MIKU_PROTOCOL_VERSION_V1,
            id: client_msg_id.to_string(),
            reply_to: None,
            msg: MikuLinkMessageV1::Command {
                session_id: self.session_id.clone(),
                action: "prompt".to_string(),
                text: prompt_text.to_string(),
            },
        };
        serde_json::to_string(&env)
            .map(Some)
            .map_err(|e| MobileCoreError::InvalidJson(e.to_string()))
    }

    pub fn flush_offline_queue(&mut self) -> Result<Vec<String>, MobileCoreError> {
        if !self.connected {
            return Ok(Vec::new());
        }
        let queued = std::mem::take(&mut self.outbound_chat_queue);
        let mut frames = Vec::with_capacity(queued.len());
        for item in queued {
            let env = MikuLinkEnvelopeV1 {
                v: MIKU_PROTOCOL_VERSION_V1,
                id: item.client_msg_id,
                reply_to: None,
                msg: MikuLinkMessageV1::Command {
                    session_id: item.session_id,
                    action: "prompt".to_string(),
                    text: item.text,
                },
            };
            let raw = serde_json::to_string(&env)
                .map_err(|e| MobileCoreError::InvalidJson(e.to_string()))?;
            frames.push(raw);
        }
        Ok(frames)
    }

    /// Builds a remote PTT transcript/dispatch command envelope.
    pub fn build_remote_ptt_command(
        &self,
        msg_id: &str,
        transcript: &str,
    ) -> Result<String, MobileCoreError> {
        if transcript.trim().is_empty() {
            return Err(MobileCoreError::EmptyField("transcript"));
        }
        let env = MikuLinkEnvelopeV1 {
            v: MIKU_PROTOCOL_VERSION_V1,
            id: msg_id.to_string(),
            reply_to: None,
            msg: MikuLinkMessageV1::Command {
                session_id: self.session_id.clone(),
                action: "remote_ptt".to_string(),
                text: transcript.trim().to_string(),
            },
        };
        serde_json::to_string(&env).map_err(|e| MobileCoreError::InvalidJson(e.to_string()))
    }

    /// Builds an emergency Global Kill Switch trigger command from mobile.
    pub fn build_kill_switch_frame(&self, msg_id: &str) -> Result<String, MobileCoreError> {
        let env = MikuLinkEnvelopeV1 {
            v: MIKU_PROTOCOL_VERSION_V1,
            id: msg_id.to_string(),
            reply_to: None,
            msg: MikuLinkMessageV1::Command {
                session_id: self.session_id.clone(),
                action: "kill_switch".to_string(),
                text: "mobile_emergency_abort".to_string(),
            },
        };
        serde_json::to_string(&env).map_err(|e| MobileCoreError::InvalidJson(e.to_string()))
    }

    /// Validates card parameters against `req_hash`, enforces `pc_only` and biometrics
    /// for `Allow`, expires stale cards safely to `Deny`, and allows 1-tap `Deny`.
    pub fn prepare_permission_decision(
        &self,
        input: &MobileDecisionInput,
    ) -> Result<MobileDecisionEnvelopeResult, MobileCoreError> {
        let recomputed = compute_req_hash_v1(
            &input.card.agent,
            &input.card.capability,
            &input.card.target,
            &input.card.args,
        );
        if !constant_time_token_eq(&recomputed, &input.card.req_hash) {
            return Err(MobileCoreError::ReqHashMismatch {
                expected: input.card.req_hash.clone(),
                actual: recomputed,
            });
        }

        let expires_at_ms = input
            .card
            .received_at_ms
            .saturating_add((input.card.ttl_sec as u64).saturating_mul(1000));
        let expired = input.now_ms > expires_at_ms;

        // Fail-safe: expiration or explicit user Deny never requires biometrics.
        if expired || input.requested_decision == PermissionDecisionKind::Deny {
            let env = MikuLinkEnvelopeV1 {
                v: MIKU_PROTOCOL_VERSION_V1,
                id: input.message_id.clone(),
                reply_to: None,
                msg: MikuLinkMessageV1::PermissionDecision {
                    req_hash: recomputed.clone(),
                    decision: PermissionDecisionKind::Deny,
                    biometric_verified: false,
                },
            };
            let envelope_json =
                serde_json::to_string(&env).map_err(|e| MobileCoreError::InvalidJson(e.to_string()))?;
            return Ok(MobileDecisionEnvelopeResult {
                final_decision: PermissionDecisionKind::Deny,
                auto_expired_to_deny: expired && input.requested_decision == PermissionDecisionKind::Allow,
                envelope_json,
                verified_req_hash: recomputed,
            });
        }

        // Allow path: block pc_only and require biometric proof bound to this exact req_hash.
        if input.card.pc_only {
            return Err(MobileCoreError::PcOnlyApprovalForbiddenOnMobile {
                capability: input.card.capability.clone(),
            });
        }

        let Some(bio) = &input.biometric else {
            return Err(MobileCoreError::BiometricRequiredForAllow);
        };
        if !bio.verified {
            return Err(MobileCoreError::BiometricRequiredForAllow);
        }
        if !constant_time_token_eq(&bio.bound_req_hash, &recomputed) {
            return Err(MobileCoreError::BiometricBoundHashMismatch);
        }

        let env = MikuLinkEnvelopeV1 {
            v: MIKU_PROTOCOL_VERSION_V1,
            id: input.message_id.clone(),
            reply_to: None,
            msg: MikuLinkMessageV1::PermissionDecision {
                req_hash: recomputed.clone(),
                decision: PermissionDecisionKind::Allow,
                biometric_verified: true,
            },
        };
        let envelope_json =
            serde_json::to_string(&env).map_err(|e| MobileCoreError::InvalidJson(e.to_string()))?;

        Ok(MobileDecisionEnvelopeResult {
            final_decision: PermissionDecisionKind::Allow,
            auto_expired_to_deny: false,
            envelope_json,
            verified_req_hash: recomputed,
        })
    }
}

/// Daemon-side bridge validating a mobile `MikuLinkMessageV1::PermissionDecision` against
/// `GatekeeperEngine` so `pc_only: true` can never be bypassed even by a modified APK.
pub fn enforce_daemon_mobile_permission_decision(
    gatekeeper: &mut GatekeeperEngine,
    msg: &MikuLinkMessageV1,
    execution_agent: &str,
    execution_capability: Capability,
    execution_raw_target: &str,
    execution_raw_args: &str,
    now_ms: u64,
) -> Result<GatekeeperVerdict, MobileCoreError> {
    match msg {
        MikuLinkMessageV1::PermissionDecision {
            req_hash,
            decision,
            biometric_verified,
        } => Ok(gatekeeper.resolve_permission_decision(
            req_hash,
            *decision,
            true,
            *biometric_verified,
            execution_agent,
            execution_capability,
            execution_raw_target,
            execution_raw_args,
            now_ms,
        )),
        _ => Err(MobileCoreError::InvalidJson(
            "expected PermissionDecision message".to_string(),
        )),
    }
}

/// Canonical feature parity table between the Windows Desktop Island and Android Mobile app.
pub fn build_island_mobile_parity_matrix() -> Vec<ParityMatrixArea> {
    vec![
        ParityMatrixArea {
            area: "Permission Approvals (Allow/Deny)",
            island_desktop: "Direct UI decision (all capabilities)",
            android_mobile: "Allow requires AndroidX BiometricPrompt bound to req_hash; pc_only disabled; 1-tap Deny",
            parity_rule: "Server Gatekeeper rejects mobile Allow if pc_only == true or !biometric_verified",
        },
        ParityMatrixArea {
            area: "Agent Feed & Live Status",
            island_desktop: "Live pill/expanded EventBus cards",
            android_mobile: "Live WebSocket AgentEvent timeline",
            parity_rule: "Identical MikuLinkMessageV1::AgentEvent payloads",
        },
        ParityMatrixArea {
            area: "Session Chat & Offline Queue",
            island_desktop: "Direct session prompt box + queue",
            android_mobile: "Compose prompt bar with local queue flush on reconnect",
            parity_rule: "Replayed sequentially via MikuLinkMessageV1::Command(action=prompt)",
        },
        ParityMatrixArea {
            area: "Voice Input (PTT)",
            island_desktop: "Alt+Space hardware mic + Groq Whisper on PC",
            android_mobile: "Remote PTT mic button sending transcript/audio command to PC session",
            parity_rule: "No provider API key ever lives on Android",
        },
        ParityMatrixArea {
            area: "Global Kill Switch",
            island_desktop: "Ctrl+Alt+Shift+K immediate hardware/session cut",
            android_mobile: "Always-pinned red Kill Switch bar",
            parity_rule: "Revokes remote sessions + halts agents < 100 ms",
        },
        ParityMatrixArea {
            area: "Credentials & Provider Secrets",
            island_desktop: "Windows Credential Manager (CredReadW/CredWriteW) by alias",
            android_mobile: "Forbidden: stores ONLY pinned TLS SHA-256 + pairing session token",
            parity_rule: "validate_mobile_config_no_provider_keys rejects all LLM keys",
        },
        ParityMatrixArea {
            area: "Transport Pairing",
            island_desktop: "LAN QR SVG + «Pair again» revocation",
            android_mobile: "QR scanner + pinned SHA-256 OkHttp CertificatePinner / TrustManager",
            parity_rule: "Zero open listeners unless pair mode or remote tunnel enabled",
        },
    ]
}
