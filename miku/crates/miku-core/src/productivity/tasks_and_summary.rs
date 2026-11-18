use std::fs;
use std::path::Path;

use serde::{Deserialize, Serialize};

use crate::productivity::focus_timer::{FocusPomodoroConfig, FocusPomodoroScheduler, FocusTimerSnapshot};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum QuickTaskPriority {
    High,
    Normal,
    Low,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum QuickTaskStatus {
    Todo,
    InProgress,
    Done,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct QuickTaskItem {
    pub id: String,
    pub title: String,
    pub note: String,
    pub priority: QuickTaskPriority,
    pub status: QuickTaskStatus,
    pub created_at_ms: u64,
    pub completed_at_ms: Option<u64>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DailyActivitySummary {
    pub date_label: String,
    pub total_tasks: usize,
    pub completed_tasks: usize,
    pub pending_tasks: usize,
    pub completed_focus_blocks: u32,
    pub focused_minutes: u64,
    pub agent_runs_today: u32,
    pub highlight_titles: Vec<String>,
    pub markdown_digest: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProductivityStoreSnapshot {
    pub tasks: Vec<QuickTaskItem>,
    pub timer: FocusPomodoroScheduler,
    pub agent_runs_today: u32,
}

fn sanitize_single_line_text(raw: &str, max_len: usize) -> String {
    let cleaned: String = raw
        .chars()
        .filter(|c| !c.is_control() && *c != '<' && *c != '>')
        .collect();
    cleaned.trim().chars().take(max_len).collect()
}

pub struct ProductivityHub {
    tasks: Vec<QuickTaskItem>,
    scheduler: FocusPomodoroScheduler,
    agent_runs_today: u32,
    next_seq: u64,
}

impl ProductivityHub {
    pub fn new(config: FocusPomodoroConfig) -> Self {
        Self {
            tasks: Vec::new(),
            scheduler: FocusPomodoroScheduler::new(config),
            agent_runs_today: 0,
            next_seq: 1,
        }
    }

    pub fn scheduler(&self) -> &FocusPomodoroScheduler {
        &self.scheduler
    }

    pub fn scheduler_mut(&mut self) -> &mut FocusPomodoroScheduler {
        &mut self.scheduler
    }

    pub fn add_quick_task(
        &mut self,
        title: &str,
        note: &str,
        priority: QuickTaskPriority,
        now_ms: u64,
    ) -> Result<QuickTaskItem, String> {
        let safe_title = sanitize_single_line_text(title, 140);
        if safe_title.is_empty() {
            return Err("task title cannot be empty".to_string());
        }
        let safe_note = sanitize_single_line_text(note, 500);
        let id = format!("task-{:04}", self.next_seq);
        self.next_seq += 1;
        let item = QuickTaskItem {
            id,
            title: safe_title,
            note: safe_note,
            priority,
            status: QuickTaskStatus::Todo,
            created_at_ms: now_ms,
            completed_at_ms: None,
        };
        self.tasks.push(item.clone());
        Ok(item)
    }

    pub fn mark_task_done(&mut self, task_id: &str, now_ms: u64) -> bool {
        if let Some(t) = self.tasks.iter_mut().find(|t| t.id == task_id) {
            t.status = QuickTaskStatus::Done;
            t.completed_at_ms = Some(now_ms);
            true
        } else {
            false
        }
    }

    pub fn record_agent_run_completed(&mut self) {
        self.agent_runs_today = self.agent_runs_today.saturating_add(1);
    }

    pub fn list_tasks(&self) -> &[QuickTaskItem] {
        &self.tasks
    }

    pub fn build_daily_summary(&self, date_label: &str, wall_now_ms: u64) -> DailyActivitySummary {
        let timer_snap: FocusTimerSnapshot = self.scheduler.snapshot(wall_now_ms);
        let completed_tasks = self
            .tasks
            .iter()
            .filter(|t| t.status == QuickTaskStatus::Done)
            .count();
        let pending_tasks = self.tasks.len().saturating_sub(completed_tasks);
        let focused_minutes = timer_snap.total_focused_ms_today / 60_000;
        let highlight_titles: Vec<String> = self
            .tasks
            .iter()
            .filter(|t| t.status == QuickTaskStatus::Done)
            .take(5)
            .map(|t| t.title.clone())
            .collect();

        let markdown_digest = format!(
            "# Miku Daily Summary ({})\n- Focused: {} min ({} blocks)\n- Tasks: {}/{} done ({} pending)\n- Agent runs: {}",
            sanitize_single_line_text(date_label, 32),
            focused_minutes,
            timer_snap.completed_focus_blocks,
            completed_tasks,
            self.tasks.len(),
            pending_tasks,
            self.agent_runs_today
        );

        DailyActivitySummary {
            date_label: sanitize_single_line_text(date_label, 32),
            total_tasks: self.tasks.len(),
            completed_tasks,
            pending_tasks,
            completed_focus_blocks: timer_snap.completed_focus_blocks,
            focused_minutes,
            agent_runs_today: self.agent_runs_today,
            highlight_titles,
            markdown_digest,
        }
    }

    pub fn save_atomic(&self, path: &Path) -> Result<(), String> {
        let snap = ProductivityStoreSnapshot {
            tasks: self.tasks.clone(),
            timer: self.scheduler.clone(),
            agent_runs_today: self.agent_runs_today,
        };
        let bytes = serde_json::to_vec_pretty(&snap).map_err(|e| e.to_string())?;
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).map_err(|e| e.to_string())?;
        }
        let tmp = path.with_extension("json.tmp");
        fs::write(&tmp, &bytes).map_err(|e| e.to_string())?;
        fs::rename(&tmp, path).map_err(|e| e.to_string())?;
        Ok(())
    }

    pub fn load_from_disk(path: &Path) -> Result<Self, String> {
        let raw = fs::read_to_string(path).map_err(|e| e.to_string())?;
        let snap: ProductivityStoreSnapshot =
            serde_json::from_str(&raw).map_err(|e| e.to_string())?;
        let max_num = snap
            .tasks
            .iter()
            .filter_map(|t| t.id.strip_prefix("task-")?.parse::<u64>().ok())
            .max()
            .unwrap_or(0);
        Ok(Self {
            tasks: snap.tasks,
            scheduler: snap.timer,
            agent_runs_today: snap.agent_runs_today,
            next_seq: max_num + 1,
        })
    }
}
