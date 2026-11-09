use serde::{Deserialize, Serialize};

/// Category of local 0-token resolution.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LocalRouteCategory {
    CasualGreeting,
    InternalCommand,
    LocalStatusQuery,
}

impl LocalRouteCategory {
    pub fn as_str(&self) -> &'static str {
        match self {
            LocalRouteCategory::CasualGreeting => "casual_greeting",
            LocalRouteCategory::InternalCommand => "internal_command",
            LocalRouteCategory::LocalStatusQuery => "local_status_query",
        }
    }
}

/// Read-only local telemetry snapshot used to answer local status queries
/// without invoking external providers, reading secrets, or bypassing Gatekeeper.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LocalSystemSnapshot {
    pub project_id: String,
    pub current_time_iso: String,
    pub power_mode: String,
    pub battery_percent: u8,
    pub active_agents_count: usize,
    pub active_model_display: String,
    pub daily_spent_micros: u64,
    pub daily_limit_micros: u64,
}

impl Default for LocalSystemSnapshot {
    fn default() -> Self {
        Self {
            project_id: "default-project".to_string(),
            current_time_iso: "2026-03-31T12:00:00Z".to_string(),
            power_mode: "balanced".to_string(),
            battery_percent: 100,
            active_agents_count: 1,
            active_model_display: "deepseek-chat".to_string(),
            daily_spent_micros: 0,
            daily_limit_micros: 1_000_000,
        }
    }
}

/// Verified 0-token local match result.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LocalRouteMatch {
    pub category: LocalRouteCategory,
    pub intent_id: String,
    pub reply: String,
    pub tokens_used: u32,
}

/// Reason why an input was forwarded to the LLM instead of handled locally.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ForwardReason {
    ExplicitForceLlm,
    AmbiguousOrTechnical,
    MultiIntentOrComplex,
    UnknownCommandOrQuery,
}

/// Routing outcome from `LocalIntentRouter`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum RouteDecision {
    LocalZeroToken(LocalRouteMatch),
    ForwardToLlm {
        stripped_prompt: String,
        reason: ForwardReason,
    },
}

/// Zero-token local router (F-09).
/// Resolves greetings, safe internal commands, and known local status queries
/// with 0 network calls, 0 secret reads, and 0 tokens.
/// Never executes FS, Shell, Browser, or Memory tools and never bypasses Gatekeeper.
pub struct LocalIntentRouter;

impl LocalIntentRouter {
    /// Checks if the user prompt starts with an explicit LLM override prefix
    /// (`/llm `, `!llm `, or `@llm `) and returns the stripped prompt.
    pub fn strip_force_llm_prefix(raw_input: &str) -> Option<String> {
        let trimmed = raw_input.trim();
        let lower = trimmed.to_lowercase();
        for prefix in ["/llm ", "!llm ", "@llm "] {
            if lower.starts_with(prefix) {
                let rest = trimmed[prefix.len()..].trim();
                if !rest.is_empty() {
                    return Some(rest.to_string());
                }
            }
        }
        None
    }

    /// Classifies and routes the input deterministically.
    /// Uncertain, multi-intent, or technical queries are always forwarded to the LLM
    /// to prevent local false positives.
    pub fn classify_and_route(
        raw_input: &str,
        force_llm: bool,
        snapshot: &LocalSystemSnapshot,
    ) -> RouteDecision {
        let trimmed = raw_input.trim();
        if let Some(stripped) = Self::strip_force_llm_prefix(trimmed) {
            return RouteDecision::ForwardToLlm {
                stripped_prompt: stripped,
                reason: ForwardReason::ExplicitForceLlm,
            };
        }

        if force_llm {
            return RouteDecision::ForwardToLlm {
                stripped_prompt: trimmed.to_string(),
                reason: ForwardReason::ExplicitForceLlm,
            };
        }

        if trimmed.is_empty() || trimmed.len() > 96 || trimmed.contains('\n') {
            return RouteDecision::ForwardToLlm {
                stripped_prompt: trimmed.to_string(),
                reason: ForwardReason::MultiIntentOrComplex,
            };
        }

        // Handle explicit slash commands first
        if trimmed.starts_with('/') {
            let cmd = Self::normalize_text(trimmed);
            return match cmd.as_str() {
                "/help" | "/ayuda" => RouteDecision::LocalZeroToken(LocalRouteMatch {
                    category: LocalRouteCategory::InternalCommand,
                    intent_id: "cmd_help".to_string(),
                    reply: "Comandos locales (0 tokens): /help, /status, /budget, /agents, /models, /version. Usa '/llm <consulta>' para forzar consulta al LLM dentro del presupuesto.".to_string(),
                    tokens_used: 0,
                }),
                "/status" | "/estado" => RouteDecision::LocalZeroToken(LocalRouteMatch {
                    category: LocalRouteCategory::InternalCommand,
                    intent_id: "cmd_status".to_string(),
                    reply: format!(
                        "Estado local [{}]: modelo={}, agentes_activos={}, modo_energia={}, bateria={}%, hora={}",
                        snapshot.project_id,
                        snapshot.active_model_display,
                        snapshot.active_agents_count,
                        snapshot.power_mode,
                        snapshot.battery_percent,
                        snapshot.current_time_iso
                    ),
                    tokens_used: 0,
                }),
                "/budget" | "/presupuesto" | "/gasto" => {
                    let pct = if snapshot.daily_limit_micros > 0 {
                        ((snapshot.daily_spent_micros as u128 * 100)
                            / snapshot.daily_limit_micros as u128) as u64
                    } else {
                        100
                    };
                    RouteDecision::LocalZeroToken(LocalRouteMatch {
                        category: LocalRouteCategory::InternalCommand,
                        intent_id: "cmd_budget".to_string(),
                        reply: format!(
                            "Presupuesto diario [{}]: {} / {} micro-USD ({}% utilizado)",
                            snapshot.project_id,
                            snapshot.daily_spent_micros,
                            snapshot.daily_limit_micros,
                            pct
                        ),
                        tokens_used: 0,
                    })
                }
                "/agents" | "/agentes" => RouteDecision::LocalZeroToken(LocalRouteMatch {
                    category: LocalRouteCategory::InternalCommand,
                    intent_id: "cmd_agents".to_string(),
                    reply: format!(
                        "Agentes activos en sesion: {} (proyecto: {})",
                        snapshot.active_agents_count, snapshot.project_id
                    ),
                    tokens_used: 0,
                }),
                "/models" | "/modelos" => RouteDecision::LocalZeroToken(LocalRouteMatch {
                    category: LocalRouteCategory::InternalCommand,
                    intent_id: "cmd_models".to_string(),
                    reply: format!(
                        "Modelo activo: {} (alternativa local gratuita: ollama/qwen2.5-coder:7b)",
                        snapshot.active_model_display
                    ),
                    tokens_used: 0,
                }),
                "/version" | "/ping" => RouteDecision::LocalZeroToken(LocalRouteMatch {
                    category: LocalRouteCategory::InternalCommand,
                    intent_id: "cmd_version".to_string(),
                    reply: "Miku Desktop Orchestrator v0.1.0 (F-09 zero-token local router ready)"
                        .to_string(),
                    tokens_used: 0,
                }),
                _ => RouteDecision::ForwardToLlm {
                    stripped_prompt: trimmed.to_string(),
                    reason: ForwardReason::UnknownCommandOrQuery,
                },
            };
        }

        // Guard against technical, code, path, shell, or multi-intent markers
        if Self::contains_technical_or_ambiguous_markers(trimmed) {
            return RouteDecision::ForwardToLlm {
                stripped_prompt: trimmed.to_string(),
                reason: ForwardReason::AmbiguousOrTechnical,
            };
        }

        let normalized = Self::normalize_text(trimmed);

        // 1. Exact casual greetings whitelist
        if Self::is_casual_greeting(&normalized) {
            return RouteDecision::LocalZeroToken(LocalRouteMatch {
                category: LocalRouteCategory::CasualGreeting,
                intent_id: "greeting_ack".to_string(),
                reply: format!(
                    "Hola. Miku activa en el proyecto '{}' ({} agente(s), modelo {}). ¿En qué trabajamos?",
                    snapshot.project_id,
                    snapshot.active_agents_count,
                    snapshot.active_model_display
                ),
                tokens_used: 0,
            });
        }

        // 2. Exact known local status queries whitelist
        if let Some((intent_id, reply)) = Self::match_local_status_query(&normalized, snapshot) {
            return RouteDecision::LocalZeroToken(LocalRouteMatch {
                category: LocalRouteCategory::LocalStatusQuery,
                intent_id,
                reply,
                tokens_used: 0,
            });
        }

        RouteDecision::ForwardToLlm {
            stripped_prompt: trimmed.to_string(),
            reason: ForwardReason::AmbiguousOrTechnical,
        }
    }

    fn normalize_text(input: &str) -> String {
        let mut out = String::with_capacity(input.len());
        for ch in input.chars() {
            let mapped = match ch {
                'á' | 'Á' => 'a',
                'é' | 'É' => 'e',
                'í' | 'Í' => 'i',
                'ó' | 'Ó' => 'o',
                'ú' | 'Ú' | 'ü' | 'Ü' => 'u',
                '¡' | '!' | '¿' | '?' | '.' | ',' => ' ',
                other => other.to_ascii_lowercase(),
            };
            out.push(mapped);
        }
        out.split_whitespace().collect::<Vec<_>>().join(" ")
    }

    fn contains_technical_or_ambiguous_markers(input: &str) -> bool {
        let lower = input.to_lowercase();
        let code_or_shell_chars = ['`', ';', '|', '&', '>', '<', '{', '}', '[', ']', '=', '\\'];
        if input.chars().any(|c| code_or_shell_chars.contains(&c)) {
            return true;
        }

        let technical_markers = [
            ".rs",
            ".toml",
            ".ts",
            ".js",
            ".json",
            ".md",
            "src/",
            "crates/",
            "cargo ",
            "git ",
            "npm ",
            "fn ",
            "struct ",
            "impl ",
            "async ",
            "await",
            "bug",
            "error",
            "panic",
            "fix",
            "refactor",
            "implement",
            "test",
            "clippy",
            "compile",
            "compila",
            "explica",
            "explain",
            "por que",
            "porque",
            "why",
            "como ",
            "cómo ",
            "how ",
            "escribe",
            "write",
            "crea ",
            "create",
            "analiza",
            "analyze",
            "revisa",
            "review",
            "lee ",
            "read ",
            "ejecuta",
            "run ",
            "borra",
            "delete",
            "modifica",
            "modify",
        ];

        technical_markers.iter().any(|m| lower.contains(m))
    }

    fn is_casual_greeting(normalized: &str) -> bool {
        matches!(
            normalized,
            "hola"
                | "hola miku"
                | "hello"
                | "hello miku"
                | "hi"
                | "hi miku"
                | "hey"
                | "hey miku"
                | "buenos dias"
                | "buenos dias miku"
                | "buenas tardes"
                | "buenas tardes miku"
                | "buenas noches"
                | "buenas noches miku"
                | "buenas"
                | "good morning"
                | "good afternoon"
                | "good evening"
                | "que tal"
                | "que tal miku"
                | "como estas"
                | "como estas miku"
                | "gracias"
                | "muchas gracias"
                | "thanks"
                | "thank you"
                | "ok"
                | "vale"
                | "listo"
                | "entendido"
        )
    }

    fn match_local_status_query(
        normalized: &str,
        snapshot: &LocalSystemSnapshot,
    ) -> Option<(String, String)> {
        match normalized {
            "que hora es"
            | "hora actual"
            | "dime la hora"
            | "what time is it"
            | "current time"
            | "fecha actual"
            | "que dia es hoy" => Some((
                "query_time".to_string(),
                format!("Hora/fecha local actual: {}", snapshot.current_time_iso),
            )),
            "estado de bateria"
            | "bateria"
            | "nivel de bateria"
            | "battery status"
            | "battery"
            | "modo de energia"
            | "power mode" => Some((
                "query_power".to_string(),
                format!(
                    "Energia local: modo={}, bateria={}%",
                    snapshot.power_mode, snapshot.battery_percent
                ),
            )),
            "agentes activos"
            | "cuantos agentes activos"
            | "active agents"
            | "estado de agentes" => Some((
                "query_agents".to_string(),
                format!(
                    "Agentes activos: {} en el proyecto '{}'",
                    snapshot.active_agents_count, snapshot.project_id
                ),
            )),
            "gasto diario"
            | "presupuesto diario"
            | "cuanto he gastado hoy"
            | "daily spend"
            | "daily budget" => {
                let pct = if snapshot.daily_limit_micros > 0 {
                    ((snapshot.daily_spent_micros as u128 * 100)
                        / snapshot.daily_limit_micros as u128) as u64
                } else {
                    100
                };
                Some((
                    "query_budget".to_string(),
                    format!(
                        "Gasto diario del proyecto '{}': {} / {} micro-USD ({}%)",
                        snapshot.project_id,
                        snapshot.daily_spent_micros,
                        snapshot.daily_limit_micros,
                        pct
                    ),
                ))
            }
            _ => None,
        }
    }
}
