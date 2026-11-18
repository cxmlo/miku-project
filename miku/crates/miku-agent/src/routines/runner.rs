use std::collections::BTreeMap;
use std::fs;
use std::path::Path;

use miku_browser::{BrowserBackend, BrowserController, BrowserWorkflowOutcome, DownloadRequest};
use miku_gatekeeper::GatekeeperEngine;
use miku_platform::{PcControlBackend, PcControlController, PcControlOutcome};
use serde::{Deserialize, Serialize};

use crate::routines::schema::{
    substitute_params, RoutineManifest, RoutinePrecondition, RoutineStepAction,
};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RoutineLlmUsageTelemetry {
    pub llm_calls_made: u64,
    pub prompt_tokens_used: u64,
    pub completion_tokens_used: u64,
}

impl RoutineLlmUsageTelemetry {
    pub const fn zero_tokens() -> Self {
        Self {
            llm_calls_made: 0,
            prompt_tokens_used: 0,
            completion_tokens_used: 0,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RoutineHaltReason {
    CompletedAllSteps,
    PreconditionFailed { step_id: String, detail: String },
    GatekeeperBlocked { step_id: String, detail: String },
    StepExecutionFailed { step_id: String, detail: String },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RoutineExecutionReport {
    pub routine_name: String,
    pub steps_executed: usize,
    pub total_steps: usize,
    pub succeeded: bool,
    pub halt_reason: RoutineHaltReason,
    pub telemetry: RoutineLlmUsageTelemetry,
    pub step_outputs: Vec<String>,
}

pub fn record_routine_to_toml(manifest: &RoutineManifest) -> Result<String, String> {
    if manifest.name.trim().is_empty() {
        return Err("routine name cannot be empty".to_string());
    }
    if manifest.steps.is_empty() {
        return Err("routine must contain at least one step".to_string());
    }
    toml::to_string_pretty(manifest).map_err(|e| format!("serialize .routine.toml failed: {e}"))
}

pub fn save_routine_toml(path: &Path, manifest: &RoutineManifest) -> Result<(), String> {
    let raw = record_routine_to_toml(manifest)?;
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).map_err(|e| e.to_string())?;
    }
    let tmp = path.with_extension("routine.toml.tmp");
    fs::write(&tmp, raw).map_err(|e| e.to_string())?;
    fs::rename(&tmp, path).map_err(|e| e.to_string())?;
    Ok(())
}

pub fn load_routine_toml(path: &Path) -> Result<RoutineManifest, String> {
    let raw = fs::read_to_string(path).map_err(|e| e.to_string())?;
    let manifest: RoutineManifest =
        toml::from_str(&raw).map_err(|e| format!("parse .routine.toml failed: {e}"))?;
    if manifest.steps.is_empty() {
        return Err("routine contains zero steps".to_string());
    }
    Ok(manifest)
}

pub struct RoutineExecutor;

impl RoutineExecutor {
    fn verify_precondition(
        pre: &RoutinePrecondition,
        runtime_params: &BTreeMap<String, String>,
        defaults: &BTreeMap<String, String>,
    ) -> Result<(), String> {
        match pre {
            RoutinePrecondition::FileExists { path } => {
                let resolved = substitute_params(path, runtime_params, defaults)?;
                if !Path::new(&resolved).is_file() {
                    return Err(format!("required file '{resolved}' missing"));
                }
                Ok(())
            }
            RoutinePrecondition::DirExists { path } => {
                let resolved = substitute_params(path, runtime_params, defaults)?;
                if !Path::new(&resolved).is_dir() {
                    return Err(format!("required dir '{resolved}' missing"));
                }
                Ok(())
            }
            RoutinePrecondition::ParameterPresent { key } => {
                if runtime_params.contains_key(key) || defaults.contains_key(key) {
                    Ok(())
                } else {
                    Err(format!("required parameter '{key}' missing"))
                }
            }
        }
    }

    pub async fn run_deterministic<P: PcControlBackend, B: BrowserBackend>(
        manifest: &RoutineManifest,
        agent: &str,
        runtime_params: &BTreeMap<String, String>,
        pc: &mut PcControlController<P>,
        browser: &mut BrowserController<B>,
        gatekeeper: &mut GatekeeperEngine,
        now_ms: u64,
    ) -> RoutineExecutionReport {
        let mut outputs = Vec::new();
        let total = manifest.steps.len();

        for (idx, step) in manifest.steps.iter().enumerate() {
            for pre in &step.preconditions {
                if let Err(detail) = Self::verify_precondition(pre, runtime_params, &manifest.parameters) {
                    return RoutineExecutionReport {
                        routine_name: manifest.name.clone(),
                        steps_executed: idx,
                        total_steps: total,
                        succeeded: false,
                        halt_reason: RoutineHaltReason::PreconditionFailed {
                            step_id: step.id.clone(),
                            detail,
                        },
                        telemetry: RoutineLlmUsageTelemetry::zero_tokens(),
                        step_outputs: outputs,
                    };
                }
            }

            let step_now = now_ms.saturating_add(idx as u64);
            match &step.action {
                RoutineStepAction::BrowserNavigate {
                    url,
                    download_filename,
                } => {
                    let resolved_url = match substitute_params(url, runtime_params, &manifest.parameters) {
                        Ok(u) => u,
                        Err(detail) => {
                            return RoutineExecutionReport {
                                routine_name: manifest.name.clone(),
                                steps_executed: idx,
                                total_steps: total,
                                succeeded: false,
                                halt_reason: RoutineHaltReason::PreconditionFailed {
                                    step_id: step.id.clone(),
                                    detail,
                                },
                                telemetry: RoutineLlmUsageTelemetry::zero_tokens(),
                                step_outputs: outputs,
                            };
                        }
                    };
                    let dl = match download_filename {
                        Some(f) => match substitute_params(f, runtime_params, &manifest.parameters) {
                            Ok(resolved_f) => Some(DownloadRequest {
                                suggested_filename: resolved_f,
                            }),
                            Err(detail) => {
                                return RoutineExecutionReport {
                                    routine_name: manifest.name.clone(),
                                    steps_executed: idx,
                                    total_steps: total,
                                    succeeded: false,
                                    halt_reason: RoutineHaltReason::PreconditionFailed {
                                        step_id: step.id.clone(),
                                        detail,
                                    },
                                    telemetry: RoutineLlmUsageTelemetry::zero_tokens(),
                                    step_outputs: outputs,
                                };
                            }
                        },
                        None => None,
                    };
                    match browser
                        .browse_and_optional_download(agent, &resolved_url, dl, gatekeeper, step_now)
                        .await
                    {
                        Ok(BrowserWorkflowOutcome::Completed(card)) => {
                            outputs.push(format!("browser:{}:{}", card.domain, card.page_title));
                        }
                        Ok(BrowserWorkflowOutcome::AwaitingApproval { ticket }) => {
                            return RoutineExecutionReport {
                                routine_name: manifest.name.clone(),
                                steps_executed: idx,
                                total_steps: total,
                                succeeded: false,
                                halt_reason: RoutineHaltReason::GatekeeperBlocked {
                                    step_id: step.id.clone(),
                                    detail: format!("browser step requires Ask approval ({})", ticket.req_hash),
                                },
                                telemetry: RoutineLlmUsageTelemetry::zero_tokens(),
                                step_outputs: outputs,
                            };
                        }
                        Ok(BrowserWorkflowOutcome::Denied { reason_code, detail }) => {
                            return RoutineExecutionReport {
                                routine_name: manifest.name.clone(),
                                steps_executed: idx,
                                total_steps: total,
                                succeeded: false,
                                halt_reason: RoutineHaltReason::GatekeeperBlocked {
                                    step_id: step.id.clone(),
                                    detail: format!("{reason_code}: {detail}"),
                                },
                                telemetry: RoutineLlmUsageTelemetry::zero_tokens(),
                                step_outputs: outputs,
                            };
                        }
                        Err(e) => {
                            return RoutineExecutionReport {
                                routine_name: manifest.name.clone(),
                                steps_executed: idx,
                                total_steps: total,
                                succeeded: false,
                                halt_reason: RoutineHaltReason::StepExecutionFailed {
                                    step_id: step.id.clone(),
                                    detail: e.to_string(),
                                },
                                telemetry: RoutineLlmUsageTelemetry::zero_tokens(),
                                step_outputs: outputs,
                            };
                        }
                    }
                }
                _ => {
                    let action = match step
                        .action
                        .to_pc_action(runtime_params, &manifest.parameters)
                    {
                        Ok(Some(act)) => act,
                        Ok(None) => continue,
                        Err(detail) => {
                            return RoutineExecutionReport {
                                routine_name: manifest.name.clone(),
                                steps_executed: idx,
                                total_steps: total,
                                succeeded: false,
                                halt_reason: RoutineHaltReason::PreconditionFailed {
                                    step_id: step.id.clone(),
                                    detail,
                                },
                                telemetry: RoutineLlmUsageTelemetry::zero_tokens(),
                                step_outputs: outputs,
                            };
                        }
                    };
                    match pc.execute_action(agent, action, step_now, gatekeeper) {
                        Ok(PcControlOutcome::Executed(res)) => {
                            outputs.push(format!("{}:{}", res.capability, res.stdout_or_value));
                        }
                        Ok(PcControlOutcome::AwaitingApproval { ticket }) => {
                            return RoutineExecutionReport {
                                routine_name: manifest.name.clone(),
                                steps_executed: idx,
                                total_steps: total,
                                succeeded: false,
                                halt_reason: RoutineHaltReason::GatekeeperBlocked {
                                    step_id: step.id.clone(),
                                    detail: format!("pc step requires Ask approval ({})", ticket.req_hash),
                                },
                                telemetry: RoutineLlmUsageTelemetry::zero_tokens(),
                                step_outputs: outputs,
                            };
                        }
                        Ok(PcControlOutcome::Denied { reason_code, detail }) => {
                            return RoutineExecutionReport {
                                routine_name: manifest.name.clone(),
                                steps_executed: idx,
                                total_steps: total,
                                succeeded: false,
                                halt_reason: RoutineHaltReason::GatekeeperBlocked {
                                    step_id: step.id.clone(),
                                    detail: format!("{reason_code}: {detail}"),
                                },
                                telemetry: RoutineLlmUsageTelemetry::zero_tokens(),
                                step_outputs: outputs,
                            };
                        }
                        Err(e) => {
                            return RoutineExecutionReport {
                                routine_name: manifest.name.clone(),
                                steps_executed: idx,
                                total_steps: total,
                                succeeded: false,
                                halt_reason: RoutineHaltReason::StepExecutionFailed {
                                    step_id: step.id.clone(),
                                    detail: e.to_string(),
                                },
                                telemetry: RoutineLlmUsageTelemetry::zero_tokens(),
                                step_outputs: outputs,
                            };
                        }
                    }
                }
            }
        }

        RoutineExecutionReport {
            routine_name: manifest.name.clone(),
            steps_executed: total,
            total_steps: total,
            succeeded: true,
            halt_reason: RoutineHaltReason::CompletedAllSteps,
            telemetry: RoutineLlmUsageTelemetry::zero_tokens(),
            step_outputs: outputs,
        }
    }
}
