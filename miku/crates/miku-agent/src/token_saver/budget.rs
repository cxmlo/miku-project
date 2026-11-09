use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};
use std::time::{SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};

use crate::harness::types::LlmProvider;

/// Clock abstraction for deterministic daily budget resets (F-09).
pub trait DailyClock: Send + Sync {
    /// Returns a canonical day key (e.g. `"2026-03-31"` or `"UTC-DAY-20543"`).
    fn current_day_key(&self) -> String;
}

/// System clock that derives the UTC day key from `SystemTime::now()`.
#[derive(Debug, Clone, Default)]
pub struct SystemUtcDayClock;

impl DailyClock for SystemUtcDayClock {
    fn current_day_key(&self) -> String {
        let secs = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs();
        let day_index = secs / 86_400;
        format!("UTC-DAY-{}", day_index)
    }
}

/// Injectable clock for deterministic unit and integration testing of daily resets.
#[derive(Debug, Clone)]
pub struct InjectableDayClock {
    day_key: Arc<Mutex<String>>,
}

impl InjectableDayClock {
    pub fn new(initial_day_key: &str) -> Self {
        Self {
            day_key: Arc::new(Mutex::new(initial_day_key.to_string())),
        }
    }

    pub fn set_day(&self, new_day_key: &str) {
        if let Ok(mut guard) = self.day_key.lock() {
            *guard = new_day_key.to_string();
        }
    }
}

impl DailyClock for InjectableDayClock {
    fn current_day_key(&self) -> String {
        self.day_key
            .lock()
            .map(|g| g.clone())
            .unwrap_or_else(|_| "1970-01-01".to_string())
    }
}

/// Configurable pricing rate per 1,000 tokens in micro-units (e.g. micro-USD where 1_000_000 = $1.00).
/// Rates are always explicitly configured by the caller or project profile—never hardcoded vendor guesses.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ModelPricingRate {
    pub provider: LlmProvider,
    pub model: String,
    pub input_micros_per_1k: u64,
    pub cached_input_micros_per_1k: u64,
    pub output_micros_per_1k: u64,
    pub is_free_or_local: bool,
}

impl ModelPricingRate {
    pub fn new_paid(
        provider: LlmProvider,
        model: &str,
        input_micros_per_1k: u64,
        cached_input_micros_per_1k: u64,
        output_micros_per_1k: u64,
    ) -> Self {
        Self {
            provider,
            model: model.to_string(),
            input_micros_per_1k,
            cached_input_micros_per_1k,
            output_micros_per_1k,
            is_free_or_local: false,
        }
    }

    pub fn new_free_local(provider: LlmProvider, model: &str) -> Self {
        Self {
            provider,
            model: model.to_string(),
            input_micros_per_1k: 0,
            cached_input_micros_per_1k: 0,
            output_micros_per_1k: 0,
            is_free_or_local: true,
        }
    }

    /// Computes exact micro-unit cost for a turn's token usage using ceiling integer division.
    pub fn compute_cost_micros(&self, usage: &TokenUsageRecord) -> u64 {
        if self.is_free_or_local || self.provider == LlmProvider::Ollama {
            return 0;
        }
        let uncached_cost = (usage.uncached_input_tokens as u128
            * self.input_micros_per_1k as u128)
            .div_ceil(1_000);
        let cached_cost = (usage.cached_input_tokens as u128
            * self.cached_input_micros_per_1k as u128)
            .div_ceil(1_000);
        let output_cost =
            (usage.output_tokens as u128 * self.output_micros_per_1k as u128).div_ceil(1_000);

        (uncached_cost + cached_cost + output_cost)
            .min(u64::MAX as u128) as u64
    }
}

/// Explicit token usage data for a single LLM invocation.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct TokenUsageRecord {
    pub uncached_input_tokens: u32,
    pub cached_input_tokens: u32,
    pub output_tokens: u32,
}

impl TokenUsageRecord {
    pub fn total_tokens(&self) -> u32 {
        self.uncached_input_tokens
            .saturating_add(self.cached_input_tokens)
            .saturating_add(self.output_tokens)
    }
}

/// Free or local fallback option offered when a project's paid daily budget reaches 100%.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FallbackAlternative {
    pub id: String,
    pub label: String,
    pub provider: LlmProvider,
    pub model: String,
    pub endpoint_url: String,
    pub is_zero_cost: bool,
}

impl FallbackAlternative {
    pub fn default_ollama_local() -> Self {
        Self {
            id: "local-ollama-coder".to_string(),
            label: "Ollama Local (qwen2.5-coder:7b, 0 costo)".to_string(),
            provider: LlmProvider::Ollama,
            model: "qwen2.5-coder:7b".to_string(),
            endpoint_url: "http://127.0.0.1:11434/api/chat".to_string(),
            is_zero_cost: true,
        }
    }

    pub fn default_local_router_only() -> Self {
        Self {
            id: "local-zero-token-router".to_string(),
            label: "Modo comandos/consultas locales (0 tokens)".to_string(),
            provider: LlmProvider::Ollama,
            model: "local-intent-router".to_string(),
            endpoint_url: "http://127.0.0.1:11434/api/chat".to_string(),
            is_zero_cost: true,
        }
    }
}

/// Daily budget configuration per project.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProjectDailyBudgetConfig {
    pub project_id: String,
    pub daily_limit_micros: u64,
    pub warning_threshold_pct: u8,
    pub hard_cap_pct: u8,
    pub fallback_alternatives: Vec<FallbackAlternative>,
}

impl ProjectDailyBudgetConfig {
    pub fn new(project_id: &str, daily_limit_micros: u64) -> Self {
        Self {
            project_id: project_id.to_string(),
            daily_limit_micros,
            warning_threshold_pct: 80,
            hard_cap_pct: 100,
            fallback_alternatives: vec![
                FallbackAlternative::default_ollama_local(),
                FallbackAlternative::default_local_router_only(),
            ],
        }
    }

    pub fn warning_threshold_micros(&self) -> u64 {
        ((self.daily_limit_micros as u128 * self.warning_threshold_pct as u128) / 100) as u64
    }

    pub fn hard_cap_micros(&self) -> u64 {
        ((self.daily_limit_micros as u128 * self.hard_cap_pct as u128) / 100) as u64
    }
}

/// Soft alert emitted when daily project spend reaches or exceeds the warning threshold (80%).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct BudgetWarningAlert {
    pub project_id: String,
    pub day_key: String,
    pub spent_micros: u64,
    pub limit_micros: u64,
    pub usage_pct: u8,
    pub warning_threshold_pct: u8,
}

/// Result of evaluating the project budget BEFORE reading any secret or dispatching a network request.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum PreFlightBudgetDecision {
    Allowed {
        project_id: String,
        day_key: String,
        spent_micros: u64,
        limit_micros: u64,
        usage_pct: u8,
        warning: Option<BudgetWarningAlert>,
        is_free_or_local: bool,
    },
    BlockedHardCap {
        project_id: String,
        day_key: String,
        spent_micros: u64,
        limit_micros: u64,
        usage_pct: u8,
        alternatives: Vec<FallbackAlternative>,
    },
}

/// Read-only snapshot of a project's current daily spend and token counters.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProjectSpendSnapshot {
    pub project_id: String,
    pub day_key: String,
    pub spent_micros: u64,
    pub limit_micros: u64,
    pub usage_pct: u8,
    pub uncached_input_tokens: u64,
    pub cached_input_tokens: u64,
    pub output_tokens: u64,
    pub local_zero_token_hits: u64,
    pub warning_emitted: bool,
    pub hard_cap_blocked_count: u64,
}

#[derive(Debug, Clone)]
struct ProjectLedgerEntry {
    day_key: String,
    spent_micros: u64,
    uncached_input_tokens: u64,
    cached_input_tokens: u64,
    output_tokens: u64,
    local_zero_token_hits: u64,
    warning_emitted: bool,
    hard_cap_blocked_count: u64,
}

impl ProjectLedgerEntry {
    fn new(day_key: String) -> Self {
        Self {
            day_key,
            spent_micros: 0,
            uncached_input_tokens: 0,
            cached_input_tokens: 0,
            output_tokens: 0,
            local_zero_token_hits: 0,
            warning_emitted: false,
            hard_cap_blocked_count: 0,
        }
    }

    fn reset_for_new_day(&mut self, new_day_key: String) {
        self.day_key = new_day_key;
        self.spent_micros = 0;
        self.uncached_input_tokens = 0;
        self.cached_input_tokens = 0;
        self.output_tokens = 0;
        self.local_zero_token_hits = 0;
        self.warning_emitted = false;
        self.hard_cap_blocked_count = 0;
    }
}

/// Per-project daily spend controller with 80% warning, 100% pre-flight block,
/// configurable pricing table, and deterministic daily clock reset.
pub struct DailySpendTracker {
    clock: Arc<dyn DailyClock>,
    configs: BTreeMap<String, ProjectDailyBudgetConfig>,
    pricing: BTreeMap<(String, String), ModelPricingRate>,
    ledgers: BTreeMap<String, ProjectLedgerEntry>,
}

impl DailySpendTracker {
    pub fn new(clock: Arc<dyn DailyClock>) -> Self {
        Self {
            clock,
            configs: BTreeMap::new(),
            pricing: BTreeMap::new(),
            ledgers: BTreeMap::new(),
        }
    }

    pub fn set_project_budget(&mut self, config: ProjectDailyBudgetConfig) {
        let day_key = self.clock.current_day_key();
        let project_id = config.project_id.clone();
        self.configs.insert(project_id.clone(), config);
        self.ledgers
            .entry(project_id)
            .or_insert_with(|| ProjectLedgerEntry::new(day_key));
    }

    pub fn set_pricing_rate(&mut self, rate: ModelPricingRate) {
        let key = (
            rate.provider.as_str().to_string(),
            rate.model.to_ascii_lowercase(),
        );
        self.pricing.insert(key, rate);
    }

    pub fn is_model_free_or_local(&self, provider: LlmProvider, model: &str) -> bool {
        if provider == LlmProvider::Ollama {
            return true;
        }
        let key = (
            provider.as_str().to_string(),
            model.to_ascii_lowercase(),
        );
        self.pricing
            .get(&key)
            .map(|r| r.is_free_or_local)
            .unwrap_or(false)
    }

    fn sync_project_day(&mut self, project_id: &str) -> String {
        let current_day = self.clock.current_day_key();
        let entry = self
            .ledgers
            .entry(project_id.to_string())
            .or_insert_with(|| ProjectLedgerEntry::new(current_day.clone()));
        if entry.day_key != current_day {
            entry.reset_for_new_day(current_day.clone());
        }
        current_day
    }

    fn compute_usage_pct(spent_micros: u64, limit_micros: u64) -> u8 {
        if limit_micros == 0 {
            return 100;
        }
        let pct = (spent_micros as u128 * 100) / limit_micros as u128;
        pct.min(255) as u8
    }

    /// Evaluates whether a provider request is allowed BEFORE reading any API key or opening a socket.
    pub fn evaluate_pre_flight(
        &mut self,
        project_id: &str,
        provider: LlmProvider,
        model: &str,
    ) -> PreFlightBudgetDecision {
        let day_key = self.sync_project_day(project_id);
        let free_or_local = self.is_model_free_or_local(provider, model);

        let config = self
            .configs
            .get(project_id)
            .cloned()
            .unwrap_or_else(|| ProjectDailyBudgetConfig::new(project_id, 1_000_000));

        let entry = self
            .ledgers
            .get_mut(project_id)
            .expect("ledger initialized by sync_project_day");

        let usage_pct = Self::compute_usage_pct(entry.spent_micros, config.daily_limit_micros);

        if free_or_local {
            return PreFlightBudgetDecision::Allowed {
                project_id: project_id.to_string(),
                day_key,
                spent_micros: entry.spent_micros,
                limit_micros: config.daily_limit_micros,
                usage_pct,
                warning: None,
                is_free_or_local: true,
            };
        }

        if entry.spent_micros >= config.hard_cap_micros() {
            entry.hard_cap_blocked_count = entry.hard_cap_blocked_count.saturating_add(1);
            return PreFlightBudgetDecision::BlockedHardCap {
                project_id: project_id.to_string(),
                day_key,
                spent_micros: entry.spent_micros,
                limit_micros: config.daily_limit_micros,
                usage_pct,
                alternatives: config.fallback_alternatives.clone(),
            };
        }

        let warning = if entry.spent_micros >= config.warning_threshold_micros() {
            entry.warning_emitted = true;
            Some(BudgetWarningAlert {
                project_id: project_id.to_string(),
                day_key: day_key.clone(),
                spent_micros: entry.spent_micros,
                limit_micros: config.daily_limit_micros,
                usage_pct,
                warning_threshold_pct: config.warning_threshold_pct,
            })
        } else {
            None
        };

        PreFlightBudgetDecision::Allowed {
            project_id: project_id.to_string(),
            day_key,
            spent_micros: entry.spent_micros,
            limit_micros: config.daily_limit_micros,
            usage_pct,
            warning,
            is_free_or_local: false,
        }
    }

    /// Records a 0-token local routing hit for telemetry.
    pub fn record_local_zero_token_hit(&mut self, project_id: &str) {
        let _ = self.sync_project_day(project_id);
        if let Some(entry) = self.ledgers.get_mut(project_id) {
            entry.local_zero_token_hits = entry.local_zero_token_hits.saturating_add(1);
        }
    }

    /// Records explicit token usage for a turn, updating daily spend and returning
    /// `(turn_cost_micros, optional_80_pct_warning)`.
    pub fn record_turn_usage(
        &mut self,
        project_id: &str,
        provider: LlmProvider,
        model: &str,
        usage: TokenUsageRecord,
    ) -> (u64, Option<BudgetWarningAlert>) {
        let day_key = self.sync_project_day(project_id);
        let key = (
            provider.as_str().to_string(),
            model.to_ascii_lowercase(),
        );
        let cost_micros = self
            .pricing
            .get(&key)
            .map(|r| r.compute_cost_micros(&usage))
            .unwrap_or(0);

        let config = self
            .configs
            .get(project_id)
            .cloned()
            .unwrap_or_else(|| ProjectDailyBudgetConfig::new(project_id, 1_000_000));

        let entry = self
            .ledgers
            .get_mut(project_id)
            .expect("ledger initialized by sync_project_day");

        entry.spent_micros = entry.spent_micros.saturating_add(cost_micros);
        entry.uncached_input_tokens = entry
            .uncached_input_tokens
            .saturating_add(usage.uncached_input_tokens as u64);
        entry.cached_input_tokens = entry
            .cached_input_tokens
            .saturating_add(usage.cached_input_tokens as u64);
        entry.output_tokens = entry
            .output_tokens
            .saturating_add(usage.output_tokens as u64);

        let usage_pct = Self::compute_usage_pct(entry.spent_micros, config.daily_limit_micros);
        let warning = if entry.spent_micros >= config.warning_threshold_micros() {
            entry.warning_emitted = true;
            Some(BudgetWarningAlert {
                project_id: project_id.to_string(),
                day_key,
                spent_micros: entry.spent_micros,
                limit_micros: config.daily_limit_micros,
                usage_pct,
                warning_threshold_pct: config.warning_threshold_pct,
            })
        } else {
            None
        };

        (cost_micros, warning)
    }

    pub fn snapshot(&mut self, project_id: &str) -> ProjectSpendSnapshot {
        let day_key = self.sync_project_day(project_id);
        let config = self
            .configs
            .get(project_id)
            .cloned()
            .unwrap_or_else(|| ProjectDailyBudgetConfig::new(project_id, 1_000_000));
        let entry = self
            .ledgers
            .get(project_id)
            .expect("ledger initialized by sync_project_day");
        let usage_pct = Self::compute_usage_pct(entry.spent_micros, config.daily_limit_micros);

        ProjectSpendSnapshot {
            project_id: project_id.to_string(),
            day_key,
            spent_micros: entry.spent_micros,
            limit_micros: config.daily_limit_micros,
            usage_pct,
            uncached_input_tokens: entry.uncached_input_tokens,
            cached_input_tokens: entry.cached_input_tokens,
            output_tokens: entry.output_tokens,
            local_zero_token_hits: entry.local_zero_token_hits,
            warning_emitted: entry.warning_emitted,
            hard_cap_blocked_count: entry.hard_cap_blocked_count,
        }
    }
}
