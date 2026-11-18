use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub(super) struct TelemetryModelDto {
    pub requests: u64,
    pub tokens: u64,
    pub cost_usd: f64,
    pub total_latency_ms: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub(super) struct UsageTelemetryState {
    pub total_requests: u64,
    pub total_tokens: u64,
    pub prompt_tokens: u64,
    pub completion_tokens: u64,
    pub estimated_usd_cost: f64,
    pub total_latency_ms: u64,
    pub errors: u64,
    pub consecutive_failures: u64,
    pub by_model: BTreeMap<String, TelemetryModelDto>,
    #[serde(default)]
    pub cached_prompt_tokens: u64,
    #[serde(default)]
    pub local_routed_requests: u64,
    #[serde(default)]
    pub estimated_tokens_saved: u64,
    #[serde(default)]
    pub daily_usd_spent: f64,
    #[serde(default)]
    pub daily_day_index: u64,
}

fn estimate_tokens_usd(model: &str, input: u64, output: u64) -> f64 {
    let m = model.to_ascii_lowercase();
    let (in_rate, out_rate) = if m.contains("reasoner") {
        (0.55, 2.19)
    } else if m.contains("deepseek") {
        (0.27, 1.10)
    } else if m.contains("sonnet") {
        (3.00, 15.00)
    } else if m.contains("claude") || m.contains("haiku") {
        (0.80, 4.00)
    } else if m.contains("2.5-pro") {
        (1.25, 5.00)
    } else if m.contains("gemini") || m.contains("4o-mini") || m.contains("o3-mini") {
        (0.15, 0.60)
    } else if m.contains("gpt-4o") {
        (2.50, 10.00)
    } else if m.contains("ollama") || m.contains("local") {
        (0.0, 0.0)
    } else {
        (0.50, 1.50)
    };
    ((input as f64) * in_rate + (output as f64) * out_rate) / 1_000_000.0
}

impl UsageTelemetryState {
    fn sync_day_bucket(&mut self, epoch_ms: u64) {
        let day = epoch_ms / 86_400_000;
        if self.daily_day_index != day {
            self.daily_day_index = day;
            self.daily_usd_spent = 0.0;
        }
    }

    #[allow(clippy::too_many_arguments)]
    pub(super) fn record_with_cache(
        &mut self,
        model: &str,
        input_tokens: u64,
        cached_input_tokens: u64,
        output_tokens: u64,
        latency_ms: u64,
        is_error: bool,
        epoch_ms: u64,
    ) {
        if epoch_ms > 0 {
            self.sync_day_bucket(epoch_ms);
        }
        let total = input_tokens.saturating_add(output_tokens);
        let effective_cached = cached_input_tokens.min(input_tokens);
        let uncached_in = input_tokens.saturating_sub(effective_cached);
        let cached_usd_eq = estimate_tokens_usd(model, effective_cached, 0) * 0.25;
        let cost = estimate_tokens_usd(model, uncached_in, output_tokens) + cached_usd_eq;
        let saved_from_cache = (effective_cached * 75) / 100;
        self.total_requests = self.total_requests.saturating_add(1);
        self.prompt_tokens = self.prompt_tokens.saturating_add(input_tokens);
        self.cached_prompt_tokens = self.cached_prompt_tokens.saturating_add(effective_cached);
        self.estimated_tokens_saved = self.estimated_tokens_saved.saturating_add(saved_from_cache);
        self.completion_tokens = self.completion_tokens.saturating_add(output_tokens);
        self.total_tokens = self.total_tokens.saturating_add(total);
        self.estimated_usd_cost += cost;
        self.daily_usd_spent += cost;
        self.total_latency_ms = self.total_latency_ms.saturating_add(latency_ms);
        if is_error {
            self.errors = self.errors.saturating_add(1);
            self.consecutive_failures = self.consecutive_failures.saturating_add(1);
        } else {
            self.consecutive_failures = 0;
        }
        let entry = self.by_model.entry(model.to_string()).or_default();
        entry.requests = entry.requests.saturating_add(1);
        entry.tokens = entry.tokens.saturating_add(total);
        entry.cost_usd += cost;
        entry.total_latency_ms = entry.total_latency_ms.saturating_add(latency_ms);
    }

    pub(super) fn record_local_route(&mut self, est_tokens_saved: u64, epoch_ms: u64) {
        if epoch_ms > 0 {
            self.sync_day_bucket(epoch_ms);
        }
        self.local_routed_requests = self.local_routed_requests.saturating_add(1);
        self.estimated_tokens_saved = self.estimated_tokens_saved.saturating_add(est_tokens_saved);
    }

    pub(super) fn add_tokens_saved(&mut self, est_tokens_saved: u64) {
        self.estimated_tokens_saved = self.estimated_tokens_saved.saturating_add(est_tokens_saved);
    }

    pub(super) fn to_snapshot_json(&self, active_provider: &str, active_model: &str) -> Value {
        let reqs = self.total_requests;
        let avg_latency_ms = self.total_latency_ms.checked_div(reqs).unwrap_or(0);
        let cb_open = self.consecutive_failures >= 3;
        let est_usd_saved = estimate_tokens_usd(active_model, self.estimated_tokens_saved, 0);
        let by_model: Vec<Value> = self
            .by_model
            .iter()
            .map(|(model, d)| {
                let lat = d.total_latency_ms.checked_div(d.requests).unwrap_or(0);
                json!({
                    "model": model,
                    "requests": d.requests,
                    "tokens": d.tokens,
                    "cost_usd": d.cost_usd,
                    "latency_ms": lat
                })
            })
            .collect();
        json!({
            "ok": true,
            "total_requests": self.total_requests,
            "total_tokens": self.total_tokens,
            "prompt_tokens": self.prompt_tokens,
            "cached_prompt_tokens": self.cached_prompt_tokens,
            "local_routed_requests": self.local_routed_requests,
            "estimated_tokens_saved": self.estimated_tokens_saved,
            "estimated_usd_saved": est_usd_saved,
            "completion_tokens": self.completion_tokens,
            "estimated_usd_cost": self.estimated_usd_cost,
            "daily_usd_spent": self.daily_usd_spent,
            "avg_latency_ms": avg_latency_ms,
            "errors": self.errors,
            "by_model": by_model,
            "circuit_breaker": {
                "primary": {
                    "name": format!("{} / {}", active_provider, active_model),
                    "status": if cb_open { "OPEN" } else { "CLOSED" },
                    "failures": self.consecutive_failures,
                    "healthy": !cb_open
                },
                "secondary": {
                    "name": "Ollama Local (Failover)",
                    "status": "CLOSED",
                    "failures": 0,
                    "healthy": true
                }
            }
        })
    }
}
