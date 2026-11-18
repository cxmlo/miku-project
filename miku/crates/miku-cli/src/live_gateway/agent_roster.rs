use std::collections::BTreeSet;
use std::fs;
use std::path::Path;

use miku_agent::{audit_custom_agent_soul, ToolRegistry, CUSTOM_AGENT_MANIFEST_FILENAME};

use super::storage_actor::AgentProfileDto;

pub(super) fn default_agents() -> Vec<AgentProfileDto> {
    vec![
        AgentProfileDto {
            id: "miku".to_string(),
            name: "Miku".to_string(),
            role: "Orquestadora local".to_string(),
            model: "auto".to_string(),
            provider: String::new(),
            c: "#39c5bb".to_string(),
            outfit: "none".to_string(),
            prompt: "Coordina tareas locales, memoria FTS5 y subagentes con respuestas directas y precisas.".to_string(),
            active: true,
            isl: 1,
            tools_mode: "auto".to_string(),
            predefined: true,
            harness_id: None,
        },
        AgentProfileDto {
            id: "antigravity".to_string(),
            name: "Antigravity".to_string(),
            role: "IDE · Arquitectura & Código".to_string(),
            model: "auto".to_string(),
            provider: String::new(),
            c: "#4e9af7".to_string(),
            outfit: "shades".to_string(),
            prompt: "Especialista en Antigravity IDE y arquitectura de software. Usa antigravity_projects para inspeccionar workspaces/sesiones reales, y git_status/git_diff para verificar cambios.".to_string(),
            active: true,
            isl: 2,
            tools_mode: "auto".to_string(),
            predefined: true,
            harness_id: Some("antigravity".to_string()),
        },
        AgentProfileDto {
            id: "github".to_string(),
            name: "GitHub".to_string(),
            role: "Repos, Commits & PRs".to_string(),
            model: "auto".to_string(),
            provider: String::new(),
            c: "#94a3b8".to_string(),
            outfit: "beanie".to_string(),
            prompt: "Especialista en Git y GitHub. Usa git_status y git_diff para revisar estado, ramas y cambios reales bajo Gatekeeper.".to_string(),
            active: true,
            isl: 3,
            tools_mode: "auto".to_string(),
            predefined: true,
            harness_id: Some("github".to_string()),
        },
    ]
}

fn canonical_predefined_id(raw_id: &str) -> Option<&'static str> {
    let low = raw_id.trim().to_ascii_lowercase();
    for canonical in ["miku", "antigravity", "claude-code", "github", "cursor", "sys-ops"] {
        if low == canonical {
            return Some(canonical);
        }
        if let Some(rest) = low.strip_prefix(&format!("{}-", canonical)) {
            if !rest.is_empty() && rest.chars().all(|c| c.is_ascii_alphanumeric()) {
                return Some(canonical);
            }
        }
    }
    if low == "claude" || low.starts_with("claude-") {
        return Some("claude-code");
    }
    None
}

/// Reconciles legacy/broken agent IDs (like `github-dead`) into curated predefined
/// customizable agents and guarantees all predefined agents exist without losing edits.
pub(super) fn reconcile_agents(loaded: Vec<AgentProfileDto>) -> (Vec<AgentProfileDto>, bool) {
    let defaults = default_agents();
    let mut changed = false;
    let mut out: Vec<AgentProfileDto> = Vec::new();
    let mut seen = BTreeSet::new();

    for mut item in loaded {
        let orig_id = item.id.trim().to_string();
        if let Some(canon) = canonical_predefined_id(&orig_id) {
            if canon != orig_id {
                changed = true;
            }
            if !seen.insert(canon.to_string()) {
                changed = true;
                continue;
            }
            if let Some(def) = defaults.iter().find(|d| d.id == canon) {
                item.id = def.id.clone();
                if item.name.trim().is_empty() {
                    item.name = def.name.clone();
                    changed = true;
                }
                if item.role.trim().is_empty() {
                    item.role = def.role.clone();
                    changed = true;
                }
                if item.model.trim().is_empty() || item.model == "miku-local" {
                    item.model = def.model.clone();
                    changed = true;
                }
                if item.c.trim().is_empty() {
                    item.c = def.c.clone();
                }
                if item.outfit.trim().is_empty()
                    || matches!(item.outfit.as_str(), "cyber" | "street" | "office" | "mecha")
                {
                    item.outfit = def.outfit.clone();
                    changed = true;
                }
                if item.prompt.trim().is_empty() {
                    item.prompt = def.prompt.clone();
                }
                if item.tools_mode.trim().is_empty() {
                    item.tools_mode = def.tools_mode.clone();
                }
                item.predefined = true;
                if item.harness_id.is_none() {
                    item.harness_id = def.harness_id.clone();
                }
                out.push(item);
            }
        } else if !orig_id.is_empty() && !item.name.trim().is_empty() && seen.insert(orig_id) {
            out.push(item);
        } else {
            changed = true;
        }
    }

    for def in &defaults {
        if !seen.contains(&def.id) {
            seen.insert(def.id.clone());
            out.push(def.clone());
            changed = true;
        }
    }

    if out.iter().all(|a| a.isl == 0) {
        for (slot_idx, id) in ["miku", "antigravity", "claude-code"].iter().enumerate() {
            if let Some(a) = out.iter_mut().find(|x| x.id == *id) {
                a.isl = (slot_idx + 1) as u8;
                a.active = true;
                changed = true;
            }
        }
    }

    (out, changed)
}

pub(super) fn discover_custom_agents(agents_dir: &Path, agents: &mut Vec<AgentProfileDto>) {
    let base_reg = ToolRegistry::with_default_internal_tools();
    if let Ok(rd) = fs::read_dir(agents_dir) {
        for entry in rd.flatten() {
            let p = entry.path();
            if p.is_dir() && p.join(CUSTOM_AGENT_MANIFEST_FILENAME).exists() {
                if let Ok(rep) = audit_custom_agent_soul(&p, &base_reg, 1_024) {
                    if !agents.iter().any(|a| a.id == rep.agent_id) {
                        agents.push(AgentProfileDto {
                            id: rep.agent_id.clone(),
                            name: rep.display_name,
                            role: format!("Custom ({} tok)", rep.soul_tokens),
                            model: "custom-agent".to_string(),
                            provider: String::new(),
                            c: "#38bdf8".to_string(),
                            outfit: "phones".to_string(),
                            prompt: String::new(),
                            active: true,
                            isl: 0,
                            tools_mode: "auto".to_string(),
                            predefined: false,
                            harness_id: None,
                        });
                    }
                }
            }
        }
    }
}
