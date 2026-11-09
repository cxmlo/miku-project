use std::collections::{BTreeMap, BTreeSet};
use std::future::Future;
use std::time::Duration;

use miku_protocol::{
    ClaudeHookEvent, ClaudeHookResult, ClaudePreToolUseParams, ClaudeSessionPhase,
    JsonRpcRequest, JsonRpcResponse, MikuLinkEnvelopeV1, MikuLinkMessageV1,
    CLAUDE_HOOK_FAIL_OPEN_TIMEOUT_MS, JSONRPC_PARSE_ERROR, MIKU_PROTOCOL_VERSION_V1,
};
use serde::{Deserialize, Serialize};

pub const CLAUDE_INTERCEPT_LATENCY_BUDGET_MS: f64 = 2.0;
pub const CLAUDE_ADDED_RAM_BUDGET_MB: f64 = 5.0;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ClaudeProcessSnapshot {
    pub pid: u32,
    pub image_name: String,
    pub cmdline: String,
    pub cwd: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ClaudeCliSessionInfo {
    pub pid: u32,
    pub session_id: String,
    pub image_name: String,
    pub cwd: String,
    pub model_override: Option<String>,
    pub detected_at_ms: u64,
}

#[derive(Debug, Clone, Default)]
pub struct ClaudeCliSessionTracker {
    sessions: BTreeMap<u32, ClaudeCliSessionInfo>,
}

impl ClaudeCliSessionTracker {
    pub fn new() -> Self {
        Self {
            sessions: BTreeMap::new(),
        }
    }

    pub fn parse_process_snapshot(
        snapshot: &ClaudeProcessSnapshot,
        now_ms: u64,
    ) -> Option<ClaudeCliSessionInfo> {
        let img = snapshot.image_name.trim().to_ascii_lowercase();
        let cmd_lower = snapshot.cmdline.to_ascii_lowercase();

        let is_claude_binary = img == "claude"
            || img == "claude.exe"
            || img == "claude-cli"
            || img == "claude-cli.exe";
        let is_node_claude = (img == "node" || img == "node.exe")
            && (cmd_lower.contains("claude-code") || cmd_lower.contains("@anthropic-ai/claude"));

        if !is_claude_binary && !is_node_claude {
            return None;
        }

        let tokens: Vec<&str> = snapshot.cmdline.split_whitespace().collect();
        let mut session_id: Option<String> = None;
        let mut model_override: Option<String> = None;

        let mut i = 0;
        while i < tokens.len() {
            match tokens[i] {
                "--session-id" | "--resume" | "-r" => {
                    if let Some(val) = tokens.get(i + 1) {
                        session_id = Some((*val).trim_matches('"').to_string());
                        i += 1;
                    }
                }
                "--model" | "-m" => {
                    if let Some(val) = tokens.get(i + 1) {
                        model_override = Some((*val).trim_matches('"').to_string());
                        i += 1;
                    }
                }
                _ => {}
            }
            i += 1;
        }

        let resolved_session_id =
            session_id.unwrap_or_else(|| format!("claude_pid_{}", snapshot.pid));

        Some(ClaudeCliSessionInfo {
            pid: snapshot.pid,
            session_id: resolved_session_id,
            image_name: snapshot.image_name.clone(),
            cwd: snapshot.cwd.clone(),
            model_override,
            detected_at_ms: now_ms,
        })
    }

    pub fn poll_processes(
        &mut self,
        snapshots: &[ClaudeProcessSnapshot],
        now_ms: u64,
    ) -> Vec<ClaudeCliSessionInfo> {
        let mut seen_pids = BTreeSet::new();
        for snap in snapshots {
            if let Some(info) = Self::parse_process_snapshot(snap, now_ms) {
                seen_pids.insert(info.pid);
                self.sessions.entry(info.pid).or_insert(info);
            }
        }
        self.sessions.retain(|pid, _| seen_pids.contains(pid));
        self.sessions.values().cloned().collect()
    }

    pub fn active_session_count(&self) -> usize {
        self.sessions.len()
    }

    pub fn get_by_pid(&self, pid: u32) -> Option<&ClaudeCliSessionInfo> {
        self.sessions.get(&pid)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AvatarMoodState {
    Idle,
    Thinking,
    Working,
    Alert,
    Happy,
}

impl AvatarMoodState {
    pub fn as_str(&self) -> &'static str {
        match self {
            AvatarMoodState::Idle => "idle",
            AvatarMoodState::Thinking => "thinking",
            AvatarMoodState::Working => "working",
            AvatarMoodState::Alert => "alert",
            AvatarMoodState::Happy => "happy",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ClaudeAvatarTelemetryEvent {
    pub session_id: String,
    pub mood: AvatarMoodState,
    pub summary: String,
    pub timestamp_ms: u64,
}

#[derive(Debug, Clone)]
pub struct AvatarActivityDispatcher {
    current_mood: AvatarMoodState,
    events: Vec<ClaudeAvatarTelemetryEvent>,
}

impl Default for AvatarActivityDispatcher {
    fn default() -> Self {
        Self::new()
    }
}

impl AvatarActivityDispatcher {
    pub fn new() -> Self {
        Self {
            current_mood: AvatarMoodState::Idle,
            events: Vec::new(),
        }
    }

    pub fn current_mood(&self) -> AvatarMoodState {
        self.current_mood
    }

    pub fn history(&self) -> &[ClaudeAvatarTelemetryEvent] {
        &self.events
    }

    pub fn transition_mood(
        &mut self,
        session_id: &str,
        mood: AvatarMoodState,
        summary: impl Into<String>,
        now_ms: u64,
    ) -> MikuLinkEnvelopeV1 {
        let summary_str = summary.into();
        self.current_mood = mood;
        self.events.push(ClaudeAvatarTelemetryEvent {
            session_id: session_id.to_string(),
            mood,
            summary: summary_str.clone(),
            timestamp_ms: now_ms,
        });

        MikuLinkEnvelopeV1 {
            v: MIKU_PROTOCOL_VERSION_V1,
            id: format!("claude-mood-{}-{}", session_id, self.events.len()),
            reply_to: None,
            msg: MikuLinkMessageV1::AgentEvent {
                agent: "claude_code".to_string(),
                kind: format!("avatar_mood:{}", mood.as_str()),
                title: summary_str,
            },
        }
    }

    pub fn on_hook_event(
        &mut self,
        event: &ClaudeHookEvent,
        now_ms: u64,
    ) -> MikuLinkEnvelopeV1 {
        match event {
            ClaudeHookEvent::Notification(notif) => {
                let mood = match notif.phase {
                    ClaudeSessionPhase::Thinking => AvatarMoodState::Thinking,
                    ClaudeSessionPhase::Working => AvatarMoodState::Working,
                    ClaudeSessionPhase::WaitingPermission => AvatarMoodState::Alert,
                    ClaudeSessionPhase::Idle => AvatarMoodState::Idle,
                };
                self.transition_mood(&notif.session_id, mood, &notif.message, now_ms)
            }
            ClaudeHookEvent::PreToolUse(pre) => self.transition_mood(
                &pre.session_id,
                AvatarMoodState::Working,
                format!("PreToolUse:{}", pre.tool_name),
                now_ms,
            ),
            ClaudeHookEvent::PostToolUse(post) => {
                let mood = if post.is_error {
                    AvatarMoodState::Alert
                } else {
                    AvatarMoodState::Happy
                };
                self.transition_mood(
                    &post.session_id,
                    mood,
                    format!("PostToolUse:{}:{}ms", post.tool_name, post.duration_ms),
                    now_ms,
                )
            }
        }
    }
}

/// Executes a Claude Code hook JSON-RPC request with strict fail-open guarantee (`< 15 ms`).
pub async fn execute_hook_with_fail_open<F, Fut>(
    raw_json_rpc_line: &str,
    timeout: Duration,
    daemon_rpc: F,
) -> String
where
    F: FnOnce(JsonRpcRequest<ClaudePreToolUseParams>) -> Fut,
    Fut: Future<Output = Result<JsonRpcResponse<ClaudeHookResult>, String>>,
{
    let parsed_req = match JsonRpcRequest::<ClaudePreToolUseParams>::decode_line(raw_json_rpc_line)
    {
        Ok(r) => r,
        Err(e) => {
            let err_resp = JsonRpcResponse::<ClaudeHookResult>::err(
                None,
                JSONRPC_PARSE_ERROR,
                e.to_string(),
                None,
            );
            return err_resp
                .encode_line()
                .unwrap_or_else(|_| "{\"jsonrpc\":\"2.0\",\"error\":{\"code\":-32700,\"message\":\"parse error\"}}\n".to_string());
        }
    };

    let req_id = parsed_req.id.clone();
    let max_budget = Duration::from_millis(CLAUDE_HOOK_FAIL_OPEN_TIMEOUT_MS);
    let effective_timeout = if timeout < max_budget {
        timeout
    } else {
        max_budget
    };

    let start = std::time::Instant::now();
    let mut fut = std::pin::pin!(daemon_rpc(parsed_req));
    let outcome = std::future::poll_fn(|cx| {
        if let std::task::Poll::Ready(val) = fut.as_mut().poll(cx) {
            return std::task::Poll::Ready(Ok(val));
        }
        if start.elapsed() >= effective_timeout {
            return std::task::Poll::Ready(Err(()));
        }
        std::hint::spin_loop();
        cx.waker().wake_by_ref();
        std::task::Poll::Pending
    })
    .await;

    let resp = match outcome {
        Ok(Ok(valid_response)) => valid_response,
        Ok(Err(daemon_err)) => JsonRpcResponse::ok(
            req_id,
            ClaudeHookResult::fail_open(format!("miku_daemon_unavailable: {}", daemon_err)),
        ),
        Err(()) => JsonRpcResponse::ok(
            req_id,
            ClaudeHookResult::fail_open("miku_daemon_timeout_fail_open"),
        ),
    };

    resp.encode_line().unwrap_or_else(|_| {
        "{\"jsonrpc\":\"2.0\",\"result\":{\"decision\":\"fail_open\"}}\n".to_string()
    })
}
