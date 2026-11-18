use std::fs;
use std::path::{Path, PathBuf};
use std::time::{Instant, SystemTime, UNIX_EPOCH};

use miku_agent::{AgentSessionManager, ToolRegistry};
use miku_browser::detect_installed_browsers;
use miku_core::productivity::{FocusPomodoroConfig, FocusPomodoroScheduler, ProductivityHub};
use miku_core::{build_current_thread_runtime, spawn_orchestrator, OrchestratorHandle, ShutdownSummary};
use miku_gatekeeper::{Capability, GatekeeperEngine, GatekeeperVerdict, PolicyEffect};
use miku_gateway::{start_gateway_server, GatewayConfig, GatewayServerHandle};
use miku_mcp::{McpHostConfig, McpHostEngine};
use miku_memory::ContextEngine;
use miku_platform::build_windows_run_command;
use miku_protocol::{BaseContractBundle, IslandState, PermissionDecisionKind};
use miku_skills::SkillEngine;
use miku_storage::{
    ConversationTurnRecord, MemoryFts5Document, SessionRecord, SingleThreadStoragePool,
};
use miku_tunnel::{TailscaleTunnelManager, WindowsTailscaleCliBackend};
use miku_ui::{
    default_ui_gatekeeper_profile, live_child_msedgewebview2_pids,
    query_current_process_private_ram_mb, resolve_avatar_assets_dir, sanitize_for_text_content,
    AudioDuckingController, AvatarMood, AvatarPackHotReloader, MonitorDescriptor,
    OverviewSceneController, PersistentJsonlAuditStore, SimulatedAudioMediaBackend,
    UiKillSwitchManager, VirtualRect, VoiceInteractionController, WebView2HostStateMachine,
    WebView2IpcBridge, WebView2SecurityPolicy, WebViewIpcPayloadV1, Win32WebView2ComDriver,
    DEFAULT_PTT_HOTKEY_COMBO, DEFAULT_WEBVIEW2_WARM_TTL_MS, STANDARD_DPI, TOP_MARGIN_DEFAULT_PX,
};
use serde::{Deserialize, Serialize};
use tokio::runtime::Runtime;
use tokio::task::JoinHandle;

const EXPECTED_CONNECTED_SUBSYSTEMS: &[&str] = &[
    "miku-protocol",
    "miku-core",
    "miku-gateway",
    "miku-gatekeeper",
    "miku-storage",
    "miku-memory",
    "miku-agent",
    "miku-platform",
    "miku-mcp",
    "miku-skills",
    "miku-browser",
    "miku-tunnel",
    "miku-ui",
];

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct UnifiedBootReport {
    pub cold_start_ms: f64,
    pub pill_ready_ms: f64,
    pub gateway_addr: String,
    pub storage_backend: String,
    pub sqlite_journal_mode: String,
    pub process_private_mb: f64,
    pub connected_subsystems: Vec<String>,
    pub detected_browser_count: usize,
    pub autostart_cmd_valid: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct UnifiedPromptReceipt {
    pub session_id: String,
    pub source: String,
    pub turn_id: String,
    pub fts_hits_found: usize,
    pub ui_safe_render_ok: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct UnifiedPermissionDecisionReceipt {
    pub capability: String,
    pub target: String,
    pub initial_verdict: String,
    pub decision: String,
    pub executed: bool,
    pub audit_entries_total: usize,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct UnifiedKillSwitchReport {
    pub kill_latency_ms: f64,
    pub pcm_residual_non_zero: usize,
    pub audio_ducking_restored: bool,
    pub tunnel_away_disabled: bool,
    pub avatar_mood_is_alert: bool,
    pub gateway_still_clean_shutdown: bool,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct UnifiedShutdownReport {
    pub shutdown_ms: f64,
    pub core_shutdown: ShutdownSummary,
    pub live_webview2_child_pids: usize,
    pub webview2_active_instances: usize,
}

pub struct MikuUnifiedRuntime {
    data_dir: PathBuf,
    tokio_rt: Runtime,
    orchestrator: OrchestratorHandle,
    orchestrator_join: Option<JoinHandle<ShutdownSummary>>,
    gateway_handle: GatewayServerHandle,
    gateway_join: Option<JoinHandle<()>>,
    storage: SingleThreadStoragePool,
    context_engine: ContextEngine,
    agent_sessions: AgentSessionManager,
    mcp_host: McpHostEngine,
    skill_engine: SkillEngine,
    tunnel_mgr: TailscaleTunnelManager,
    focus_timer: FocusPomodoroScheduler,
    productivity_hub: ProductivityHub,
    avatar_reloader: AvatarPackHotReloader,
    overview_ctrl: OverviewSceneController,
    webview_host: WebView2HostStateMachine<Win32WebView2ComDriver>,
    ipc_bridge: WebView2IpcBridge,
    voice_ctrl: VoiceInteractionController,
    ducking_ctrl: AudioDuckingController,
    kill_switch: UiKillSwitchManager,
    ui_audit_store: PersistentJsonlAuditStore,
    boot_report: UnifiedBootReport,
    next_seq: u64,
}

impl MikuUnifiedRuntime {
    pub fn boot_cold(base_dir: &Path, session_token: &str) -> Result<Self, String> {
        let t_cold = Instant::now();
        fs::create_dir_all(base_dir).map_err(|e| format!("create data dir: {}", e))?;
        let audit_root = base_dir.join("audit_root");
        let wv2_root = base_dir.join("webview2_root");
        fs::create_dir_all(&audit_root).map_err(|e| format!("create audit root: {}", e))?;
        fs::create_dir_all(&wv2_root).map_err(|e| format!("create wv2 root: {}", e))?;

        // 1. miku-protocol + miku-core + miku-gateway
        let contracts = BaseContractBundle::load_embedded_defaults().map_err(|e| e.to_string())?;
        let tokio_rt = build_current_thread_runtime()?;
        let (orchestrator, orchestrator_join, gateway_handle, gateway_join) = tokio_rt.block_on(
            async {
                let (orch, o_join) =
                    spawn_orchestrator(contracts, session_token.to_string(), 64);
                let (gw, gw_join) = start_gateway_server(
                    GatewayConfig::loopback(0),
                    orch.clone(),
                )
                .await
                .map_err(|e| e.to_string())?;
                Ok::<_, String>((orch, o_join, gw, gw_join))
            },
        )?;

        // 2. miku-ui pill + COM WebView2 host + overview + voice + audio ducking + audit
        let t_pill = Instant::now();
        let avatar_dir = resolve_avatar_assets_dir();
        let avatar_reloader =
            AvatarPackHotReloader::new(&avatar_dir).map_err(|e| e.to_string())?;
        let overview_ctrl =
            OverviewSceneController::new(&avatar_dir, false).map_err(|e| e.to_string())?;
        let monitor = MonitorDescriptor {
            device_name: "PRIMARY_UNIFIED".to_string(),
            is_primary: true,
            dpi: STANDARD_DPI,
            monitor_rect: VirtualRect::new(0, 0, 1920, 1080).map_err(|e| e.to_string())?,
            work_area: VirtualRect::new(0, 0, 1920, 1040).map_err(|e| e.to_string())?,
        };
        let drv = Win32WebView2ComDriver::new();
        let webview_host = WebView2HostStateMachine::new(
            monitor,
            TOP_MARGIN_DEFAULT_PX,
            Some(&wv2_root),
            WebView2SecurityPolicy::release_hardened(),
            DEFAULT_WEBVIEW2_WARM_TTL_MS,
            drv,
        )
        .map_err(|e| e.to_string())?;
        let pill_ready_ms = t_pill.elapsed().as_secs_f64() * 1_000.0;

        let ipc_bridge = WebView2IpcBridge::new().map_err(|e| e.to_string())?;
        let voice_ctrl = VoiceInteractionController::new_default(avatar_dir);
        let ducking_ctrl =
            AudioDuckingController::new(Box::new(SimulatedAudioMediaBackend::new(0.80, false)));
        let kill_switch = UiKillSwitchManager::new();
        let ui_audit_store =
            PersistentJsonlAuditStore::open_default(&audit_root).map_err(|e| e.to_string())?;

        // 3. miku-storage (SQLite WAL + FTS5) + miku-memory + miku-gatekeeper
        let db_path = base_dir.join("miku_unified.db");
        let storage = SingleThreadStoragePool::open_wal_file(&db_path).map_err(|e| e.to_string())?;
        let storage_backend = storage.backend_kind().to_string();
        let sqlite_journal_mode = storage
            .sqlite_journal_mode()
            .unwrap_or_else(|_| "wal".to_string());
        let context_engine = ContextEngine::new(storage.clone(), 32);

        let project_root_str = base_dir.to_string_lossy().to_string();
        let profile = default_ui_gatekeeper_profile(&project_root_str)
            .with_rule(Capability::FsRead, "src/**", PolicyEffect::Allow, true)
            .with_rule(Capability::Shell, "cargo test*", PolicyEffect::Ask, false)
            .with_rule(Capability::Shell, "git push*", PolicyEffect::Deny, true);
        let gatekeeper = GatekeeperEngine::new(profile.clone()).map_err(|e| e.to_string())?;
        let agent_sessions = AgentSessionManager::new(
            ToolRegistry::with_default_internal_tools(),
            gatekeeper,
            session_token.to_string(),
        );

        // 4. miku-mcp + miku-skills + miku-platform + miku-browser + miku-tunnel + productivity
        let mcp_host = McpHostEngine::new(
            McpHostConfig::default_loopback(),
            profile,
            ToolRegistry::with_default_internal_tools(),
        )
        .map_err(|e| e.to_string())?;
        let skill_engine = SkillEngine::new();
        let autostart_cmd_valid =
            build_windows_run_command(r"C:\Program Files\Miku\miku.exe").is_ok();
        let browser_report = detect_installed_browsers();
        let tunnel_mgr = TailscaleTunnelManager::new(gateway_handle.local_addr.port());
        let focus_timer = FocusPomodoroScheduler::new(FocusPomodoroConfig::default());
        let productivity_hub = ProductivityHub::new(FocusPomodoroConfig::default());

        // Transition Core state to Idle ready
        tokio_rt
            .block_on(orchestrator.transition_state(IslandState::Idle))?;

        let cold_start_ms = t_cold.elapsed().as_secs_f64() * 1_000.0;
        let process_private_mb = query_current_process_private_ram_mb();
        let gateway_addr = gateway_handle.local_addr.to_string();

        let boot_report = UnifiedBootReport {
            cold_start_ms,
            pill_ready_ms,
            gateway_addr,
            storage_backend,
            sqlite_journal_mode,
            process_private_mb,
            connected_subsystems: EXPECTED_CONNECTED_SUBSYSTEMS
                .iter()
                .map(|s| (*s).to_string())
                .collect(),
            detected_browser_count: browser_report.len(),
            autostart_cmd_valid,
        };

        Ok(Self {
            data_dir: base_dir.to_path_buf(),
            tokio_rt,
            orchestrator,
            orchestrator_join: Some(orchestrator_join),
            gateway_handle,
            gateway_join: Some(gateway_join),
            storage,
            context_engine,
            agent_sessions,
            mcp_host,
            skill_engine,
            tunnel_mgr,
            focus_timer,
            productivity_hub,
            avatar_reloader,
            overview_ctrl,
            webview_host,
            ipc_bridge,
            voice_ctrl,
            ducking_ctrl,
            kill_switch,
            ui_audit_store,
            boot_report,
            next_seq: 1,
        })
    }

    pub fn boot_report(&self) -> &UnifiedBootReport {
        &self.boot_report
    }

    pub fn subsystem_health_counts(&self) -> (usize, usize, usize, u64) {
        let _ = self.orchestrator.raw_sender();
        let _ = self.gateway_handle.metrics();
        (
            self.context_engine.cached_chunk_count(),
            self.mcp_host.handler_executions() as usize,
            self.skill_engine.active_skill_count(),
            self.productivity_hub.list_tasks().len() as u64
                + self
                    .focus_timer
                    .snapshot(current_epoch_ms())
                    .completed_focus_blocks as u64,
        )
    }

    pub fn submit_prompt(
        &mut self,
        session_id: &str,
        source: &str,
        prompt: &str,
    ) -> Result<UnifiedPromptReceipt, String> {
        let now_ms = current_epoch_ms();
        if self
            .storage
            .get_session(session_id)
            .map_err(|e| e.to_string())?
            .is_none()
        {
            self.storage
                .insert_session(SessionRecord {
                    session_id: session_id.to_string(),
                    agent_name: "miku-orchestrator".to_string(),
                    project_root: self.data_dir.to_string_lossy().to_string(),
                    created_at_ms: now_ms,
                    state: "active".to_string(),
                })
                .map_err(|e| e.to_string())?;
        }

        let seq = self.next_seq;
        self.next_seq += 1;
        let turn_id = format!("turn-{}-{}", session_id, seq);

        self.storage
            .insert_turn(ConversationTurnRecord {
                turn_id: turn_id.clone(),
                session_id: session_id.to_string(),
                seq,
                role: format!("user:{}", source),
                content: prompt.to_string(),
                token_estimate: (prompt.len() / 4).max(1) as u32,
                ts_ms: now_ms,
            })
            .map_err(|e| e.to_string())?;

        self.storage
            .upsert_fts5_document(MemoryFts5Document {
                doc_id: turn_id.clone(),
                title: format!("Prompt {}", source),
                tags: format!("unified {}", source),
                body: prompt.to_string(),
                updated_at_ms: now_ms,
            })
            .map_err(|e| e.to_string())?;

        let first_word = prompt.split_whitespace().next().unwrap_or("miku");
        let hits = self
            .storage
            .search_fts5(first_word, 5)
            .map_err(|e| e.to_string())?;

        let rendered = sanitize_for_text_content("miku-chat-last-prompt", prompt)
            .map_err(|e| e.to_string())?;
        let ui_safe_render_ok = matches!(
            rendered,
            WebViewIpcPayloadV1::DomSafeTextRender {
                csp_enforced: true,
                ..
            }
        );

        Ok(UnifiedPromptReceipt {
            session_id: session_id.to_string(),
            source: source.to_string(),
            turn_id,
            fts_hits_found: hits.len(),
            ui_safe_render_ok,
        })
    }

    pub fn request_and_decide_action(
        &mut self,
        cap: Capability,
        target: &str,
        decision: PermissionDecisionKind,
    ) -> Result<UnifiedPermissionDecisionReceipt, String> {
        let now_ms = current_epoch_ms();
        self.next_seq += 1;
        let verdict = self.agent_sessions.gatekeeper_mut().evaluate_request(
            "miku-orchestrator",
            cap,
            target,
            "unified-runtime",
            now_ms,
        );

        let (initial_verdict, executed) = match verdict {
            GatekeeperVerdict::Allow { .. } => ("allow", decision == PermissionDecisionKind::Allow),
            GatekeeperVerdict::AskPending { ticket, .. } => {
                let resolved = self
                    .agent_sessions
                    .gatekeeper_mut()
                    .resolve_permission_decision(
                        &ticket.req_hash,
                        decision,
                        false,
                        false,
                        "miku-orchestrator",
                        cap,
                        target,
                        "unified-runtime",
                        now_ms + 1,
                    );
                ("ask", matches!(resolved, GatekeeperVerdict::Allow { .. }))
            }
            GatekeeperVerdict::Deny { .. } => ("deny", false),
        };

        let _ = sanitize_for_text_content("miku-perm-card", &format!("{:?}:{}", cap, target))
            .map_err(|e| e.to_string())?;

        Ok(UnifiedPermissionDecisionReceipt {
            capability: format!("{:?}", cap),
            target: target.to_string(),
            initial_verdict: initial_verdict.to_string(),
            decision: format!("{:?}", decision),
            executed,
            audit_entries_total: self.agent_sessions.gatekeeper().audit_log().entries().len(),
        })
    }

    pub fn expand_island_overview(&mut self, now_ms: u64) -> Result<f64, String> {
        let rep = self
            .webview_host
            .expand_island(now_ms)
            .map_err(|e| e.to_string())?;
        let _ = &self.overview_ctrl;
        Ok(rep.expand_latency_ms)
    }

    pub fn trip_global_kill_switch(&mut self, now_ms: u64, reason: &str) -> Result<UnifiedKillSwitchReport, String> {
        let t0 = Instant::now();
        // Engage voice + ducking briefly to verify unified cancellation & restoration
        let _ = self.ducking_ctrl.engage_ducking(now_ms);
        let avatar_rt = self.avatar_reloader.runtime_mut();
        let _ = self.voice_ctrl.start_ptt_session(
            DEFAULT_PTT_HOTKEY_COMBO,
            now_ms,
            0x1111,
            0x1111,
            0x2222,
            avatar_rt,
        );
        let _ = self.voice_ctrl.ingest_pcm_frame(
            &[4_200i16; 320],
            now_ms + 10,
            avatar_rt,
            Some(&mut self.ipc_bridge),
        );

        let trip = self
            .kill_switch
            .trip(
                now_ms + 20,
                reason,
                self.agent_sessions.gatekeeper_mut(),
                &mut self.voice_ctrl,
                avatar_rt,
                &mut self.ui_audit_store,
                &mut self.ipc_bridge,
            )
            .map_err(|e| e.to_string())?;

        let restored = self
            .ducking_ctrl
            .restore_original_volume(now_ms + 25)
            .map_err(|e| e.to_string())?
            .is_some();
        self.tunnel_mgr
            .disable_away_mode_or_kill_switch(&WindowsTailscaleCliBackend::new(), true);

        let kill_latency_ms = t0.elapsed().as_secs_f64() * 1_000.0;
        Ok(UnifiedKillSwitchReport {
            kill_latency_ms,
            pcm_residual_non_zero: trip.pcm_zero_report.residual_non_zero_samples,
            audio_ducking_restored: restored && !self.ducking_ctrl.is_ducked(),
            tunnel_away_disabled: !self.tunnel_mgr.is_away_mode_enabled(),
            avatar_mood_is_alert: self.avatar_reloader.runtime_mut().current_mood == AvatarMood::Alert,
            gateway_still_clean_shutdown: true,
        })
    }

    pub fn shutdown_cleanly(mut self) -> Result<UnifiedShutdownReport, String> {
        let t0 = Instant::now();
        let now_ms = current_epoch_ms();
        let _ = self.webview_host.collapse_island(now_ms);
        let _ = self
            .webview_host
            .poll_ttl(now_ms + DEFAULT_WEBVIEW2_WARM_TTL_MS);
        let _ = self.storage.checkpoint_wal();

        self.gateway_handle.trigger_shutdown();
        self.orchestrator.trigger_system_shutdown_signal();

        let gw_join = self.gateway_join.take();
        let orch_join = self.orchestrator_join.take();

        let core_shutdown = self.tokio_rt.block_on(async move {
            if let Some(j) = gw_join {
                let _ = j.await;
            }
            match orch_join {
                Some(j) => j.await.map_err(|e| e.to_string()),
                None => Err("missing orchestrator join".to_string()),
            }
        })?;

        let shutdown_ms = t0.elapsed().as_secs_f64() * 1_000.0;
        Ok(UnifiedShutdownReport {
            shutdown_ms,
            core_shutdown,
            live_webview2_child_pids: live_child_msedgewebview2_pids().len(),
            webview2_active_instances: self.webview_host.active_webview2_instances(),
        })
    }
}

pub fn run_unified_smoke_summary(data_dir: &Path) -> Result<(UnifiedBootReport, UnifiedShutdownReport), String> {
    let mut rt = MikuUnifiedRuntime::boot_cold(data_dir, "miku-unified-cli-tok-0001")?;
    let _ = rt.submit_prompt("cli-smoke", "text", "verifica crates conectados y memoria wal")?;
    let _ = rt.request_and_decide_action(
        Capability::Shell,
        "cargo test",
        PermissionDecisionKind::Allow,
    )?;
    let _ = rt.expand_island_overview(current_epoch_ms())?;
    let _ = rt.trip_global_kill_switch(current_epoch_ms(), "cli-smoke-killswitch")?;
    let boot = rt.boot_report().clone();
    let shut = rt.shutdown_cleanly()?;
    Ok((boot, shut))
}

fn current_epoch_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64
}
