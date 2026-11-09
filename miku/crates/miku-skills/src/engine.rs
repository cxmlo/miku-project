use std::collections::BTreeMap;
use std::future::Future;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use miku_gatekeeper::{
    wildcard_match, Capability, GatekeeperEngine, GatekeeperVerdict, PolicyEffect,
};
use serde::{Deserialize, Serialize};

use crate::{normalize_os_path_for_jail, SkillError, SkillManifest};

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct LoadedSkill {
    pub manifest: SkillManifest,
    pub skill_dir: PathBuf,
    pub reload_generation: u64,
    pub loaded_at_ms: u64,
    pub last_reload_latency_ms: f64,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum SkillReloadOutcome {
    Loaded {
        skill_id: String,
        version: String,
        generation: u64,
        latency_ms: f64,
    },
    Reloaded {
        skill_id: String,
        previous_version: String,
        new_version: String,
        generation: u64,
        latency_ms: f64,
    },
    Unchanged {
        skill_id: String,
        version: String,
        generation: u64,
        latency_ms: f64,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SkillReloadDiagnostic {
    pub timestamp_ms: u64,
    pub skill_dir: String,
    pub error: String,
    pub preserved_active_version: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AuthorizedSkillAction {
    pub skill_id: String,
    pub skill_version: String,
    pub capability: Capability,
    pub canonical_target: String,
    pub audit_seq: u64,
    pub audit_hash: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SkillExecutionReport {
    pub skill_id: String,
    pub skill_version: String,
    pub capability: Capability,
    pub canonical_target: String,
    pub output: String,
    pub output_bytes: usize,
    pub elapsed_ms: f64,
    pub audit_seq: u64,
    pub audit_hash: String,
}

/// Validates that every capability declared in `manifest.capabilities` is permitted to be declared
/// under the active `GatekeeperEngine` profile (`Deny > Ask > Allow`, `deny-by-default`).
///
/// Rules:
/// 1. If the global Kill Switch is active, reject immediately.
/// 2. If the `ProjectProfile` has a wildcard `Deny` rule (`pattern == "*"`) for a declared capability,
///    reject with `CapabilityForbiddenByGatekeeper`.
/// 3. If the `ProjectProfile` has zero `Allow` or `Ask` rules for a declared capability (meaning the
///    capability is 100% denied by default in this project profile), reject with `CapabilityForbiddenByGatekeeper`.
pub fn validate_manifest_against_gatekeeper(
    manifest: &SkillManifest,
    gatekeeper: &GatekeeperEngine,
) -> Result<(), SkillError> {
    if gatekeeper.is_kill_switch_active() {
        return Err(SkillError::KillSwitchActive);
    }

    let profile = gatekeeper.profile();
    for &cap in &manifest.capabilities {
        let mut has_wildcard_deny = false;
        let mut has_any_allow_or_ask = false;

        for rule in &profile.rules {
            if rule.capability != cap {
                continue;
            }
            match rule.effect {
                PolicyEffect::Deny => {
                    if rule.pattern.trim() == "*" || rule.pattern.trim() == "**" {
                        has_wildcard_deny = true;
                    }
                }
                PolicyEffect::Allow | PolicyEffect::Ask => {
                    has_any_allow_or_ask = true;
                }
            }
        }

        if has_wildcard_deny {
            return Err(SkillError::CapabilityForbiddenByGatekeeper {
                skill_id: manifest.id.clone(),
                capability: cap,
                reason: format!(
                    "project profile '{}' explicitly denies capability '{}' with wildcard rule",
                    profile.project_name,
                    cap.as_str()
                ),
            });
        }

        if !has_any_allow_or_ask {
            return Err(SkillError::CapabilityForbiddenByGatekeeper {
                skill_id: manifest.id.clone(),
                capability: cap,
                reason: format!(
                    "project profile '{}' has no Allow/Ask rule for capability '{}' (deny-by-default)",
                    profile.project_name,
                    cap.as_str()
                ),
            });
        }
    }

    Ok(())
}

#[derive(Debug, Default)]
pub struct SkillEngine {
    active_skills: BTreeMap<String, LoadedSkill>,
    dir_to_skill_id: BTreeMap<String, String>,
    rejected_reload_count: usize,
    reload_diagnostics: Vec<SkillReloadDiagnostic>,
}

impl SkillEngine {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn active_skill_count(&self) -> usize {
        self.active_skills.len()
    }

    pub fn get_skill(&self, skill_id: &str) -> Option<&LoadedSkill> {
        self.active_skills.get(skill_id)
    }

    pub fn active_skills(&self) -> &BTreeMap<String, LoadedSkill> {
        &self.active_skills
    }

    pub fn rejected_reload_count(&self) -> usize {
        self.rejected_reload_count
    }

    pub fn reload_diagnostics(&self) -> &[SkillReloadDiagnostic] {
        &self.reload_diagnostics
    }

    /// Loads or hot-reloads a Skill from a directory on disk (`<skill_dir>/skill.toml`)
    /// using the transactional Shadow Buffer pattern:
    /// - Candidate manifest and Gatekeeper capability constraints are validated in isolation first.
    /// - If anything fails, the currently active skill version remains untouched and operational.
    pub fn load_or_reload_from_dir(
        &mut self,
        skill_dir: &Path,
        gatekeeper: &GatekeeperEngine,
        now_ms: u64,
    ) -> Result<SkillReloadOutcome, SkillError> {
        let t0 = Instant::now();
        let dir_key = normalize_os_path_for_jail(skill_dir);

        let candidate_res = SkillManifest::load_from_dir(skill_dir).and_then(|manifest| {
            validate_manifest_against_gatekeeper(&manifest, gatekeeper)?;
            Ok(manifest)
        });

        let candidate = match candidate_res {
            Ok(m) => m,
            Err(err) => {
                self.record_rejected_reload(&dir_key, &err, now_ms);
                return Err(err);
            }
        };

        Ok(self.commit_validated_candidate(candidate, skill_dir, dir_key, now_ms, t0.elapsed()))
    }

    /// Transactional hot-reload from an in-memory `skill.toml` string (used by tests and shadow checks).
    /// Preserves the active version if the new TOML is malformed or requests forbidden capabilities.
    pub fn load_or_reload_from_toml_str(
        &mut self,
        skill_dir: &Path,
        raw_toml: &str,
        gatekeeper: &GatekeeperEngine,
        now_ms: u64,
    ) -> Result<SkillReloadOutcome, SkillError> {
        let t0 = Instant::now();
        let dir_key = normalize_os_path_for_jail(skill_dir);

        let candidate_res =
            SkillManifest::parse_and_validate(raw_toml, &dir_key).and_then(|manifest| {
                validate_manifest_against_gatekeeper(&manifest, gatekeeper)?;
                Ok(manifest)
            });

        let candidate = match candidate_res {
            Ok(m) => m,
            Err(err) => {
                self.record_rejected_reload(&dir_key, &err, now_ms);
                return Err(err);
            }
        };

        Ok(self.commit_validated_candidate(candidate, skill_dir, dir_key, now_ms, t0.elapsed()))
    }

    fn record_rejected_reload(&mut self, dir_key: &str, err: &SkillError, now_ms: u64) {
        self.rejected_reload_count += 1;
        let preserved = self
            .dir_to_skill_id
            .get(dir_key)
            .and_then(|id| self.active_skills.get(id))
            .map(|s| s.manifest.version.to_string());
        self.reload_diagnostics.push(SkillReloadDiagnostic {
            timestamp_ms: now_ms,
            skill_dir: dir_key.to_string(),
            error: err.to_string(),
            preserved_active_version: preserved,
        });
    }

    fn commit_validated_candidate(
        &mut self,
        candidate: SkillManifest,
        skill_dir: &Path,
        dir_key: String,
        now_ms: u64,
        elapsed: Duration,
    ) -> SkillReloadOutcome {
        let latency_ms = elapsed.as_secs_f64() * 1_000.0;
        let skill_id = candidate.id.clone();
        let new_version = candidate.version.to_string();

        if let Some(existing) = self.active_skills.get(&skill_id) {
            if existing.manifest.manifest_sha256 == candidate.manifest_sha256 {
                return SkillReloadOutcome::Unchanged {
                    skill_id,
                    version: new_version,
                    generation: existing.reload_generation,
                    latency_ms,
                };
            }
            let prev_version = existing.manifest.version.to_string();
            let next_gen = existing.reload_generation + 1;
            self.active_skills.insert(
                skill_id.clone(),
                LoadedSkill {
                    manifest: candidate,
                    skill_dir: skill_dir.to_path_buf(),
                    reload_generation: next_gen,
                    loaded_at_ms: now_ms,
                    last_reload_latency_ms: latency_ms,
                },
            );
            self.dir_to_skill_id.insert(dir_key, skill_id.clone());
            SkillReloadOutcome::Reloaded {
                skill_id,
                previous_version: prev_version,
                new_version,
                generation: next_gen,
                latency_ms,
            }
        } else {
            self.active_skills.insert(
                skill_id.clone(),
                LoadedSkill {
                    manifest: candidate,
                    skill_dir: skill_dir.to_path_buf(),
                    reload_generation: 1,
                    loaded_at_ms: now_ms,
                    last_reload_latency_ms: latency_ms,
                },
            );
            self.dir_to_skill_id.insert(dir_key, skill_id.clone());
            SkillReloadOutcome::Loaded {
                skill_id,
                version: new_version,
                generation: 1,
                latency_ms,
            }
        }
    }

    /// Scans a parent skills directory (e.g. `skills/examples`) and loads all immediate subdirectories
    /// containing a `skill.toml` file.
    pub fn load_all_from_parent_dir(
        &mut self,
        parent_dir: &Path,
        gatekeeper: &GatekeeperEngine,
        now_ms: u64,
    ) -> Result<Vec<SkillReloadOutcome>, SkillError> {
        let entries = std::fs::read_dir(parent_dir).map_err(|e| {
            SkillError::IoError(format!("read_dir {}: {}", parent_dir.display(), e))
        })?;
        let mut dirs: Vec<PathBuf> = Vec::new();
        for entry in entries {
            let entry = entry.map_err(|e| SkillError::IoError(e.to_string()))?;
            let p = entry.path();
            if p.is_dir() && p.join(crate::SKILL_MANIFEST_FILENAME).is_file() {
                dirs.push(p);
            }
        }
        dirs.sort();

        let mut outcomes = Vec::with_capacity(dirs.len());
        for dir in dirs {
            let outcome = self.load_or_reload_from_dir(&dir, gatekeeper, now_ms)?;
            outcomes.push(outcome);
        }
        Ok(outcomes)
    }

    /// Two-layer runtime capability & Gatekeeper authorization for a Skill action:
    /// 1. Verifies the Skill declared `capability` in `skill.toml` (otherwise fails closed with `UndeclaredSkillCapability`).
    /// 2. Verifies target matches `allowed_targets` if the Skill declared target restrictions.
    /// 3. Evaluates the request through `GatekeeperEngine::evaluate_request` (declared capability != granted permission).
    pub fn authorize_skill_action(
        &self,
        skill_id: &str,
        capability: Capability,
        raw_target: &str,
        raw_args: &str,
        gatekeeper: &mut GatekeeperEngine,
        now_ms: u64,
    ) -> Result<AuthorizedSkillAction, SkillError> {
        let loaded = self
            .active_skills
            .get(skill_id)
            .ok_or_else(|| SkillError::SkillNotFound(skill_id.to_string()))?;

        // Layer 1: Skill manifest capability ceiling
        if !loaded.manifest.declares_capability(capability) {
            return Err(SkillError::UndeclaredSkillCapability {
                skill_id: skill_id.to_string(),
                capability,
            });
        }

        // Optional Skill-level target scope check
        if !loaded.manifest.allowed_targets.is_empty() {
            let trimmed_target = raw_target.trim();
            let normalized_target = trimmed_target.replace('\\', "/");
            let matched = loaded.manifest.allowed_targets.iter().any(|pat| {
                wildcard_match(pat, trimmed_target) || wildcard_match(pat, &normalized_target)
            });
            if !matched {
                return Err(SkillError::TargetOutOfSkillScope {
                    skill_id: skill_id.to_string(),
                    target: raw_target.to_string(),
                });
            }
        }

        // Layer 2: Mandatory Gatekeeper evaluation (declared capability is never an automatic grant)
        let agent_label = format!("skill:{}@{}", skill_id, loaded.manifest.version);
        let verdict =
            gatekeeper.evaluate_request(&agent_label, capability, raw_target, raw_args, now_ms);

        match verdict {
            GatekeeperVerdict::Allow {
                canonical_target,
                audit_seq,
                audit_hash,
            } => Ok(AuthorizedSkillAction {
                skill_id: skill_id.to_string(),
                skill_version: loaded.manifest.version.to_string(),
                capability,
                canonical_target,
                audit_seq,
                audit_hash,
            }),
            GatekeeperVerdict::AskPending {
                ticket, audit_seq, ..
            } => Err(SkillError::GatekeeperApprovalRequired {
                skill_id: skill_id.to_string(),
                capability,
                ticket_hash: ticket.req_hash,
                audit_seq,
            }),
            GatekeeperVerdict::Deny {
                reason,
                detail,
                audit_seq,
                ..
            } => Err(SkillError::GatekeeperDenied {
                skill_id: skill_id.to_string(),
                capability,
                reason,
                detail,
                audit_seq,
            }),
        }
    }

    /// Executes an isolated skill action after enforcing both the `skill.toml` capability ceiling
    /// and `GatekeeperEngine` authorization, applying strict `timeout_ms` and `max_output_bytes` limits.
    pub async fn execute_skill_isolated<F, Fut>(
        &self,
        skill_id: &str,
        capability: Capability,
        raw_target: &str,
        raw_args: &str,
        gatekeeper: &mut GatekeeperEngine,
        now_ms: u64,
        runner: F,
    ) -> Result<SkillExecutionReport, SkillError>
    where
        F: FnOnce(AuthorizedSkillAction) -> Fut,
        Fut: Future<Output = Result<String, String>>,
    {
        let auth = self.authorize_skill_action(
            skill_id,
            capability,
            raw_target,
            raw_args,
            gatekeeper,
            now_ms,
        )?;

        let loaded = self
            .active_skills
            .get(skill_id)
            .ok_or_else(|| SkillError::SkillNotFound(skill_id.to_string()))?;
        let timeout_ms = loaded.manifest.limits.timeout_ms;
        let max_output_bytes = loaded.manifest.limits.max_output_bytes;

        let t0 = Instant::now();
        let fut = runner(auth.clone());
        let output_res = tokio::time::timeout(Duration::from_millis(timeout_ms), fut)
            .await
            .map_err(|_| SkillError::ExecutionTimeout {
                skill_id: skill_id.to_string(),
                timeout_ms,
            })?;

        let output = output_res.map_err(|reason| SkillError::ExecutionFailed {
            skill_id: skill_id.to_string(),
            reason,
        })?;

        let output_bytes = output.len();
        if output_bytes > max_output_bytes {
            return Err(SkillError::OutputLimitExceeded {
                skill_id: skill_id.to_string(),
                produced_bytes: output_bytes,
                max_output_bytes,
            });
        }

        let elapsed_ms = t0.elapsed().as_secs_f64() * 1_000.0;
        Ok(SkillExecutionReport {
            skill_id: auth.skill_id,
            skill_version: auth.skill_version,
            capability: auth.capability,
            canonical_target: auth.canonical_target,
            output,
            output_bytes,
            elapsed_ms,
            audit_seq: auth.audit_seq,
            audit_hash: auth.audit_hash,
        })
    }
}

/// Resolves the canonical `skills/examples` directory from `CARGO_MANIFEST_DIR` or workspace root.
pub fn resolve_example_skills_dir() -> PathBuf {
    let manifest_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    // crates/miku-skills -> crates -> miku -> repo_root
    if let Some(miku_dir) = manifest_dir.parent().and_then(|p| p.parent()) {
        if let Some(repo_root) = miku_dir.parent() {
            let root_examples = repo_root.join("skills").join("examples");
            if root_examples.is_dir() {
                return root_examples;
            }
        }
        let miku_examples = miku_dir.join("skills").join("examples");
        if miku_examples.is_dir() {
            return miku_examples;
        }
    }
    PathBuf::from("skills/examples")
}
