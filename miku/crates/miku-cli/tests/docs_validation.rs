//! Live Documentation Validation Suite (Step 5.5)
//!
//! Validates two invariants across root markdown files and `docs/**/*.md`:
//! 1. Every internal relative link `[text](path)` resolves to an existing file on disk
//!    (excluding `http(s)://`, `mailto:`, and same-page `#anchor` links).
//! 2. Every tagged example block (`miku-toml`, `skill-toml`, `agent-toml`, `mcp-servers`,
//!    `miku-link`) parses cleanly through Miku's production deserializers and validators.

use std::env;
use std::fs;
use std::path::{Path, PathBuf};

use miku_agent::{CustomAgentDescriptor, ToolRegistry};
use miku_cli::parse_and_validate_miku_toml;
use miku_gatekeeper::{Capability, GatekeeperEngine, PolicyEffect, ProjectProfile};
use miku_mcp::parse_mcp_servers_from_toml;
use miku_protocol::MikuLinkEnvelopeV1;
use miku_skills::SkillEngine;

fn repo_root() -> PathBuf {
    // CARGO_MANIFEST_DIR is `<repo>/miku/crates/miku-cli`
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .expect("crates dir")
        .parent()
        .expect("miku workspace dir")
        .parent()
        .expect("repository root")
        .to_path_buf()
}

fn collect_markdown_files(root: &Path) -> Vec<PathBuf> {
    let mut files = Vec::new();
    for top in ["README.md", "CONTRIBUTING.md", "SECURITY.md"] {
        let p = root.join(top);
        if p.exists() {
            files.push(p);
        }
    }
    collect_docs_recursive(&root.join("docs"), &mut files);
    files.sort();
    files
}

fn collect_docs_recursive(dir: &Path, out: &mut Vec<PathBuf>) {
    let Ok(entries) = fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            collect_docs_recursive(&path, out);
        } else if path.extension().and_then(|e| e.to_str()) == Some("md") {
            out.push(path);
        }
    }
}

#[derive(Debug)]
struct FencedBlock {
    file: PathBuf,
    start_line: usize,
    tag: String,
    body: String,
}

fn extract_markdown_elements(
    md_path: &Path,
    content: &str,
    links_out: &mut Vec<(PathBuf, usize, String)>,
    blocks_out: &mut Vec<FencedBlock>,
) {
    let mut in_fence = false;
    let mut fence_tag = String::new();
    let mut fence_start = 0usize;
    let mut fence_body = String::new();

    for (idx, raw_line) in content.lines().enumerate() {
        let line_no = idx + 1;
        let trimmed = raw_line.trim();
        if let Some(after_ticks) = trimmed.strip_prefix("```") {
            if !in_fence {
                in_fence = true;
                fence_tag = after_ticks.trim().to_string();
                fence_start = line_no;
                fence_body.clear();
            } else {
                in_fence = false;
                blocks_out.push(FencedBlock {
                    file: md_path.to_path_buf(),
                    start_line: fence_start,
                    tag: std::mem::take(&mut fence_tag),
                    body: std::mem::take(&mut fence_body),
                });
            }
            continue;
        }

        if in_fence {
            fence_body.push_str(raw_line);
            fence_body.push('\n');
            continue;
        }

        extract_links_on_line(raw_line, md_path, line_no, links_out);
    }
}

fn extract_links_on_line(
    line: &str,
    md_path: &Path,
    line_no: usize,
    links_out: &mut Vec<(PathBuf, usize, String)>,
) {
    // Strip inline code spans (`...`) so literal code syntax isn't treated as a link.
    let mut scrubbed = String::with_capacity(line.len());
    let mut in_code = false;
    for ch in line.chars() {
        if ch == '`' {
            in_code = !in_code;
        } else if !in_code {
            scrubbed.push(ch);
        }
    }

    let bytes = scrubbed.as_bytes();
    let mut i = 0usize;
    while i < bytes.len() {
        if bytes[i] == b'[' {
            if let Some(close_bracket) = scrubbed[i + 1..].find(']') {
                let after = i + 1 + close_bracket + 1;
                if after < bytes.len() && bytes[after] == b'(' {
                    if let Some(close_paren) = scrubbed[after + 1..].find(')') {
                        let target = scrubbed[after + 1..after + 1 + close_paren].trim();
                        if !target.is_empty() {
                            links_out.push((md_path.to_path_buf(), line_no, target.to_string()));
                        }
                        i = after + 1 + close_paren + 1;
                        continue;
                    }
                }
            }
        }
        i += 1;
    }
}

// LEDGER-5.5-1: Every internal link in README.md, CONTRIBUTING.md, SECURITY.md & docs/**/*.md resolves
#[test]
fn test_5_5_docs_internal_markdown_links_all_resolve() {
    let root = repo_root();
    let md_files = collect_markdown_files(&root);
    assert!(
        md_files.len() >= 10,
        "expected full docs tree (got {} files)",
        md_files.len()
    );

    let mut all_links = Vec::new();
    let mut dummy_blocks = Vec::new();
    for file in &md_files {
        let raw = fs::read_to_string(file)
            .unwrap_or_else(|e| panic!("failed reading '{}': {e}", file.display()));
        extract_markdown_elements(file, &raw, &mut all_links, &mut dummy_blocks);
    }

    let mut checked_internal = 0usize;
    for (source_file, line_no, raw_target) in all_links {
        let lower = raw_target.to_ascii_lowercase();
        if lower.starts_with("http://")
            || lower.starts_with("https://")
            || lower.starts_with("mailto:")
            || raw_target.starts_with('#')
        {
            continue;
        }

        // Strip optional markdown title (`path "title"`) and `#anchor`
        let target_no_title = raw_target.split_whitespace().next().unwrap_or("");
        let target_path_part = target_no_title.split('#').next().unwrap_or("");
        if target_path_part.is_empty() {
            continue;
        }

        let resolved = if let Some(abs_part) = target_path_part.strip_prefix("file:///") {
            PathBuf::from(abs_part)
        } else {
            source_file
                .parent()
                .expect("md parent dir")
                .join(target_path_part)
        };

        assert!(
            resolved.exists(),
            "Broken markdown link at {}:{}: '{}' resolved to missing path '{}'",
            source_file.display(),
            line_no,
            raw_target,
            resolved.display()
        );
        checked_internal += 1;
    }

    assert!(
        checked_internal >= 15,
        "expected >= 15 verified internal markdown links, got {}",
        checked_internal
    );
}

// LEDGER-5.5-2: All TOML/JSON configuration examples across docs validate against real parsers
#[test]
fn test_5_5_docs_fenced_toml_and_json_examples_validate_live() {
    let root = repo_root();
    let md_files = collect_markdown_files(&root);

    let mut dummy_links = Vec::new();
    let mut blocks = Vec::new();
    for file in &md_files {
        let raw = fs::read_to_string(file)
            .unwrap_or_else(|e| panic!("failed reading '{}': {e}", file.display()));
        extract_markdown_elements(file, &raw, &mut dummy_links, &mut blocks);
    }

    let tmp_root = env::temp_dir().join(format!("miku_docs_validation_{}", std::process::id()));
    let _ = fs::remove_dir_all(&tmp_root);
    fs::create_dir_all(&tmp_root).expect("create docs validation tmp");

    let mut miku_toml_count = 0usize;
    let mut skill_toml_count = 0usize;
    let mut agent_toml_count = 0usize;
    let mut mcp_servers_count = 0usize;
    let mut miku_link_count = 0usize;

    for (idx, block) in blocks.iter().enumerate() {
        match block.tag.as_str() {
            "toml miku-toml" => {
                parse_and_validate_miku_toml(&block.body).unwrap_or_else(|e| {
                    panic!(
                        "Invalid `miku-toml` block at {}:{}: {e}",
                        block.file.display(),
                        block.start_line
                    )
                });
                miku_toml_count += 1;
            }
            "toml mcp-servers" => {
                let parsed = parse_mcp_servers_from_toml(&block.body).unwrap_or_else(|e| {
                    panic!(
                        "Invalid `mcp-servers` block at {}:{}: {e}",
                        block.file.display(),
                        block.start_line
                    )
                });
                assert!(
                    !parsed.is_empty(),
                    "Empty mcp_servers block at {}:{}",
                    block.file.display(),
                    block.start_line
                );
                mcp_servers_count += 1;
            }
            "toml skill-toml" => {
                let skill_dir = tmp_root.join(format!("skill_{idx}"));
                let scripts_dir = skill_dir.join("scripts");
                fs::create_dir_all(&scripts_dir).expect("create temp skill dir");
                fs::write(scripts_dir.join("run.ps1"), "# docs validation placeholder\n")
                    .expect("write skill entrypoint");

                let profile = ProjectProfile::new("docs-skill", &skill_dir.to_string_lossy(), 30_000)
                    .with_rule(Capability::FsRead, "**", PolicyEffect::Allow, false)
                    .with_rule(Capability::FsWrite, "**", PolicyEffect::Allow, false);
                let gk = GatekeeperEngine::new(profile).expect("gatekeeper");

                let mut engine = SkillEngine::new();
                engine
                    .load_or_reload_from_toml_str(&skill_dir, &block.body, &gk, 10_000)
                    .unwrap_or_else(|e| {
                        panic!(
                            "Invalid `skill-toml` block at {}:{}: {e}",
                            block.file.display(),
                            block.start_line
                        )
                    });
                skill_toml_count += 1;
            }
            "toml agent-toml" => {
                let agent_dir = tmp_root.join(format!("agent_{idx}"));
                fs::create_dir_all(&agent_dir).expect("create temp agent dir");
                fs::write(
                    agent_dir.join("SOUL.md"),
                    "# Reviewer Soul\nReview changes strictly within Gatekeeper boundaries.\n",
                )
                .expect("write SOUL.md");

                let registry = ToolRegistry::with_default_internal_tools();
                CustomAgentDescriptor::parse_and_validate(&block.body, &agent_dir, &registry)
                    .unwrap_or_else(|e| {
                        panic!(
                            "Invalid `agent-toml` block at {}:{}: {e}",
                            block.file.display(),
                            block.start_line
                        )
                    });
                agent_toml_count += 1;
            }
            "json miku-link" => {
                MikuLinkEnvelopeV1::decode_and_validate(&block.body).unwrap_or_else(|e| {
                    panic!(
                        "Invalid `miku-link` JSON block at {}:{}: {e}",
                        block.file.display(),
                        block.start_line
                    )
                });
                miku_link_count += 1;
            }
            _ => {}
        }
    }

    let _ = fs::remove_dir_all(&tmp_root);

    assert!(miku_toml_count >= 1, "must validate >= 1 `.miku.toml` doc block");
    assert!(skill_toml_count >= 1, "must validate >= 1 `skill.toml` doc block");
    assert!(agent_toml_count >= 1, "must validate >= 1 `agent.toml` doc block");
    assert!(mcp_servers_count >= 1, "must validate >= 1 `[mcp_servers]` doc block");
    assert!(miku_link_count >= 4, "must validate >= 4 `miku-link` JSON doc blocks");
}
