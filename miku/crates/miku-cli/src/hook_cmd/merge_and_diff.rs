use serde_json::{Map, Value};

use super::{HookInstallError, HookTargetAgent, MIKU_HOOK_ENTRY_ID};

/// Redacts quoted string literals and token-like substrings from JSON parser errors
/// so sensitive values in a broken `settings.json` are never echoed in logs or diagnostics.
pub fn sanitize_json_error_reason(raw_err: &str) -> String {
    let mut out = String::with_capacity(raw_err.len());
    let mut in_quotes = false;
    for ch in raw_err.chars() {
        if ch == '"' {
            in_quotes = !in_quotes;
            out.push('"');
            if in_quotes {
                out.push_str("[REDACTED]");
            }
        } else if !in_quotes {
            out.push(ch);
        }
    }
    out
}

/// Validates that `raw` is syntactically valid JSON and has a top-level JSON Object.
/// Reports exact 1-based line and column numbers on failure before any file is touched.
pub fn validate_and_parse_settings_json(
    raw: &str,
) -> Result<Map<String, Value>, HookInstallError> {
    if raw.trim().is_empty() {
        return Err(HookInstallError::InvalidJsonSyntax {
            line: 1,
            column: 1,
            reason: "settings.json is empty".to_string(),
        });
    }

    let val: Value = serde_json::from_str(raw).map_err(|err| HookInstallError::InvalidJsonSyntax {
        line: err.line().max(1),
        column: err.column().max(1),
        reason: sanitize_json_error_reason(&err.to_string()),
    })?;

    match val {
        Value::Object(map) => Ok(map),
        _ => Err(HookInstallError::InvalidJsonRootType {
            line: 1,
            reason: "top-level value in settings.json must be a JSON object ({ ... })".to_string(),
        }),
    }
}

fn build_claude_hook_entry(event_snake: &str) -> Value {
    serde_json::json!({
        "id": MIKU_HOOK_ENTRY_ID,
        "matcher": "*",
        "type": "command",
        "command": format!("miku-cli hook stdio --agent claude-code --event {}", event_snake),
        "timeout_ms": 15
    })
}

fn build_cursor_hook_entry() -> Value {
    serde_json::json!({
        "id": MIKU_HOOK_ENTRY_ID,
        "matcher": "*",
        "type": "command",
        "command": "miku-cli hook stdio --agent cursor --event pre_tool_use",
        "timeout_ms": 15
    })
}

fn build_cursor_mcp_entry() -> Value {
    serde_json::json!({
        "id": MIKU_HOOK_ENTRY_ID,
        "command": "miku-cli",
        "args": ["hook", "mcp", "--agent", "cursor"],
        "transport": "stdio"
    })
}

fn is_miku_hook_object(item: &Value) -> bool {
    item.get("id")
        .and_then(|v| v.as_str())
        == Some(MIKU_HOOK_ENTRY_ID)
}

fn upsert_miku_entry_in_event_array(
    hooks_obj: &mut Map<String, Value>,
    event_key: &str,
    miku_entry: Value,
) -> Result<(), HookInstallError> {
    let arr_val = hooks_obj
        .entry(event_key.to_string())
        .or_insert_with(|| Value::Array(Vec::new()));

    let arr = arr_val.as_array_mut().ok_or_else(|| {
        HookInstallError::InvalidJsonRootType {
            line: 1,
            reason: format!(
                "hooks.{} must be a JSON array in settings.json",
                event_key
            ),
        }
    })?;

    let mut replaced_first = false;
    let mut cleaned = Vec::with_capacity(arr.len() + 1);
    for existing in arr.drain(..) {
        if is_miku_hook_object(&existing) {
            if !replaced_first {
                cleaned.push(miku_entry.clone());
                replaced_first = true;
            }
        } else {
            cleaned.push(existing);
        }
    }

    if !replaced_first {
        cleaned.push(miku_entry);
    }

    *arr = cleaned;
    Ok(())
}

/// Merges ONLY the Miku hook/MCP entry into `raw_existing`, preserving all user keys,
/// foreign hooks, and foreign MCP servers untouched. Returns `(merged_pretty_json, idempotent_noop)`.
pub fn merge_miku_hook_settings(
    raw_existing: &str,
    agent: HookTargetAgent,
) -> Result<(String, bool), HookInstallError> {
    let mut root_map = validate_and_parse_settings_json(raw_existing)?;
    let before_map = root_map.clone();

    match agent {
        HookTargetAgent::ClaudeCode => {
            let hooks_val = root_map
                .entry("hooks".to_string())
                .or_insert_with(|| Value::Object(Map::new()));
            let hooks_obj = hooks_val.as_object_mut().ok_or_else(|| {
                HookInstallError::InvalidJsonRootType {
                    line: 1,
                    reason: "'hooks' key in settings.json must be a JSON object".to_string(),
                }
            })?;

            upsert_miku_entry_in_event_array(
                hooks_obj,
                "PreToolUse",
                build_claude_hook_entry("pre_tool_use"),
            )?;
            upsert_miku_entry_in_event_array(
                hooks_obj,
                "PostToolUse",
                build_claude_hook_entry("post_tool_use"),
            )?;
            upsert_miku_entry_in_event_array(
                hooks_obj,
                "Notification",
                build_claude_hook_entry("notification"),
            )?;
        }
        HookTargetAgent::Cursor => {
            let mcp_val = root_map
                .entry("mcpServers".to_string())
                .or_insert_with(|| Value::Object(Map::new()));
            let mcp_obj = mcp_val.as_object_mut().ok_or_else(|| {
                HookInstallError::InvalidJsonRootType {
                    line: 1,
                    reason: "'mcpServers' key in settings.json must be a JSON object".to_string(),
                }
            })?;
            mcp_obj.insert(MIKU_HOOK_ENTRY_ID.to_string(), build_cursor_mcp_entry());

            let hooks_val = root_map
                .entry("hooks".to_string())
                .or_insert_with(|| Value::Object(Map::new()));
            let hooks_obj = hooks_val.as_object_mut().ok_or_else(|| {
                HookInstallError::InvalidJsonRootType {
                    line: 1,
                    reason: "'hooks' key in settings.json must be a JSON object".to_string(),
                }
            })?;
            upsert_miku_entry_in_event_array(
                hooks_obj,
                "PreToolUse",
                build_cursor_hook_entry(),
            )?;
        }
    }

    let mut merged_text = serde_json::to_string_pretty(&Value::Object(root_map.clone()))
        .map_err(|e| HookInstallError::IoError(format!("failed to serialize merged JSON: {}", e)))?;
    merged_text.push('\n');

    let idempotent_noop = (before_map == root_map) && (raw_existing == merged_text);
    Ok((merged_text, idempotent_noop))
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum DiffEdit<'a> {
    Equal(&'a str),
    Delete(&'a str),
    Insert(&'a str),
}

/// Computes a deterministic unified diff (`--- a/settings.json`, `+++ b/settings.json`)
/// using Longest Common Subsequence (LCS) without external dependencies.
pub fn compute_unified_diff(
    old_text: &str,
    new_text: &str,
    old_label: &str,
    new_label: &str,
) -> String {
    let old_lines: Vec<&str> = old_text.lines().collect();
    let new_lines: Vec<&str> = new_text.lines().collect();

    let mut out = String::new();
    out.push_str(&format!("--- {}\n", old_label));
    out.push_str(&format!("+++ {}\n", new_label));

    if old_lines == new_lines {
        out.push_str("@@ -1,0 +1,0 @@\n (no changes - Miku hook entry already up to date)\n");
        return out;
    }

    let n = old_lines.len();
    let m = new_lines.len();
    let mut dp = vec![vec![0usize; m + 1]; n + 1];

    for i in (0..n).rev() {
        for j in (0..m).rev() {
            if old_lines[i] == new_lines[j] {
                dp[i][j] = 1 + dp[i + 1][j + 1];
            } else {
                dp[i][j] = dp[i + 1][j].max(dp[i][j + 1]);
            }
        }
    }

    let mut edits = Vec::with_capacity(n + m);
    let mut i = 0;
    let mut j = 0;
    while i < n && j < m {
        if old_lines[i] == new_lines[j] {
            edits.push(DiffEdit::Equal(old_lines[i]));
            i += 1;
            j += 1;
        } else if dp[i + 1][j] >= dp[i][j + 1] {
            edits.push(DiffEdit::Delete(old_lines[i]));
            i += 1;
        } else {
            edits.push(DiffEdit::Insert(new_lines[j]));
            j += 1;
        }
    }
    while i < n {
        edits.push(DiffEdit::Delete(old_lines[i]));
        i += 1;
    }
    while j < m {
        edits.push(DiffEdit::Insert(new_lines[j]));
        j += 1;
    }

    out.push_str(&format!("@@ -1,{} +1,{} @@\n", n.max(1), m.max(1)));
    for edit in edits {
        match edit {
            DiffEdit::Equal(line) => {
                out.push(' ');
                out.push_str(line);
                out.push('\n');
            }
            DiffEdit::Delete(line) => {
                out.push('-');
                out.push_str(line);
                out.push('\n');
            }
            DiffEdit::Insert(line) => {
                out.push('+');
                out.push_str(line);
                out.push('\n');
            }
        }
    }

    out
}
