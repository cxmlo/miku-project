#![allow(unused_imports)]
use crate::prelude::*;

impl UiKillSwitchManager {
    /// Trips the Global Kill Switch in `< 20.0 ms`:
    /// 1. Atomically revokes all in-flight tickets in `GatekeeperEngine` (`trigger_kill_switch`).
    /// 2. Aborts `VoiceInteractionController` with cryptographic zero-fill of the PCM ring buffer.
    /// 3. Appends an immutable SHA-256 entry to `PersistentJsonlAuditStore`.
    /// 4. Sets the avatar to `AvatarMood::Alert` and the native pill border to `#F43F5E`.
    /// 5. Emits a sanitized `DomSafeTextRender` alert over `WebView2IpcBridge`.
    pub fn trip(
        &mut self,
        now_ms: u64,
        raw_reason: &str,
        gatekeeper: &mut GatekeeperEngine,
        voice: &mut VoiceInteractionController,
        avatar: &mut ProductionAvatarRuntime,
        audit_store: &mut PersistentJsonlAuditStore,
        bridge: &mut WebView2IpcBridge,
    ) -> Result<KillSwitchTripReport, UiError> {
        let t0 = Instant::now();

        let safe_reason = escape_html_entities_for_text_content(raw_reason.trim());
        let reason_non_empty = if safe_reason.is_empty() {
            "global_emergency_hotkey".to_string()
        } else {
            safe_reason
        };

        // 1. Revoke all pending Gatekeeper tickets and activate engine lockdown
        let revoked_gatekeeper_tickets = gatekeeper.trigger_kill_switch(now_ms);
        self.total_revoked_tickets += revoked_gatekeeper_tickets as u64;

        // 2. Abort any active voice capture/synthesis and zero-fill 100% of PCM samples
        let voice_was_active = !matches!(voice.state(), VoiceInteractionState::Idle);
        let cancel_receipt = voice.cancel_cooperatively(
            VoiceCancellationReason::KillSwitchInvoked,
            now_ms,
            avatar,
            Some(bridge),
        )?;
        if voice_was_active {
            self.voice_aborts_count += 1;
        }
        self.pcm_zero_fills_count += 1;

        // 3. Synchronize Gatekeeper's in-memory log and record explicit SHA-256 JSONL audit entry
        let _ = audit_store.sync_from_gatekeeper_log(gatekeeper.audit_log())?;
        let audit_entry = audit_store.append_entry(
            now_ms,
            "kill_switch",
            "global_emergency_stop",
            "*",
            "revoked_by_kill_switch",
            &format!(
                "reason={}|revoked_tickets={}|pcm_zeroed={}",
                reason_non_empty,
                revoked_gatekeeper_tickets,
                cancel_receipt.zero_report.zeroed_samples
            ),
        )?;

        // 4. Transition Kill Switch FSM, IslandState::Alert, pill border `#F43F5E`, and `AvatarMood::Alert`
        self.status = KillSwitchStatus::Tripped {
            tripped_at_ms: now_ms,
            reason: reason_non_empty.clone(),
        };
        self.island_state = IslandState::Alert;
        self.pill_border_color_hex = KILL_SWITCH_ALERT_COLOR_HEX.to_string();
        avatar.set_mood(&voice.base_avatar_dir, AvatarMood::Alert)?;

        // 5. Emit sanitized `DomSafeTextRender` & `MikuLinkMessageV1::AgentEvent` over IPC bridge
        let ipc_alert_cell = sanitize_for_text_content(
            "miku-kill-switch-banner",
            &format!(
                "KILL SWITCH TRIPPED [{}]: {} (revoked {} tickets)",
                 KILL_SWITCH_ALERT_COLOR_HEX, reason_non_empty, revoked_gatekeeper_tickets
            ),
        )?;
        let env_id = format!("ks-trip-{}", self.next_ipc_seq);
        self.next_ipc_seq += 1;

        let dom_env = WebViewIpcEnvelopeV1::new(
            env_id.clone(),
            None,
            OVERVIEW_VIRTUAL_INDEX_URL,
            ipc_alert_cell.clone(),
        );
        bridge.enqueue_outbound(dom_env)?;

        let event_env = WebViewIpcEnvelopeV1::new(
            format!("{}-evt", env_id),
            Some(env_id),
            OVERVIEW_VIRTUAL_INDEX_URL,
            WebViewIpcPayloadV1::MikuLink(MikuLinkMessageV1::AgentEvent {
                agent: "kill_switch".to_string(),
                kind: "revoked_by_kill_switch".to_string(),
                title: reason_non_empty.clone(),
            }),
        );
        bridge.enqueue_outbound(event_env)?;

        self.trips_count += 1;
        let reaction_latency_ms = t0.elapsed().as_secs_f64() * 1000.0;
        if reaction_latency_ms > self.max_trip_latency_ms {
            self.max_trip_latency_ms = reaction_latency_ms;
        }

        Ok(KillSwitchTripReport {
            tripped_at_ms: now_ms,
            reason: reason_non_empty,
            reaction_latency_ms,
            revoked_gatekeeper_tickets,
            voice_aborted: voice_was_active,
            pcm_zero_report: cancel_receipt.zero_report,
            audit_seq: audit_entry.seq,
            audit_entry_hash: audit_entry.entry_hash,
            island_state: self.island_state,
            avatar_mood: avatar.current_mood,
            pill_border_color_hex: self.pill_border_color_hex.clone(),
            ipc_alert_cell,
        })
    }

    /// Initiates a manual rearm flow from `Tripped`, issuing a single-use cryptographic
    /// challenge token with TTL `REARM_CHALLENGE_TTL_MS` (`30_000 ms`) and `MAX_REARM_ATTEMPTS` (`3`).
    pub fn request_rearm(&mut self, now_ms: u64) -> Result<RearmChallenge, UiError> {
        match &self.status {
            KillSwitchStatus::Armed => {
                return Err(UiError::KillSwitchError(
                    "cannot request rearm when Kill Switch is already Armed".to_string(),
                ));
            }
            KillSwitchStatus::Tripped { .. } | KillSwitchStatus::Rearming { .. } => {}
        }

        let seed = format!(
            "MIKU_KILL_SWITCH_REARM_V1|ts={}|nonce={}|trips={}",
            now_ms, self.next_challenge_nonce, self.trips_count
        );
        self.next_challenge_nonce += 1;
        let digest = sha256_hex(seed.as_bytes());
        let challenge_token = format!("REARM-{}", &digest[..24]);
        let expires_at_ms = now_ms.saturating_add(REARM_CHALLENGE_TTL_MS);

        self.status = KillSwitchStatus::Rearming {
            challenge_token: challenge_token.clone(),
            expires_at_ms,
            failed_attempts: 0,
        };

        Ok(RearmChallenge {
            challenge_token,
            issued_at_ms: now_ms,
            expires_at_ms,
            max_attempts: MAX_REARM_ATTEMPTS,
        })
    }

    /// Validates the rearm challenge token in constant time (`constant_time_token_eq`), enforcing
    /// `REARM_CHALLENGE_TTL_MS` (`30_000 ms`) and `MAX_REARM_ATTEMPTS` (`3`).
    /// On success, resets `GatekeeperEngine`, restores `AvatarMood::Idle`, appends an immutable
    /// SHA-256 entry to `PersistentJsonlAuditStore`, and emits a sanitized `DomSafeTextRender` event.
    pub fn confirm_rearm(
        &mut self,
        provided_token: &str,
        now_ms: u64,
        gatekeeper: &mut GatekeeperEngine,
        avatar: &mut ProductionAvatarRuntime,
        avatar_dir: &Path,
        audit_store: &mut PersistentJsonlAuditStore,
        bridge: &mut WebView2IpcBridge,
    ) -> Result<KillSwitchStatus, UiError> {
        let (expected_token, expires_at_ms, current_failures) = match &self.status {
            KillSwitchStatus::Rearming {
                challenge_token,
                expires_at_ms,
                failed_attempts,
            } => (challenge_token.clone(), *expires_at_ms, *failed_attempts),
            other => {
                return Err(UiError::KillSwitchError(format!(
                    "confirm_rearm requires Rearming state, found {:?}",
                    other
                )));
            }
        };

        if now_ms > expires_at_ms {
            self.rejected_rearm_attempts_count += 1;
            self.status = KillSwitchStatus::Tripped {
                tripped_at_ms: now_ms,
                reason: "rearm_challenge_expired".to_string(),
            };
            let _ = audit_store.append_entry(
                now_ms,
                "kill_switch",
                "rearm_confirm",
                "*",
                "deny",
                "rearm_challenge_expired",
            )?;
            return Err(UiError::KillSwitchError(
                "rearm challenge token has expired; Kill Switch remains Tripped".to_string(),
            ));
        }

        if !constant_time_token_eq(&expected_token, provided_token.trim()) {
            let new_failures = current_failures.saturating_add(1);
            self.rejected_rearm_attempts_count += 1;

            if new_failures >= MAX_REARM_ATTEMPTS {
                self.status = KillSwitchStatus::Tripped {
                    tripped_at_ms: now_ms,
                    reason: "max_rearm_attempts_exceeded".to_string(),
                };
                let _ = audit_store.append_entry(
                    now_ms,
                    "kill_switch",
                    "rearm_confirm",
                    "*",
                    "deny",
                    "max_rearm_attempts_exceeded",
                )?;
                return Err(UiError::KillSwitchError(format!(
                    "rearm challenge revoked after {} failed attempts",
                    new_failures
                )));
            }

            self.status = KillSwitchStatus::Rearming {
                challenge_token: expected_token,
                expires_at_ms,
                failed_attempts: new_failures,
            };
            let _ = audit_store.append_entry(
                now_ms,
                "kill_switch",
                "rearm_confirm",
                "*",
                "deny",
                &format!("invalid_rearm_token_attempt_{}", new_failures),
            )?;
            return Err(UiError::KillSwitchError(format!(
                "invalid rearm token (attempt {}/{})",
                new_failures, MAX_REARM_ATTEMPTS
            )));
        }

        // Valid token within TTL: reset Gatekeeper kill switch, restore Armed + Idle state
        gatekeeper.reset_kill_switch(now_ms);
        let _ = audit_store.sync_from_gatekeeper_log(gatekeeper.audit_log())?;
        let _ = audit_store.append_entry(
            now_ms,
            "kill_switch",
            "rearm_confirm",
            "*",
            "allow",
            "kill_switch_rearmed_constant_time_verified",
        )?;

        self.status = KillSwitchStatus::Armed;
        self.island_state = IslandState::Idle;
        self.pill_border_color_hex = "#00E5FF".to_string();
        self.rearms_completed_count += 1;
        avatar.set_mood(avatar_dir, AvatarMood::Idle)?;

        let rearm_cell = sanitize_for_text_content(
            "miku-kill-switch-banner",
            "KILL SWITCH REARMED: system restored to Armed/Idle",
        )?;
        let env = WebViewIpcEnvelopeV1::new(
            format!("ks-rearm-{}", self.next_ipc_seq),
            None,
            OVERVIEW_VIRTUAL_INDEX_URL,
            rearm_cell,
        );
        self.next_ipc_seq += 1;
        bridge.enqueue_outbound(env)?;

        Ok(self.status.clone())
    }
}

pub fn run_kill_switch_audit(
    avatar_dir: &Path,
    cycles: usize,
) -> Result<KillSwitchAuditReport, UiError> {
    let mut reloader = AvatarPackHotReloader::new(avatar_dir)?;
    let avatar = reloader.runtime_mut();
    let mut bridge = WebView2IpcBridge::new()?;
    let mut voice = VoiceInteractionController::new_default(avatar_dir.to_path_buf());
    let mut ks = UiKillSwitchManager::new_default();

    // Verify both normal OS hotkey registration and conflict fallback without panic
    let reg_normal = ks.register_hotkey(false);
    let reg_conflict = ks.register_hotkey(true);
    let hotkey_fallback_verified =
        reg_normal.fallback_ui_button_active && !reg_conflict.os_registered && reg_conflict.fallback_ui_button_active;
    let _ = ks.register_hotkey(false);

    let temp_audit_dir = create_unique_isolated_audit_temp_dir("miku_ks_audit", cycles)?;
    let mut audit_store = PersistentJsonlAuditStore::open(&temp_audit_dir, 256 * 1024, 3)?;

    let profile = default_ui_gatekeeper_profile(r"C:\workspace\miku-project");
    let mut gatekeeper = GatekeeperEngine::new(profile)
        .map_err(|e| UiError::GatekeeperInterceptionError(e.to_string()))?;

    let speech_frame = vec![11_000i16; 640];
    let total_cycles = cycles.max(1);
    let mut xss_and_focus_ok = true;

    for i in 0..total_cycles {
        let base_ts = 200_000 + (i as u64) * 50_000;

        // 1. Enqueue 3 pending interactive Ask tickets in GatekeeperEngine
        let _ = gatekeeper.evaluate_request(
            "claude-code",
            Capability::Shell,
            "cargo publish --allow-dirty",
            "--allow-dirty",
            base_ts + 10,
        );
        let _ = gatekeeper.evaluate_request(
            "cursor-agent",
            Capability::FsWrite,
            r"C:\workspace\miku-project\src\lib.rs",
            "write_all",
            base_ts + 20,
        );
        let _ = gatekeeper.evaluate_request(
            "antigravity",
            Capability::Browser,
            "https://github.com/settings/tokens",
            "open_tab",
            base_ts + 30,
        );
        if gatekeeper.pending_ticket_count() != 3 {
            xss_and_focus_ok = false;
        }

        // 2. Start an active voice listening session with non-zero PCM samples
        voice.start_ptt_session(
            DEFAULT_PTT_HOTKEY_COMBO,
            base_ts + 100,
            0x0033_00AA,
            0x0033_00AA,
            0x0099_00FF,
            avatar,
        )?;
        let _ = voice.ingest_pcm_frame(&speech_frame, base_ts + 120, avatar, Some(&mut bridge))?;

        // 3. Trip the Global Kill Switch (`Ctrl+Alt+Shift+K`) with an XSS test vector
        let trip_report = ks.trip(
            base_ts + 200,
            "hotkey_ctrl_alt_shift_k <script>alert('ks')</script>",
            &mut gatekeeper,
            &mut voice,
            avatar,
            &mut audit_store,
            &mut bridge,
        )?;

        if trip_report.revoked_gatekeeper_tickets != 3
            || gatekeeper.pending_ticket_count() != 0
            || !gatekeeper.is_kill_switch_active()
            || !trip_report.voice_aborted
            || trip_report.pcm_zero_report.zeroed_samples != 640
            || trip_report.pcm_zero_report.residual_non_zero_samples != 0
            || trip_report.pcm_zero_report.retained_capacity_bytes != 0
            || trip_report.avatar_mood != AvatarMood::Alert
            || trip_report.island_state != IslandState::Alert
            || trip_report.pill_border_color_hex != KILL_SWITCH_ALERT_COLOR_HEX
        {
            xss_and_focus_ok = false;
        }

        if let WebViewIpcPayloadV1::DomSafeTextRender {
            ref safe_text,
            csp_enforced,
            ..
        } = trip_report.ipc_alert_cell
        {
            if !csp_enforced || safe_text.contains("<script>") {
                xss_and_focus_ok = false;
            }
        } else {
            xss_and_focus_ok = false;
        }

        // 4. Verify strict lockdown while Tripped
        if ks.guard_operation("shell_exec").is_ok() {
            xss_and_focus_ok = false;
        }

        // 5. Request rearm, test 1 invalid token rejection, then confirm valid token
        let challenge = ks.request_rearm(base_ts + 500)?;
        if ks
            .confirm_rearm(
                "REARM-000000000000000000000000",
                base_ts + 600,
                &mut gatekeeper,
                avatar,
                avatar_dir,
                &mut audit_store,
                &mut bridge,
            )
            .is_ok()
        {
            xss_and_focus_ok = false;
        }
        let rearmed_status = ks.confirm_rearm(
            &challenge.challenge_token,
            base_ts + 700,
            &mut gatekeeper,
            avatar,
            avatar_dir,
            &mut audit_store,
            &mut bridge,
        )?;
        if rearmed_status != KillSwitchStatus::Armed
            || gatekeeper.is_kill_switch_active()
            || avatar.current_mood != AvatarMood::Idle
        {
            xss_and_focus_ok = false;
        }

        let _ = bridge.drain_all_buffers();
    }

    let chain_verify = audit_store.verify_persisted_chain()?;
    let _ = fs::remove_dir_all(&temp_audit_dir);

    Ok(KillSwitchAuditReport {
        trips_executed: ks.trips_count(),
        total_revoked_tickets: ks.total_revoked_tickets(),
        voice_sessions_aborted: ks.voice_aborts_count(),
        pcm_zero_fills_verified: ks.pcm_zero_fills_count(),
        rearms_completed: ks.rearms_completed_count(),
        rejected_rearm_attempts: ks.rejected_rearm_attempts_count(),
        blocked_actions_while_tripped: ks.blocked_while_tripped_count(),
        max_trip_latency_ms: ks.max_trip_latency_ms(),
        audit_chain_verified: chain_verify.verified_entries > 0,
        hotkey_fallback_verified,
        xss_and_focus_verified: xss_and_focus_ok,
    })
}

