use crate::bridge::{
    AgentBridge, AgentBridgeError, AgentBridgeEvent, AgentBridgeResponse, AgentBridgeStatus,
};
use crate::catalog::{AgentCard, AgentManifest};

/// Bridge adapter for GitHub webhook notifications and API status events (F-04, F-29).
/// Enforces ETag conditional caching (304 Not Modified) to eliminate aggressive polling,
/// and guarantees zero secret leakage in cards, logs, and error messages.
pub struct GitHubBridge {
    manifest: AgentManifest,
    api_endpoint: String,
    auth_token: Option<String>,
    cached_etag: Option<String>,
    status: AgentBridgeStatus,
    sync_count: u64,
}

impl GitHubBridge {
    pub fn new(manifest: AgentManifest, api_endpoint: &str) -> Self {
        Self {
            manifest,
            api_endpoint: api_endpoint.to_string(),
            auth_token: None,
            cached_etag: None,
            status: AgentBridgeStatus::Idle,
            sync_count: 0,
        }
    }

    pub fn with_auth_token(
        manifest: AgentManifest,
        api_endpoint: &str,
        auth_token: Option<String>,
    ) -> Self {
        Self {
            manifest,
            api_endpoint: api_endpoint.to_string(),
            auth_token,
            cached_etag: None,
            status: AgentBridgeStatus::Idle,
            sync_count: 0,
        }
    }

    pub fn cached_etag(&self) -> Option<&str> {
        self.cached_etag.as_deref()
    }

    pub fn sync_count(&self) -> u64 {
        self.sync_count
    }

    fn sanitize_secret(&self, input: &str) -> String {
        if let Some(token) = &self.auth_token {
            if !token.is_empty() {
                return input.replace(token, "[REDACTED_SECRET]");
            }
        }
        input.to_string()
    }
}

impl AgentBridge for GitHubBridge {
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
            AgentBridgeStatus::Active => format!(
                "GitHub integration active (endpoint: {}, {} syncs)",
                self.api_endpoint, self.sync_count
            ),
            AgentBridgeStatus::Idle => format!(
                "GitHub integration standing by (endpoint: {})",
                self.api_endpoint
            ),
            AgentBridgeStatus::Thinking => "GitHub API request in flight".to_string(),
            AgentBridgeStatus::Offline => "GitHub integration disconnected".to_string(),
            AgentBridgeStatus::Unsupported { reason } => {
                format!("GitHub integration unsupported: {}", reason)
            }
            AgentBridgeStatus::Unavailable { reason } => {
                format!("GitHub integration unavailable: {}", reason)
            }
            AgentBridgeStatus::Error { message, .. } => {
                format!("GitHub integration error: {}", message)
            }
        };

        // Redact any accidentally embedded secrets
        let clean_summary = self.sanitize_secret(&summary);

        AgentCard {
            agent_id: self.manifest.id.clone(),
            display_name: self.manifest.display_name.clone(),
            status: self.status.clone(),
            mood_indicator,
            capabilities: self.manifest.capabilities.clone(),
            icon_slug: self.manifest.icon_slug.clone(),
            summary: clean_summary,
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
            AgentBridgeEvent::StatusSync { etag, .. } => {
                // ETag conditional check: if matching, avoid redundant parsing and return 304 NotModified
                if let (Some(incoming_tag), Some(cached)) = (etag, &self.cached_etag) {
                    if incoming_tag == cached {
                        return Ok(AgentBridgeResponse::NotModified {
                            etag: incoming_tag.clone(),
                        });
                    }
                }

                // New or updated ETag: update state without aggressive polling
                self.cached_etag = etag.clone();
                self.sync_count += 1;
                self.status = AgentBridgeStatus::Active;

                Ok(AgentBridgeResponse::Acknowledged {
                    status: self.status.clone(),
                })
            }
            AgentBridgeEvent::ToolCall {
                tool_name,
                payload_json,
                ..
            } => {
                let sanitized_payload = self.sanitize_secret(payload_json);
                let sanitized_tool = self.sanitize_secret(tool_name);
                Err(AgentBridgeError::InvalidPayload(format!(
                    "tool call '{}' with payload '{}' is not supported on GitHub webhook bridge",
                    sanitized_tool, sanitized_payload
                )))
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
