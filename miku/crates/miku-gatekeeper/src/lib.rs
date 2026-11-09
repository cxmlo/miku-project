#![allow(
    unknown_lints,
    clippy::too_many_arguments,
    clippy::manual_range_contains,
    clippy::needless_range_loop,
    clippy::large_enum_variant,
    clippy::chunks_exact_to_as_chunks,
    clippy::type_complexity,
    clippy::invisible_characters
)]

use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};

use miku_protocol::{
    constant_time_token_eq, sha256_hex, MikuLinkEnvelopeV1, MikuLinkMessageV1,
    PermissionDecisionKind, MIKU_PROTOCOL_VERSION_V1,
};
use serde::{Deserialize, Serialize};

pub const GENESIS_AUDIT_HASH: &str =
    "0000000000000000000000000000000000000000000000000000000000000000";

const WINDOWS_RESERVED_DOS_DEVICES: &[&str] = &[
    "CON", "PRN", "AUX", "NUL", "COM1", "COM2", "COM3", "COM4", "COM5", "COM6", "COM7", "COM8",
    "COM9", "LPT1", "LPT2", "LPT3", "LPT4", "LPT5", "LPT6", "LPT7", "LPT8", "LPT9",
];

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Capability {
    FsRead,
    FsWrite,
    Shell,
    Network,
    Browser,
    Vision,
    Input,
    Clipboard,
    UiAutomation,
}

impl Capability {
    pub fn as_str(&self) -> &'static str {
        match self {
            Capability::FsRead => "fs_read",
            Capability::FsWrite => "fs_write",
            Capability::Shell => "shell",
            Capability::Network => "network",
            Capability::Browser => "browser",
            Capability::Vision => "vision",
            Capability::Input => "input",
            Capability::Clipboard => "clipboard",
            Capability::UiAutomation => "ui_automation",
        }
    }

    pub fn parse(raw: &str) -> Option<Self> {
        match raw.trim().to_ascii_lowercase().as_str() {
            "fs_read" => Some(Capability::FsRead),
            "fs_write" => Some(Capability::FsWrite),
            "shell" => Some(Capability::Shell),
            "network" => Some(Capability::Network),
            "browser" => Some(Capability::Browser),
            "vision" => Some(Capability::Vision),
            "input" => Some(Capability::Input),
            "clipboard" => Some(Capability::Clipboard),
            "ui_automation" => Some(Capability::UiAutomation),
            _ => None,
        }
    }

    pub fn requires_path_jail(&self) -> bool {
        matches!(self, Capability::FsRead | Capability::FsWrite)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PolicyEffect {
    Allow,
    Ask,
    Deny,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PolicyRule {
    pub capability: Capability,
    pub pattern: String,
    pub effect: PolicyEffect,
    pub pc_only: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProjectProfile {
    pub project_name: String,
    pub project_root: String,
    pub default_ttl_ms: u64,
    pub rules: Vec<PolicyRule>,
}

impl ProjectProfile {
    pub fn new(project_name: &str, project_root: &str, default_ttl_ms: u64) -> Self {
        Self {
            project_name: project_name.to_string(),
            project_root: project_root.to_string(),
            default_ttl_ms,
            rules: Vec::new(),
        }
    }

    pub fn with_rule(
        mut self,
        capability: Capability,
        pattern: &str,
        effect: PolicyEffect,
        pc_only: bool,
    ) -> Self {
        self.rules.push(PolicyRule {
            capability,
            pattern: pattern.to_string(),
            effect,
            pc_only,
        });
        self
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum JailError {
    EmptyPath,
    NullOrControlByte,
    UncPathForbidden(String),
    AlternateDataStreamForbidden(String),
    DosReservedDeviceForbidden(String),
    PathTraversalEscape {
        requested: String,
        jail_root: String,
    },
    InvalidRoot(String),
}

impl std::fmt::Display for JailError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            JailError::EmptyPath => write!(f, "path cannot be empty"),
            JailError::NullOrControlByte => write!(f, "path contains null or control byte"),
            JailError::UncPathForbidden(p) => write!(f, "UNC path '{}' is forbidden", p),
            JailError::AlternateDataStreamForbidden(p) => {
                write!(f, "NTFS Alternate Data Stream '{}' is forbidden", p)
            }
            JailError::DosReservedDeviceForbidden(dev) => {
                write!(f, "Windows reserved DOS device '{}' is forbidden", dev)
            }
            JailError::PathTraversalEscape {
                requested,
                jail_root,
            } => write!(
                f,
                "path '{}' escapes canonical project jail '{}'",
                requested, jail_root
            ),
            JailError::InvalidRoot(r) => write!(f, "invalid jail root '{}'", r),
        }
    }
}

impl std::error::Error for JailError {}

/// Canonical logical path jail that operates deterministically in memory without
/// requiring target files to exist on disk beforehand.
#[derive(Debug, Clone)]
pub struct CanonicalPathJail {
    canonical_root: String,
    drive_prefix_upper: Option<String>,
    root_segments: Vec<String>,
}

impl CanonicalPathJail {
    pub fn new(project_root: &str) -> Result<Self, JailError> {
        let (drive_prefix_upper, root_segments) = Self::parse_root_components(project_root)?;
        let canonical_root = Self::format_canonical(&drive_prefix_upper, &root_segments);
        Ok(Self {
            canonical_root,
            drive_prefix_upper,
            root_segments,
        })
    }

    pub fn canonical_root(&self) -> &str {
        &self.canonical_root
    }

    pub fn resolve_and_confine(&self, raw_target: &str) -> Result<String, JailError> {
        Self::validate_raw_characters(raw_target)?;
        let trimmed = raw_target.trim();
        if trimmed.is_empty() {
            return Err(JailError::EmptyPath);
        }

        // Block UNC and extended-length device paths immediately
        if trimmed.starts_with(r"\\")
            || trimmed.starts_with("//")
            || trimmed.to_ascii_uppercase().starts_with(r"\\?\UNC\")
        {
            return Err(JailError::UncPathForbidden(trimmed.to_string()));
        }

        let normalized = trimmed.replace('\\', "/");

        // Check drive letter and NTFS Alternate Data Streams (colon outside index 1)
        let (target_drive, rel_slice) = if normalized.len() >= 2
            && normalized.as_bytes()[0].is_ascii_alphabetic()
            && normalized.as_bytes()[1] == b':'
        {
            let drv = format!("{}:", (normalized.as_bytes()[0] as char).to_ascii_uppercase());
            let rest = &normalized[2..];
            if rest.contains(':') {
                return Err(JailError::AlternateDataStreamForbidden(trimmed.to_string()));
            }
            (Some(drv), rest)
        } else {
            if normalized.contains(':') {
                return Err(JailError::AlternateDataStreamForbidden(trimmed.to_string()));
            }
            (None, normalized.as_str())
        };

        let mut working_segments: Vec<String> = match target_drive {
            Some(ref d) => {
                if self.drive_prefix_upper.as_ref() != Some(d) {
                    return Err(JailError::PathTraversalEscape {
                        requested: trimmed.to_string(),
                        jail_root: self.canonical_root.clone(),
                    });
                }
                Vec::new()
            }
            None => {
                if rel_slice.starts_with('/') {
                    if self.drive_prefix_upper.is_some() {
                        return Err(JailError::PathTraversalEscape {
                            requested: trimmed.to_string(),
                            jail_root: self.canonical_root.clone(),
                        });
                    }
                    Vec::new()
                } else {
                    self.root_segments.clone()
                }
            }
        };

        for part in rel_slice.split('/') {
            if part.is_empty() || part == "." {
                continue;
            }
            if part == ".." {
                if working_segments.is_empty() {
                    return Err(JailError::PathTraversalEscape {
                        requested: trimmed.to_string(),
                        jail_root: self.canonical_root.clone(),
                    });
                }
                working_segments.pop();
                // If popping moved above root_segments, reject immediately!
                if !Self::starts_with_segments(&working_segments, &self.root_segments) {
                    return Err(JailError::PathTraversalEscape {
                        requested: trimmed.to_string(),
                        jail_root: self.canonical_root.clone(),
                    });
                }
                continue;
            }

            // Strip trailing dots/spaces (Windows Win32 canonicalization quirk)
            let cleaned = part.trim_end_matches(['.', ' ']);
            if cleaned.is_empty() {
                return Err(JailError::PathTraversalEscape {
                    requested: trimmed.to_string(),
                    jail_root: self.canonical_root.clone(),
                });
            }

            Self::check_dos_reserved_segment(cleaned)?;
            working_segments.push(cleaned.to_string());
        }

        if !Self::starts_with_segments(&working_segments, &self.root_segments) {
            return Err(JailError::PathTraversalEscape {
                requested: trimmed.to_string(),
                jail_root: self.canonical_root.clone(),
            });
        }

        Ok(Self::format_canonical(
            &self.drive_prefix_upper,
            &working_segments,
        ))
    }

    pub fn relative_to_root(&self, canonical_path: &str) -> String {
        if canonical_path == self.canonical_root {
            return ".".to_string();
        }
        let prefix = format!("{}/", self.canonical_root);
        if let Some(rel) = canonical_path.strip_prefix(&prefix) {
            rel.to_string()
        } else {
            canonical_path.to_string()
        }
    }

    fn validate_raw_characters(raw: &str) -> Result<(), JailError> {
        for b in raw.bytes() {
            if b == 0 || b < 0x20 {
                return Err(JailError::NullOrControlByte);
            }
        }
        Ok(())
    }

    fn check_dos_reserved_segment(segment: &str) -> Result<(), JailError> {
        let stem = segment.split('.').next().unwrap_or(segment);
        let upper = stem.trim().to_ascii_uppercase();
        if WINDOWS_RESERVED_DOS_DEVICES.contains(&upper.as_str()) {
            return Err(JailError::DosReservedDeviceForbidden(segment.to_string()));
        }
        Ok(())
    }

    fn starts_with_segments(candidate: &[String], root: &[String]) -> bool {
        if candidate.len() < root.len() {
            return false;
        }
        for i in 0..root.len() {
            if !candidate[i].eq_ignore_ascii_case(&root[i]) {
                return false;
            }
        }
        true
    }

    fn parse_root_components(
        project_root: &str,
    ) -> Result<(Option<String>, Vec<String>), JailError> {
        Self::validate_raw_characters(project_root)?;
        let trimmed = project_root.trim();
        if trimmed.is_empty() || trimmed.starts_with(r"\\") || trimmed.starts_with("//") {
            return Err(JailError::InvalidRoot(project_root.to_string()));
        }
        let normalized = trimmed.replace('\\', "/");
        let (drive, rest) = if normalized.len() >= 2
            && normalized.as_bytes()[0].is_ascii_alphabetic()
            && normalized.as_bytes()[1] == b':'
        {
            let drv = format!("{}:", (normalized.as_bytes()[0] as char).to_ascii_uppercase());
            (Some(drv), &normalized[2..])
        } else if normalized.starts_with('/') {
            (None, normalized.as_str())
        } else {
            return Err(JailError::InvalidRoot(project_root.to_string()));
        };

        let mut segments = Vec::new();
        for seg in rest.split('/') {
            if seg.is_empty() || seg == "." {
                continue;
            }
            if seg == ".." {
                return Err(JailError::InvalidRoot(project_root.to_string()));
            }
            Self::check_dos_reserved_segment(seg)?;
            segments.push(seg.to_string());
        }
        if segments.is_empty() {
            return Err(JailError::InvalidRoot(project_root.to_string()));
        }
        Ok((drive, segments))
    }

    fn format_canonical(drive: &Option<String>, segments: &[String]) -> String {
        match drive {
            Some(d) => format!("{}/{}", d, segments.join("/")),
            None => format!("/{}", segments.join("/")),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PermissionTicket {
    pub req_hash: String,
    pub agent: String,
    pub capability: Capability,
    pub canonical_target: String,
    pub canonical_args: String,
    pub issued_at_ms: u64,
    pub expires_at_ms: u64,
    pub pc_only: bool,
    pub generation: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DenyReason {
    DefaultDeny,
    ExplicitDenyRule,
    PathJailViolation,
    ToctouHashMismatch,
    TicketNotFoundOrAlreadyUsed,
    TicketExpired,
    KillSwitchActive,
    PcOnlyFromMobileForbidden,
    BiometricRequiredOnMobile,
    UserDenied,
}

impl DenyReason {
    pub fn as_code(&self) -> &'static str {
        match self {
            DenyReason::DefaultDeny => "deny_default",
            DenyReason::ExplicitDenyRule => "deny_rule",
            DenyReason::PathJailViolation => "deny_jail",
            DenyReason::ToctouHashMismatch => "deny_toctou",
            DenyReason::TicketNotFoundOrAlreadyUsed => "deny_ticket_invalid",
            DenyReason::TicketExpired => "deny_ticket_expired",
            DenyReason::KillSwitchActive => "deny_kill_switch",
            DenyReason::PcOnlyFromMobileForbidden => "deny_pc_only_mobile",
            DenyReason::BiometricRequiredOnMobile => "deny_biometric_required",
            DenyReason::UserDenied => "deny_user_rejected",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum GatekeeperVerdict {
    Allow {
        canonical_target: String,
        audit_seq: u64,
        audit_hash: String,
    },
    AskPending {
        ticket: PermissionTicket,
        audit_seq: u64,
        audit_hash: String,
    },
    Deny {
        reason: DenyReason,
        detail: String,
        audit_seq: u64,
        audit_hash: String,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AuditEntry {
    pub seq: u64,
    pub timestamp_ms: u64,
    pub agent: String,
    pub capability: String,
    pub target: String,
    pub verdict: String,
    pub reason: String,
    pub prev_hash: String,
    pub entry_hash: String,
}

impl AuditEntry {
    pub fn compute_hash(
        prev_hash: &str,
        seq: u64,
        timestamp_ms: u64,
        agent: &str,
        capability: &str,
        target: &str,
        verdict: &str,
        reason: &str,
    ) -> String {
        let canonical_line = format!(
            "MIKU_AUDIT_V1|prev={}|seq={}|ts={}|agent={}|cap={}|target={}|verdict={}|reason={}",
            prev_hash, seq, timestamp_ms, agent, capability, target, verdict, reason
        );
        sha256_hex(canonical_line.as_bytes())
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TamperError {
    pub corrupted_index: usize,
    pub expected_seq: u64,
    pub reason: String,
}

impl std::fmt::Display for TamperError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "audit log tamper detected at index {} (expected seq {}): {}",
            self.corrupted_index, self.expected_seq, self.reason
        )
    }
}

impl std::error::Error for TamperError {}

#[derive(Debug, Clone, Default)]
pub struct TamperEvidentAuditLog {
    entries: Vec<AuditEntry>,
}

impl TamperEvidentAuditLog {
    pub fn new() -> Self {
        Self {
            entries: Vec::new(),
        }
    }

    pub fn len(&self) -> usize {
        self.entries.len()
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    pub fn entries(&self) -> &[AuditEntry] {
        &self.entries
    }

    pub fn entries_mut_for_tamper_simulation(&mut self) -> &mut Vec<AuditEntry> {
        &mut self.entries
    }

    pub fn append(
        &mut self,
        timestamp_ms: u64,
        agent: &str,
        capability: &str,
        target: &str,
        verdict: &str,
        reason: &str,
    ) -> (u64, String) {
        let seq = (self.entries.len() as u64) + 1;
        let prev_hash = self
            .entries
            .last()
            .map(|e| e.entry_hash.clone())
            .unwrap_or_else(|| GENESIS_AUDIT_HASH.to_string());

        let entry_hash = AuditEntry::compute_hash(
            &prev_hash,
            seq,
            timestamp_ms,
            agent,
            capability,
            target,
            verdict,
            reason,
        );

        self.entries.push(AuditEntry {
            seq,
            timestamp_ms,
            agent: agent.to_string(),
            capability: capability.to_string(),
            target: target.to_string(),
            verdict: verdict.to_string(),
            reason: reason.to_string(),
            prev_hash,
            entry_hash: entry_hash.clone(),
        });

        (seq, entry_hash)
    }

    pub fn verify_chain(&self) -> Result<(), TamperError> {
        let mut expected_prev = GENESIS_AUDIT_HASH;
        for (idx, entry) in self.entries.iter().enumerate() {
            let expected_seq = (idx as u64) + 1;
            if entry.seq != expected_seq {
                return Err(TamperError {
                    corrupted_index: idx,
                    expected_seq,
                    reason: format!("sequence gap or reorder: found seq {}", entry.seq),
                });
            }
            if entry.prev_hash != expected_prev {
                return Err(TamperError {
                    corrupted_index: idx,
                    expected_seq,
                    reason: format!(
                        "broken prev_hash link: expected {}, found {}",
                        expected_prev, entry.prev_hash
                    ),
                });
            }
            let recomputed = AuditEntry::compute_hash(
                &entry.prev_hash,
                entry.seq,
                entry.timestamp_ms,
                &entry.agent,
                &entry.capability,
                &entry.target,
                &entry.verdict,
                &entry.reason,
            );
            if !constant_time_token_eq(&recomputed, &entry.entry_hash) {
                return Err(TamperError {
                    corrupted_index: idx,
                    expected_seq,
                    reason: "entry_hash cryptographic mismatch".to_string(),
                });
            }
            expected_prev = &entry.entry_hash;
        }
        Ok(())
    }

    pub fn export_jsonl(&self) -> String {
        let mut out = String::new();
        for entry in &self.entries {
            if let Ok(line) = serde_json::to_string(entry) {
                out.push_str(&line);
                out.push('\n');
            }
        }
        out
    }

    pub fn verify_jsonl(raw_jsonl: &str) -> Result<usize, TamperError> {
        let mut log = TamperEvidentAuditLog::new();
        for (idx, line) in raw_jsonl.lines().enumerate() {
            let trimmed = line.trim();
            if trimmed.is_empty() {
                continue;
            }
            let parsed: AuditEntry = serde_json::from_str(trimmed).map_err(|e| TamperError {
                corrupted_index: idx,
                expected_seq: (idx as u64) + 1,
                reason: format!("malformed JSONL line: {}", e),
            })?;
            log.entries.push(parsed);
        }
        log.verify_chain()?;
        Ok(log.len())
    }
}

pub struct GatekeeperEngine {
    profile: ProjectProfile,
    jail: CanonicalPathJail,
    audit_log: TamperEvidentAuditLog,
    pending_tickets: HashMap<String, PermissionTicket>,
    kill_switch_active: AtomicBool,
    generation: AtomicU64,
    ticket_nonce: AtomicU64,
}

impl GatekeeperEngine {
    pub fn new(profile: ProjectProfile) -> Result<Self, JailError> {
        let jail = CanonicalPathJail::new(&profile.project_root)?;
        Ok(Self {
            profile,
            jail,
            audit_log: TamperEvidentAuditLog::new(),
            pending_tickets: HashMap::new(),
            kill_switch_active: AtomicBool::new(false),
            generation: AtomicU64::new(1),
            ticket_nonce: AtomicU64::new(1),
        })
    }

    pub fn profile(&self) -> &ProjectProfile {
        &self.profile
    }

    pub fn project_name(&self) -> &str {
        &self.profile.project_name
    }

    pub fn audit_log(&self) -> &TamperEvidentAuditLog {
        &self.audit_log
    }

    pub fn audit_log_mut_for_tamper_test(&mut self) -> &mut TamperEvidentAuditLog {
        &mut self.audit_log
    }

    pub fn pending_ticket_count(&self) -> usize {
        self.pending_tickets.len()
    }

    pub fn is_kill_switch_active(&self) -> bool {
        self.kill_switch_active.load(Ordering::SeqCst)
    }

    /// Activates the global Kill Switch atomically, revoking all in-flight interactive tickets
    /// and recording an immutable entry in the SHA-256 audit chain.
    pub fn trigger_kill_switch(&mut self, now_ms: u64) -> usize {
        self.kill_switch_active.store(true, Ordering::SeqCst);
        self.generation.fetch_add(1, Ordering::SeqCst);
        let revoked_count = self.pending_tickets.len();
        self.pending_tickets.clear();
        self.audit_log.append(
            now_ms,
            "system",
            "kill_switch",
            "*",
            "deny",
            &format!("kill_switch_activated_revoked_{}", revoked_count),
        );
        revoked_count
    }

    pub fn reset_kill_switch(&mut self, now_ms: u64) {
        self.kill_switch_active.store(false, Ordering::SeqCst);
        self.generation.fetch_add(1, Ordering::SeqCst);
        self.audit_log
            .append(now_ms, "system", "kill_switch", "*", "reset", "manual_reset");
    }

    pub fn compute_request_hash(
        agent: &str,
        capability: Capability,
        canonical_target: &str,
        canonical_args: &str,
    ) -> String {
        let payload = format!(
            "MIKU_REQ_HASH_V1|agent={}|cap={}|target={}|args={}",
            agent.trim(),
            capability.as_str(),
            canonical_target,
            canonical_args.trim()
        );
        format!("sha256:{}", sha256_hex(payload.as_bytes()))
    }

    /// Evaluates a capability request under strict `Deny > Ask > Allow` precedence and `deny-by-default`.
    pub fn evaluate_request(
        &mut self,
        agent: &str,
        capability: Capability,
        raw_target: &str,
        raw_args: &str,
        now_ms: u64,
    ) -> GatekeeperVerdict {
        // 1. Check Global Kill Switch first
        if self.is_kill_switch_active() {
            let (seq, hash) = self.audit_log.append(
                now_ms,
                agent,
                capability.as_str(),
                raw_target,
                "deny",
                DenyReason::KillSwitchActive.as_code(),
            );
            return GatekeeperVerdict::Deny {
                reason: DenyReason::KillSwitchActive,
                detail: "global Kill Switch is active".to_string(),
                audit_seq: seq,
                audit_hash: hash,
            };
        }

        // 2. Enforce CanonicalPathJail for filesystem capabilities
        let (canonical_target, match_target) = if capability.requires_path_jail() {
            match self.jail.resolve_and_confine(raw_target) {
                Ok(canon) => {
                    let rel = self.jail.relative_to_root(&canon);
                    (canon, rel)
                }
                Err(jail_err) => {
                    let (seq, hash) = self.audit_log.append(
                        now_ms,
                        agent,
                        capability.as_str(),
                        raw_target,
                        "deny",
                        DenyReason::PathJailViolation.as_code(),
                    );
                    return GatekeeperVerdict::Deny {
                        reason: DenyReason::PathJailViolation,
                        detail: jail_err.to_string(),
                        audit_seq: seq,
                        audit_hash: hash,
                    };
                }
            }
        } else {
            if raw_target.bytes().any(|b| b == 0 || b < 0x20) {
                let (seq, hash) = self.audit_log.append(
                    now_ms,
                    agent,
                    capability.as_str(),
                    raw_target,
                    "deny",
                    DenyReason::PathJailViolation.as_code(),
                );
                return GatekeeperVerdict::Deny {
                    reason: DenyReason::PathJailViolation,
                    detail: "target contains null or control byte".to_string(),
                    audit_seq: seq,
                    audit_hash: hash,
                };
            }
            let trimmed = raw_target.trim().to_string();
            (trimmed.clone(), trimmed)
        };

        // 3. Evaluate all matching rules with strict precedence: Deny > Ask > Allow
        let mut matched_deny = false;
        let mut matched_ask: Option<bool> = None; // Some(pc_only)
        let mut matched_allow = false;

        for rule in &self.profile.rules {
            if rule.capability != capability {
                continue;
            }
            if wildcard_match(&rule.pattern, &match_target)
                || wildcard_match(&rule.pattern, &canonical_target)
            {
                match rule.effect {
                    PolicyEffect::Deny => {
                        matched_deny = true;
                    }
                    PolicyEffect::Ask => {
                        let prev = matched_ask.unwrap_or(false);
                        matched_ask = Some(prev || rule.pc_only);
                    }
                    PolicyEffect::Allow => {
                        matched_allow = true;
                    }
                }
            }
        }

        if matched_deny {
            let (seq, hash) = self.audit_log.append(
                now_ms,
                agent,
                capability.as_str(),
                &canonical_target,
                "deny",
                DenyReason::ExplicitDenyRule.as_code(),
            );
            return GatekeeperVerdict::Deny {
                reason: DenyReason::ExplicitDenyRule,
                detail: format!(
                    "explicit deny rule matched target '{}'",
                    canonical_target
                ),
                audit_seq: seq,
                audit_hash: hash,
            };
        }

        if let Some(pc_only) = matched_ask {
            let _nonce = self.ticket_nonce.fetch_add(1, Ordering::SeqCst);
            let req_hash =
                Self::compute_request_hash(agent, capability, &canonical_target, raw_args);
            let ticket = PermissionTicket {
                req_hash: req_hash.clone(),
                agent: agent.to_string(),
                capability,
                canonical_target: canonical_target.clone(),
                canonical_args: raw_args.trim().to_string(),
                issued_at_ms: now_ms,
                expires_at_ms: now_ms.saturating_add(self.profile.default_ttl_ms),
                pc_only,
                generation: self.generation.load(Ordering::SeqCst),
            };
            self.pending_tickets.insert(req_hash.clone(), ticket.clone());

            let (seq, hash) = self.audit_log.append(
                now_ms,
                agent,
                capability.as_str(),
                &canonical_target,
                "ask_pending",
                &req_hash,
            );
            return GatekeeperVerdict::AskPending {
                ticket,
                audit_seq: seq,
                audit_hash: hash,
            };
        }

        if matched_allow {
            let (seq, hash) = self.audit_log.append(
                now_ms,
                agent,
                capability.as_str(),
                &canonical_target,
                "allow",
                "allow_rule",
            );
            return GatekeeperVerdict::Allow {
                canonical_target,
                audit_seq: seq,
                audit_hash: hash,
            };
        }

        // 4. Default Deny (zero implicit permissions)
        let (seq, hash) = self.audit_log.append(
            now_ms,
            agent,
            capability.as_str(),
            &canonical_target,
            "deny",
            DenyReason::DefaultDeny.as_code(),
        );
        GatekeeperVerdict::Deny {
            reason: DenyReason::DefaultDeny,
            detail: format!(
                "capability '{}' on '{}' is denied by default",
                capability.as_str(),
                canonical_target
            ),
            audit_seq: seq,
            audit_hash: hash,
        }
    }

    /// Resolves and consumes a single-use interactive `Ask` ticket, enforcing:
    /// - Global Kill Switch check
    /// - TTL expiration check
    /// - Mobile `pc_only` and `biometric_verified` policy
    /// - Cryptographic anti-TOCTOU binding on `(agent, capability, target, args)`
    pub fn resolve_permission_decision(
        &mut self,
        req_hash: &str,
        decision: PermissionDecisionKind,
        from_mobile_client: bool,
        biometric_verified: bool,
        execution_agent: &str,
        execution_capability: Capability,
        execution_raw_target: &str,
        execution_raw_args: &str,
        now_ms: u64,
    ) -> GatekeeperVerdict {
        if self.is_kill_switch_active() {
            let (seq, hash) = self.audit_log.append(
                now_ms,
                execution_agent,
                execution_capability.as_str(),
                execution_raw_target,
                "deny",
                DenyReason::KillSwitchActive.as_code(),
            );
            return GatekeeperVerdict::Deny {
                reason: DenyReason::KillSwitchActive,
                detail: "Kill Switch active during permission resolution".to_string(),
                audit_seq: seq,
                audit_hash: hash,
            };
        }

        let Some(ticket) = self.pending_tickets.remove(req_hash) else {
            let (seq, hash) = self.audit_log.append(
                now_ms,
                execution_agent,
                execution_capability.as_str(),
                execution_raw_target,
                "deny",
                DenyReason::TicketNotFoundOrAlreadyUsed.as_code(),
            );
            return GatekeeperVerdict::Deny {
                reason: DenyReason::TicketNotFoundOrAlreadyUsed,
                detail: format!("ticket '{}' not found or already consumed", req_hash),
                audit_seq: seq,
                audit_hash: hash,
            };
        };

        if ticket.generation != self.generation.load(Ordering::SeqCst) {
            let (seq, hash) = self.audit_log.append(
                now_ms,
                execution_agent,
                execution_capability.as_str(),
                &ticket.canonical_target,
                "deny",
                DenyReason::KillSwitchActive.as_code(),
            );
            return GatekeeperVerdict::Deny {
                reason: DenyReason::KillSwitchActive,
                detail: "ticket invalidated by generation change".to_string(),
                audit_seq: seq,
                audit_hash: hash,
            };
        }

        if now_ms > ticket.expires_at_ms {
            let (seq, hash) = self.audit_log.append(
                now_ms,
                execution_agent,
                execution_capability.as_str(),
                &ticket.canonical_target,
                "deny",
                DenyReason::TicketExpired.as_code(),
            );
            return GatekeeperVerdict::Deny {
                reason: DenyReason::TicketExpired,
                detail: format!(
                    "ticket '{}' expired at {} ms (now {} ms)",
                    req_hash, ticket.expires_at_ms, now_ms
                ),
                audit_seq: seq,
                audit_hash: hash,
            };
        }

        if decision == PermissionDecisionKind::Deny {
            let (seq, hash) = self.audit_log.append(
                now_ms,
                execution_agent,
                execution_capability.as_str(),
                &ticket.canonical_target,
                "deny",
                DenyReason::UserDenied.as_code(),
            );
            return GatekeeperVerdict::Deny {
                reason: DenyReason::UserDenied,
                detail: "user explicitly denied permission request".to_string(),
                audit_seq: seq,
                audit_hash: hash,
            };
        }

        if from_mobile_client {
            if ticket.pc_only {
                let (seq, hash) = self.audit_log.append(
                    now_ms,
                    execution_agent,
                    execution_capability.as_str(),
                    &ticket.canonical_target,
                    "deny",
                    DenyReason::PcOnlyFromMobileForbidden.as_code(),
                );
                return GatekeeperVerdict::Deny {
                    reason: DenyReason::PcOnlyFromMobileForbidden,
                    detail: "high-risk pc_only capability cannot be approved from mobile".to_string(),
                    audit_seq: seq,
                    audit_hash: hash,
                };
            }
            if !biometric_verified {
                let (seq, hash) = self.audit_log.append(
                    now_ms,
                    execution_agent,
                    execution_capability.as_str(),
                    &ticket.canonical_target,
                    "deny",
                    DenyReason::BiometricRequiredOnMobile.as_code(),
                );
                return GatekeeperVerdict::Deny {
                    reason: DenyReason::BiometricRequiredOnMobile,
                    detail: "mobile Allow requires biometric_verified == true".to_string(),
                    audit_seq: seq,
                    audit_hash: hash,
                };
            }
        }

        // Anti-TOCTOU verification: canonicalize execution target and recompute req_hash
        let canonical_exec_target = if execution_capability.requires_path_jail() {
            match self.jail.resolve_and_confine(execution_raw_target) {
                Ok(c) => c,
                Err(e) => {
                    let (seq, hash) = self.audit_log.append(
                        now_ms,
                        execution_agent,
                        execution_capability.as_str(),
                        execution_raw_target,
                        "deny",
                        DenyReason::PathJailViolation.as_code(),
                    );
                    return GatekeeperVerdict::Deny {
                        reason: DenyReason::PathJailViolation,
                        detail: e.to_string(),
                        audit_seq: seq,
                        audit_hash: hash,
                    };
                }
            }
        } else {
            execution_raw_target.trim().to_string()
        };

        let recomputed_hash = Self::compute_request_hash(
            execution_agent,
            execution_capability,
            &canonical_exec_target,
            execution_raw_args,
        );

        if !constant_time_token_eq(&recomputed_hash, &ticket.req_hash) {
            let (seq, hash) = self.audit_log.append(
                now_ms,
                execution_agent,
                execution_capability.as_str(),
                &canonical_exec_target,
                "deny",
                DenyReason::ToctouHashMismatch.as_code(),
            );
            return GatekeeperVerdict::Deny {
                reason: DenyReason::ToctouHashMismatch,
                detail: "execution arguments do not match signed ticket req_hash (TOCTOU blocked)"
                    .to_string(),
                audit_seq: seq,
                audit_hash: hash,
            };
        }

        let (seq, hash) = self.audit_log.append(
            now_ms,
            execution_agent,
            execution_capability.as_str(),
            &canonical_exec_target,
            "allow",
            "ask_approved_verified",
        );
        GatekeeperVerdict::Allow {
            canonical_target: canonical_exec_target,
            audit_seq: seq,
            audit_hash: hash,
        }
    }

    /// Converts a Gatekeeper evaluation into a `miku-link v1` protocol envelope for `miku-gateway`.
    pub fn evaluate_into_protocol_envelope(
        &mut self,
        request_id: &str,
        agent: &str,
        capability: Capability,
        raw_target: &str,
        raw_args: &str,
        now_ms: u64,
    ) -> (GatekeeperVerdict, MikuLinkEnvelopeV1) {
        let verdict = self.evaluate_request(agent, capability, raw_target, raw_args, now_ms);
        let env = match &verdict {
            GatekeeperVerdict::Allow {
                canonical_target, ..
            } => MikuLinkEnvelopeV1 {
                v: MIKU_PROTOCOL_VERSION_V1,
                id: format!("gk-allow-{}", request_id),
                reply_to: Some(request_id.to_string()),
                msg: MikuLinkMessageV1::AgentEvent {
                    agent: agent.to_string(),
                    kind: "gatekeeper_allowed".to_string(),
                    title: canonical_target.clone(),
                },
            },
            GatekeeperVerdict::AskPending { ticket, .. } => MikuLinkEnvelopeV1 {
                v: MIKU_PROTOCOL_VERSION_V1,
                id: format!("gk-ask-{}", request_id),
                reply_to: Some(request_id.to_string()),
                msg: MikuLinkMessageV1::PermissionRequest {
                    req_hash: ticket.req_hash.clone(),
                    agent: ticket.agent.clone(),
                    capability: ticket.capability.as_str().to_string(),
                    target: ticket.canonical_target.clone(),
                    ttl_sec: (self.profile.default_ttl_ms / 1000).max(1) as u32,
                    pc_only: ticket.pc_only,
                },
            },
            GatekeeperVerdict::Deny { reason, detail, .. } => MikuLinkEnvelopeV1 {
                v: MIKU_PROTOCOL_VERSION_V1,
                id: format!("gk-deny-{}", request_id),
                reply_to: Some(request_id.to_string()),
                msg: MikuLinkMessageV1::Error {
                    code: "gatekeeper_denied".to_string(),
                    supported: None,
                    detail: Some(format!("{}: {}", reason.as_code(), detail)),
                },
            },
        };
        (verdict, env)
    }
}

/// Deterministic wildcard matcher supporting `*` and `**`.
pub fn wildcard_match(pattern: &str, text: &str) -> bool {
    let p = pattern.trim().replace('\\', "/");
    let t = text.trim().replace('\\', "/");
    if p == "*" || p == "**" {
        return true;
    }
    wildcard_bytes(p.as_bytes(), t.as_bytes())
}

fn wildcard_bytes(p: &[u8], t: &[u8]) -> bool {
    let mut pi = 0usize;
    let mut ti = 0usize;
    let mut star_idx: Option<usize> = None;
    let mut match_idx = 0usize;

    while ti < t.len() {
        if pi < p.len() && (p[pi] == b'?' || p[pi].eq_ignore_ascii_case(&t[ti])) {
            pi += 1;
            ti += 1;
        } else if pi < p.len() && p[pi] == b'*' {
            while pi < p.len() && p[pi] == b'*' {
                pi += 1;
            }
            if pi == p.len() {
                return true;
            }
            star_idx = Some(pi);
            match_idx = ti;
        } else if let Some(s_pi) = star_idx {
            pi = s_pi;
            match_idx += 1;
            ti = match_idx;
        } else {
            return false;
        }
    }

    while pi < p.len() && p[pi] == b'*' {
        pi += 1;
    }
    pi == p.len()
}

#[cfg(test)]
mod tests {
    use super::*;
    use miku_core::{build_current_thread_runtime, spawn_orchestrator};
    use miku_gateway::{start_gateway_server, GatewayConfig, LoopbackWsClient};
    use miku_protocol::BaseContractBundle;

    fn sample_production_profile() -> ProjectProfile {
        ProjectProfile::new("miku-workspace", "C:/Users/cmiranda/dev/miku", 60_000)
            .with_rule(Capability::FsRead, "src/**", PolicyEffect::Allow, false)
            .with_rule(Capability::FsRead, "Cargo.toml", PolicyEffect::Allow, false)
            .with_rule(
                Capability::FsRead,
                "src/secrets/**",
                PolicyEffect::Deny,
                true,
            )
            .with_rule(Capability::FsWrite, "src/**", PolicyEffect::Ask, false)
            .with_rule(
                Capability::FsWrite,
                "src/core_lock/**",
                PolicyEffect::Deny,
                true,
            )
            .with_rule(Capability::Shell, "cargo test*", PolicyEffect::Allow, false)
            .with_rule(Capability::Shell, "cargo clippy*", PolicyEffect::Ask, true)
            .with_rule(Capability::Shell, "*rm -rf*", PolicyEffect::Deny, true)
            .with_rule(
                Capability::Network,
                "*.github.com",
                PolicyEffect::Allow,
                false,
            )
            .with_rule(Capability::Network, "x.com", PolicyEffect::Ask, false)
    }

    // LEDGER-1.3-1
    #[test]
    fn test_deny_by_default_policy_precedence_and_profile_evaluation() {
        let mut engine = GatekeeperEngine::new(sample_production_profile()).expect("init engine");

        // 1. Unlisted capability (Input / Vision) -> DefaultDeny
        let v_unlisted =
            engine.evaluate_request("claude_code", Capability::Input, "mouse_click", "", 1_000);
        assert!(matches!(
            v_unlisted,
            GatekeeperVerdict::Deny {
                reason: DenyReason::DefaultDeny,
                ..
            }
        ));

        // 2. Allowed rule -> Allow
        let v_allow =
            engine.evaluate_request("claude_code", Capability::FsRead, "src/lib.rs", "", 1_001);
        assert!(matches!(v_allow, GatekeeperVerdict::Allow { .. }));

        // 3. Conflict between Allow ("src/**") and Deny ("src/secrets/**") -> Deny MUST win
        let v_conflict_allow_deny = engine.evaluate_request(
            "claude_code",
            Capability::FsRead,
            "src/secrets/api_keys.txt",
            "",
            1_002,
        );
        assert!(matches!(
            v_conflict_allow_deny,
            GatekeeperVerdict::Deny {
                reason: DenyReason::ExplicitDenyRule,
                ..
            }
        ));

        // 4. Conflict between Ask ("src/**") and Deny ("src/core_lock/**") -> Deny MUST win
        let v_conflict_ask_deny = engine.evaluate_request(
            "cursor",
            Capability::FsWrite,
            "src/core_lock/boot.rs",
            "patch",
            1_003,
        );
        assert!(matches!(
            v_conflict_ask_deny,
            GatekeeperVerdict::Deny {
                reason: DenyReason::ExplicitDenyRule,
                ..
            }
        ));

        // 5. Conflict between Shell Allow ("cargo test*") and Shell Deny ("*rm -rf*") -> Deny MUST win
        let v_shell_conflict = engine.evaluate_request(
            "claude_code",
            Capability::Shell,
            "cargo test && rm -rf /",
            "",
            1_004,
        );
        assert!(matches!(
            v_shell_conflict,
            GatekeeperVerdict::Deny {
                reason: DenyReason::ExplicitDenyRule,
                ..
            }
        ));
    }

    // LEDGER-1.3-2
    #[test]
    fn test_canonical_path_jail_neutralizes_traversal_unc_dos_devices_and_null_bytes() {
        let jail =
            CanonicalPathJail::new(r"C:\Users\cmiranda\dev\miku").expect("valid canonical jail");
        assert_eq!(jail.canonical_root(), "C:/Users/cmiranda/dev/miku");

        // Non-existent nested file inside jail succeeds logically
        let ok_new_file = jail
            .resolve_and_confine(r"crates\miku-gatekeeper\.\src\..\src\new_module.rs")
            .expect("valid relative path inside jail");
        assert_eq!(
            ok_new_file,
            "C:/Users/cmiranda/dev/miku/crates/miku-gatekeeper/src/new_module.rs"
        );

        // Traversal escaping project root must be blocked
        for traversal in [
            r"..\outside.txt",
            r"src\..\..\..\..\Windows\System32\cmd.exe",
            r"D:\other_drive\file.rs",
        ] {
            assert!(matches!(
                jail.resolve_and_confine(traversal),
                Err(JailError::PathTraversalEscape { .. })
            ));
        }

        // UNC paths must be blocked
        for unc in [r"\\?\UNC\attacker\share\payload.dll", r"\\192.168.1.9\c$\secret"] {
            assert!(matches!(
                jail.resolve_and_confine(unc),
                Err(JailError::UncPathForbidden(_))
            ));
        }

        // Windows DOS reserved devices (with or without extension) must be blocked
        for dos in [
            "NUL",
            "con",
            "src/AUX.txt",
            "crates/COM1.log",
            "crates/LPT9.json",
            "PRN",
        ] {
            assert!(matches!(
                jail.resolve_and_confine(dos),
                Err(JailError::DosReservedDeviceForbidden(_))
            ));
        }

        // Null bytes and control characters must be blocked
        assert!(matches!(
            jail.resolve_and_confine("src/main.rs\0.txt"),
            Err(JailError::NullOrControlByte)
        ));
    }

    // LEDGER-1.3-3
    #[test]
    fn test_anti_toctou_ticket_request_hash_ttl_expiry_and_atomic_kill_switch() {
        let mut engine = GatekeeperEngine::new(sample_production_profile()).expect("init engine");

        // 1. Issue Ask ticket for fs_write on src/app.rs
        let v_ask = engine.evaluate_request(
            "claude_code",
            Capability::FsWrite,
            "src/app.rs",
            "fn main() {}",
            10_000,
        );
        let ticket = match v_ask {
            GatekeeperVerdict::AskPending { ticket, .. } => ticket,
            other => panic!("expected AskPending, got {:?}", other),
        };

        // 2. Attempt TOCTOU attack: approve with valid req_hash but mutate args by 1 character
        let v_toctou = engine.resolve_permission_decision(
            &ticket.req_hash,
            PermissionDecisionKind::Allow,
            false,
            false,
            "claude_code",
            Capability::FsWrite,
            "src/app.rs",
            "fn main() { std::process::Command::new(\"calc\"); }",
            10_500,
        );
        assert!(matches!(
            v_toctou,
            GatekeeperVerdict::Deny {
                reason: DenyReason::ToctouHashMismatch,
                ..
            }
        ));

        // 3. Re-issue ticket and let TTL expire (TTL = 60_000 ms)
        let v_ask2 = engine.evaluate_request(
            "claude_code",
            Capability::FsWrite,
            "src/app.rs",
            "fn main() {}",
            20_000,
        );
        let ticket2 = match v_ask2 {
            GatekeeperVerdict::AskPending { ticket, .. } => ticket,
            _ => unreachable!(),
        };
        let v_expired = engine.resolve_permission_decision(
            &ticket2.req_hash,
            PermissionDecisionKind::Allow,
            false,
            false,
            "claude_code",
            Capability::FsWrite,
            "src/app.rs",
            "fn main() {}",
            80_001, // > 20_000 + 60_000
        );
        assert!(matches!(
            v_expired,
            GatekeeperVerdict::Deny {
                reason: DenyReason::TicketExpired,
                ..
            }
        ));

        // 4. pc_only rule ("cargo clippy*") must reject mobile approval even with biometric_verified=true
        let v_ask_pconly = engine.evaluate_request(
            "cursor",
            Capability::Shell,
            "cargo clippy --all-targets",
            "",
            30_000,
        );
        let ticket_pconly = match v_ask_pconly {
            GatekeeperVerdict::AskPending { ticket, .. } => ticket,
            _ => unreachable!(),
        };
        assert!(ticket_pconly.pc_only);
        let v_mobile_pconly = engine.resolve_permission_decision(
            &ticket_pconly.req_hash,
            PermissionDecisionKind::Allow,
            true,
            true,
            "cursor",
            Capability::Shell,
            "cargo clippy --all-targets",
            "",
            30_100,
        );
        assert!(matches!(
            v_mobile_pconly,
            GatekeeperVerdict::Deny {
                reason: DenyReason::PcOnlyFromMobileForbidden,
                ..
            }
        ));

        // 5. Issue 2 valid tickets, then trigger Global Kill Switch -> revokes all in-flight tickets
        let _ = engine.evaluate_request("a1", Capability::FsWrite, "src/one.rs", "1", 40_000);
        let _ = engine.evaluate_request("a2", Capability::FsWrite, "src/two.rs", "2", 40_001);
        assert_eq!(engine.pending_ticket_count(), 2);

        let revoked = engine.trigger_kill_switch(40_010);
        assert_eq!(revoked, 2);
        assert_eq!(engine.pending_ticket_count(), 0);

        let v_after_ks =
            engine.evaluate_request("a1", Capability::FsRead, "Cargo.toml", "", 40_020);
        assert!(matches!(
            v_after_ks,
            GatekeeperVerdict::Deny {
                reason: DenyReason::KillSwitchActive,
                ..
            }
        ));
    }

    // LEDGER-1.3-4
    #[test]
    fn test_tamper_evident_sha256_audit_log_chain_and_gateway_enforcement() {
        let rt = build_current_thread_runtime().expect("current_thread runtime");
        rt.block_on(async {
            let mut engine =
                GatekeeperEngine::new(sample_production_profile()).expect("init engine");

            // Generate several audit events and protocol envelopes
            let (_v1, env_allow) = engine.evaluate_into_protocol_envelope(
                "req-1",
                "claude_code",
                Capability::FsRead,
                "src/main.rs",
                "",
                1_000,
            );
            assert!(matches!(env_allow.msg, MikuLinkMessageV1::AgentEvent { .. }));

            let (_v2, env_ask) = engine.evaluate_into_protocol_envelope(
                "req-2",
                "claude_code",
                Capability::FsWrite,
                "src/main.rs",
                "edit",
                1_010,
            );
            assert!(matches!(
                env_ask.msg,
                MikuLinkMessageV1::PermissionRequest { .. }
            ));

            let (_v3, env_deny) = engine.evaluate_into_protocol_envelope(
                "req-3",
                "claude_code",
                Capability::FsWrite,
                r"..\..\Windows\win.ini",
                "hack",
                1_020,
            );
            assert!(matches!(
                env_deny.msg,
                MikuLinkMessageV1::Error { ref code, .. } if code == "gatekeeper_denied"
            ));

            // Verify intact SHA-256 chain & JSONL export
            assert!(engine.audit_log().verify_chain().is_ok());
            let jsonl = engine.audit_log().export_jsonl();
            assert_eq!(TamperEvidentAuditLog::verify_jsonl(&jsonl), Ok(3));

            // Simulate 1-bit tamper on entry #2 -> verify_chain MUST pinpoint index 1
            let mut tampered_log = engine.audit_log().clone();
            tampered_log.entries_mut_for_tamper_simulation()[1].target =
                "src/tampered.rs".to_string();
            let err = tampered_log
                .verify_chain()
                .expect_err("must detect 1-bit tamper");
            assert_eq!(err.corrupted_index, 1);

            // Simulate deletion of entry #1 -> verify_chain MUST detect sequence / link break at index 0
            let mut deleted_log = engine.audit_log().clone();
            deleted_log.entries_mut_for_tamper_simulation().remove(0);
            assert!(deleted_log.verify_chain().is_err());

            // Verify live roundtrip over miku-gateway loopback WebSocket
            let bundle = BaseContractBundle::load_embedded_defaults().expect("contracts");
            let (orch_handle, orch_join) =
                spawn_orchestrator(bundle, "miku_gk_gw_token".to_string(), 64);
            let (gw_handle, gw_join) =
                start_gateway_server(GatewayConfig::loopback(0), orch_handle.clone())
                    .await
                    .expect("start gateway");

            let mut client = LoopbackWsClient::connect(gw_handle.local_addr)
                .await
                .expect("connect");
            let hello = r##"{"v":1,"id":"h-gk","msg":{"type":"hello","data":{"client":"miku_android","token":"miku_gk_gw_token","capabilities":["permission_decision"]}}}"##;
            client.send_text(hello).await.expect("send hello");
            let _ = client.recv_envelope().await.expect("hello_ack");

            // Send a valid permission_decision envelope over the authenticated Gateway
            if let MikuLinkMessageV1::PermissionRequest { req_hash, .. } = env_ask.msg {
                let decision_env = MikuLinkEnvelopeV1 {
                    v: 1,
                    id: "dec-1".to_string(),
                    reply_to: Some("gk-ask-req-2".to_string()),
                    msg: MikuLinkMessageV1::PermissionDecision {
                        req_hash: req_hash.clone(),
                        decision: PermissionDecisionKind::Allow,
                        biometric_verified: true,
                    },
                };
                let raw_dec = decision_env.encode_json().expect("encode");
                client.send_text(&raw_dec).await.expect("send decision");
                let ack = client.recv_envelope().await.expect("recv ack");
                assert_eq!(ack.reply_to.as_deref(), Some("dec-1"));

                // Resolve in GatekeeperEngine with matching args & mobile biometric_verified=true
                let resolved = engine.resolve_permission_decision(
                    &req_hash,
                    PermissionDecisionKind::Allow,
                    true,
                    true,
                    "claude_code",
                    Capability::FsWrite,
                    "src/main.rs",
                    "edit",
                    1_050,
                );
                assert!(matches!(resolved, GatekeeperVerdict::Allow { .. }));
            }

            client.close_cleanly().await.expect("close");
            gw_handle.trigger_shutdown();
            gw_join.await.expect("gw join");
            orch_handle.trigger_system_shutdown_signal();
            orch_join.await.expect("orch join");
        });
    }
}
