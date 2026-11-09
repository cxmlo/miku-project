#![allow(unused_imports)]
use crate::feed::types_and_cache::*;
use crate::prelude::*;

/// Reason why feed synchronization was paused (`F-14`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FeedPauseReason {
    GamingModeActive,
    Offline,
}

/// Summary returned after a technical feed synchronization pass.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FeedSyncSummary {
    pub paused_reason: Option<FeedPauseReason>,
    pub sources_checked: usize,
    pub http_200_count: usize,
    pub http_304_count: usize,
    pub rate_limited_skips: usize,
    pub source_errors: usize,
    pub total_items: usize,
    pub reparse_delta: u64,
    pub ui_wake_delta: u64,
}

/// Verified outcome of opening a feed item URL in Zen Browser via Gatekeeper (`F-14`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ZenBrowserOpenOutcome {
    pub item_id: String,
    pub url: String,
    pub command_args: Vec<String>,
}

/// Island Technical Feed Controller (`F-14` / Micro-step 3.11).
///
/// Coordinates conditional HTTP fetching (`ETag`/`If-None-Match`, `Last-Modified`/`If-Modified-Since`),
/// zero-reparse/zero-UI-wake on `304 Not Modified`, per-source fault isolation, GitHub
/// `X-RateLimit-Remaining` enforcement, Gaming Mode & offline pause/resume, sanitized Island
/// rendering, and Gatekeeper-authorized link opening in Zen Browser.
pub struct IslandTechFeedController {
    sources: BTreeMap<String, TechFeedSourceConfig>,
    validators: BTreeMap<String, ConditionalValidators>,
    rate_limits: BTreeMap<String, RateLimitState>,
    source_cache: BTreeMap<String, Vec<TechFeedItem>>,
    merged_items: Vec<TechFeedItem>,
    last_rendered_fingerprint: Option<String>,
    reparse_count: u64,
    ui_wake_count: u64,
    gaming_mode_active: bool,
    network_online: bool,
    diagnostics: Vec<String>,
}

impl Default for IslandTechFeedController {
    fn default() -> Self {
        Self::new()
    }
}

impl IslandTechFeedController {
    pub fn new() -> Self {
        Self {
            sources: BTreeMap::new(),
            validators: BTreeMap::new(),
            rate_limits: BTreeMap::new(),
            source_cache: BTreeMap::new(),
            merged_items: Vec::new(),
            last_rendered_fingerprint: None,
            reparse_count: 0,
            ui_wake_count: 0,
            gaming_mode_active: false,
            network_online: true,
            diagnostics: Vec::new(),
        }
    }

    pub fn register_source(&mut self, config: TechFeedSourceConfig) {
        let source_id = config.source_id.clone();
        self.sources.insert(source_id.clone(), config);
        self.validators.entry(source_id.clone()).or_default();
        self.rate_limits.entry(source_id.clone()).or_default();
        self.source_cache.entry(source_id).or_default();
    }

    pub fn set_gaming_mode_active(&mut self, active: bool) {
        self.gaming_mode_active = active;
    }

    pub fn sync_gaming_mode_from_coordinator(&mut self, coordinator: &IdleSuspensionCoordinator) {
        self.gaming_mode_active = coordinator.is_gaming_mode_active();
    }

    pub fn is_gaming_mode_active(&self) -> bool {
        self.gaming_mode_active
    }

    pub fn set_network_online(&mut self, online: bool) {
        self.network_online = online;
    }

    pub fn is_network_online(&self) -> bool {
        self.network_online
    }

    pub fn validators_for(&self, source_id: &str) -> Option<&ConditionalValidators> {
        self.validators.get(source_id)
    }

    pub fn rate_limit_for(&self, source_id: &str) -> Option<&RateLimitState> {
        self.rate_limits.get(source_id)
    }

    pub fn merged_items(&self) -> &[TechFeedItem] {
        &self.merged_items
    }

    pub fn reparse_count(&self) -> u64 {
        self.reparse_count
    }

    pub fn ui_wake_count(&self) -> u64 {
        self.ui_wake_count
    }

    pub fn diagnostics(&self) -> &[String] {
        &self.diagnostics
    }

    /// Synchronizes all registered feed sources with conditional caching (`ETag` / `Last-Modified`),
    /// respecting Gaming Mode, offline status, GitHub `X-RateLimit-Remaining`, and Gatekeeper
    /// `Capability::Network` rules.
    pub fn sync_all_sources(
        &mut self,
        transport: &mut dyn FeedTransport,
        gatekeeper: &mut GatekeeperEngine,
        overview: &mut OverviewSceneController,
        bridge: &mut WebView2IpcBridge,
        now_ms: u64,
    ) -> Result<FeedSyncSummary, UiError> {
        if self.gaming_mode_active {
            return Ok(FeedSyncSummary {
                paused_reason: Some(FeedPauseReason::GamingModeActive),
                sources_checked: 0,
                http_200_count: 0,
                http_304_count: 0,
                rate_limited_skips: 0,
                source_errors: 0,
                total_items: self.merged_items.len(),
                reparse_delta: 0,
                ui_wake_delta: 0,
            });
        }

        if !self.network_online {
            return Ok(FeedSyncSummary {
                paused_reason: Some(FeedPauseReason::Offline),
                sources_checked: 0,
                http_200_count: 0,
                http_304_count: 0,
                rate_limited_skips: 0,
                source_errors: 0,
                total_items: self.merged_items.len(),
                reparse_delta: 0,
                ui_wake_delta: 0,
            });
        }

        let reparse_before = self.reparse_count;
        let ui_wake_before = self.ui_wake_count;
        let mut sources_checked = 0usize;
        let mut http_200_count = 0usize;
        let mut http_304_count = 0usize;
        let mut rate_limited_skips = 0usize;
        let mut source_errors = 0usize;

        let source_configs: Vec<TechFeedSourceConfig> = self.sources.values().cloned().collect();

        for source in source_configs {
            let source_id = &source.source_id;

            // 1. Check GitHub rate-limit state before issuing any network request
            if source.is_github_api {
                if let Some(rl) = self.rate_limits.get(source_id) {
                    if rl.is_exhausted(now_ms) {
                        rate_limited_skips += 1;
                        continue;
                    }
                }
            }

            // 2. Enforce Gatekeeper Capability::Network authorization
            let gk_verdict = gatekeeper.evaluate_request(
                "island_tech_feed",
                Capability::Network,
                &source.url,
                "GET",
                now_ms,
            );
            if !matches!(gk_verdict, GatekeeperVerdict::Allow { .. }) {
                source_errors += 1;
                self.diagnostics.push(sanitize_feed_diagnostic(
                    &format!("gatekeeper blocked feed source '{}' ({})", source_id, source.url),
                    &[],
                ));
                continue;
            }

            sources_checked += 1;
            let current_validators = self
                .validators
                .get(source_id)
                .cloned()
                .unwrap_or_default();

            let req = FeedHttpRequest {
                source_id: source_id.clone(),
                url: source.url.clone(),
                if_none_match: current_validators.etag.clone(),
                if_modified_since: current_validators.last_modified.clone(),
                timeout_ms: source.timeout_ms,
            };

            match transport.fetch(&req) {
                Err(err) => {
                    source_errors += 1;
                    self.diagnostics
                        .push(sanitize_feed_diagnostic(&err.to_string(), &[]));
                    // Preserve cached items for this source without failing other sources
                }
                Ok(resp) => {
                    if source.is_github_api
                        && (resp.rate_limit_remaining.is_some()
                            || resp.rate_limit_reset_ms.is_some())
                    {
                        let entry = self.rate_limits.entry(source_id.clone()).or_default();
                        if let Some(rem) = resp.rate_limit_remaining {
                            entry.remaining = Some(rem);
                        }
                        if let Some(reset) = resp.rate_limit_reset_ms {
                            entry.reset_at_ms = Some(reset);
                        }
                    }

                    if resp.status_code == 304 {
                        http_304_count += 1;
                        let val_entry = self.validators.entry(source_id.clone()).or_default();
                        if let Some(etag) = resp.etag {
                            val_entry.etag = Some(etag);
                        }
                        if let Some(lm) = resp.last_modified {
                            val_entry.last_modified = Some(lm);
                        }
                        // Zero reparse and preserve existing source_cache
                    } else if resp.status_code == 200 {
                        http_200_count += 1;
                        self.reparse_count += 1;
                        let parsed_res = if source.kind == TechFeedSourceKind::RssFeed {
                            parse_rss_or_atom_items(source_id, &resp.body)
                        } else {
                            parse_github_json_items(source_id, source.kind, &resp.body)
                        };

                        match parsed_res {
                            Ok(items) => {
                                let val_entry =
                                    self.validators.entry(source_id.clone()).or_default();
                                if let Some(etag) = resp.etag {
                                    val_entry.etag = Some(etag);
                                }
                                if let Some(lm) = resp.last_modified {
                                    val_entry.last_modified = Some(lm);
                                }
                                self.source_cache.insert(source_id.clone(), items);
                            }
                            Err(parse_err) => {
                                source_errors += 1;
                                self.diagnostics
                                    .push(sanitize_feed_diagnostic(&parse_err, &[]));
                                // Retain prior valid source_cache and prior validators
                            }
                        }
                    } else {
                        source_errors += 1;
                        self.diagnostics.push(sanitize_feed_diagnostic(
                            &format!(
                                "feed source '{}' returned HTTP {}",
                                source_id, resp.status_code
                            ),
                            &[],
                        ));
                    }
                }
            }
        }

        // Rebuild deduplicated merged items across all sources
        let mut deduped = Vec::new();
        let mut seen_ids = BTreeMap::new();
        for items in self.source_cache.values() {
            for item in items {
                if !seen_ids.contains_key(&item.item_id) {
                    seen_ids.insert(item.item_id.clone(), true);
                    deduped.push(item.clone());
                }
            }
        }
        self.merged_items = deduped;

        // Compute content fingerprint to avoid unnecessary UI wakes/DOM updates (e.g. on 304)
        let serialized = serde_json::to_string(&self.merged_items).unwrap_or_default();
        let fingerprint = sha256_hex(serialized.as_bytes());

        if self.last_rendered_fingerprint.as_deref() != Some(fingerprint.as_str()) {
            self.last_rendered_fingerprint = Some(fingerprint);
            self.ui_wake_count += 1;
            self.render_into_island(overview, bridge)?;
        }

        Ok(FeedSyncSummary {
            paused_reason: None,
            sources_checked,
            http_200_count,
            http_304_count,
            rate_limited_skips,
            source_errors,
            total_items: self.merged_items.len(),
            reparse_delta: self.reparse_count - reparse_before,
            ui_wake_delta: self.ui_wake_count - ui_wake_before,
        })
    }

    /// Renders merged feed items into the Island (`OverviewSceneController` & `WebView2IpcBridge`)
    /// using strict `textContent` sanitization (`sanitize_for_text_content`).
    pub fn render_into_island(
        &self,
        overview: &mut OverviewSceneController,
        bridge: &mut WebView2IpcBridge,
    ) -> Result<Vec<WebViewIpcPayloadV1>, UiError> {
        let mut renders = Vec::with_capacity(self.merged_items.len() + 1);
        let summary_render = sanitize_for_text_content(
            "miku-feed-count",
            &format!("{} novedades técnicas", self.merged_items.len()),
        )?;
        renders.push(summary_render);

        for (idx, item) in self.merged_items.iter().enumerate() {
            let line = format!("[{}] {} — {}", item.badge, item.title, item.summary);
            let cell = sanitize_for_text_content(&format!("miku-feed-item-{}", idx), &line)?;
            renders.push(cell);
        }

        // Record sanitized DOM update in OverviewSceneController via AgentEvent
        let feed_title = if let Some(first) = self.merged_items.first() {
            format!("[{}] {} (+{})", first.badge, first.title, self.merged_items.len())
        } else {
            "Feed técnico sin novedades".to_string()
        };
        let _ = overview.apply_inbound_message(
            bridge,
            "feed-ui-sync",
            &MikuLinkMessageV1::AgentEvent {
                agent: "island_tech_feed".to_string(),
                kind: "tech_feed_updated".to_string(),
                title: feed_title,
            },
        )?;

        Ok(renders)
    }

    /// Opens a feed item's URL in Zen Browser on click, strictly enforcing HTTPS and Gatekeeper
    /// `Capability::Browser` policy before invoking the browser launcher.
    pub fn open_item_in_zen_browser(
        &mut self,
        item_id: &str,
        gatekeeper: &mut GatekeeperEngine,
        launcher: &mut dyn ZenBrowserLauncher,
        now_ms: u64,
    ) -> Result<ZenBrowserOpenOutcome, UiError> {
        let item = self
            .merged_items
            .iter()
            .find(|i| i.item_id == item_id)
            .ok_or_else(|| {
                UiError::GatekeeperInterceptionError(format!(
                    "feed item '{}' not found in Island feed",
                    item_id
                ))
            })?
            .clone();

        if !item.url.starts_with("https://") {
            return Err(UiError::GatekeeperInterceptionError(format!(
                "unsafe non-HTTPS feed link blocked: {}",
                item.url
            )));
        }

        let verdict = gatekeeper.evaluate_request(
            "island_tech_feed",
            Capability::Browser,
            &item.url,
            "zen.exe --new-tab",
            now_ms,
        );

        match verdict {
            GatekeeperVerdict::Allow {
                canonical_target, ..
            } => {
                let command_args = launcher
                    .open_in_zen_browser(&canonical_target)
                    .map_err(UiError::GatekeeperInterceptionError)?;
                Ok(ZenBrowserOpenOutcome {
                    item_id: item.item_id,
                    url: canonical_target,
                    command_args,
                })
            }
            GatekeeperVerdict::AskPending { ticket, .. } => Err(
                UiError::GatekeeperInterceptionError(format!(
                    "Zen Browser navigation requires interactive confirmation (req_hash={})",
                    ticket.req_hash
                )),
            ),
            GatekeeperVerdict::Deny { reason, detail, .. } => Err(
                UiError::GatekeeperInterceptionError(format!(
                    "Gatekeeper denied Zen Browser navigation [{}]: {}",
                    reason.as_code(),
                    detail
                )),
            ),
        }
    }
}
