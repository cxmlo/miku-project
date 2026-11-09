use std::time::{SystemTime, UNIX_EPOCH};

use miku_gatekeeper::{GatekeeperEngine, GatekeeperVerdict};

use crate::bridge::{
    AgentBridge, AgentBridgeError, AgentBridgeEvent, AgentBridgeResponse, AgentBridgeStatus,
};
use crate::catalog::{AgentCard, AgentManifest};

fn current_timestamp_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

/// Bridge adapter for Anthropic Claude Code CLI assistant (F-04).
/// Integrates bidirectional stdio hook framing with Gatekeeper verification.
pub struct ClaudeBridge {
    manifest: AgentManifest,
    gatekeeper: GatekeeperEngine,
    status: AgentBridgeStatus,
}

impl ClaudeBridge {
    pub fn new(manifest: AgentManifest, gatekeeper: GatekeeperEngine) -> Self {
        Self {
            manifest,
            gatekeeper,
            status: AgentBridgeStatus::Idle,
        }
    }

    pub fn set_status(&mut self, status: AgentBridgeStatus) {
        self.status = status;
    }
}

impl AgentBridge for ClaudeBridge {
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
            AgentBridgeStatus::Active => "Claude Code is active and executing commands".to_string(),
            AgentBridgeStatus::Idle => "Claude Code is connected and standing by".to_string(),
            AgentBridgeStatus::Thinking => "Claude Code is reasoning or planning".to_string(),
            AgentBridgeStatus::Offline => "Claude Code session is disconnected".to_string(),
            AgentBridgeStatus::Unsupported { reason } => {
                format!("Claude Code unsupported: {}", reason)
            }
            AgentBridgeStatus::Unavailable { reason } => {
                format!("Claude Code unavailable: {}", reason)
            }
            AgentBridgeStatus::Error { message, .. } => {
                format!("Claude Code error: {}", message)
            }
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
            AgentBridgeEvent::ToolCall {
                tool_name,
                capability,
                target,
                payload_json,
            } => {
                let now = current_timestamp_ms();
                let verdict = self.gatekeeper.evaluate_request(
                    &self.manifest.id,
                    *capability,
                    target,
                    payload_json,
                    now,
                );

                match verdict {
                    GatekeeperVerdict::Allow { .. } => {
                        self.status = AgentBridgeStatus::Active;
                        Ok(AgentBridgeResponse::ToolExecuted {
                            output: format!("dispatched {} to {}", tool_name, target),
                        })
                    }
                    GatekeeperVerdict::Deny { reason, detail, .. } => {
                        Err(AgentBridgeError::GatekeeperDenied(format!(
                            "{}: {}",
                            reason.as_code(),
                            detail
                        )))
                    }
                    GatekeeperVerdict::AskPending { ticket, .. } => {
                        Err(AgentBridgeError::GatekeeperDenied(format!(
                            "confirmation_pending: {}",
                            ticket.req_hash
                        )))
                    }
                }
            }
            AgentBridgeEvent::StatusSync { payload, .. } => {
                if let Some(data) = payload {
                    if data.contains("thinking") {
                        self.status = AgentBridgeStatus::Thinking;
                    } else if data.contains("working") || data.contains("active") {
                        self.status = AgentBridgeStatus::Active;
                    } else {
                        self.status = AgentBridgeStatus::Idle;
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
