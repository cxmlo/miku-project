use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use miku_gatekeeper::{Capability, GatekeeperEngine, GatekeeperVerdict, PermissionTicket};
use miku_protocol::PermissionDecisionKind;
use serde::{Deserialize, Serialize};

use crate::discovery_and_profile::{
    build_isolated_launch_spec, discover_default_windows_browser, BrowserEngineKind,
    BrowserLaunchSpec,
};
use crate::protocol_drivers::BrowserBackend;
use crate::types_and_guards::{
    validate_browser_url, BrowserError, BrowserPageSnapshot, BrowserProtocolKind,
    DownloadJailPolicy, DownloadReceipt,
};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DownloadRequest {
    pub suggested_filename: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct BrowserApprovalOrigin {
    pub from_mobile: bool,
    pub biometric_verified: bool,
}

impl BrowserApprovalOrigin {
    pub const fn desktop() -> Self {
        Self {
            from_mobile: false,
            biometric_verified: false,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct IslandBrowserCard {
    pub domain: String,
    pub url: String,
    pub page_title: String,
    pub dom_excerpt: String,
    pub protocol_used: BrowserProtocolKind,
    pub isolated_profile_used: bool,
    pub downloaded_file: Option<DownloadReceipt>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum BrowserWorkflowOutcome {
    Completed(IslandBrowserCard),
    AwaitingApproval { ticket: PermissionTicket },
    Denied { reason_code: String, detail: String },
}

#[derive(Debug, Clone)]
struct PendingBrowserTask {
    agent: String,
    url: String,
    host: String,
    download: Option<DownloadRequest>,
    req_hash: String,
}

pub struct BrowserController<B: BrowserBackend> {
    backend: B,
    download_jail: DownloadJailPolicy,
    allow_loopback_http: bool,
    isolated_profile_used: bool,
    abort_flag: Arc<AtomicBool>,
    pending: HashMap<String, PendingBrowserTask>,
}

impl<B: BrowserBackend> BrowserController<B> {
    pub fn new(backend: B, download_jail: DownloadJailPolicy, allow_loopback_http: bool) -> Self {
        Self {
            backend,
            download_jail,
            allow_loopback_http,
            isolated_profile_used: true,
            abort_flag: Arc::new(AtomicBool::new(false)),
            pending: HashMap::new(),
        }
    }

    pub fn active_browser_processes(&self) -> usize {
        self.backend.active_processes()
    }

    pub fn is_abort_active(&self) -> bool {
        self.abort_flag.load(Ordering::SeqCst)
    }

    pub fn trip_kill_switch(&mut self, gatekeeper: &mut GatekeeperEngine, now_ms: u64) -> usize {
        self.abort_flag.store(true, Ordering::SeqCst);
        let cleared = self.pending.len();
        self.pending.clear();
        let revoked = gatekeeper.trigger_kill_switch(now_ms);
        cleared + revoked
    }

    pub fn prepare_visible_login_profile(
        &self,
        preferred_engine: Option<BrowserEngineKind>,
        profile_base: Option<&std::path::Path>,
    ) -> Result<BrowserLaunchSpec, BrowserError> {
        let report = discover_default_windows_browser();
        let candidate = match preferred_engine {
            Some(eng) => report
                .installed_candidates
                .into_iter()
                .find(|c| c.engine == eng)
                .or(report.selected),
            None => report.selected,
        }
        .ok_or_else(|| {
            BrowserError::ProtocolError("no supported Windows browser detected".to_string())
        })?;
        // Never headless when opening Miku's isolated profile for one-time manual login
        build_isolated_launch_spec(&candidate, profile_base, 0, false)
    }

    pub async fn browse_and_optional_download(
        &mut self,
        agent: &str,
        raw_url: &str,
        download: Option<DownloadRequest>,
        gatekeeper: &mut GatekeeperEngine,
        now_ms: u64,
    ) -> Result<BrowserWorkflowOutcome, BrowserError> {
        if self.abort_flag.load(Ordering::SeqCst) {
            return Err(BrowserError::KillSwitchAborted);
        }
        let (_scheme, host, _port) = validate_browser_url(raw_url, self.allow_loopback_http)?;

        match gatekeeper.evaluate_request(agent, Capability::Browser, &host, "", now_ms) {
            GatekeeperVerdict::Allow { .. } => {
                let card = self.run_authorized(raw_url, &host, download).await?;
                Ok(BrowserWorkflowOutcome::Completed(card))
            }
            GatekeeperVerdict::AskPending { ticket, .. } => {
                self.pending.insert(
                    ticket.req_hash.clone(),
                    PendingBrowserTask {
                        agent: agent.to_string(),
                        url: raw_url.to_string(),
                        host,
                        download,
                        req_hash: ticket.req_hash.clone(),
                    },
                );
                Ok(BrowserWorkflowOutcome::AwaitingApproval { ticket })
            }
            GatekeeperVerdict::Deny { reason, .. } => Ok(BrowserWorkflowOutcome::Denied {
                reason_code: format!("{:?}", reason),
                detail: format!("Gatekeeper denied Browser navigation to '{host}'"),
            }),
        }
    }

    pub async fn resolve_and_execute(
        &mut self,
        ticket_req_hash: &str,
        decision: PermissionDecisionKind,
        presented_req_hash: &str,
        origin: BrowserApprovalOrigin,
        gatekeeper: &mut GatekeeperEngine,
        now_ms: u64,
    ) -> Result<BrowserWorkflowOutcome, BrowserError> {
        let Some(pending) = self.pending.get(ticket_req_hash).cloned() else {
            return Ok(BrowserWorkflowOutcome::Denied {
                reason_code: "UnknownRequest".to_string(),
                detail: "no pending browser task for req_hash".to_string(),
            });
        };
        if presented_req_hash != pending.req_hash {
            return Ok(BrowserWorkflowOutcome::Denied {
                reason_code: "ReqHashMismatch".to_string(),
                detail: "anti-TOCTOU req_hash mismatch".to_string(),
            });
        }
        self.pending.remove(ticket_req_hash);
        match gatekeeper.resolve_permission_decision(
            ticket_req_hash,
            decision,
            origin.from_mobile,
            origin.biometric_verified,
            &pending.agent,
            Capability::Browser,
            &pending.host,
            "",
            now_ms,
        ) {
            GatekeeperVerdict::Allow { .. } => {
                let card = self
                    .run_authorized(&pending.url, &pending.host, pending.download)
                    .await?;
                Ok(BrowserWorkflowOutcome::Completed(card))
            }
            GatekeeperVerdict::Deny { reason, .. } => Ok(BrowserWorkflowOutcome::Denied {
                reason_code: format!("{:?}", reason),
                detail: format!(
                    "pending browser task for '{}' ({}) was denied",
                    pending.host, pending.agent
                ),
            }),
            GatekeeperVerdict::AskPending { ticket, .. } => {
                Ok(BrowserWorkflowOutcome::AwaitingApproval { ticket })
            }
        }
    }

    async fn run_authorized(
        &self,
        url: &str,
        host: &str,
        download: Option<DownloadRequest>,
    ) -> Result<IslandBrowserCard, BrowserError> {
        let snap: BrowserPageSnapshot = self
            .backend
            .navigate_and_read_dom(url, host, self.isolated_profile_used, &self.abort_flag)
            .await?;

        let receipt = if let Some(dl) = download {
            Some(
                self.backend
                    .download_artifact(url, &dl.suggested_filename, &self.download_jail, &self.abort_flag)
                    .await?,
            )
        } else {
            None
        };

        Ok(IslandBrowserCard {
            domain: snap.host,
            url: snap.url,
            page_title: snap.title,
            dom_excerpt: snap.dom_text,
            protocol_used: snap.protocol_used,
            isolated_profile_used: snap.isolated_profile_used,
            downloaded_file: receipt,
        })
    }
}
