use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

pub const GENESIS_PREV_HASH: &str =
    "0000000000000000000000000000000000000000000000000000000000000000";
pub const MAX_IPC_FRAME_BYTES: usize = 64 * 1024;
pub const DECISION_LATENCY_P95_MAX_MS: f64 = 5.0;
pub const IDLE_RAM_MAX_MB: f64 = 30.0;
pub const RELEASE_BINARY_MAX_MB: f64 = 15.0;

// ============================================================================
// 1. SELF-CONTAINED FIPS 180-4 SHA-256 + CONSTANT-TIME AUTH
// ============================================================================

const K256: [u32; 64] = [
    0x428a2f98, 0x71374491, 0xb5c0fbcf, 0xe9b5dba5, 0x3956c25b, 0x59f111f1, 0x923f82a4, 0xab1c5ed5,
    0xd807aa98, 0x12835b01, 0x243185be, 0x550c7dc3, 0x72be5d74, 0x80deb1fe, 0x9bdc06a7, 0xc19bf174,
    0xe49b69c1, 0xefbe4786, 0x0fc19dc6, 0x240ca1cc, 0x2de92c6f, 0x4a7484aa, 0x5cb0a9dc, 0x76f988da,
    0x983e5152, 0xa831c66d, 0xb00327c8, 0xbf597fc7, 0xc6e00bf3, 0xd5a79147, 0x06ca6351, 0x14292967,
    0x27b70a85, 0x2e1b2138, 0x4d2c6dfc, 0x53380d13, 0x650a7354, 0x766a0abb, 0x81c2c92e, 0x92722c85,
    0xa2bfe8a1, 0xa81a664b, 0xc24b8b70, 0xc76c51a3, 0xd192e819, 0xd6990624, 0xf40e3585, 0x106aa070,
    0x19a4c116, 0x1e376c08, 0x2748774c, 0x34b0bcb5, 0x391c0cb3, 0x4ed8aa4a, 0x5b9cca4f, 0x682e6ff3,
    0x748f82ee, 0x78a5636f, 0x84c87814, 0x8cc70208, 0x90befffa, 0xa4506ceb, 0xbef9a3f7, 0xc67178f2,
];

pub fn sha256_digest(data: &[u8]) -> [u8; 32] {
    let mut state: [u32; 8] = [
        0x6a09e667, 0xbb67ae85, 0x3c6ef372, 0xa54ff53a,
        0x510e527f, 0x9b05688c, 0x1f83d9ab, 0x5be0cd19,
    ];

    let bit_len = (data.len() as u64) * 8;
    let mut padded = Vec::with_capacity(((data.len() + 9 + 63) / 64) * 64);
    padded.extend_from_slice(data);
    padded.push(0x80);
    while (padded.len() % 64) != 56 {
        padded.push(0x00);
    }
    padded.extend_from_slice(&bit_len.to_be_bytes());

    for chunk in padded.chunks_exact(64) {
        let mut w = [0u32; 64];
        for i in 0..16 {
            w[i] = u32::from_be_bytes([
                chunk[i * 4],
                chunk[i * 4 + 1],
                chunk[i * 4 + 2],
                chunk[i * 4 + 3],
            ]);
        }
        for i in 16..64 {
            let s0 = w[i - 15].rotate_right(7) ^ w[i - 15].rotate_right(18) ^ (w[i - 15] >> 3);
            let s1 = w[i - 2].rotate_right(17) ^ w[i - 2].rotate_right(19) ^ (w[i - 2] >> 10);
            w[i] = w[i - 16]
                .wrapping_add(s0)
                .wrapping_add(w[i - 7])
                .wrapping_add(s1);
        }

        let mut a = state[0];
        let mut b = state[1];
        let mut c = state[2];
        let mut d = state[3];
        let mut e = state[4];
        let mut f = state[5];
        let mut g = state[6];
        let mut h = state[7];

        for i in 0..64 {
            let s1 = e.rotate_right(6) ^ e.rotate_right(11) ^ e.rotate_right(25);
            let ch = (e & f) ^ ((!e) & g);
            let temp1 = h
                .wrapping_add(s1)
                .wrapping_add(ch)
                .wrapping_add(K256[i])
                .wrapping_add(w[i]);
            let s0 = a.rotate_right(2) ^ a.rotate_right(13) ^ a.rotate_right(22);
            let maj = (a & b) ^ (a & c) ^ (b & c);
            let temp2 = s0.wrapping_add(maj);

            h = g;
            g = f;
            f = e;
            e = d.wrapping_add(temp1);
            d = c;
            c = b;
            b = a;
            a = temp1.wrapping_add(temp2);
        }

        state[0] = state[0].wrapping_add(a);
        state[1] = state[1].wrapping_add(b);
        state[2] = state[2].wrapping_add(c);
        state[3] = state[3].wrapping_add(d);
        state[4] = state[4].wrapping_add(e);
        state[5] = state[5].wrapping_add(f);
        state[6] = state[6].wrapping_add(g);
        state[7] = state[7].wrapping_add(h);
    }

    let mut out = [0u8; 32];
    for (i, word) in state.iter().enumerate() {
        out[i * 4..i * 4 + 4].copy_from_slice(&word.to_be_bytes());
    }
    out
}

pub fn sha256_hex(data: &[u8]) -> String {
    let digest = sha256_digest(data);
    let mut hex = String::with_capacity(64);
    for b in digest {
        use std::fmt::Write;
        let _ = write!(&mut hex, "{:02x}", b);
    }
    hex
}

pub fn constant_time_eq(a: &[u8], b: &[u8]) -> bool {
    if a.len() != b.len() {
        return false;
    }
    let mut diff = 0u8;
    for (&x, &y) in a.iter().zip(b.iter()) {
        diff |= x ^ y;
    }
    diff == 0
}

// ============================================================================
// 2. CANONICAL PATH JAIL (TOLERATES NON-EXISTENT FILES ON DISK)
// ============================================================================

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum PathJailError {
    EmptyPath,
    NullByteDetected,
    ControlCharacterDetected,
    UncOrDevicePrefixBlocked(String),
    ReservedWindowsDeviceName(String),
    TraversalEscapeOutsideRoot {
        candidate: String,
        jail_root: String,
    },
    DriveLetterMismatch {
        candidate_drive: String,
        root_drive: String,
    },
}

fn is_reserved_windows_device(stem_upper: &str) -> bool {
    matches!(
        stem_upper,
        "CON"
            | "PRN"
            | "AUX"
            | "NUL"
            | "CONIN$"
            | "CONOUT$"
            | "COM1"
            | "COM2"
            | "COM3"
            | "COM4"
            | "COM5"
            | "COM6"
            | "COM7"
            | "COM8"
            | "COM9"
            | "LPT1"
            | "LPT2"
            | "LPT3"
            | "LPT4"
            | "LPT5"
            | "LPT6"
            | "LPT7"
            | "LPT8"
            | "LPT9"
    )
}

#[derive(Debug, Clone)]
pub struct CanonicalPathJail {
    root_drive: Option<String>,
    root_segments: Vec<String>,
    canonical_root_display: String,
}

impl CanonicalPathJail {
    pub fn new(root_path: &str) -> Result<Self, PathJailError> {
        let (drive, segments) = Self::parse_clean_absolute(root_path)?;
        let canonical_root_display = Self::format_canonical(&drive, &segments);
        Ok(Self {
            root_drive: drive,
            root_segments: segments,
            canonical_root_display,
        })
    }

    pub fn canonical_root(&self) -> &str {
        &self.canonical_root_display
    }

    /// Resolves `candidate` logically against `self.canonical_root` without requiring the target
    /// file or directory to exist on disk. Blocks `..` escapes, UNC paths (`\\`), extended device
    /// prefixes (`\\?\`, `\\.\`), Windows reserved device names (`CON`, `NUL`, `COM1.txt`),
    /// Alternate Data Streams (`:`), and null/control characters.
    pub fn resolve_in_jail(&self, candidate: &str) -> Result<String, PathJailError> {
        let trimmed = candidate.trim();
        if trimmed.is_empty() {
            return Err(PathJailError::EmptyPath);
        }
        if trimmed.bytes().any(|b| b == 0) {
            return Err(PathJailError::NullByteDetected);
        }
        if trimmed.chars().any(|c| c.is_control()) {
            return Err(PathJailError::ControlCharacterDetected);
        }

        let normalized = trimmed.replace('\\', "/");
        if normalized.starts_with("//") {
            return Err(PathJailError::UncOrDevicePrefixBlocked(trimmed.to_string()));
        }

        let (cand_drive, raw_path) = Self::split_drive_prefix(&normalized)?;
        let is_abs = cand_drive.is_some() || raw_path.starts_with('/');

        let mut stack: Vec<String> = if is_abs {
            if let (Some(ref r_drive), Some(ref c_drive)) = (&self.root_drive, &cand_drive) {
                if !r_drive.eq_ignore_ascii_case(c_drive) {
                    return Err(PathJailError::DriveLetterMismatch {
                        candidate_drive: c_drive.clone(),
                        root_drive: r_drive.clone(),
                    });
                }
            } else if self.root_drive.is_some() && cand_drive.is_none() {
                // Rooted path like `/Windows/System32` without drive letter when jail has drive
                return Err(PathJailError::TraversalEscapeOutsideRoot {
                    candidate: trimmed.to_string(),
                    jail_root: self.canonical_root_display.clone(),
                });
            }
            Vec::new()
        } else {
            self.root_segments.clone()
        };

        for part in raw_path.split('/') {
            if part.is_empty() || part == "." {
                continue;
            }
            if part == ".." {
                if stack.pop().is_none() {
                    return Err(PathJailError::TraversalEscapeOutsideRoot {
                        candidate: trimmed.to_string(),
                        jail_root: self.canonical_root_display.clone(),
                    });
                }
                if stack.len() < self.root_segments.len()
                    || !Self::prefix_matches(&self.root_segments, &stack)
                {
                    return Err(PathJailError::TraversalEscapeOutsideRoot {
                        candidate: trimmed.to_string(),
                        jail_root: self.canonical_root_display.clone(),
                    });
                }
                continue;
            }

            Self::validate_segment(part)?;
            stack.push(part.to_string());
        }

        if stack.len() < self.root_segments.len()
            || !Self::prefix_matches(&self.root_segments, &stack)
        {
            return Err(PathJailError::TraversalEscapeOutsideRoot {
                candidate: trimmed.to_string(),
                jail_root: self.canonical_root_display.clone(),
            });
        }

        Ok(Self::format_canonical(&self.root_drive, &stack))
    }

    fn parse_clean_absolute(path: &str) -> Result<(Option<String>, Vec<String>), PathJailError> {
        let trimmed = path.trim();
        if trimmed.is_empty() {
            return Err(PathJailError::EmptyPath);
        }
        if trimmed.bytes().any(|b| b == 0) {
            return Err(PathJailError::NullByteDetected);
        }
        let normalized = trimmed.replace('\\', "/");
        if normalized.starts_with("//") {
            return Err(PathJailError::UncOrDevicePrefixBlocked(trimmed.to_string()));
        }
        let (drive, rest) = Self::split_drive_prefix(&normalized)?;
        let mut segments = Vec::new();
        for part in rest.split('/') {
            if part.is_empty() || part == "." {
                continue;
            }
            if part == ".." {
                if segments.pop().is_none() {
                    return Err(PathJailError::TraversalEscapeOutsideRoot {
                        candidate: trimmed.to_string(),
                        jail_root: trimmed.to_string(),
                    });
                }
                continue;
            }
            Self::validate_segment(part)?;
            segments.push(part.to_string());
        }
        Ok((drive, segments))
    }

    fn split_drive_prefix(normalized: &str) -> Result<(Option<String>, &str), PathJailError> {
        let bytes = normalized.as_bytes();
        if bytes.len() >= 2 && bytes[0].is_ascii_alphabetic() && bytes[1] == b':' {
            let drive = normalized[..2].to_ascii_uppercase();
            let rest = &normalized[2..];
            if rest.contains(':') {
                return Err(PathJailError::UncOrDevicePrefixBlocked(
                    normalized.to_string(),
                ));
            }
            Ok((Some(drive), rest))
        } else {
            if normalized.contains(':') {
                return Err(PathJailError::UncOrDevicePrefixBlocked(
                    normalized.to_string(),
                ));
            }
            Ok((None, normalized))
        }
    }

    fn validate_segment(segment: &str) -> Result<(), PathJailError> {
        let trimmed_dots = segment.trim_end_matches(|c| c == '.' || c == ' ');
        if trimmed_dots.is_empty() {
            return Err(PathJailError::UncOrDevicePrefixBlocked(segment.to_string()));
        }
        let stem = trimmed_dots
            .split('.')
            .next()
            .unwrap_or(trimmed_dots)
            .to_ascii_uppercase();
        if is_reserved_windows_device(&stem) {
            return Err(PathJailError::ReservedWindowsDeviceName(segment.to_string()));
        }
        Ok(())
    }

    fn prefix_matches(root: &[String], candidate: &[String]) -> bool {
        if candidate.len() < root.len() {
            return false;
        }
        root.iter()
            .zip(candidate.iter())
            .all(|(r, c)| r.eq_ignore_ascii_case(c))
    }

    fn format_canonical(drive: &Option<String>, segments: &[String]) -> String {
        let joined = segments.join("/");
        match drive {
            Some(d) => format!("{}/{}", d, joined),
            None => format!("/{}", joined),
        }
    }
}

// ============================================================================
// 3. GATEKEEPER POLICY ENGINE (DENY-BY-DEFAULT & DENY > ASK > ALLOW)
// ============================================================================

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum CapabilityScope {
    FsRead,
    FsWrite,
    ShellExec,
    InputControl,
    ScreenCapture,
    NetworkOut,
    McpToolCall,
}

impl CapabilityScope {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::FsRead => "fs.read",
            Self::FsWrite => "fs.write",
            Self::ShellExec => "shell.exec",
            Self::InputControl => "input.control",
            Self::ScreenCapture => "screen.capture",
            Self::NetworkOut => "network.out",
            Self::McpToolCall => "mcp.tool_call",
        }
    }

    pub fn requires_path_jail(&self) -> bool {
        matches!(self, Self::FsRead | Self::FsWrite)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum RuleEffect {
    Allow,
    Ask,
    Deny,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PolicyRule {
    pub id: String,
    pub capability: CapabilityScope,
    pub target_pattern: String,
    pub effect: RuleEffect,
}

impl PolicyRule {
    pub fn matches(&self, capability: CapabilityScope, canonical_target: &str) -> bool {
        if self.capability != capability {
            return false;
        }
        wildcard_match(&self.target_pattern, canonical_target)
    }
}

fn wildcard_match(pattern: &str, text: &str) -> bool {
    let pat = pattern.replace('\\', "/").to_ascii_lowercase();
    let txt = text.replace('\\', "/").to_ascii_lowercase();
    if pat == "*" {
        return true;
    }
    if let Some(prefix) = pat.strip_suffix('*') {
        return txt.starts_with(prefix);
    }
    if let Some(suffix) = pat.strip_prefix('*') {
        return txt.ends_with(suffix);
    }
    pat == txt
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum DecisionVerdict {
    Allow {
        matched_rule_id: String,
        canonical_target: String,
    },
    Ask {
        matched_rule_id: String,
        ticket: PermissionTicket,
    },
    Deny {
        reason: DenyReason,
    },
}

impl DecisionVerdict {
    pub fn short_code(&self) -> &'static str {
        match self {
            Self::Allow { .. } => "ALLOW",
            Self::Ask { .. } => "ASK",
            Self::Deny { .. } => "DENY",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum DenyReason {
    GlobalKillSwitchActive,
    UnlistedCapabilityDefaultDeny,
    ExplicitDenyRule { rule_id: String },
    PathJailViolation(PathJailError),
    PermissionTicketNotFound,
    PermissionRequestHashMismatch,
    PermissionTicketExpired { expired_at_ms: u64, now_ms: u64 },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ActionRequest {
    pub session_id: String,
    pub agent_id: String,
    pub capability: CapabilityScope,
    pub target: String,
    pub canonical_args_json: String,
    pub timestamp_ms: u64,
    pub grant_token: Option<GrantToken>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PermissionTicket {
    pub ticket_id: String,
    pub request_hash: String,
    pub issued_at_ms: u64,
    pub expires_at_ms: u64,
    pub canonical_target: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct GrantToken {
    pub ticket_id: String,
    pub approved_request_hash: String,
}

pub fn compute_request_hash(
    session_id: &str,
    agent_id: &str,
    capability: CapabilityScope,
    canonical_target: &str,
    canonical_args_json: &str,
) -> String {
    let payload = format!(
        "v1|{}|{}|{}|{}|{}",
        session_id,
        agent_id,
        capability.as_str(),
        canonical_target,
        canonical_args_json
    );
    sha256_hex(payload.as_bytes())
}

// ============================================================================
// 4. TAMPER-EVIDENT SHA-256 HASH CHAIN AUDIT LEDGER
// ============================================================================

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AuditLogEntry {
    pub seq: u64,
    pub timestamp_ms: u64,
    pub session_id: String,
    pub agent_id: String,
    pub capability: String,
    pub target: String,
    pub verdict: String,
    pub detail: String,
    pub request_hash: String,
    pub prev_hash: String,
    pub entry_hash: String,
}

impl AuditLogEntry {
    pub fn compute_hash(
        seq: u64,
        timestamp_ms: u64,
        session_id: &str,
        agent_id: &str,
        capability: &str,
        target: &str,
        verdict: &str,
        detail: &str,
        request_hash: &str,
        prev_hash: &str,
    ) -> String {
        let canonical = format!(
            "{}|{}|{}|{}|{}|{}|{}|{}|{}|{}",
            prev_hash,
            seq,
            timestamp_ms,
            session_id,
            agent_id,
            capability,
            target,
            verdict,
            detail,
            request_hash
        );
        sha256_hex(canonical.as_bytes())
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum AuditChainError {
    SequenceDiscontinuity {
        index: usize,
        expected_seq: u64,
        found_seq: u64,
    },
    PrevHashMismatch {
        index: usize,
        expected_prev_hash: String,
        found_prev_hash: String,
    },
    EntryHashTampered {
        index: usize,
        seq: u64,
        expected_hash: String,
        computed_hash: String,
    },
    JsonParseError {
        line_index: usize,
        message: String,
    },
}

#[derive(Debug, Clone, Default)]
pub struct TamperEvidentAuditLedger {
    entries: Vec<AuditLogEntry>,
    last_hash: String,
}

impl TamperEvidentAuditLedger {
    pub fn new() -> Self {
        Self {
            entries: Vec::new(),
            last_hash: GENESIS_PREV_HASH.to_string(),
        }
    }

    pub fn append(
        &mut self,
        timestamp_ms: u64,
        session_id: &str,
        agent_id: &str,
        capability: CapabilityScope,
        target: &str,
        verdict: &str,
        detail: &str,
        request_hash: &str,
    ) -> AuditLogEntry {
        let seq = self.entries.len() as u64;
        let prev_hash = self.last_hash.clone();
        let cap_str = capability.as_str();
        let entry_hash = AuditLogEntry::compute_hash(
            seq,
            timestamp_ms,
            session_id,
            agent_id,
            cap_str,
            target,
            verdict,
            detail,
            request_hash,
            &prev_hash,
        );
        let entry = AuditLogEntry {
            seq,
            timestamp_ms,
            session_id: session_id.to_string(),
            agent_id: agent_id.to_string(),
            capability: cap_str.to_string(),
            target: target.to_string(),
            verdict: verdict.to_string(),
            detail: detail.to_string(),
            request_hash: request_hash.to_string(),
            prev_hash,
            entry_hash: entry_hash.clone(),
        };
        self.last_hash = entry_hash;
        self.entries.push(entry.clone());
        entry
    }

    pub fn entries(&self) -> &[AuditLogEntry] {
        &self.entries
    }

    pub fn head_hash(&self) -> &str {
        &self.last_hash
    }

    pub fn to_jsonl(&self) -> String {
        let mut out = String::new();
        for entry in &self.entries {
            if let Ok(line) = serde_json::to_string(entry) {
                out.push_str(&line);
                out.push('\n');
            }
        }
        out
    }

    pub fn verify_slice(entries: &[AuditLogEntry]) -> Result<(), AuditChainError> {
        let mut expected_prev = GENESIS_PREV_HASH.to_string();
        for (idx, entry) in entries.iter().enumerate() {
            let expected_seq = idx as u64;
            if entry.seq != expected_seq {
                return Err(AuditChainError::SequenceDiscontinuity {
                    index: idx,
                    expected_seq,
                    found_seq: entry.seq,
                });
            }
            if entry.prev_hash != expected_prev {
                return Err(AuditChainError::PrevHashMismatch {
                    index: idx,
                    expected_prev_hash: expected_prev,
                    found_prev_hash: entry.prev_hash.clone(),
                });
            }
            let recomputed = AuditLogEntry::compute_hash(
                entry.seq,
                entry.timestamp_ms,
                &entry.session_id,
                &entry.agent_id,
                &entry.capability,
                &entry.target,
                &entry.verdict,
                &entry.detail,
                &entry.request_hash,
                &entry.prev_hash,
            );
            if recomputed != entry.entry_hash {
                return Err(AuditChainError::EntryHashTampered {
                    index: idx,
                    seq: entry.seq,
                    expected_hash: entry.entry_hash.clone(),
                    computed_hash: recomputed,
                });
            }
            expected_prev = entry.entry_hash.clone();
        }
        Ok(())
    }

    pub fn verify_jsonl(jsonl: &str) -> Result<usize, AuditChainError> {
        let mut parsed = Vec::new();
        for (idx, line) in jsonl.lines().enumerate() {
            let trimmed = line.trim();
            if trimmed.is_empty() {
                continue;
            }
            let entry: AuditLogEntry =
                serde_json::from_str(trimmed).map_err(|e| AuditChainError::JsonParseError {
                    line_index: idx,
                    message: e.to_string(),
                })?;
            parsed.push(entry);
        }
        Self::verify_slice(&parsed)?;
        Ok(parsed.len())
    }
}

// ============================================================================
// 5. LOCAL GATEKEEPER ENGINE + GLOBAL KILL SWITCH + IPC GATEWAY
// ============================================================================

pub struct LocalGatekeeper {
    jail: CanonicalPathJail,
    rules: Vec<PolicyRule>,
    default_ticket_ttl_ms: u64,
    pending_tickets: HashMap<String, PermissionTicket>,
    next_ticket_counter: u64,
    kill_switch: Arc<AtomicBool>,
    audit_ledger: TamperEvidentAuditLedger,
}

impl LocalGatekeeper {
    pub fn new(
        jail: CanonicalPathJail,
        rules: Vec<PolicyRule>,
        default_ticket_ttl_ms: u64,
    ) -> Self {
        Self {
            jail,
            rules,
            default_ticket_ttl_ms,
            pending_tickets: HashMap::new(),
            next_ticket_counter: 1,
            kill_switch: Arc::new(AtomicBool::new(false)),
            audit_ledger: TamperEvidentAuditLedger::new(),
        }
    }

    pub fn kill_switch_handle(&self) -> Arc<AtomicBool> {
        Arc::clone(&self.kill_switch)
    }

    pub fn trigger_kill_switch(&mut self, timestamp_ms: u64) {
        self.kill_switch.store(true, Ordering::SeqCst);
        self.pending_tickets.clear();
        let _ = self.audit_ledger.append(
            timestamp_ms,
            "system",
            "kill-switch",
            CapabilityScope::ShellExec,
            "*",
            "KILL_SWITCH_ENGAGED",
            "All active permission tickets revoked immediately",
            GENESIS_PREV_HASH,
        );
    }

    pub fn reset_kill_switch(&mut self, timestamp_ms: u64) {
        self.kill_switch.store(false, Ordering::SeqCst);
        let _ = self.audit_ledger.append(
            timestamp_ms,
            "system",
            "kill-switch",
            CapabilityScope::ShellExec,
            "*",
            "KILL_SWITCH_RESET",
            "Kill switch disarmed by explicit operator action",
            GENESIS_PREV_HASH,
        );
    }

    pub fn is_kill_switch_active(&self) -> bool {
        self.kill_switch.load(Ordering::SeqCst)
    }

    pub fn audit_ledger(&self) -> &TamperEvidentAuditLedger {
        &self.audit_ledger
    }

    pub fn evaluate(&mut self, req: &ActionRequest) -> (DecisionVerdict, AuditLogEntry) {
        // 1. Global Kill Switch check (highest priority)
        if self.kill_switch.load(Ordering::SeqCst) {
            let req_hash = compute_request_hash(
                &req.session_id,
                &req.agent_id,
                req.capability,
                &req.target,
                &req.canonical_args_json,
            );
            let verdict = DecisionVerdict::Deny {
                reason: DenyReason::GlobalKillSwitchActive,
            };
            let entry = self.audit_ledger.append(
                req.timestamp_ms,
                &req.session_id,
                &req.agent_id,
                req.capability,
                &req.target,
                verdict.short_code(),
                "GlobalKillSwitchActive",
                &req_hash,
            );
            return (verdict, entry);
        }

        // 2. Canonical Path Jail enforcement for filesystem capabilities
        let canonical_target = if req.capability.requires_path_jail() {
            match self.jail.resolve_in_jail(&req.target) {
                Ok(clean) => clean,
                Err(jail_err) => {
                    let req_hash = compute_request_hash(
                        &req.session_id,
                        &req.agent_id,
                        req.capability,
                        &req.target,
                        &req.canonical_args_json,
                    );
                    let detail = format!("PathJailViolation({:?})", jail_err);
                    let verdict = DecisionVerdict::Deny {
                        reason: DenyReason::PathJailViolation(jail_err),
                    };
                    let entry = self.audit_ledger.append(
                        req.timestamp_ms,
                        &req.session_id,
                        &req.agent_id,
                        req.capability,
                        &req.target,
                        verdict.short_code(),
                        &detail,
                        &req_hash,
                    );
                    return (verdict, entry);
                }
            }
        } else {
            req.target.trim().to_string()
        };

        let request_hash = compute_request_hash(
            &req.session_id,
            &req.agent_id,
            req.capability,
            &canonical_target,
            &req.canonical_args_json,
        );

        // 3. Collect all matching rules and enforce strict precedence: Deny > Ask > Allow
        let matching: Vec<&PolicyRule> = self
            .rules
            .iter()
            .filter(|r| r.matches(req.capability, &canonical_target))
            .collect();

        if matching.is_empty() {
            let verdict = DecisionVerdict::Deny {
                reason: DenyReason::UnlistedCapabilityDefaultDeny,
            };
            let entry = self.audit_ledger.append(
                req.timestamp_ms,
                &req.session_id,
                &req.agent_id,
                req.capability,
                &canonical_target,
                verdict.short_code(),
                "UnlistedCapabilityDefaultDeny",
                &request_hash,
            );
            return (verdict, entry);
        }

        if let Some(deny_rule) = matching.iter().find(|r| r.effect == RuleEffect::Deny) {
            let detail = format!("ExplicitDenyRule({})", deny_rule.id);
            let verdict = DecisionVerdict::Deny {
                reason: DenyReason::ExplicitDenyRule {
                    rule_id: deny_rule.id.clone(),
                },
            };
            let entry = self.audit_ledger.append(
                req.timestamp_ms,
                &req.session_id,
                &req.agent_id,
                req.capability,
                &canonical_target,
                verdict.short_code(),
                &detail,
                &request_hash,
            );
            return (verdict, entry);
        }

        if let Some(ask_rule) = matching.iter().find(|r| r.effect == RuleEffect::Ask) {
            // Check if the caller supplied an interactive GrantToken
            if let Some(ref grant) = req.grant_token {
                let Some(ticket) = self.pending_tickets.get(&grant.ticket_id).cloned() else {
                    let verdict = DecisionVerdict::Deny {
                        reason: DenyReason::PermissionTicketNotFound,
                    };
                    let entry = self.audit_ledger.append(
                        req.timestamp_ms,
                        &req.session_id,
                        &req.agent_id,
                        req.capability,
                        &canonical_target,
                        verdict.short_code(),
                        "PermissionTicketNotFound",
                        &request_hash,
                    );
                    return (verdict, entry);
                };

                // Verify cryptographic anti-TOCTOU binding in constant time
                if !constant_time_eq(
                    ticket.request_hash.as_bytes(),
                    request_hash.as_bytes(),
                ) || !constant_time_eq(
                    grant.approved_request_hash.as_bytes(),
                    request_hash.as_bytes(),
                ) {
                    let verdict = DecisionVerdict::Deny {
                        reason: DenyReason::PermissionRequestHashMismatch,
                    };
                    let entry = self.audit_ledger.append(
                        req.timestamp_ms,
                        &req.session_id,
                        &req.agent_id,
                        req.capability,
                        &canonical_target,
                        verdict.short_code(),
                        "PermissionRequestHashMismatch(TOCTOU_BLOCKED)",
                        &request_hash,
                    );
                    return (verdict, entry);
                }

                // Verify TTL expiry
                if req.timestamp_ms > ticket.expires_at_ms {
                    self.pending_tickets.remove(&grant.ticket_id);
                    let verdict = DecisionVerdict::Deny {
                        reason: DenyReason::PermissionTicketExpired {
                            expired_at_ms: ticket.expires_at_ms,
                            now_ms: req.timestamp_ms,
                        },
                    };
                    let entry = self.audit_ledger.append(
                        req.timestamp_ms,
                        &req.session_id,
                        &req.agent_id,
                        req.capability,
                        &canonical_target,
                        verdict.short_code(),
                        "PermissionTicketExpired",
                        &request_hash,
                    );
                    return (verdict, entry);
                }

                // One-time ticket consumption succeeds
                self.pending_tickets.remove(&grant.ticket_id);
                let detail = format!("GrantedViaTicket({})", grant.ticket_id);
                let verdict = DecisionVerdict::Allow {
                    matched_rule_id: ask_rule.id.clone(),
                    canonical_target: canonical_target.clone(),
                };
                let entry = self.audit_ledger.append(
                    req.timestamp_ms,
                    &req.session_id,
                    &req.agent_id,
                    req.capability,
                    &canonical_target,
                    verdict.short_code(),
                    &detail,
                    &request_hash,
                );
                return (verdict, entry);
            }

            // Issue new PermissionTicket bound to `request_hash`
            let ticket_id = format!("tkt-{:06}", self.next_ticket_counter);
            self.next_ticket_counter += 1;
            let ticket = PermissionTicket {
                ticket_id: ticket_id.clone(),
                request_hash: request_hash.clone(),
                issued_at_ms: req.timestamp_ms,
                expires_at_ms: req.timestamp_ms.saturating_add(self.default_ticket_ttl_ms),
                canonical_target: canonical_target.clone(),
            };
            self.pending_tickets.insert(ticket_id.clone(), ticket.clone());
            let detail = format!("AskTicketIssued({})", ticket_id);
            let verdict = DecisionVerdict::Ask {
                matched_rule_id: ask_rule.id.clone(),
                ticket,
            };
            let entry = self.audit_ledger.append(
                req.timestamp_ms,
                &req.session_id,
                &req.agent_id,
                req.capability,
                &canonical_target,
                verdict.short_code(),
                &detail,
                &request_hash,
            );
            return (verdict, entry);
        }

        // Otherwise at least one Allow rule matched and no Deny/Ask rules matched
        let allow_rule = matching[0];
        let detail = format!("AllowRule({})", allow_rule.id);
        let verdict = DecisionVerdict::Allow {
            matched_rule_id: allow_rule.id.clone(),
            canonical_target: canonical_target.clone(),
        };
        let entry = self.audit_ledger.append(
            req.timestamp_ms,
            &req.session_id,
            &req.agent_id,
            req.capability,
            &canonical_target,
            verdict.short_code(),
            &detail,
            &request_hash,
        );
        (verdict, entry)
    }
}

// ============================================================================
// 6. GATEWAY IPC ENVELOPE & CONSTANT-TIME AUTHENTICATED BRIDGE
// ============================================================================

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct IpcRequestEnvelope {
    pub v: u8,
    pub msg_id: String,
    pub auth_token: String,
    pub request: ActionRequest,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct IpcResponseEnvelope {
    pub v: u8,
    pub reply_to: String,
    pub verdict: Option<DecisionVerdict>,
    pub audit_seq: Option<u64>,
    pub audit_entry_hash: Option<String>,
    pub ipc_error: Option<String>,
}

pub struct GatewayIpcServer {
    expected_token: String,
    gatekeeper: LocalGatekeeper,
}

impl GatewayIpcServer {
    pub fn new(expected_token: String, gatekeeper: LocalGatekeeper) -> Self {
        Self {
            expected_token,
            gatekeeper,
        }
    }

    pub fn gatekeeper_mut(&mut self) -> &mut LocalGatekeeper {
        &mut self.gatekeeper
    }

    pub fn gatekeeper(&self) -> &LocalGatekeeper {
        &self.gatekeeper
    }

    pub fn handle_raw_json_frame(&mut self, raw_json: &str) -> String {
        if raw_json.len() > MAX_IPC_FRAME_BYTES {
            let resp = IpcResponseEnvelope {
                v: 1,
                reply_to: "unknown".to_string(),
                verdict: None,
                audit_seq: None,
                audit_entry_hash: None,
                ipc_error: Some("ERR_FRAME_TOO_LARGE".to_string()),
            };
            return serde_json::to_string(&resp).unwrap_or_default();
        }

        let env: IpcRequestEnvelope = match serde_json::from_str(raw_json) {
            Ok(v) => v,
            Err(e) => {
                let resp = IpcResponseEnvelope {
                    v: 1,
                    reply_to: "unknown".to_string(),
                    verdict: None,
                    audit_seq: None,
                    audit_entry_hash: None,
                    ipc_error: Some(format!("ERR_INVALID_JSON: {}", e)),
                };
                return serde_json::to_string(&resp).unwrap_or_default();
            }
        };

        if env.v != 1 {
            let resp = IpcResponseEnvelope {
                v: 1,
                reply_to: env.msg_id,
                verdict: None,
                audit_seq: None,
                audit_entry_hash: None,
                ipc_error: Some(format!("ERR_UNSUPPORTED_VERSION: {}", env.v)),
            };
            return serde_json::to_string(&resp).unwrap_or_default();
        }

        if !constant_time_eq(env.auth_token.as_bytes(), self.expected_token.as_bytes()) {
            let resp = IpcResponseEnvelope {
                v: 1,
                reply_to: env.msg_id,
                verdict: None,
                audit_seq: None,
                audit_entry_hash: None,
                ipc_error: Some("ERR_UNAUTHORIZED_TOKEN".to_string()),
            };
            return serde_json::to_string(&resp).unwrap_or_default();
        }

        let (verdict, audit_entry) = self.gatekeeper.evaluate(&env.request);
        let resp = IpcResponseEnvelope {
            v: 1,
            reply_to: env.msg_id,
            verdict: Some(verdict),
            audit_seq: Some(audit_entry.seq),
            audit_entry_hash: Some(audit_entry.entry_hash),
            ipc_error: None,
        };
        serde_json::to_string(&resp).unwrap_or_default()
    }
}

// ============================================================================
// 7. PROCESS TREE MEMORY & ORPHAN AUDITOR (COMPOSITE CREATION_DATE IDENTITY)
// ============================================================================

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct ProcessIdentity {
    pub pid: u32,
    pub name_lower: String,
    pub creation_date: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProcessRow {
    pub pid: u32,
    pub parent_pid: u32,
    pub private_bytes: u64,
    pub name: String,
    pub creation_date: String,
}

pub fn parse_process_snapshot_tsv(tsv: &str) -> Vec<ProcessRow> {
    let mut rows = Vec::new();
    for line in tsv.lines() {
        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }
        let parts: Vec<&str> = trimmed.split('\t').collect();
        if parts.len() < 5 {
            continue;
        }
        let Ok(pid) = parts[0].trim().parse::<u32>() else {
            continue;
        };
        let Ok(parent_pid) = parts[1].trim().parse::<u32>() else {
            continue;
        };
        let Ok(private_bytes) = parts[2].trim().parse::<u64>() else {
            continue;
        };
        let name = parts[3].trim().to_string();
        let creation_date = parts[4].trim().to_string();
        rows.push(ProcessRow {
            pid,
            parent_pid,
            private_bytes,
            name,
            creation_date,
        });
    }
    rows
}

pub fn aggregate_tree_private_mb(
    rows: &[ProcessRow],
    root_pid: u32,
) -> (f64, Vec<ProcessIdentity>) {
    let mut included_pids = Vec::new();
    let mut frontier = vec![root_pid];
    while let Some(curr) = frontier.pop() {
        if !included_pids.contains(&curr) {
            included_pids.push(curr);
            for r in rows {
                if r.parent_pid == curr && !included_pids.contains(&r.pid) {
                    frontier.push(r.pid);
                }
            }
        }
    }

    let mut total_bytes = 0u64;
    let mut identities = Vec::new();
    for r in rows {
        if included_pids.contains(&r.pid) {
            total_bytes = total_bytes.saturating_add(r.private_bytes);
            identities.push(ProcessIdentity {
                pid: r.pid,
                name_lower: r.name.to_ascii_lowercase(),
                creation_date: r.creation_date.clone(),
            });
        }
    }
    let mb = (total_bytes as f64) / (1024.0 * 1024.0);
    (mb, identities)
}

pub fn detect_surviving_orphans(
    spawned_tree: &[ProcessIdentity],
    post_kill_snapshot: &[ProcessRow],
) -> Vec<ProcessIdentity> {
    let mut orphans = Vec::new();
    for id in spawned_tree {
        let still_alive = post_kill_snapshot.iter().any(|r| {
            r.pid == id.pid
                && r.name.to_ascii_lowercase() == id.name_lower
                && r.creation_date == id.creation_date
        });
        if still_alive {
            orphans.push(id.clone());
        }
    }
    orphans
}

pub fn percentile(sorted: &[f64], pct: f64) -> f64 {
    if sorted.is_empty() {
        return 0.0;
    }
    let idx = ((sorted.len() as f64 - 1.0) * (pct / 100.0)).round() as usize;
    sorted[idx.min(sorted.len() - 1)]
}

// ============================================================================
// 8. TDD UNIT, PROPERTY & CONTRACT TESTS (LEDGER-0.6-1 .. LEDGER-0.6-4)
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    /// LEDGER-0.6-1: Deny-by-default and invariant precedence Deny > Ask > Allow
    #[test]
    fn test_deny_by_default_and_precedence_deny_over_ask_and_allow() {
        let jail = CanonicalPathJail::new("C:/Users/dev/workspace/miku").unwrap();
        let rules = vec![
            // Broad allow rule declared FIRST
            PolicyRule {
                id: "allow-workspace-read".to_string(),
                capability: CapabilityScope::FsRead,
                target_pattern: "C:/Users/dev/workspace/miku/*".to_string(),
                effect: RuleEffect::Allow,
            },
            // Ask rule on secrets folder declared SECOND
            PolicyRule {
                id: "ask-secrets-folder".to_string(),
                capability: CapabilityScope::FsRead,
                target_pattern: "C:/Users/dev/workspace/miku/secrets/*".to_string(),
                effect: RuleEffect::Ask,
            },
            // Specific deny rule on master key declared LAST (must still beat Allow and Ask)
            PolicyRule {
                id: "deny-master-key".to_string(),
                capability: CapabilityScope::FsRead,
                target_pattern: "C:/Users/dev/workspace/miku/secrets/master.key".to_string(),
                effect: RuleEffect::Deny,
            },
        ];

        let mut gk = LocalGatekeeper::new(jail, rules, 30_000);

        // 1. Unlisted capability (NetworkOut) -> Deny by default
        let unlisted_req = ActionRequest {
            session_id: "sess-01".to_string(),
            agent_id: "cursor-mcp".to_string(),
            capability: CapabilityScope::NetworkOut,
            target: "https://evil.example.com".to_string(),
            canonical_args_json: "{}".to_string(),
            timestamp_ms: 1_000,
            grant_token: None,
        };
        let (verdict_unlisted, _) = gk.evaluate(&unlisted_req);
        assert_eq!(
            verdict_unlisted,
            DecisionVerdict::Deny {
                reason: DenyReason::UnlistedCapabilityDefaultDeny
            }
        );

        // 2. Normal file inside workspace -> Allow
        let read_readme = ActionRequest {
            session_id: "sess-01".to_string(),
            agent_id: "cursor-mcp".to_string(),
            capability: CapabilityScope::FsRead,
            target: "src/main.rs".to_string(),
            canonical_args_json: "{}".to_string(),
            timestamp_ms: 1_010,
            grant_token: None,
        };
        let (verdict_allow, _) = gk.evaluate(&read_readme);
        assert_eq!(
            verdict_allow,
            DecisionVerdict::Allow {
                matched_rule_id: "allow-workspace-read".to_string(),
                canonical_target: "C:/Users/dev/workspace/miku/src/main.rs".to_string(),
            }
        );

        // 3. File matching both Allow and Ask -> Ask wins over Allow
        let read_secret_cfg = ActionRequest {
            session_id: "sess-01".to_string(),
            agent_id: "cursor-mcp".to_string(),
            capability: CapabilityScope::FsRead,
            target: "secrets/staging.env".to_string(),
            canonical_args_json: "{}".to_string(),
            timestamp_ms: 1_020,
            grant_token: None,
        };
        let (verdict_ask, _) = gk.evaluate(&read_secret_cfg);
        assert!(matches!(
            verdict_ask,
            DecisionVerdict::Ask {
                ref matched_rule_id,
                ..
            } if matched_rule_id == "ask-secrets-folder"
        ));

        // 4. File matching Allow, Ask, AND Deny -> Deny wins unconditionally
        let read_master_key = ActionRequest {
            session_id: "sess-01".to_string(),
            agent_id: "cursor-mcp".to_string(),
            capability: CapabilityScope::FsRead,
            target: "secrets/master.key".to_string(),
            canonical_args_json: "{}".to_string(),
            timestamp_ms: 1_030,
            grant_token: None,
        };
        let (verdict_deny, _) = gk.evaluate(&read_master_key);
        assert_eq!(
            verdict_deny,
            DecisionVerdict::Deny {
                reason: DenyReason::ExplicitDenyRule {
                    rule_id: "deny-master-key".to_string()
                }
            }
        );
    }

    /// LEDGER-0.6-2: Canonical path jail neutralizes traversal, UNC, DOS devices, and null bytes
    #[test]
    fn test_canonical_path_jail_blocks_traversal_unc_and_symlink_escapes() {
        let jail = CanonicalPathJail::new("C:\\Users\\cmiranda\\Projects\\miku").unwrap();
        assert_eq!(jail.canonical_root(), "C:/Users/cmiranda/Projects/miku");

        // Valid relative and non-existent nested paths inside jail succeed
        let ok_rel = jail
            .resolve_in_jail("crates\\miku-core\\..\\miku-protocol\\src\\lib.rs")
            .unwrap();
        assert_eq!(
            ok_rel,
            "C:/Users/cmiranda/Projects/miku/crates/miku-protocol/src/lib.rs"
        );

        let ok_abs = jail
            .resolve_in_jail("c:\\Users\\cmiranda\\Projects\\miku\\docs\\adr-001.md")
            .unwrap();
        assert_eq!(
            ok_abs,
            "C:/Users/cmiranda/Projects/miku/docs/adr-001.md"
        );

        // Hostile vectors that must be rejected deterministically
        let hostile_vectors = [
            "..\\..\\Windows\\System32\\cmd.exe",
            "crates/../../../etc/passwd",
            "C:\\Windows\\System32\\drivers\\etc\\hosts",
            "D:\\Users\\cmiranda\\Projects\\miku\\file.txt",
            "\\\\?\\UNC\\evil.attacker.local\\share\\payload.dll",
            "\\\\192.168.1.99\\c$\\boot.ini",
            "\\\\.\\PhysicalDrive0",
            "src/NUL",
            "src/con.txt",
            "logs/COM1.log",
            "logs/lpt9",
            "src/file.txt:hidden_ads",
            "src/normal\0evil.rs",
        ];

        for vector in hostile_vectors {
            let res = jail.resolve_in_jail(vector);
            assert!(
                res.is_err(),
                "Hostile path vector '{}' should have been blocked, but resolved to {:?}",
                vector,
                res
            );
        }
    }

    /// LEDGER-0.6-3: Tamper-evident SHA-256 hash chain detects 1-bit edits, deletions, or reordering
    #[test]
    fn test_tamper_evident_sha256_hash_chain_detects_any_modified_or_deleted_log_entry() {
        let mut ledger = TamperEvidentAuditLedger::new();
        for i in 0..6 {
            let req_hash = sha256_hex(format!("req-{}", i).as_bytes());
            ledger.append(
                10_000 + i * 50,
                "sess-audit",
                "claude-code",
                CapabilityScope::FsWrite,
                &format!("C:/workspace/file_{}.rs", i),
                "ALLOW",
                "AllowRule(workspace-write)",
                &req_hash,
            );
        }

        let jsonl = ledger.to_jsonl();
        let verified_count = TamperEvidentAuditLedger::verify_jsonl(&jsonl).unwrap();
        assert_eq!(verified_count, 6);

        // 1. Modify 1 character in entry #3's target without recomputing chain
        let mut tampered_bit = ledger.entries().to_vec();
        tampered_bit[3].target = "C:/workspace/file_3_TAMPERED.rs".to_string();
        assert!(matches!(
            TamperEvidentAuditLedger::verify_slice(&tampered_bit),
            Err(AuditChainError::EntryHashTampered { index: 3, seq: 3, .. })
        ));

        // 2. Delete middle entry #2 -> sequence discontinuity at index 2
        let mut tampered_deleted = ledger.entries().to_vec();
        tampered_deleted.remove(2);
        assert!(matches!(
            TamperEvidentAuditLedger::verify_slice(&tampered_deleted),
            Err(AuditChainError::SequenceDiscontinuity {
                index: 2,
                expected_seq: 2,
                found_seq: 3
            })
        ));

        // 3. Delete middle entry #2 AND renumber `seq` -> caught by PrevHashMismatch at index 2
        let mut tampered_renumbered = ledger.entries().to_vec();
        tampered_renumbered.remove(2);
        for (idx, item) in tampered_renumbered.iter_mut().enumerate() {
            item.seq = idx as u64;
        }
        assert!(matches!(
            TamperEvidentAuditLedger::verify_slice(&tampered_renumbered),
            Err(AuditChainError::PrevHashMismatch { index: 2, .. })
        ));
    }

    /// LEDGER-0.6-4: Interactive permission request_hash anti-TOCTOU binding, TTL expiry, and Kill Switch
    #[test]
    fn test_interactive_permission_request_hash_binding_ttl_expiry_and_kill_switch() {
        let jail = CanonicalPathJail::new("C:/workspace/miku").unwrap();
        let rules = vec![PolicyRule {
            id: "ask-shell".to_string(),
            capability: CapabilityScope::ShellExec,
            target_pattern: "git *".to_string(),
            effect: RuleEffect::Ask,
        }];
        let mut gk = LocalGatekeeper::new(jail, rules, 5_000);

        // Step A: Request `git status` -> receives Ask ticket
        let req_initial = ActionRequest {
            session_id: "sess-99".to_string(),
            agent_id: "claude-code".to_string(),
            capability: CapabilityScope::ShellExec,
            target: "git status".to_string(),
            canonical_args_json: r#"{"cwd":"C:/workspace/miku"}"#.to_string(),
            timestamp_ms: 20_000,
            grant_token: None,
        };
        let (verdict_ask, _) = gk.evaluate(&req_initial);
        let ticket = match verdict_ask {
            DecisionVerdict::Ask { ticket, .. } => ticket,
            other => panic!("Expected Ask ticket, got {:?}", other),
        };

        // Step B: TOCTOU Attack! Agent reuses the ticket ID and approved_request_hash,
        // but mutates `canonical_args_json` or `target` to `git push --force`.
        let req_toctou = ActionRequest {
            session_id: "sess-99".to_string(),
            agent_id: "claude-code".to_string(),
            capability: CapabilityScope::ShellExec,
            target: "git push --force".to_string(),
            canonical_args_json: r#"{"cwd":"C:/workspace/miku"}"#.to_string(),
            timestamp_ms: 21_000,
            grant_token: Some(GrantToken {
                ticket_id: ticket.ticket_id.clone(),
                approved_request_hash: ticket.request_hash.clone(),
            }),
        };
        let (verdict_toctou, _) = gk.evaluate(&req_toctou);
        assert_eq!(
            verdict_toctou,
            DecisionVerdict::Deny {
                reason: DenyReason::PermissionRequestHashMismatch
            }
        );

        // Step C: Legitimate approval within TTL (22_000 <= 25_000) succeeds once
        let req_legit = ActionRequest {
            grant_token: Some(GrantToken {
                ticket_id: ticket.ticket_id.clone(),
                approved_request_hash: ticket.request_hash.clone(),
            }),
            timestamp_ms: 22_000,
            ..req_initial.clone()
        };
        let (verdict_legit, _) = gk.evaluate(&req_legit);
        assert!(matches!(verdict_legit, DecisionVerdict::Allow { .. }));

        // Step D: Issue second ticket and let TTL expire (now = 36_000 > 35_000)
        let req_second = ActionRequest {
            timestamp_ms: 30_000,
            grant_token: None,
            ..req_initial.clone()
        };
        let (verdict_ask_2, _) = gk.evaluate(&req_second);
        let ticket_2 = match verdict_ask_2 {
            DecisionVerdict::Ask { ticket, .. } => ticket,
            other => panic!("Expected second Ask ticket, got {:?}", other),
        };
        let req_expired = ActionRequest {
            timestamp_ms: 36_000,
            grant_token: Some(GrantToken {
                ticket_id: ticket_2.ticket_id,
                approved_request_hash: ticket_2.request_hash,
            }),
            ..req_initial.clone()
        };
        let (verdict_expired, _) = gk.evaluate(&req_expired);
        assert!(matches!(
            verdict_expired,
            DecisionVerdict::Deny {
                reason: DenyReason::PermissionTicketExpired { .. }
            }
        ));

        // Step E: Trigger Global Kill Switch -> blocks everything immediately
        gk.trigger_kill_switch(40_000);
        assert!(gk.is_kill_switch_active());
        let (verdict_killed, _) = gk.evaluate(&req_initial);
        assert_eq!(
            verdict_killed,
            DecisionVerdict::Deny {
                reason: DenyReason::GlobalKillSwitchActive
            }
        );

        // Audit chain of the entire sequence must verify cleanly
        assert!(TamperEvidentAuditLedger::verify_slice(gk.audit_ledger().entries()).is_ok());
    }
}
