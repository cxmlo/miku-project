use serde::{Deserialize, Serialize};

/// Supported LLM providers for Miku's autonomous agent harness.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LlmProvider {
    DeepSeek,
    Anthropic,
    OpenAi,
    Gemini,
    Ollama,
}

impl LlmProvider {
    pub fn as_str(&self) -> &'static str {
        match self {
            LlmProvider::DeepSeek => "deepseek",
            LlmProvider::Anthropic => "anthropic",
            LlmProvider::OpenAi => "openai",
            LlmProvider::Gemini => "gemini",
            LlmProvider::Ollama => "ollama",
        }
    }

    pub fn default_endpoint(&self, model: &str) -> String {
        match self {
            LlmProvider::DeepSeek => "https://api.deepseek.com/v1/chat/completions".to_string(),
            LlmProvider::Anthropic => "https://api.anthropic.com/v1/messages".to_string(),
            LlmProvider::OpenAi => "https://api.openai.com/v1/chat/completions".to_string(),
            LlmProvider::Gemini => format!(
                "https://generativelanguage.googleapis.com/v1beta/models/{}:generateContent",
                model
            ),
            LlmProvider::Ollama => "http://127.0.0.1:11434/api/chat".to_string(),
        }
    }

    pub fn default_auth_header(&self) -> Option<&'static str> {
        match self {
            LlmProvider::DeepSeek | LlmProvider::OpenAi => Some("authorization"),
            LlmProvider::Anthropic => Some("x-api-key"),
            LlmProvider::Gemini => Some("x-goog-api-key"),
            LlmProvider::Ollama => None,
        }
    }
}

/// Configuration for the Agent Harness.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct HarnessConfig {
    pub provider: LlmProvider,
    pub model: String,
    pub endpoint_url: String,
    pub key_aliases: Vec<String>,
    pub max_steps: usize,
    pub initial_backoff_ms: u64,
    pub max_backoff_ms: u64,
}

impl HarnessConfig {
    pub fn new(
        provider: LlmProvider,
        model: &str,
        endpoint_url: &str,
        key_aliases: Vec<String>,
    ) -> Self {
        Self {
            provider,
            model: model.to_string(),
            endpoint_url: endpoint_url.to_string(),
            key_aliases,
            max_steps: 10,
            initial_backoff_ms: 10,
            max_backoff_ms: 500,
        }
    }
}

/// Granular, typed errors for the Agent Harness.
/// INVARIANT: No secret values are EVER present in error variants or format strings.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum HarnessError {
    AllKeysExhausted {
        provider: String,
        attempted_aliases: Vec<String>,
        last_status: u16,
    },
    MaxStepsExceeded {
        step: usize,
        max_steps: usize,
    },
    IrreparableToolJson {
        tool_name: String,
        detail: String,
    },
    ToolExecutionFailed {
        tool_name: String,
        detail: String,
    },
    InsecureTransport(String),
    TlsCertificateValidationFailed(String),
    SecretStoreError(String),
    GatekeeperDenied(String),
    NetworkError(String),
    Cancelled,
}

impl std::fmt::Display for HarnessError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            HarnessError::AllKeysExhausted {
                provider,
                attempted_aliases,
                last_status,
            } => write!(
                f,
                "all API keys exhausted for provider '{}' after {} attempts (aliases: {:?}, last HTTP status: {})",
                provider,
                attempted_aliases.len(),
                attempted_aliases,
                last_status
            ),
            HarnessError::MaxStepsExceeded { step, max_steps } => write!(
                f,
                "agent turn reached maximum iteration budget (step {} of {}), confirmation required to proceed",
                step, max_steps
            ),
            HarnessError::IrreparableToolJson { tool_name, detail } => write!(
                f,
                "tool '{}' JSON payload is irreparably malformed: {}",
                tool_name, detail
            ),
            HarnessError::ToolExecutionFailed { tool_name, detail } => write!(
                f,
                "tool '{}' execution failed: {}",
                tool_name, detail
            ),
            HarnessError::InsecureTransport(msg) => write!(
                f,
                "insecure transport rejected: {}",
                msg
            ),
            HarnessError::TlsCertificateValidationFailed(msg) => write!(
                f,
                "TLS certificate validation failed: {}",
                msg
            ),
            HarnessError::SecretStoreError(msg) => write!(f, "secret store error: {}", msg),
            HarnessError::GatekeeperDenied(msg) => write!(f, "denied by Gatekeeper: {}", msg),
            HarnessError::NetworkError(msg) => write!(f, "network/HTTP transport error: {}", msg),
            HarnessError::Cancelled => write!(f, "agent turn cooperatively cancelled"),
        }
    }
}

impl std::error::Error for HarnessError {}

/// Result of executing a step or completing an agent turn.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum TurnStepOutcome {
    Completed {
        reply: String,
        total_steps: usize,
    },
    ToolExecuted {
        step: usize,
        tool: String,
        output: String,
    },
    NeedsConfirmation {
        step: usize,
        max_steps: usize,
        pending_tool: Option<String>,
    },
    KeyRotated {
        from_alias: String,
        to_alias: String,
        reason_status: u16,
    },
}

/// Extracted tool call request from an LLM response.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ToolCallRequest {
    pub tool_name: String,
    pub raw_args: String,
}
