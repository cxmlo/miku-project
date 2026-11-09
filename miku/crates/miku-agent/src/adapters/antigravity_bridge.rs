use crate::bridge::{
    AgentBridge, AgentBridgeError, AgentBridgeEvent, AgentBridgeResponse, AgentBridgeStatus,
};
use crate::catalog::{AgentCard, AgentManifest};

pub const ANTIGRAVITY_UNSUPPORTED_REASON: &str =
    "No confirmed direct integration protocol in repository; Gatekeeper policy verified for 'antigravity' in Spike 0.6; passive observation only";

pub const ANTIGRAVITY_SPIKE_06_REASON: &str = ANTIGRAVITY_UNSUPPORTED_REASON;

/// Bridge adapter for Google Antigravity (F-04, F-29).
/// Adopts a conservative stance: reports typed `Unsupported` status because the repository
/// does not contain a confirmed direct integration protocol or verified IPC hooks.
/// Spike 0.6 verified Gatekeeper policy enforcement (path jail violation) when `agent_id`
/// is "antigravity", but did not test protocol existence. Direct connections remain
/// `Unsupported` without simulating unconfirmed mechanisms.
pub struct AntigravityBridge {
    manifest: AgentManifest,
    status: AgentBridgeStatus,
    observation_log_count: u64,
}

impl AntigravityBridge {
    pub fn new(manifest: AgentManifest) -> Self {
        Self {
            manifest,
            status: AgentBridgeStatus::Unsupported {
                reason: ANTIGRAVITY_UNSUPPORTED_REASON.to_string(),
            },
            observation_log_count: 0,
        }
    }

    pub fn observation_log_count(&self) -> u64 {
        self.observation_log_count
    }
}

impl AgentBridge for AntigravityBridge {
    fn agent_id(&self) -> &str {
        &self.manifest.id
    }

    fn manifest(&self) -> &AgentManifest {
        &self.manifest
    }

    fn status(&self) -> AgentBridgeStatus {
        self.status.clone()
    }

    fn render_card(&self) -> AgentCard {
        let mood_indicator = self.status.mood_indicator().to_string();
        let summary = match &self.status {
            AgentBridgeStatus::Unsupported { reason } => {
                format!(
                    "Google Antigravity: unsupported direct IPC ({}); passive observation available",
                    reason
                )
            }
            AgentBridgeStatus::Active => {
                format!(
                    "Google Antigravity passive observation active ({} logs ingested)",
                    self.observation_log_count
                )
            }
            AgentBridgeStatus::Idle => {
                "Google Antigravity passive observation standing by".to_string()
            }
            AgentBridgeStatus::Offline => {
                "Google Antigravity passive observation offline".to_string()
            }
            other => format!("Google Antigravity: {:?}", other),
        };

        AgentCard {
            agent_id: self.manifest.id.clone(),
            display_name: self.manifest.display_name.clone(),
            status: self.status.clone(),
            mood_indicator,
            capabilities: self.manifest.capabilities.clone(),
            icon_slug: self.manifest.icon_slug.clone(),
            summary,
        }
    }

    fn handle_event(
        &mut self,
        event: &AgentBridgeEvent,
    ) -> Result<AgentBridgeResponse, AgentBridgeError> {
        match event {
            AgentBridgeEvent::Heartbeat => Ok(AgentBridgeResponse::Acknowledged {
                status: self.status.clone(),
            }),
            AgentBridgeEvent::ToolCall { .. } => {
                Err(AgentBridgeError::Unsupported(format!(
                    "Direct IPC tool dispatch is unsupported for {} (no confirmed protocol in repository; passive observation only)",
                    self.manifest.id
                )))
            }
            AgentBridgeEvent::StatusSync { payload, .. } => {
                if let Some(log_payload) = payload {
                    self.observation_log_count += 1;
                    if log_payload.contains("active") {
                        self.status = AgentBridgeStatus::Active;
                    }
                }
                Ok(AgentBridgeResponse::Acknowledged {
                    status: self.status.clone(),
                })
            }
            AgentBridgeEvent::Disconnect => {
                self.status = AgentBridgeStatus::Offline;
                Ok(AgentBridgeResponse::Acknowledged {
                    status: self.status.clone(),
                })
            }
        }
    }
}
