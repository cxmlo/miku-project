use std::collections::BTreeMap;

use miku_platform::PcControlAction;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum RoutinePrecondition {
    FileExists { path: String },
    DirExists { path: String },
    ParameterPresent { key: String },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum RoutineStepAction {
    FsRead {
        path: String,
    },
    FsWrite {
        path: String,
        content: String,
        #[serde(default = "default_true")]
        create_dirs: bool,
    },
    ShellCommand {
        program: String,
        #[serde(default)]
        args: Vec<String>,
        cwd: Option<String>,
        #[serde(default = "default_shell_timeout")]
        timeout_ms: u64,
    },
    BrowserNavigate {
        url: String,
        download_filename: Option<String>,
    },
}

fn default_true() -> bool {
    true
}

fn default_shell_timeout() -> u64 {
    10_000
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RoutineStep {
    pub id: String,
    pub description: String,
    #[serde(default)]
    pub preconditions: Vec<RoutinePrecondition>,
    pub action: RoutineStepAction,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RoutineManifest {
    pub name: String,
    pub version: u32,
    #[serde(default)]
    pub parameters: BTreeMap<String, String>,
    pub steps: Vec<RoutineStep>,
}

pub fn substitute_params(
    template: &str,
    runtime_params: &BTreeMap<String, String>,
    defaults: &BTreeMap<String, String>,
) -> Result<String, String> {
    let mut out = template.to_string();
    while let Some(start) = out.find("{{") {
        let Some(rel_end) = out[start + 2..].find("}}") else {
            return Err(format!("unclosed template parameter in '{template}'"));
        };
        let key = out[start + 2..start + 2 + rel_end].trim();
        let val = runtime_params
            .get(key)
            .or_else(|| defaults.get(key))
            .ok_or_else(|| format!("missing routine parameter '{key}'"))?;
        out.replace_range(start..start + 2 + rel_end + 2, val);
    }
    Ok(out)
}

impl RoutineStepAction {
    pub fn to_pc_action(
        &self,
        runtime_params: &BTreeMap<String, String>,
        defaults: &BTreeMap<String, String>,
    ) -> Result<Option<PcControlAction>, String> {
        match self {
            RoutineStepAction::FsRead { path } => Ok(Some(PcControlAction::FsRead {
                path: substitute_params(path, runtime_params, defaults)?,
            })),
            RoutineStepAction::FsWrite {
                path,
                content,
                create_dirs,
            } => Ok(Some(PcControlAction::FsWrite {
                path: substitute_params(path, runtime_params, defaults)?,
                content: substitute_params(content, runtime_params, defaults)?,
                create_dirs: *create_dirs,
            })),
            RoutineStepAction::ShellCommand {
                program,
                args,
                cwd,
                timeout_ms,
            } => {
                let mut resolved_args = Vec::with_capacity(args.len());
                for a in args {
                    resolved_args.push(substitute_params(a, runtime_params, defaults)?);
                }
                let resolved_cwd = match cwd {
                    Some(c) => Some(substitute_params(c, runtime_params, defaults)?),
                    None => None,
                };
                Ok(Some(PcControlAction::ShellCommand {
                    program: substitute_params(program, runtime_params, defaults)?,
                    args: resolved_args,
                    cwd: resolved_cwd,
                    timeout_ms: *timeout_ms,
                }))
            }
            RoutineStepAction::BrowserNavigate { .. } => Ok(None),
        }
    }
}
