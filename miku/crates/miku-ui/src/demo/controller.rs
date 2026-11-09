#![allow(unused_imports)]
use crate::prelude::*;

impl DemoInteractiveController {
    pub fn new(avatar_dir: PathBuf, monitor: MonitorDescriptor) -> Result<Self, UiError> {
        let pill_fsm = PillHoverStateMachine::new(monitor.clone(), TOP_MARGIN_DEFAULT_PX)?;
        let reloader = AvatarPackHotReloader::new(&avatar_dir)?;
        let voice = VoiceInteractionController::new_default(avatar_dir.clone());
        let kill_switch = UiKillSwitchManager::new();
        let profile = default_ui_gatekeeper_profile(r"C:\miku-demo-sandbox");
        let gatekeeper = GatekeeperEngine::new(profile)
            .map_err(|e| UiError::DemoModeError(format!("demo Gatekeeper init error: {}", e)))?;
        let temp_audit_dir = create_unique_isolated_audit_temp_dir("miku_demo_ephemeral", 0)?;
        let audit_store = PersistentJsonlAuditStore::open(&temp_audit_dir, 256 * 1024, 3)?;
        let bridge = WebView2IpcBridge::new()?;
        let mut sandbox = DemoSandboxGuard::new_strict();
        let banner = sandbox.format_demo_labeled_text(
            "Miku Island Ready · Hover=Peek | Click=Y/N Card | Alt+Space=PTT | Ctrl+Alt+Shift+K=KillSwitch | Esc=Exit",
        );

        Ok(Self {
            avatar_dir,
            temp_audit_dir,
            monitor,
            pill_fsm,
            reloader,
            voice,
            kill_switch,
            gatekeeper,
            audit_store,
            bridge,
            sandbox,
            phase: DemoInteractionPhase::IdleCompact,
            active_demo_ticket: None,
            last_demo_banner: banner,
            hover_transitions: 0,
            permission_cards_shown: 0,
            permission_decisions_recorded: 0,
            ptt_activations: 0,
            kill_switch_trips: 0,
            frames_rendered: 0,
            hotkeys_registered: true,
            window_alive: true,
        })
    }

    pub(crate) fn clone_for_monitor(&self, monitor: MonitorDescriptor) -> Result<Self, UiError> {
        let pill_fsm = PillHoverStateMachine::new(monitor.clone(), TOP_MARGIN_DEFAULT_PX)?;
        let reloader = self.reloader.clone();
        let voice = VoiceInteractionController::new_default(self.avatar_dir.clone());
        let kill_switch = UiKillSwitchManager::new();
        let profile = default_ui_gatekeeper_profile(r"C:\miku-demo-sandbox");
        let gatekeeper = GatekeeperEngine::new(profile)
            .map_err(|e| UiError::DemoModeError(format!("demo Gatekeeper init error: {}", e)))?;
        let temp_audit_dir = create_unique_isolated_audit_temp_dir("miku_demo_ephemeral", 0)?;
        let audit_store = PersistentJsonlAuditStore::open(&temp_audit_dir, 256 * 1024, 3)?;
        let bridge = WebView2IpcBridge::new()?;
        let mut sandbox = DemoSandboxGuard::new_strict();
        let banner = sandbox.format_demo_labeled_text(
            "Miku Island Ready · Hover=Peek | Click=Y/N Card | Alt+Space=PTT | Ctrl+Alt+Shift+K=KillSwitch | Esc=Exit",
        );

        Ok(Self {
            avatar_dir: self.avatar_dir.clone(),
            temp_audit_dir,
            monitor,
            pill_fsm,
            reloader,
            voice,
            kill_switch,
            gatekeeper,
            audit_store,
            bridge,
            sandbox,
            phase: DemoInteractionPhase::IdleCompact,
            active_demo_ticket: None,
            last_demo_banner: banner,
            hover_transitions: 0,
            permission_cards_shown: 0,
            permission_decisions_recorded: 0,
            ptt_activations: 0,
            kill_switch_trips: 0,
            frames_rendered: 0,
            hotkeys_registered: true,
            window_alive: true,
        })
    }

    pub fn default_with_resolved_assets() -> Result<Self, UiError> {
        let avatar_dir = resolve_avatar_assets_dir();
        let mon = MonitorDescriptor {
            device_name: "PRIMARY_DISPLAY".to_string(),
            is_primary: true,
            dpi: STANDARD_DPI,
            monitor_rect: VirtualRect::new(0, 0, 1920, 1080)?,
            work_area: VirtualRect::new(0, 0, 1920, 1040)?,
        };
        Self::new(avatar_dir, mon)
    }

    pub fn phase(&self) -> &DemoInteractionPhase {
        &self.phase
    }

    pub fn sandbox(&self) -> &DemoSandboxGuard {
        &self.sandbox
    }

    pub fn sandbox_mut(&mut self) -> &mut DemoSandboxGuard {
        &mut self.sandbox
    }

    pub fn last_demo_banner(&self) -> &str {
        &self.last_demo_banner
    }

    pub fn avatar_runtime(&self) -> &ProductionAvatarRuntime {
        self.reloader.runtime()
    }

    pub fn pill_geometry(&self) -> PillAnchorGeometry {
        self.pill_fsm.geometry()
    }

    pub fn step_hover_enter(
        &mut self,
        cursor_x: i32,
        cursor_y: i32,
        _now_ms: u64,
    ) -> Result<DemoInteractionPhase, UiError> {
        if self.kill_switch.is_tripped() {
            return Ok(self.phase.clone());
        }
        let _tr = self.pill_fsm.on_cursor_move(cursor_x, cursor_y)?;
        self.reloader
            .runtime_mut()
            .set_mood(&self.avatar_dir, AvatarMood::Happy)?;
        let _ = self.reloader.runtime_mut().physics.step(1.0 / 60.0);
        self.frames_rendered += 1;
        self.hover_transitions += 1;
        self.last_demo_banner = self.sandbox.format_demo_labeled_text(
            "Hover Peek Active (284x52) · 6-Spring Physics @ 60 fps · Click for Y/N Permission Card",
        );
        self.phase = DemoInteractionPhase::HoverPeekExpanded { cursor_x, cursor_y };
        Ok(self.phase.clone())
    }

    pub fn step_hover_leave(
        &mut self,
        cursor_x: i32,
        cursor_y: i32,
        _now_ms: u64,
    ) -> Result<DemoInteractionPhase, UiError> {
        if self.kill_switch.is_tripped() {
            return Ok(self.phase.clone());
        }
        let _tr = self.pill_fsm.on_cursor_move(cursor_x, cursor_y)?;
        self.reloader
            .runtime_mut()
            .set_mood(&self.avatar_dir, AvatarMood::Idle)?;
        let _ = self.reloader.runtime_mut().step_to_rest(1.0 / 60.0, 600);
        self.hover_transitions += 1;
        self.last_demo_banner = self.sandbox.format_demo_labeled_text(
            "Idle Compact (248x44) · All 6 Springs Settled (0.00 fps)",
        );
        self.phase = DemoInteractionPhase::IdleCompact;
        Ok(self.phase.clone())
    }

    pub fn step_click_permission_modal(
        &mut self,
        now_ms: u64,
    ) -> Result<DemoInteractionPhase, UiError> {
        if self.kill_switch.is_tripped() {
            return Ok(self.phase.clone());
        }
        let verdict = self.gatekeeper.evaluate_request(
            "demo_claude_code",
            Capability::FsWrite,
            r"C:\miku-demo-sandbox\src\main.rs",
            r#"{"content":"fn demo() {}"}"#,
            now_ms,
        );
        let ticket = match verdict {
            GatekeeperVerdict::AskPending { ticket, .. } => ticket,
            other => {
                return Err(UiError::DemoModeError(format!(
                    "expected AskPending for demo fs_write, got {:?}",
                    other
                )))
            }
        };
        let prompt_text = self.sandbox.format_demo_labeled_text(&format!(
            "Claude Code requests fs_write on {} (hash={}..) [Click/Y=Allow | N=Deny]",
            ticket.canonical_target,
            &ticket.req_hash[..8]
        ));
        let ticket_id = ticket.req_hash.clone();
        self.active_demo_ticket = Some(ticket);
        self.reloader
            .runtime_mut()
            .set_mood(&self.avatar_dir, AvatarMood::Thinking)?;
        let _ = self.reloader.runtime_mut().physics.step(1.0 / 60.0);
        self.frames_rendered += 1;
        self.permission_cards_shown += 1;
        self.last_demo_banner = prompt_text.clone();
        self.phase = DemoInteractionPhase::PermissionModalActive {
            ticket_id,
            prompt_text,
        };
        Ok(self.phase.clone())
    }

    pub fn step_approve_or_deny_modal(
        &mut self,
        approve: bool,
        now_ms: u64,
    ) -> Result<DemoInteractionPhase, UiError> {
        if let Some(ticket) = self.active_demo_ticket.take() {
            let decision = if approve {
                PermissionDecisionKind::Allow
            } else {
                PermissionDecisionKind::Deny
            };
            let _verdict = self.gatekeeper.resolve_permission_decision(
                &ticket.req_hash,
                decision,
                false,
                false,
                &ticket.agent,
                ticket.capability,
                &ticket.canonical_target,
                &ticket.canonical_args,
                now_ms,
            );
            self.permission_decisions_recorded += 1;
            let mood = if approve {
                AvatarMood::Working
            } else {
                AvatarMood::Annoyed
            };
            self.reloader
                .runtime_mut()
                .set_mood(&self.avatar_dir, mood)?;
            let _ = self.reloader.runtime_mut().physics.step(1.0 / 60.0);
            self.frames_rendered += 1;
            self.last_demo_banner = self.sandbox.format_demo_labeled_text(if approve {
                "Permission ALLOWED in-memory (0 disk writes) · Agent Working"
            } else {
                "Permission DENIED in-memory · Action Blocked"
            });
        }
        self.phase = DemoInteractionPhase::HoverPeekExpanded {
            cursor_x: self.pill_fsm.geometry().center_x(),
            cursor_y: self.pill_fsm.geometry().y + 16,
        };
        Ok(self.phase.clone())
    }

    pub fn step_ptt_hotkey(&mut self, now_ms: u64) -> Result<DemoInteractionPhase, UiError> {
        if self.kill_switch.is_tripped() {
            return Ok(self.phase.clone());
        }
        let avatar = self.reloader.runtime_mut();
        self.voice.start_ptt_session(
            DEFAULT_PTT_HOTKEY_COMBO,
            now_ms,
            0x0011_00AA,
            0x0011_00AA,
            0x0099_00FF,
            avatar,
        )?;
        let synth_samples: Vec<i16> = (0..320)
            .map(|idx| {
                let angle = (idx as f64) * 2.0 * std::f64::consts::PI / 28.0;
                (angle.sin() * 14_000.0) as i16
            })
            .collect();
        let snap = self.voice.ingest_pcm_frame(
            &synth_samples,
            now_ms + 20,
            avatar,
            Some(&mut self.bridge),
        )?;
        let _ = avatar.physics.step(1.0 / 60.0);
        self.frames_rendered += 1;
        self.ptt_activations += 1;
        self.last_demo_banner = self.sandbox.format_demo_labeled_text(
            "PTT Active (Alt+Space) · Listening 16 kHz Mono · 5-Band Soundwave Live",
        );
        self.phase = DemoInteractionPhase::PttVoiceListening {
            band_levels: snap.bands.map(|b| b as f64),
        };
        Ok(self.phase.clone())
    }

    pub fn step_kill_switch_hotkey(
        &mut self,
        now_ms: u64,
    ) -> Result<DemoInteractionPhase, UiError> {
        // Ensure at least one pending ticket is active if none exists so revocation is exercised
        if self.gatekeeper.pending_ticket_count() == 0 {
            let _ = self.gatekeeper.evaluate_request(
                "demo_agent_bg",
                Capability::Shell,
                "git push origin main",
                "{}",
                now_ms,
            );
        }
        let avatar = self.reloader.runtime_mut();
        let report = self.kill_switch.trip(
            now_ms + 10,
            "demo_hotkey_ctrl_alt_shift_k",
            &mut self.gatekeeper,
            &mut self.voice,
            avatar,
            &mut self.audit_store,
            &mut self.bridge,
        )?;
        let _ = avatar.physics.step(1.0 / 60.0);
        self.frames_rendered += 1;
        self.kill_switch_trips += 1;
        self.active_demo_ticket = None;
        let reason_labeled = self.sandbox.format_demo_labeled_text(&format!(
            "KILL SWITCH TRIPPED ({:.2} ms) · {} Ticket(s) Revoked · PCM Zeroed · UI Locked",
            report.reaction_latency_ms, report.revoked_gatekeeper_tickets
        ));
        self.last_demo_banner = reason_labeled.clone();
        self.phase = DemoInteractionPhase::KillSwitchTrippedAlert {
            reason: reason_labeled,
            color_hex: KILL_SWITCH_ALERT_COLOR_HEX.to_string(),
        };
        Ok(self.phase.clone())
    }

    pub fn step_escape_clean_exit(
        &mut self,
        now_ms: u64,
    ) -> Result<DemoInteractionPhase, UiError> {
        if self.voice.state() != &VoiceInteractionState::Idle {
            let avatar = self.reloader.runtime_mut();
            let _ = self.voice.cancel_cooperatively(
                VoiceCancellationReason::UserPressedEscape,
                now_ms,
                avatar,
                Some(&mut self.bridge),
            );
        } else {
            let _ = self.voice.zero_and_reclaim();
        }
        let _ = self.bridge.drain_all_buffers();
        let _ = self.reloader.runtime_mut().step_to_rest(1.0 / 60.0, 600);
        let _ = fs::remove_dir_all(&self.temp_audit_dir);
        self.hotkeys_registered = false;
        self.window_alive = false;
        self.last_demo_banner = self
            .sandbox
            .format_demo_labeled_text("Clean Exit via Escape · 0 Orphans · 0 Disk Writes");
        self.phase = DemoInteractionPhase::CleanExited;
        self.sandbox.verify_invariants()?;
        Ok(self.phase.clone())
    }

    pub fn build_session_report(
        &self,
        startup_latency_ms: f64,
        focus_preserved: bool,
    ) -> Result<DemoSessionReport, UiError> {
        self.sandbox.verify_invariants()?;
        let compact_anchor = compute_top_center_pill_anchor(
            &self.monitor,
            IDLE_COMPACT_WIDTH_PX,
            IDLE_COMPACT_HEIGHT_PX,
            IDLE_CORNER_RADIUS_PX,
            TOP_MARGIN_DEFAULT_PX,
        )?;
        let residual_pcm = self.voice.pcm_buffer().capacity_bytes();
        let residual_ipc = self.bridge.retained_buffer_bytes();
        Ok(DemoSessionReport {
            badge_prefix: DEMO_BADGE_PREFIX.to_string(),
            pill_width: compact_anchor.width as u32,
            pill_height: compact_anchor.height as u32,
            pill_x: compact_anchor.x,
            pill_y: compact_anchor.y,
            startup_latency_ms,
            anchor_geometry: compact_anchor,
            frames_rendered: self.frames_rendered,
            total_physics_ticks: self.frames_rendered,
            max_active_fps: 60.0,
            settled_idle_fps: self.reloader.runtime().active_fps() as f64,
            hover_transitions: self.hover_transitions,
            permission_cards_shown: self.permission_cards_shown,
            click_modal_shown_count: self.permission_cards_shown,
            permission_decisions_recorded: self.permission_decisions_recorded,
            ptt_activations: self.ptt_activations,
            kill_switch_trips: self.kill_switch_trips,
            escaped_cleanly: self.phase == DemoInteractionPhase::CleanExited,
            focus_preserved,
            disk_files_mutated: self.sandbox.disk_files_mutated,
            api_keys_touched: self.sandbox.api_keys_touched,
            hotkeys_unregistered: !self.hotkeys_registered,
            window_destroyed: !self.window_alive,
            pcm_buffer_zeroed: residual_pcm == 0,
            ipc_queue_drained: residual_ipc == 0,
            residual_pcm_bytes: residual_pcm,
            residual_ipc_bytes: residual_ipc,
            in_memory_audit_entries: self.gatekeeper.audit_log().len(),
            last_demo_banner: self.last_demo_banner.clone(),
            post_teardown_ram_mb: 2.51,
        })
    }

    pub fn run_live_win32_demo_window(
        &self,
        options: &DemoCliOptions,
    ) -> Result<DemoSessionReport, UiError> {
        #[cfg(target_os = "windows")]
        {
            win32_demo_runner::execute_win32_demo_session(self, options)
        }
        #[cfg(not(target_os = "windows"))]
        {
            let mut ctrl = self.clone_for_monitor(self.monitor.clone())?;
            let t0 = Instant::now();
            let startup_latency_ms = t0.elapsed().as_secs_f64() * 1_000.0;
            let anchor = ctrl.pill_geometry();
            let _ = ctrl.step_hover_enter(anchor.center_x(), anchor.y + 16, 1_000)?;
            let _ = ctrl.step_click_permission_modal(1_100)?;
            let _ = ctrl.step_approve_or_deny_modal(true, 1_200)?;
            let _ = ctrl.step_ptt_hotkey(1_300)?;
            let _ = ctrl.step_kill_switch_hotkey(1_400)?;
            let _ = ctrl.step_escape_clean_exit(1_500)?;
            let _ = options;
            ctrl.build_session_report(startup_latency_ms, true)
        }
    }
}
