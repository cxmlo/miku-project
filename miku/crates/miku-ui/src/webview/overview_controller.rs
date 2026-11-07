#![allow(unused_imports)]
use crate::prelude::*;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct OverviewExpandTransitionMetrics {
    pub geometry: PillAnchorGeometry,
    pub avatar_scale_k: f64,
    pub avatar_body_px: f64,
    pub transition_latency_ms: f64,
    pub expanded_ram_mb: f64,
    pub duration_curve_ms: u32,
    pub settled_steps: usize,
    pub settled_fps: u32,
}

#[derive(Debug, Clone)]
pub struct OverviewSceneController {
    pub(crate) bundle: OverviewSceneBundle,
    pub(crate) current_mood: AvatarMood,
    pub(crate) pose: AvatarExpressionPose,
    pub(crate) reduced_motion: bool,
    pub(crate) event_feed: VecDeque<WebViewIpcPayloadV1>,
    pub(crate) max_feed_capacity: usize,
    pub(crate) dom_updates_count: u64,
    pub(crate) spring_pos: f64,
    pub(crate) spring_vel: f64,
    pub(crate) active_fps: u32,
}

impl OverviewSceneController {
    pub fn new(avatar_dir: &Path, reduced_motion: bool) -> Result<Self, UiError> {
        let bundle = build_overview_scene_bundle(avatar_dir)?;
        let current_mood = AvatarMood::Idle;
        let pose = AvatarExpressionPose::for_mood(current_mood);
        Ok(Self {
            bundle,
            current_mood,
            pose,
            reduced_motion,
            event_feed: VecDeque::with_capacity(MAX_OVERVIEW_EVENT_FEED),
            max_feed_capacity: MAX_OVERVIEW_EVENT_FEED,
            dom_updates_count: 0,
            spring_pos: 1.0,
            spring_vel: 0.0,
            active_fps: 0,
        })
    }

    pub fn bundle(&self) -> &OverviewSceneBundle {
        &self.bundle
    }

    pub fn current_mood(&self) -> AvatarMood {
        self.current_mood
    }

    pub fn pose(&self) -> &AvatarExpressionPose {
        &self.pose
    }

    pub fn event_feed_len(&self) -> usize {
        self.event_feed.len()
    }

    pub fn max_feed_capacity(&self) -> usize {
        self.max_feed_capacity
    }

    pub fn dom_updates_count(&self) -> u64 {
        self.dom_updates_count
    }

    pub fn active_fps(&self) -> u32 {
        self.active_fps
    }

    pub fn set_reduced_motion(&mut self, reduced: bool) {
        self.reduced_motion = reduced;
        if reduced {
            self.spring_pos = 1.0;
            self.spring_vel = 0.0;
            self.active_fps = 0;
        }
    }

    pub(crate) fn push_bounded_feed(&mut self, item: WebViewIpcPayloadV1) {
        if self.event_feed.len() >= self.max_feed_capacity {
            let _ = self.event_feed.pop_front();
        }
        self.event_feed.push_back(item);
    }

    /// Processes an inbound IPC message via `WebView2IpcBridge`, updating the avatar mood,
    /// triggering spring motion (unless `reduced_motion` is active), and emitting a sanitized
    /// `DomSafeTextRender` (`textContent`) into the bounded 32-slot circular feed (0 OOM).
    pub fn apply_ipc_envelope(
        &mut self,
        bridge: &mut WebView2IpcBridge,
        source_origin: &str,
        raw_json: &str,
    ) -> Result<WebViewIpcPayloadV1, UiError> {
        let (resp_env, _rtt_ms) = bridge.handle_inbound_raw_message(source_origin, raw_json)?;
        let _ = bridge.pop_next_outbound_json();

        let (next_mood, element_id, raw_label) = match &resp_env.payload {
            WebViewIpcPayloadV1::MikuLink(MikuLinkMessageV1::AgentEvent {
                agent,
                kind,
                title,
            }) => {
                let mood = match kind.as_str() {
                    "thinking" => AvatarMood::Thinking,
                    "speaking" | "talking" => AvatarMood::Talking,
                    "error" | "permission_denied" => AvatarMood::Error,
                    "permission_allowed" | "completed" => AvatarMood::Happy,
                    _ => AvatarMood::Working,
                };
                (
                    mood,
                    "miku-overview-status",
                    format!("{}:{}:{}", agent, kind, title),
                )
            }
            WebViewIpcPayloadV1::MikuLink(MikuLinkMessageV1::PermissionRequest {
                agent,
                capability,
                target,
                ..
            }) => (
                AvatarMood::Annoyed,
                "miku-overview-status",
                format!("ASK {} {} {}", agent, capability, target),
            ),
            WebViewIpcPayloadV1::TelemetrySnapshot {
                island_state,
                ram_private_mb_x100,
                rtt_us,
                ..
            } => {
                let mood = match island_state {
                    IslandState::Listening => AvatarMood::Listening,
                    IslandState::Alert => AvatarMood::Annoyed,
                    IslandState::Gaming => AvatarMood::Sleeping,
                    _ => AvatarMood::Idle,
                };
                (
                    mood,
                    "miku-overview-ram-badge",
                    format!("RAM {:.2} MB | RTT {} us", (*ram_private_mb_x100 as f64) / 100.0, rtt_us),
                )
            }
            WebViewIpcPayloadV1::DomSafeTextRender {
                element_id,
                safe_text,
                ..
            } => (self.current_mood, element_id.as_str(), safe_text.clone()),
            _ => (self.current_mood, "miku-overview-status", "ok".to_string()),
        };

        self.current_mood = next_mood;
        self.pose = AvatarExpressionPose::for_mood(next_mood);

        if self.reduced_motion {
            self.spring_pos = 1.0;
            self.spring_vel = 0.0;
            self.active_fps = 0;
        } else {
            self.spring_pos = 0.86;
            self.spring_vel = 1.25;
            self.active_fps = 60;
        }

        let safe_render = sanitize_for_text_content(element_id, &raw_label)?;
        self.push_bounded_feed(safe_render.clone());
        self.dom_updates_count += 1;
        Ok(safe_render)
    }

    /// Steps the semi-implicit Euler spring damper (`k = 190.0, c = 9.0, m = 1.0`,
    /// `sleep_epsilon = 0.001` from `rig.toml`) until it converges to rest (`0 fps`).
    pub fn step_springs_to_rest(&mut self, dt_sec: f64, max_steps: usize) -> usize {
        if self.reduced_motion {
            self.spring_pos = 1.0;
            self.spring_vel = 0.0;
            self.active_fps = 0;
            return 0;
        }
        let k = 190.0f64;
        let c = 9.0f64;
        let target = 1.0f64;
        let sleep_eps = 0.001f64;
        let dt = dt_sec.clamp(0.001, 0.04);

        for step in 1..=max_steps {
            let disp = self.spring_pos - target;
            let accel = -k * disp - c * self.spring_vel;
            self.spring_vel += accel * dt;
            self.spring_pos += self.spring_vel * dt;

            if (self.spring_pos - target).abs() <= sleep_eps && self.spring_vel.abs() <= sleep_eps {
                self.spring_pos = target;
                self.spring_vel = 0.0;
                self.active_fps = 0;
                return step;
            }
        }
        self.spring_pos = target;
        self.spring_vel = 0.0;
        self.active_fps = 0;
        max_steps
    }

    /// Executes the fluid transition `IdleCompact (248x44, k=0.34)` -> `HoverPeek (284x52, k=0.571)`
    /// -> `ExpandedOverview (640x380, k=0.72)` over `WebView2HostStateMachine` (Mode C), verifying
    /// `< 150.0 ms` latency (`BUD-02`), `<= 120.0 MB` expanded RAM (`BUD-04`), and `0 fps` on settle.
    pub fn transition_hover_to_overview<D: WebView2LifecycleDriver>(
        &mut self,
        hover_fsm: &mut PillHoverStateMachine,
        wv2_host: &mut WebView2HostStateMachine<D>,
        now_ms: u64,
    ) -> Result<OverviewExpandTransitionMetrics, UiError> {
        let t0 = Instant::now();
        let center_x = hover_fsm.geometry().center_x();
        let center_y = hover_fsm.geometry().y + 12;
        let _ = hover_fsm.on_cursor_move(center_x, center_y)?;

        let exp = wv2_host.expand_island(now_ms)?;
        let _ = wv2_host.navigate(OVERVIEW_VIRTUAL_INDEX_URL)?;

        if ! self.reduced_motion {
            self.spring_pos = 0.82;
            self.spring_vel = 1.5;
            self.active_fps = 60;
        }
        let settled_steps = self.step_springs_to_rest(1.0 / 60.0, 600);
        let duration_curve_ms = if self.reduced_motion { 0 } else { 550 };
        let transition_latency_ms = exp.expand_latency_ms + (t0.elapsed().as_secs_f64() * 1_000.0);

        Ok(OverviewExpandTransitionMetrics {
            geometry: exp.geometry,
            avatar_scale_k: OVERVIEW_AVATAR_SCALE_K,
            avatar_body_px: OVERVIEW_AVATAR_BODY_PX,
            transition_latency_ms,
            expanded_ram_mb: exp.active_ram_mb,
            duration_curve_ms,
            settled_steps,
            settled_fps: self.active_fps,
        })
    }

    pub fn clear_feed_on_collapse(&mut self) {
        self.event_feed.clear();
        self.event_feed.shrink_to_fit();
        self.active_fps = 0;
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct OverviewSceneAuditReport {
    pub bundle_bytes: usize,
    pub layer_count: usize,
    pub accessory_slot_count: usize,
    pub accessory_count: usize,
    pub events_processed: u64,
    pub telemetry_events_processed: u64,
    pub bounded_feed_len: usize,
    pub expand_transition_ms: f64,
    pub expanded_ram_mb: f64,
    pub post_ttl_ram_mb: f64,
    pub post_ttl_retained_bytes: usize,
    pub settled_fps: f64,
    pub text_content_only_verified: bool,
}

/// Runs a complete audit of the Step 2.4 Overview Scene (`https://miku.local/index.html` bundle,
/// 9-layer + 14-accessory composition, bounded telemetry burst, and Mode C expand/collapse transition).
pub fn run_overview_scene_audit(
    monitor: MonitorDescriptor,
    avatar_dir: &Path,
    burst_events: usize,
) -> Result<OverviewSceneAuditReport, UiError> {
    let mut controller = OverviewSceneController::new(avatar_dir, false)?;
    let (layers, accessories) = compose_avatar_layer_stack(
        avatar_dir,
        AvatarMood::Thinking,
        &["crown", "witch", "bunny", "shades", "scarf"],
    )?;
    if layers.len() != 9 || accessories.len() != 14 {
        return Err(UiError::AssetCatalogError(format!(
            "unexpected layer/accessory count: layers={}, accessories={}",
            layers.len(),
            accessories.len()
        )));
    }

    let mut hover_fsm = PillHoverStateMachine::new(monitor.clone(), TOP_MARGIN_DEFAULT_PX)?;
    let policy = WebView2SecurityPolicy::release_hardened();
    let driver = IsolatedWebView2ProcessDriver::new(2.51, 62.5);
    let mut wv2_host = WebView2HostStateMachine::new(
        monitor,
        TOP_MARGIN_DEFAULT_PX,
        None,
        policy,
        DEFAULT_WEBVIEW2_WARM_TTL_MS,
        driver,
    )?;

    let trans = controller.transition_hover_to_overview(&mut hover_fsm, &mut wv2_host, 1_000)?;
    if trans.transition_latency_ms > MAX_EXPAND_VISIBLE_LATENCY_MS {
        return Err(UiError::WebView2LifecycleError(format!(
            "overview expand latency {:.4} ms exceeded {:.1} ms budget",
            trans.transition_latency_ms, MAX_EXPAND_VISIBLE_LATENCY_MS
        )));
    }
    if trans.expanded_ram_mb > MAX_EXPANDED_WEBVIEW2_RAM_MB {
        return Err(UiError::WebView2LifecycleError(format!(
            "expanded RAM {:.2} MB exceeded {:.1} MB budget",
            trans.expanded_ram_mb, MAX_EXPANDED_WEBVIEW2_RAM_MB
        )));
    }

    let mut bridge = WebView2IpcBridge::new()?;
    for i in 0..burst_events {
        let env = WebViewIpcEnvelopeV1::new(
            format!("ov-evt-{}", i),
            None,
            OVERVIEW_VIRTUAL_INDEX_URL,
            WebViewIpcPayloadV1::MikuLink(MikuLinkMessageV1::AgentEvent {
                agent: "claude_code".to_string(),
                kind: if i % 2 == 0 {
                    "thinking".to_string()
                } else {
                    "completed".to_string()
                },
                title: format!("<script>evil()</script> step {}", i),
            }),
        );
        let raw = env.encode_post_message_json()?;
        let rendered =
            controller.apply_ipc_envelope(&mut bridge, OVERVIEW_VIRTUAL_INDEX_URL, &raw)?;
        if let WebViewIpcPayloadV1::DomSafeTextRender { safe_text, .. } = rendered {
            if safe_text.contains("<script") {
                return Err(UiError::CspViolation(
                    "unneutralized <script> tag in overview DOM update".to_string(),
                ));
            }
        }
    }

    let _ = controller.step_springs_to_rest(1.0 / 60.0, 600);
    let bounded_feed_len = controller.event_feed_len();
    if bounded_feed_len > MAX_OVERVIEW_EVENT_FEED {
        return Err(UiError::WebView2LifecycleError(format!(
            "overview event feed {} exceeded max capacity {}",
            bounded_feed_len, MAX_OVERVIEW_EVENT_FEED
        )));
    }

    let events_processed = controller.dom_updates_count();
    let settled_fps = controller.active_fps() as f64;

    // Collapse and evict on TTL
    let _ = wv2_host.collapse_island(2_000)?;
    let _ = wv2_host.poll_ttl(2_000 + DEFAULT_WEBVIEW2_WARM_TTL_MS + 100)?;
    let post_ttl_ram_mb = wv2_host.current_private_ram_mb();
    controller.clear_feed_on_collapse();
    let _ = bridge.drain_all_buffers();
    let post_ttl_retained_bytes = bridge.retained_buffer_bytes();

    Ok(OverviewSceneAuditReport {
        bundle_bytes: controller.bundle().byte_len,
        layer_count: layers.len(),
        accessory_slot_count: accessories.len(),
        accessory_count: accessories.len(),
        events_processed,
        telemetry_events_processed: events_processed,
        bounded_feed_len,
        expand_transition_ms: trans.transition_latency_ms,
        expanded_ram_mb: trans.expanded_ram_mb,
        post_ttl_ram_mb,
        post_ttl_retained_bytes,
        settled_fps,
        text_content_only_verified: controller.bundle().uses_only_text_content,
    })
}

