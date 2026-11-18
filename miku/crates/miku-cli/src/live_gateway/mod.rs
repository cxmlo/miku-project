//! Live loopback gateway runner bridging `miku-gateway` (`127.0.0.1:47400`) to all
//! workspace crates (`miku-storage`, `miku-gatekeeper`, `miku-agent`, `miku-skills`,
//! `miku-browser`, `miku-ui`) in compact modular submodules (< 600 lines each).

mod agent_roster;
mod agent_tool_exec;
mod agent_tool_vision;
mod agent_tools;
mod api_handler;
mod api_routes;
mod app_catalog;
mod app_launcher;
mod extended_routes;
mod harness_routes;
mod harness_tools;
mod link_routes;
mod llm_client;
mod llm_multimodal;
mod media_routes;
mod model_sync;
mod neural_tts;
mod runtime_state;
mod storage_actor;
mod telemetry;
mod token_pipeline;
mod voice_dictation;
mod winhttp_https;

use std::env;
use std::net::{IpAddr, Ipv4Addr, SocketAddr};
use std::path::PathBuf;
use std::sync::{mpsc, Arc};
use std::thread::JoinHandle as ThreadJoinHandle;

use miku_core::{build_current_thread_runtime, spawn_orchestrator};
use miku_gateway::{
    start_gateway_server_with_loopback, GatewayConfig, LoopbackServerConfig,
    DEFAULT_LOOPBACK_PORT,
};
use miku_protocol::{BaseContractBundle, IslandState};
use tokio::sync::oneshot;

pub use self::api_handler::LiveMikuApiHandler;
pub use self::storage_actor::{AgentProfileDto, McpServerEntry};

#[derive(Debug, Clone)]
pub struct LiveGatewayConfig {
    pub data_dir: PathBuf,
    pub port: u16,
    pub default_ui_dir: Option<PathBuf>,
    pub user_override_dir: Option<PathBuf>,
    pub use_memory_secrets: bool,
}

impl Default for LiveGatewayConfig {
    fn default() -> Self {
        let local_app = env::var_os("LOCALAPPDATA")
            .map(PathBuf::from)
            .unwrap_or_else(env::temp_dir);
        let data_dir = local_app.join("Miku").join("data");
        Self {
            data_dir,
            port: DEFAULT_LOOPBACK_PORT,
            default_ui_dir: None,
            user_override_dir: None,
            use_memory_secrets: false,
        }
    }
}

pub struct LiveGatewayRunner {
    pub local_addr: SocketAddr,
    pub session_token: String,
    api_handler: Arc<LiveMikuApiHandler>,
    stop_tx: Option<oneshot::Sender<()>>,
    driver_thread: Option<ThreadJoinHandle<()>>,
}

impl LiveGatewayRunner {
    pub fn start(config: LiveGatewayConfig) -> Result<Self, String> {
        Self::start_with_lan(config, false)
    }

    pub fn start_with_lan(config: LiveGatewayConfig, allow_lan: bool) -> Result<Self, String> {
        let api_handler = Arc::new(LiveMikuApiHandler::new(
            &config.data_dir,
            config.port,
            config.use_memory_secrets,
        )?);
        let lan_active = allow_lan;
        let (init_tx, init_rx) = mpsc::channel::<Result<(SocketAddr, String), String>>();
        let (stop_tx, stop_rx) = oneshot::channel::<()>();

        let handler_for_thread = Arc::clone(&api_handler);
        let driver_thread = std::thread::Builder::new()
            .name("miku-gateway-driver".to_string())
            .spawn(move || {
                let rt = match build_current_thread_runtime() {
                    Ok(rt) => rt,
                    Err(e) => {
                        let _ = init_tx.send(Err(e));
                        return;
                    }
                };
                rt.block_on(async move {
                    let contracts = match BaseContractBundle::load_embedded_defaults() {
                        Ok(c) => c,
                        Err(e) => {
                            let _ = init_tx.send(Err(e.to_string()));
                            return;
                        }
                    };
                    let (orchestrator, orch_join) =
                        spawn_orchestrator(contracts, "miku-live-gateway-orch".to_string(), 64);
                    let _ = orchestrator.transition_state(IslandState::Idle).await;

                    let handler_for_token = Arc::clone(&handler_for_thread);
                    let loop_cfg = LoopbackServerConfig {
                        user_override_dir: config.user_override_dir.clone(),
                        default_ui_dir: config.default_ui_dir.clone(),
                        session_token: None,
                        api_handler: Some(handler_for_thread),
                        allow_lan_pairing: lan_active,
                    };

                    let primary_gw_cfg = if lan_active {
                        GatewayConfig {
                            bind_addr: SocketAddr::new(
                                IpAddr::V4(Ipv4Addr::UNSPECIFIED),
                                config.port,
                            ),
                            allow_lan_pairing: true,
                            ..GatewayConfig::loopback(config.port)
                        }
                    } else {
                        GatewayConfig::loopback(config.port)
                    };

                    let start_res = match start_gateway_server_with_loopback(
                        primary_gw_cfg,
                        orchestrator.clone(),
                        loop_cfg.clone(),
                    )
                    .await
                    {
                        Ok(pair) => Ok(pair),
                        Err(e) if lan_active => {
                            eprintln!("MIKU_LAN_BIND_WARN: fallback loopback ({})", e);
                            start_gateway_server_with_loopback(
                                GatewayConfig::loopback(config.port),
                                orchestrator.clone(),
                                loop_cfg,
                            )
                            .await
                        }
                        Err(e) => Err(e),
                    };

                    let (gw_handle, gw_join) = match start_res {
                        Ok(pair) => pair,
                        Err(e) => {
                            let _ = init_tx.send(Err(e.to_string()));
                            orchestrator.trigger_system_shutdown_signal();
                            let _ = orch_join.await;
                            return;
                        }
                    };

                    let client_addr = if gw_handle.local_addr.ip().is_unspecified() {
                        SocketAddr::new(
                            IpAddr::V4(Ipv4Addr::LOCALHOST),
                            gw_handle.local_addr.port(),
                        )
                    } else {
                        gw_handle.local_addr
                    };
                    let tok = gw_handle.session_token().unwrap_or_default().to_string();
                    handler_for_token.set_gateway_endpoint(
                        client_addr.port(),
                        "miku-live-gateway-orch",
                    );
                    let _ = init_tx.send(Ok((client_addr, tok)));

                    let _ = stop_rx.await;
                    gw_handle.trigger_shutdown();
                    orchestrator.trigger_system_shutdown_signal();
                    let _ = gw_join.await;
                    let _ = orch_join.await;
                });
            })
            .map_err(|e| format!("spawn gateway driver thread: {}", e))?;

        let (local_addr, session_token) = init_rx
            .recv()
            .map_err(|_| "gateway driver exited before binding".to_string())??;

        let relay_info = serde_json::json!({
            "port": local_addr.port(),
            "token": session_token
        });
        let _ = std::fs::create_dir_all(&config.data_dir);
        let _ = std::fs::write(
            config.data_dir.join("hook_relay.json"),
            relay_info.to_string(),
        );

        Ok(Self {
            local_addr,
            session_token,
            api_handler,
            stop_tx: Some(stop_tx),
            driver_thread: Some(driver_thread),
        })
    }

    pub fn api_handler(&self) -> &Arc<LiveMikuApiHandler> {
        &self.api_handler
    }

    pub fn run_until_ctrl_c(self) -> Result<(), String> {
        let rt = build_current_thread_runtime()?;
        rt.block_on(async {
            let _ = tokio::signal::ctrl_c().await;
        });
        self.shutdown_cleanly()
    }

    pub fn shutdown_cleanly(mut self) -> Result<(), String> {
        let _ = self.api_handler.checkpoint_storage();
        if let Some(tx) = self.stop_tx.take() {
            let _ = tx.send(());
        }
        if let Some(j) = self.driver_thread.take() {
            j.join()
                .map_err(|_| "gateway driver thread panicked on shutdown".to_string())?;
        }
        Ok(())
    }
}

impl Drop for LiveGatewayRunner {
    fn drop(&mut self) {
        if let Some(tx) = self.stop_tx.take() {
            let _ = tx.send(());
        }
        if let Some(j) = self.driver_thread.take() {
            let _ = j.join();
        }
    }
}
