use std::collections::BTreeSet;
use std::fs;
use std::path::Path;

use miku_gatekeeper::{CanonicalPathJail, Capability};
use miku_protocol::sha256_hex;
use serde::{Deserialize, Serialize};

use crate::{
    SkillError, DEFAULT_SKILL_MAX_OUTPUT_BYTES, DEFAULT_SKILL_TIMEOUT_MS,
    MAX_SKILL_OUTPUT_BYTES_CEILING, MAX_SKILL_TIMEOUT_MS, SKILL_MANIFEST_FILENAME,
    SKILL_SCHEMA_VERSION_V1,
};

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub struct SemVer {
    pub major: u32,
    pub minor: u32,
    pub patch: u32,
}

impl SemVer {
    pub fn parse(raw: &str) -> Result<Self, SkillError> {
        let trimmed = raw.trim();
        let parts: Vec<&str> = trimmed.split('.').collect();
        if parts.len() != 3 {
            return Err(SkillError::InvalidSemVer(raw.to_string()));
        }
        for part in &parts {
            if part.is_empty() || (part.len() > 1 && part.starts_with('0')) {
                return Err(SkillError::InvalidSemVer(raw.to_string()));
            }
            if !part.bytes().all(|b| b.is_ascii_digit()) {
                return Err(SkillError::InvalidSemVer(raw.to_string()));
            }
        }
        let major = parts[0]
            .parse::<u32>()
            .map_err(|_| SkillError::InvalidSemVer(raw.to_string()))?;
        let minor = parts[1]
            .parse::<u32>()
            .map_err(|_| SkillError::InvalidSemVer(raw.to_string()))?;
        let patch = parts[2]
            .parse::<u32>()
            .map_err(|_| SkillError::InvalidSemVer(raw.to_string()))?;
        Ok(Self {
            major,
            minor,
            patch,
        })
    }
}

impl std::fmt::Display for SemVer {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}.{}.{}", self.major, self.minor, self.patch)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SkillExecutionLimits {
    #[serde(default = "default_timeout_ms")]
    pub timeout_ms: u64,
    #[serde(default = "default_max_output_bytes")]
    pub max_output_bytes: usize,
    #[serde(default = "default_isolated_process")]
    pub isolated_process: bool,
}

fn default_timeout_ms() -> u64 {
    DEFAULT_SKILL_TIMEOUT_MS
}

fn default_max_output_bytes() -> usize {
    DEFAULT_SKILL_MAX_OUTPUT_BYTES
}

fn default_isolated_process() -> bool {
    true
}

impl Default for SkillExecutionLimits {
    fn default() -> Self {
        Self {
            timeout_ms: DEFAULT_SKILL_TIMEOUT_MS,
            max_output_bytes: DEFAULT_SKILL_MAX_OUTPUT_BYTES,
            isolated_process: true,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SkillMetadataRaw {
    pub id: String,
    pub name: String,
    pub version: String,
    pub description: String,
    pub entrypoint: String,
    pub capabilities: Vec<String>,
    #[serde(default)]
    pub allowed_targets: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SkillManifestRaw {
    pub schema_version: u16,
    pub skill: SkillMetadataRaw,
    #[serde(default)]
    pub limits: SkillExecutionLimits,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SkillManifest {
    pub schema_version: u16,
    pub id: String,
    pub name: String,
    pub version: SemVer,
    pub description: String,
    pub entrypoint_rel: String,
    pub canonical_entrypoint: String,
    pub capabilities: Vec<Capability>,
    pub allowed_targets: Vec<String>,
    pub limits: SkillExecutionLimits,
    pub manifest_sha256: String,
}

impl SkillManifest {
    /// Parses and validates a `skill.toml` string inside a `CanonicalPathJail` rooted at `skill_dir_jail_root`.
    /// Never executes `entrypoint` or any external command during manifest loading.
    pub fn parse_and_validate(
        raw_toml: &str,
        skill_dir_jail_root: &str,
    ) -> Result<Self, SkillError> {
        let parsed: SkillManifestRaw = toml::from_str(raw_toml)
            .map_err(|e| SkillError::ManifestParseError(e.to_string()))?;

        if parsed.schema_version != SKILL_SCHEMA_VERSION_V1 {
            return Err(SkillError::UnsupportedSchemaVersion(parsed.schema_version));
        }

        validate_skill_id(&parsed.skill.id)?;
        validate_text_field("name", &parsed.skill.name, 120)?;
        validate_text_field("description", &parsed.skill.description, 500)?;
        let version = SemVer::parse(&parsed.skill.version)?;

        if parsed.skill.capabilities.is_empty() {
            return Err(SkillError::EmptyCapabilities);
        }

        let mut cap_set = BTreeSet::new();
        let mut capabilities = Vec::new();
        for raw_cap in &parsed.skill.capabilities {
            let cap = Capability::parse(raw_cap)
                .ok_or_else(|| SkillError::UnknownCapability(raw_cap.clone()))?;
            if cap_set.insert(cap.as_str()) {
                capabilities.push(cap);
            }
        }

        for target_pattern in &parsed.skill.allowed_targets {
            validate_text_field("allowed_targets", target_pattern, 260)?;
        }

        if parsed.limits.timeout_ms < 10 || parsed.limits.timeout_ms > MAX_SKILL_TIMEOUT_MS {
            return Err(SkillError::InvalidLimits(format!(
                "timeout_ms ({}) must be in 10..={}",
                parsed.limits.timeout_ms, MAX_SKILL_TIMEOUT_MS
            )));
        }
        if parsed.limits.max_output_bytes < 64
            || parsed.limits.max_output_bytes > MAX_SKILL_OUTPUT_BYTES_CEILING
        {
            return Err(SkillError::InvalidLimits(format!(
                "max_output_bytes ({}) must be in 64..={}",
                parsed.limits.max_output_bytes, MAX_SKILL_OUTPUT_BYTES_CEILING
            )));
        }
        if !parsed.limits.isolated_process {
            return Err(SkillError::InvalidLimits(
                "isolated_process must be true for F-11 sandboxed skills".to_string(),
            ));
        }

        let entrypoint_trimmed = parsed.skill.entrypoint.trim();
        if entrypoint_trimmed.is_empty() {
            return Err(SkillError::InvalidField {
                field: "entrypoint".to_string(),
                reason: "must not be empty".to_string(),
            });
        }

        // Enforce that entrypoint is strictly relative and confined inside skill_dir_jail_root
        let normalized_ep = entrypoint_trimmed.replace('\\', "/");
        if normalized_ep.starts_with('/')
            || (normalized_ep.len() >= 2 && normalized_ep.as_bytes()[1] == b':')
        {
            return Err(SkillError::PathJailViolation(format!(
                "entrypoint '{}' must be a relative path inside the skill folder",
                entrypoint_trimmed
            )));
        }

        let jail = CanonicalPathJail::new(skill_dir_jail_root)
            .map_err(|e| SkillError::PathJailViolation(e.to_string()))?;
        let canonical_entrypoint = jail
            .resolve_and_confine(entrypoint_trimmed)
            .map_err(|e| SkillError::PathJailViolation(e.to_string()))?;
        let entrypoint_rel = jail.relative_to_root(&canonical_entrypoint);
        if entrypoint_rel == "." || entrypoint_rel.is_empty() {
            return Err(SkillError::PathJailViolation(
                "entrypoint must reference a file inside the skill directory".to_string(),
            ));
        }

        let normalized_manifest = raw_toml.replace("\r\n", "\n");
        let manifest_sha256 = sha256_hex(normalized_manifest.as_bytes());

        Ok(Self {
            schema_version: parsed.schema_version,
            id: parsed.skill.id.trim().to_string(),
            name: parsed.skill.name.trim().to_string(),
            version,
            description: parsed.skill.description.trim().to_string(),
            entrypoint_rel,
            canonical_entrypoint,
            capabilities,
            allowed_targets: parsed
                .skill
                .allowed_targets
                .into_iter()
                .map(|s| s.trim().to_string())
                .collect(),
            limits: parsed.limits,
            manifest_sha256,
        })
    }

    /// Loads `skill.toml` from `skill_dir`, verifies that `entrypoint` exists on disk as a regular file,
    /// and never executes the entrypoint during manifest loading.
    pub fn load_from_dir(skill_dir: &Path) -> Result<Self, SkillError> {
        let manifest_path = skill_dir.join(SKILL_MANIFEST_FILENAME);
        let raw_toml = fs::read_to_string(&manifest_path).map_err(|e| {
            SkillError::IoError(format!("read {}: {}", manifest_path.display(), e))
        })?;

        let jail_root = normalize_os_path_for_jail(skill_dir);
        let manifest = Self::parse_and_validate(&raw_toml, &jail_root)?;

        let ep_path = skill_dir.join(&manifest.entrypoint_rel);
        let meta = fs::metadata(&ep_path).map_err(|_| {
            SkillError::MissingEntrypointFile(ep_path.to_string_lossy().to_string())
        })?;
        if !meta.is_file() {
            return Err(SkillError::MissingEntrypointFile(
                ep_path.to_string_lossy().to_string(),
            ));
        }

        Ok(manifest)
    }

    pub fn declares_capability(&self, capability: Capability) -> bool {
        self.capabilities.contains(&capability)
    }
}

pub fn normalize_os_path_for_jail(path: &Path) -> String {
    let abs = if path.is_absolute() {
        path.to_path_buf()
    } else {
        std::env::current_dir()
            .unwrap_or_else(|_| Path::new("/workspace").to_path_buf())
            .join(path)
    };
    let s = abs.to_string_lossy().replace('\\', "/");
    let stripped = s.strip_prefix("//?/").unwrap_or(&s);
    if stripped.starts_with('/')
        || (stripped.len() >= 2
            && stripped.as_bytes()[0].is_ascii_alphabetic()
            && stripped.as_bytes()[1] == b':')
    {
        stripped.to_string()
    } else {
        format!("/{}", stripped)
    }
}

fn validate_skill_id(raw_id: &str) -> Result<(), SkillError> {
    let trimmed = raw_id.trim();
    if trimmed.len() < 3 || trimmed.len() > 64 {
        return Err(SkillError::InvalidSkillId(raw_id.to_string()));
    }
    if trimmed.starts_with('-') || trimmed.ends_with('-') || trimmed.contains("--") {
        return Err(SkillError::InvalidSkillId(raw_id.to_string()));
    }
    if !trimmed
        .bytes()
        .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-')
    {
        return Err(SkillError::InvalidSkillId(raw_id.to_string()));
    }
    Ok(())
}

fn validate_text_field(field: &str, value: &str, max_len: usize) -> Result<(), SkillError> {
    let trimmed = value.trim();
    if trimmed.is_empty() {
        return Err(SkillError::InvalidField {
            field: field.to_string(),
            reason: "must not be empty".to_string(),
        });
    }
    if trimmed.len() > max_len {
        return Err(SkillError::InvalidField {
            field: field.to_string(),
            reason: format!("exceeds maximum length of {} chars", max_len),
        });
    }
    if value.bytes().any(|b| b == 0 || (b < 0x20 && b != b'\n' && b != b'\r' && b != b'\t')) {
        return Err(SkillError::InvalidField {
            field: field.to_string(),
            reason: "contains null or control characters".to_string(),
        });
    }
    Ok(())
}
