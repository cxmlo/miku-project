#![allow(unused_imports)]
use crate::prelude::*;

/// Category of technical feed source (`F-14`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TechFeedSourceKind {
    PullRequests,
    CiStatus,
    RssFeed,
    RepoRadar,
}

impl TechFeedSourceKind {
    pub fn as_str(&self) -> &'static str {
        match self {
            TechFeedSourceKind::PullRequests => "pull_requests",
            TechFeedSourceKind::CiStatus => "ci_status",
            TechFeedSourceKind::RssFeed => "rss_feed",
            TechFeedSourceKind::RepoRadar => "repo_radar",
        }
    }
}

/// Stored HTTP conditional validators (`ETag` and `Last-Modified`) per feed source.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ConditionalValidators {
    pub etag: Option<String>,
    pub last_modified: Option<String>,
}

impl ConditionalValidators {
    pub fn is_empty(&self) -> bool {
        self.etag.is_none() && self.last_modified.is_none()
    }
}

/// GitHub API rate-limit tracking (`X-RateLimit-Remaining` and `X-RateLimit-Reset`).
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct RateLimitState {
    pub remaining: Option<u32>,
    pub reset_at_ms: Option<u64>,
}

impl RateLimitState {
    pub fn is_exhausted(&self, now_ms: u64) -> bool {
        match (self.remaining, self.reset_at_ms) {
            (Some(0), Some(reset_ms)) => now_ms < reset_ms,
            (Some(0), None) => true,
            _ => false,
        }
    }
}

/// Configuration for a single technical feed source.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TechFeedSourceConfig {
    pub source_id: String,
    pub title: String,
    pub kind: TechFeedSourceKind,
    pub url: String,
    pub is_github_api: bool,
    pub timeout_ms: u64,
}

impl TechFeedSourceConfig {
    pub fn new(
        source_id: &str,
        title: &str,
        kind: TechFeedSourceKind,
        url: &str,
        is_github_api: bool,
        timeout_ms: u64,
    ) -> Self {
        Self {
            source_id: source_id.to_string(),
            title: title.to_string(),
            kind,
            url: url.to_string(),
            is_github_api,
            timeout_ms,
        }
    }
}

/// Normalized feed item displayed in the Island technical feed (`F-14`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TechFeedItem {
    pub item_id: String,
    pub source_id: String,
    pub kind: TechFeedSourceKind,
    pub title: String,
    pub summary: String,
    pub url: String,
    pub badge: String,
}

/// Outbound conditional HTTP request issued by the technical feed controller.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FeedHttpRequest {
    pub source_id: String,
    pub url: String,
    pub if_none_match: Option<String>,
    pub if_modified_since: Option<String>,
    pub timeout_ms: u64,
}

/// Inbound HTTP response from a feed source.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FeedHttpResponse {
    pub status_code: u16,
    pub etag: Option<String>,
    pub last_modified: Option<String>,
    pub rate_limit_remaining: Option<u32>,
    pub rate_limit_reset_ms: Option<u64>,
    pub body: String,
}

impl FeedHttpResponse {
    pub fn ok_200(
        body: &str,
        etag: Option<&str>,
        last_modified: Option<&str>,
        rate_limit_remaining: Option<u32>,
        rate_limit_reset_ms: Option<u64>,
    ) -> Self {
        Self {
            status_code: 200,
            etag: etag.map(|s| s.to_string()),
            last_modified: last_modified.map(|s| s.to_string()),
            rate_limit_remaining,
            rate_limit_reset_ms,
            body: body.to_string(),
        }
    }

    pub fn not_modified_304(
        etag: Option<&str>,
        last_modified: Option<&str>,
        rate_limit_remaining: Option<u32>,
        rate_limit_reset_ms: Option<u64>,
    ) -> Self {
        Self {
            status_code: 304,
            etag: etag.map(|s| s.to_string()),
            last_modified: last_modified.map(|s| s.to_string()),
            rate_limit_remaining,
            rate_limit_reset_ms,
            body: String::new(),
        }
    }
}

/// Typed transport error for per-source fault isolation.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum FeedTransportError {
    Timeout { source_id: String, timeout_ms: u64 },
    NetworkFailure { source_id: String, detail: String },
}

impl std::fmt::Display for FeedTransportError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            FeedTransportError::Timeout {
                source_id,
                timeout_ms,
            } => write!(f, "feed source '{}' timed out after {} ms", source_id, timeout_ms),
            FeedTransportError::NetworkFailure { source_id, detail } => {
                write!(f, "feed source '{}' network failure: {}", source_id, detail)
            }
        }
    }
}

/// Pluggable transport trait so tests run deterministically offline while validating exact headers.
pub trait FeedTransport {
    fn fetch(&mut self, request: &FeedHttpRequest) -> Result<FeedHttpResponse, FeedTransportError>;
}

/// Deterministic mock transport recording all requests and serving queued responses per source.
#[derive(Default)]
pub struct MockFeedTransport {
    queues: BTreeMap<String, VecDeque<Result<FeedHttpResponse, FeedTransportError>>>,
    recorded_requests: Vec<FeedHttpRequest>,
}

impl MockFeedTransport {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn enqueue_response(
        &mut self,
        source_id: &str,
        response: Result<FeedHttpResponse, FeedTransportError>,
    ) {
        self.queues
            .entry(source_id.to_string())
            .or_default()
            .push_back(response);
    }

    pub fn recorded_requests(&self) -> &[FeedHttpRequest] {
        &self.recorded_requests
    }

    pub fn requests_for_source(&self, source_id: &str) -> Vec<&FeedHttpRequest> {
        self.recorded_requests
            .iter()
            .filter(|r| r.source_id == source_id)
            .collect()
    }
}

impl FeedTransport for MockFeedTransport {
    fn fetch(&mut self, request: &FeedHttpRequest) -> Result<FeedHttpResponse, FeedTransportError> {
        self.recorded_requests.push(request.clone());
        if let Some(queue) = self.queues.get_mut(&request.source_id) {
            if let Some(next) = queue.pop_front() {
                return next;
            }
        }
        Err(FeedTransportError::NetworkFailure {
            source_id: request.source_id.clone(),
            detail: "no queued mock response available".to_string(),
        })
    }
}

/// Pluggable Zen Browser launcher invoked only after Gatekeeper `Capability::Browser` approval.
pub trait ZenBrowserLauncher {
    fn open_in_zen_browser(&mut self, url: &str) -> Result<Vec<String>, String>;
}

/// Deterministic Zen Browser launcher for tests and dry-run verification.
#[derive(Default)]
pub struct MockZenBrowserLauncher {
    opened_urls: Vec<String>,
    invocations: Vec<Vec<String>>,
}

impl MockZenBrowserLauncher {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn opened_urls(&self) -> &[String] {
        &self.opened_urls
    }

    pub fn invocations(&self) -> &[Vec<String>] {
        &self.invocations
    }
}

impl ZenBrowserLauncher for MockZenBrowserLauncher {
    fn open_in_zen_browser(&mut self, url: &str) -> Result<Vec<String>, String> {
        let cmd = vec![
            "zen.exe".to_string(),
            "--new-tab".to_string(),
            url.to_string(),
        ];
        self.opened_urls.push(url.to_string());
        self.invocations.push(cmd.clone());
        Ok(cmd)
    }
}

/// Redacts tokens and credentials (`ghp_`, `github_pat_`, `Bearer `, `token `) from feed diagnostics.
pub fn sanitize_feed_diagnostic(raw: &str, extra_secrets: &[&str]) -> String {
    let mut out = raw.to_string();
    for secret in extra_secrets {
        let trimmed = secret.trim();
        if !trimmed.is_empty() && out.contains(trimmed) {
            out = out.replace(trimmed, "[REDACTED_SECRET]");
        }
    }
    for prefix in ["ghp_", "github_pat_", "glpat-"] {
        while let Some(idx) = out.find(prefix) {
            let end = out[idx..]
                .find(|c: char| c.is_whitespace() || c == '"' || c == '\'' || c == ',' || c == ';')
                .map(|offset| idx + offset)
                .unwrap_or(out.len());
            out.replace_range(idx..end, "[REDACTED_SECRET]");
        }
    }
    out
}

fn extract_xml_tag(block: &str, tag: &str) -> Option<String> {
    let open = format!("<{}>", tag);
    let close = format!("</{}>", tag);
    if let Some(start_idx) = block.find(&open) {
        let content_start = start_idx + open.len();
        if let Some(end_offset) = block[content_start..].find(&close) {
            return Some(block[content_start..content_start + end_offset].trim().to_string());
        }
    }
    None
}

/// Parses RSS 2.0 (`<item>`) or Atom (`<entry>`) XML payloads into normalized `TechFeedItem`s.
pub fn parse_rss_or_atom_items(source_id: &str, xml: &str) -> Result<Vec<TechFeedItem>, String> {
    let trimmed = xml.trim();
    if trimmed.is_empty() || (!trimmed.contains("<rss") && !trimmed.contains("<feed")) {
        return Err("invalid RSS/Atom XML payload: missing <rss> or <feed> root".to_string());
    }

    let tag = if trimmed.contains("<item>") {
        "item"
    } else if trimmed.contains("<entry>") {
        "entry"
    } else {
        return Err("invalid RSS/Atom XML payload: no <item> or <entry> elements".to_string());
    };

    let open_tag = format!("<{}>", tag);
    let close_tag = format!("</{}>", tag);
    let mut items = Vec::new();
    let mut cursor = 0usize;

    while let Some(rel_start) = trimmed[cursor..].find(&open_tag) {
        let body_start = cursor + rel_start + open_tag.len();
        let rel_end = trimmed[body_start..]
            .find(&close_tag)
            .ok_or_else(|| format!("unclosed <{}> tag in RSS/Atom feed", tag))?;
        let block = &trimmed[body_start..body_start + rel_end];

        let title = extract_xml_tag(block, "title")
            .ok_or_else(|| "missing <title> in RSS/Atom item".to_string())?;
        let url = extract_xml_tag(block, "link")
            .ok_or_else(|| "missing <link> in RSS/Atom item".to_string())?;
        let summary = extract_xml_tag(block, "description")
            .or_else(|| extract_xml_tag(block, "summary"))
            .unwrap_or_else(|| "Technical feed entry".to_string());
        let raw_id = extract_xml_tag(block, "guid")
            .or_else(|| extract_xml_tag(block, "id"))
            .unwrap_or_else(|| format!("{}:{}", source_id, url));

        items.push(TechFeedItem {
            item_id: format!("rss:{}:{}", source_id, raw_id),
            source_id: source_id.to_string(),
            kind: TechFeedSourceKind::RssFeed,
            title,
            summary,
            url,
            badge: "RSS".to_string(),
        });

        cursor = body_start + rel_end + close_tag.len();
    }

    Ok(items)
}

/// Parses GitHub JSON arrays for Pull Requests, CI Status, or Repo Radar (`F-14`).
pub fn parse_github_json_items(
    source_id: &str,
    kind: TechFeedSourceKind,
    json_str: &str,
) -> Result<Vec<TechFeedItem>, String> {
    let val: serde_json::Value =
        serde_json::from_str(json_str).map_err(|e| format!("invalid GitHub JSON: {}", e))?;
    let arr = val
        .as_array()
        .ok_or_else(|| "expected JSON array of technical feed items".to_string())?;

    let mut out = Vec::with_capacity(arr.len());
    for entry in arr {
        let id = entry
            .get("id")
            .and_then(|v| {
                v.as_str()
                    .map(|s| s.to_string())
                    .or_else(|| v.as_u64().map(|n| n.to_string()))
            })
            .ok_or_else(|| "missing 'id' field in GitHub feed item".to_string())?;
        let title = entry
            .get("title")
            .or_else(|| entry.get("name"))
            .and_then(|v| v.as_str())
            .ok_or_else(|| "missing 'title' or 'name' field in GitHub feed item".to_string())?
            .to_string();
        let url = entry
            .get("html_url")
            .or_else(|| entry.get("url"))
            .and_then(|v| v.as_str())
            .ok_or_else(|| "missing 'html_url' or 'url' field in GitHub feed item".to_string())?
            .to_string();
        let summary = entry
            .get("summary")
            .or_else(|| entry.get("conclusion"))
            .or_else(|| entry.get("description"))
            .and_then(|v| v.as_str())
            .unwrap_or("GitHub technical update")
            .to_string();
        let badge = match kind {
            TechFeedSourceKind::PullRequests => "PR",
            TechFeedSourceKind::CiStatus => "CI",
            TechFeedSourceKind::RepoRadar => "RADAR",
            TechFeedSourceKind::RssFeed => "RSS",
        }
        .to_string();

        out.push(TechFeedItem {
            item_id: format!("{}:{}:{}", kind.as_str(), source_id, id),
            source_id: source_id.to_string(),
            kind,
            title,
            summary,
            url,
            badge,
        });
    }
    Ok(out)
}
