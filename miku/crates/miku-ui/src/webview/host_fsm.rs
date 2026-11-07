#![allow(unused_imports)]
use crate::prelude::*;

#[derive(Debug, Clone)]
pub struct WebView2HostStateMachine<D: WebView2LifecycleDriver> {
    pub(crate) monitor: MonitorDescriptor,
    pub(crate) top_margin_px: u32,
    pub(crate) user_data_dir: PathBuf,
    pub(crate) policy: WebView2SecurityPolicy,
    pub(crate) ttl_ms: u64,
    pub(crate) state: WebView2LifecycleState,
    pub(crate) driver: D,
    pub(crate) cold_spawns_count: u64,
    pub(crate) warm_reuses_count: u64,
    pub(crate) ttl_evictions_count: u64,
    pub(crate) blocked_navigations_count: u64,
    pub(crate) allowed_navigations_count: u64,
}

impl<D: WebView2LifecycleDriver> WebView2HostStateMachine<D> {
    pub fn new(
        monitor: MonitorDescriptor,
        top_margin_px: u32,
        local_appdata_override: Option<&Path>,
        policy: WebView2SecurityPolicy,
        ttl_ms: u64,
        driver: D,
    ) -> Result<Self, UiError> {
        if ttl_ms == 0 {
            return Err(UiError::WebView2LifecycleError(
                "TTL milliseconds must be > 0".to_string(),
            ));
        }
        policy.validate_release_hardening()?;
        let user_data_dir = resolve_isolated_webview2_user_data_dir(local_appdata_override)?;
        Ok(Self {
            monitor,
            top_margin_px,
            user_data_dir,
            policy,
            ttl_ms,
            state: WebView2LifecycleState::Unloaded,
            driver,
            cold_spawns_count: 0,
            warm_reuses_count: 0,
            ttl_evictions_count: 0,
            blocked_navigations_count: 0,
            allowed_navigations_count: 0,
        })
    }

    pub fn state(&self) -> WebView2LifecycleState {
        self.state
    }

    pub fn user_data_dir(&self) -> &Path {
        &self.user_data_dir
    }

    pub fn policy(&self) -> &WebView2SecurityPolicy {
        &self.policy
    }

    pub fn cold_spawns_count(&self) -> u64 {
        self.cold_spawns_count
    }

    pub fn warm_reuses_count(&self) -> u64 {
        self.warm_reuses_count
    }

    pub fn ttl_evictions_count(&self) -> u64 {
        self.ttl_evictions_count
    }

    pub fn blocked_navigations_count(&self) -> u64 {
        self.blocked_navigations_count
    }

    pub fn allowed_navigations_count(&self) -> u64 {
        self.allowed_navigations_count
    }

    pub fn active_webview2_instances(&self) -> usize {
        match self.state {
            WebView2LifecycleState::Unloaded | WebView2LifecycleState::EvictedIdle { .. } => 0,
            WebView2LifecycleState::ActiveExpanded { .. }
            | WebView2LifecycleState::CollapsedWarmTtl { .. } => 1,
        }
    }

    pub fn active_child_pids(&self) -> Vec<u32> {
        self.driver.active_child_webview2_pids()
    }

    pub fn current_private_ram_mb(&self) -> f64 {
        self.driver.current_private_ram_mb()
    }

    pub fn active_timers(&self) -> u32 {
        match self.state {
            WebView2LifecycleState::CollapsedWarmTtl { .. } => 1,
            WebView2LifecycleState::Unloaded
            | WebView2LifecycleState::ActiveExpanded { .. }
            | WebView2LifecycleState::EvictedIdle { .. } => 0,
        }
    }

    /// Expands the Island to `640x380` (scaled by DPI).
    /// - If `Unloaded` or `EvictedIdle`: lazily spawns WebView2 using `%LOCALAPPDATA%\miku\webview2`.
    /// - If `CollapsedWarmTtl` and within TTL: reuses the warm controller in O(1) without spawning.
    /// - If `CollapsedWarmTtl` and TTL expired: evicts first, then spawns cleanly.
    pub fn expand_island(&mut self, now_ms: u64) -> Result<ExpandTransitionReport, UiError> {
        let geometry = compute_top_center_pill_anchor(
            &self.monitor,
            EXPANDED_DEFAULT_WIDTH_PX,
            EXPANDED_DEFAULT_HEIGHT_PX,
            EXPANDED_CORNER_RADIUS_PX,
            self.top_margin_px,
        )?;

        if let WebView2LifecycleState::CollapsedWarmTtl {
            browser_pid,
            collapsed_at_ms,
            ttl_ms,
        } = self.state
        {
            if now_ms.saturating_sub(collapsed_at_ms) < ttl_ms {
                let warm_ms = self.driver.show_warm_controller(browser_pid, geometry)?;
                self.warm_reuses_count += 1;
                self.state = WebView2LifecycleState::ActiveExpanded {
                    browser_pid,
                    reused_warm: true,
                    spawned_at_ms: now_ms,
                };
                return Ok(ExpandTransitionReport {
                    state: self.state,
                    geometry,
                    user_data_dir: self.user_data_dir.to_string_lossy().to_string(),
                    reused_warm: true,
                    expand_latency_ms: warm_ms,
                    active_ram_mb: self.driver.current_private_ram_mb(),
                });
            } else {
                let _ = self.poll_ttl(now_ms)?;
            }
        }

        if let WebView2LifecycleState::ActiveExpanded {
            browser_pid,
            reused_warm,
            ..
        } = self.state
        {
            let warm_ms = self.driver.show_warm_controller(browser_pid, geometry)?;
            return Ok(ExpandTransitionReport {
                state: self.state,
                geometry,
                user_data_dir: self.user_data_dir.to_string_lossy().to_string(),
                reused_warm,
                expand_latency_ms: warm_ms,
                active_ram_mb: self.driver.current_private_ram_mb(),
            });
        }

        let (browser_pid, spawn_ms) =
            self.driver
                .spawn_isolated_controller(&self.user_data_dir, &self.policy, geometry)?;
        self.cold_spawns_count += 1;
        self.state = WebView2LifecycleState::ActiveExpanded {
            browser_pid,
            reused_warm: false,
            spawned_at_ms: now_ms,
        };

        // Navigate initial embedded local origin shell
        let _ = self.navigate("https://miku.local/index.html")?;

        Ok(ExpandTransitionReport {
            state: self.state,
            geometry,
            user_data_dir: self.user_data_dir.to_string_lossy().to_string(),
            reused_warm: false,
            expand_latency_ms: spawn_ms,
            active_ram_mb: self.driver.current_private_ram_mb(),
        })
    }

    pub fn navigate(&mut self, uri: &str) -> Result<NavigationVerdict, UiError> {
        match evaluate_navigation_target(uri, &self.policy) {
            verdict @ NavigationVerdict::AllowLocalOrigin { .. } => {
                self.allowed_navigations_count += 1;
                Ok(verdict)
            }
            NavigationVerdict::BlockExternalOrUnsafe { reason, .. } => {
                self.blocked_navigations_count += 1;
                Err(UiError::UnsafeNavigationBlocked(reason))
            }
        }
    }

    pub fn collapse_island(&mut self, now_ms: u64) -> Result<WebView2LifecycleState, UiError> {
        match self.state {
            WebView2LifecycleState::ActiveExpanded { browser_pid, .. } => {
                self.driver.hide_controller_on_collapse(browser_pid)?;
                self.state = WebView2LifecycleState::CollapsedWarmTtl {
                    browser_pid,
                    collapsed_at_ms: now_ms,
                    ttl_ms: self.ttl_ms,
                };
                Ok(self.state)
            }
            other => Ok(other),
        }
    }

    /// Evaluates whether the Mode C warm TTL has expired. When expired, closes the WebView2
    /// controller, verifies 0 child `msedgewebview2.exe` processes remain, and transitions to `EvictedIdle`.
    pub fn poll_ttl(&mut self, now_ms: u64) -> Result<bool, UiError> {
        if let WebView2LifecycleState::CollapsedWarmTtl {
            browser_pid,
            collapsed_at_ms,
            ttl_ms,
        } = self.state
        {
            if now_ms.saturating_sub(collapsed_at_ms) >= ttl_ms {
                let remaining = self.driver.close_and_evict_controller(browser_pid)?;
                if remaining != 0 || !self.driver.active_child_webview2_pids().is_empty() {
                    return Err(UiError::WebView2LifecycleError(format!(
                        "leaked {} msedgewebview2.exe child processes after TTL eviction",
                        remaining
                    )));
                }
                self.ttl_evictions_count += 1;
                self.state = WebView2LifecycleState::EvictedIdle {
                    last_evicted_at_ms: now_ms,
                    total_evictions: self.ttl_evictions_count,
                };
                return Ok(true);
            }
        }
        Ok(false)
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct FiveCycleWebView2AuditReport {
    pub cycles_completed: usize,
    pub cold_spawns: u64,
    pub warm_reuses: u64,
    pub ttl_evictions: u64,
    pub blocked_external_navs: u64,
    pub max_expand_latency_ms: f64,
    pub max_post_ttl_ram_mb: f64,
    pub residual_webview2_pids: usize,
    pub residual_idle_timers: u32,
    pub isolated_user_data_dir: String,
    pub devtools_disabled_in_release: bool,
}

/// Executes 5 consecutive cycles of `expand_island -> navigate (allow local + block external) ->
/// collapse_island -> poll_ttl (evict)` plus 1 warm-reuse verification, confirming that after
/// every cycle RAM returns to `<= 30.0 MB` and `0` child `msedgewebview2.exe` processes remain.
pub fn run_five_cycle_webview2_ttl_recovery_audit(
    monitor: MonitorDescriptor,
    local_appdata_override: Option<&Path>,
    base_idle_ram_mb: f64,
) -> Result<FiveCycleWebView2AuditReport, UiError> {
    let policy = WebView2SecurityPolicy::release_hardened();
    policy.validate_release_hardening()?;
    let driver = IsolatedWebView2ProcessDriver::new(base_idle_ram_mb, 62.5);
    let ttl_ms = 15_000u64;
    let mut host = WebView2HostStateMachine::new(
        monitor,
        TOP_MARGIN_DEFAULT_PX,
        local_appdata_override,
        policy,
        ttl_ms,
        driver,
    )?;

    let mut now_ms = 1_000u64;
    let mut max_expand_latency_ms = 0.0f64;
    let mut max_post_ttl_ram_mb = 0.0f64;

    for cycle_idx in 0..FIVE_CYCLE_STRESS_REPS {
        let exp = host.expand_island(now_ms)?;
        if exp.expand_latency_ms > max_expand_latency_ms {
            max_expand_latency_ms = exp.expand_latency_ms;
        }
        if host.active_child_pids().is_empty() {
            return Err(UiError::WebView2LifecycleError(
                "expected active WebView2 child PIDs while expanded".to_string(),
            ));
        }

        // Verify external navigation is blocked during each cycle
        let blocked = host.navigate("https://evil.example.com/phish");
        if blocked.is_ok() {
            return Err(UiError::UnsafeNavigationBlocked(
                "external navigation was unexpectedly allowed".to_string(),
            ));
        }

        // In cycle 0, also verify warm reuse before TTL expiry
        if cycle_idx == 0 {
            now_ms += 200;
            let _ = host.collapse_island(now_ms)?;
            now_ms += 500; // 500 ms < 15_000 ms TTL -> warm reuse!
            let warm_exp = host.expand_island(now_ms)?;
            if !warm_exp.reused_warm {
                return Err(UiError::WebView2LifecycleError(
                    "expected warm reuse within TTL window".to_string(),
                ));
            }
        }

        // Collapse and advance clock past TTL to trigger full eviction
        now_ms += 250;
        let _ = host.collapse_island(now_ms)?;
        if host.active_timers() != 1 {
            return Err(UiError::WebView2LifecycleError(
                "expected exactly 1 TTL timer while in CollapsedWarmTtl".to_string(),
            ));
        }

        now_ms += ttl_ms + 100;
        let evicted = host.poll_ttl(now_ms)?;
        if !evicted {
            return Err(UiError::WebView2LifecycleError(
                "expected TTL eviction on poll_ttl after ttl_ms".to_string(),
            ));
        }

        let post_ram = host.current_private_ram_mb();
        if post_ram > max_post_ttl_ram_mb {
            max_post_ttl_ram_mb = post_ram;
        }
        if post_ram > MAX_IDLE_POST_TTL_RAM_MB {
            return Err(UiError::WebView2LifecycleError(format!(
                "post-TTL RAM {:.2} MB exceeded budget {:.1} MB in cycle {}",
                post_ram,
                MAX_IDLE_POST_TTL_RAM_MB,
                cycle_idx + 1
            )));
        }
        if !host.active_child_pids().is_empty() || host.active_webview2_instances() != 0 {
            return Err(UiError::WebView2LifecycleError(
                "residual WebView2 instances or child PIDs remained after TTL eviction".to_string(),
            ));
        }
        if host.active_timers() != 0 {
            return Err(UiError::WebView2LifecycleError(
                "residual idle timer remained after TTL eviction".to_string(),
            ));
        }
    }

    Ok(FiveCycleWebView2AuditReport {
        cycles_completed: FIVE_CYCLE_STRESS_REPS,
        cold_spawns: host.cold_spawns_count(),
        warm_reuses: host.warm_reuses_count(),
        ttl_evictions: host.ttl_evictions_count(),
        blocked_external_navs: host.blocked_navigations_count(),
        max_expand_latency_ms,
        max_post_ttl_ram_mb,
        residual_webview2_pids: host.active_child_pids().len(),
        residual_idle_timers: host.active_timers(),
        isolated_user_data_dir: host.user_data_dir().to_string_lossy().to_string(),
        devtools_disabled_in_release: !host.policy().are_devtools_enabled,
    })
}

