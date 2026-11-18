use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FocusPhaseKind {
    Idle,
    Focus,
    ShortBreak,
    LongBreak,
    Paused,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FocusPomodoroConfig {
    pub focus_duration_ms: u64,
    pub short_break_ms: u64,
    pub long_break_ms: u64,
    pub cycles_before_long_break: u32,
}

impl Default for FocusPomodoroConfig {
    fn default() -> Self {
        Self {
            focus_duration_ms: 25 * 60 * 1_000,
            short_break_ms: 5 * 60 * 1_000,
            long_break_ms: 15 * 60 * 1_000,
            cycles_before_long_break: 4,
        }
    }
}

/// Exact next wakeup plan without any 1 Hz polling tick when Island is collapsed.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct SleepUntilPlan {
    pub target_wall_ms: u64,
    pub sleep_duration_ms: u64,
    pub periodic_ticks_per_second: u8,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FocusTimerSnapshot {
    pub phase: FocusPhaseKind,
    pub remaining_ms: u64,
    pub completed_focus_blocks: u32,
    pub total_focused_ms_today: u64,
    pub periodic_ticks_per_second: u8,
    pub estimated_idle_cpu_pct: &'static str,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FocusCycleOutcome {
    pub phase_before: FocusPhaseKind,
    pub phase_after: FocusPhaseKind,
    pub focus_block_completed: bool,
    pub focused_ms_added: u64,
    pub resumed_from_sleep: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FocusPomodoroScheduler {
    config: FocusPomodoroConfig,
    phase: FocusPhaseKind,
    phase_before_pause: Option<FocusPhaseKind>,
    deadline_wall_ms: Option<u64>,
    last_monotonic_ms: Option<u64>,
    last_wall_ms: Option<u64>,
    paused_remaining_ms: u64,
    completed_focus_blocks: u32,
    total_focused_ms_today: u64,
    linked_task_id: Option<String>,
    island_collapsed: bool,
    scheduled_wakeups_count: u64,
}

impl FocusPomodoroScheduler {
    pub fn new(config: FocusPomodoroConfig) -> Self {
        Self {
            config,
            phase: FocusPhaseKind::Idle,
            phase_before_pause: None,
            deadline_wall_ms: None,
            last_monotonic_ms: None,
            last_wall_ms: None,
            paused_remaining_ms: 0,
            completed_focus_blocks: 0,
            total_focused_ms_today: 0,
            linked_task_id: None,
            island_collapsed: true,
            scheduled_wakeups_count: 0,
        }
    }

    pub fn set_island_collapsed(&mut self, collapsed: bool) {
        self.island_collapsed = collapsed;
    }

    pub fn is_island_collapsed(&self) -> bool {
        self.island_collapsed
    }

    pub fn scheduled_wakeups_count(&self) -> u64 {
        self.scheduled_wakeups_count
    }

    pub fn phase(&self) -> FocusPhaseKind {
        self.phase
    }

    pub fn start_focus(
        &mut self,
        wall_now_ms: u64,
        monotonic_now_ms: u64,
        linked_task_id: Option<String>,
    ) -> SleepUntilPlan {
        self.phase = FocusPhaseKind::Focus;
        self.phase_before_pause = None;
        self.linked_task_id = linked_task_id;
        let deadline = wall_now_ms.saturating_add(self.config.focus_duration_ms);
        self.deadline_wall_ms = Some(deadline);
        self.last_wall_ms = Some(wall_now_ms);
        self.last_monotonic_ms = Some(monotonic_now_ms);
        self.scheduled_wakeups_count = self.scheduled_wakeups_count.saturating_add(1);
        SleepUntilPlan {
            target_wall_ms: deadline,
            sleep_duration_ms: self.config.focus_duration_ms,
            periodic_ticks_per_second: 0,
        }
    }

    pub fn pause(&mut self, wall_now_ms: u64) {
        if !matches!(
            self.phase,
            FocusPhaseKind::Focus | FocusPhaseKind::ShortBreak | FocusPhaseKind::LongBreak
        ) {
            return;
        }
        let rem = self
            .deadline_wall_ms
            .map(|d| d.saturating_sub(wall_now_ms))
            .unwrap_or(0);
        if self.phase == FocusPhaseKind::Focus {
            if let Some(prev) = self.last_wall_ms {
                let elapsed = wall_now_ms.saturating_sub(prev).min(self.config.focus_duration_ms);
                self.total_focused_ms_today = self.total_focused_ms_today.saturating_add(elapsed);
            }
        }
        self.phase_before_pause = Some(self.phase);
        self.phase = FocusPhaseKind::Paused;
        self.paused_remaining_ms = rem;
        self.deadline_wall_ms = None;
    }

    pub fn resume(&mut self, wall_now_ms: u64, monotonic_now_ms: u64) -> Option<SleepUntilPlan> {
        if self.phase != FocusPhaseKind::Paused {
            return None;
        }
        let prev_phase = self.phase_before_pause.take().unwrap_or(FocusPhaseKind::Focus);
        self.phase = prev_phase;
        let deadline = wall_now_ms.saturating_add(self.paused_remaining_ms);
        self.deadline_wall_ms = Some(deadline);
        self.last_wall_ms = Some(wall_now_ms);
        self.last_monotonic_ms = Some(monotonic_now_ms);
        self.scheduled_wakeups_count = self.scheduled_wakeups_count.saturating_add(1);
        Some(SleepUntilPlan {
            target_wall_ms: deadline,
            sleep_duration_ms: self.paused_remaining_ms,
            periodic_ticks_per_second: 0,
        })
    }

    pub fn stop(&mut self) {
        self.phase = FocusPhaseKind::Idle;
        self.phase_before_pause = None;
        self.deadline_wall_ms = None;
        self.paused_remaining_ms = 0;
        self.linked_task_id = None;
    }

    /// Computes a single sleep_until plan straight to the next state change.
    /// Guarantees ZERO periodic ticks per second while Island is collapsed (and even when idle).
    pub fn next_sleep_until_plan(&self, wall_now_ms: u64) -> Option<SleepUntilPlan> {
        let deadline = self.deadline_wall_ms?;
        match self.phase {
            FocusPhaseKind::Idle | FocusPhaseKind::Paused => None,
            FocusPhaseKind::Focus | FocusPhaseKind::ShortBreak | FocusPhaseKind::LongBreak => {
                Some(SleepUntilPlan {
                    target_wall_ms: deadline,
                    sleep_duration_ms: deadline.saturating_sub(wall_now_ms),
                    periodic_ticks_per_second: 0,
                })
            }
        }
    }

    /// Reconciles timer state when either deadline hits OR Windows wakes from S3/Modern Standby.
    pub fn reconcile_at_wall_and_monotonic(
        &mut self,
        wall_now_ms: u64,
        monotonic_now_ms: u64,
    ) -> FocusCycleOutcome {
        let phase_before = self.phase;
        let wall_delta = self
            .last_wall_ms
            .map(|w| wall_now_ms.saturating_sub(w))
            .unwrap_or(0);
        let mono_delta = self
            .last_monotonic_ms
            .map(|m| monotonic_now_ms.saturating_sub(m))
            .unwrap_or(0);
        let resumed_from_sleep = wall_delta > mono_delta.saturating_add(5_000);

        self.last_wall_ms = Some(wall_now_ms);
        self.last_monotonic_ms = Some(monotonic_now_ms);

        let Some(deadline) = self.deadline_wall_ms else {
            return FocusCycleOutcome {
                phase_before,
                phase_after: self.phase,
                focus_block_completed: false,
                focused_ms_added: 0,
                resumed_from_sleep,
            };
        };

        if wall_now_ms < deadline {
            return FocusCycleOutcome {
                phase_before,
                phase_after: self.phase,
                focus_block_completed: false,
                focused_ms_added: 0,
                resumed_from_sleep,
            };
        }

        match self.phase {
            FocusPhaseKind::Focus => {
                self.completed_focus_blocks = self.completed_focus_blocks.saturating_add(1);
                let added = self.config.focus_duration_ms;
                self.total_focused_ms_today = self.total_focused_ms_today.saturating_add(added);

                // If Windows slept way past the break too, return safely to Idle rather than chaining stale alarms
                let long_break_turn = self.config.cycles_before_long_break > 0
                    && self
                        .completed_focus_blocks
                        .is_multiple_of(self.config.cycles_before_long_break);
                let next_break_dur = if long_break_turn {
                    self.config.long_break_ms
                } else {
                    self.config.short_break_ms
                };
                if wall_now_ms.saturating_sub(deadline) >= next_break_dur {
                    self.phase = FocusPhaseKind::Idle;
                    self.deadline_wall_ms = None;
                } else {
                    self.phase = if long_break_turn {
                        FocusPhaseKind::LongBreak
                    } else {
                        FocusPhaseKind::ShortBreak
                    };
                    self.deadline_wall_ms = Some(deadline.saturating_add(next_break_dur));
                }
                FocusCycleOutcome {
                    phase_before,
                    phase_after: self.phase,
                    focus_block_completed: true,
                    focused_ms_added: added,
                    resumed_from_sleep,
                }
            }
            FocusPhaseKind::ShortBreak | FocusPhaseKind::LongBreak => {
                self.phase = FocusPhaseKind::Idle;
                self.deadline_wall_ms = None;
                FocusCycleOutcome {
                    phase_before,
                    phase_after: FocusPhaseKind::Idle,
                    focus_block_completed: false,
                    focused_ms_added: 0,
                    resumed_from_sleep,
                }
            }
            FocusPhaseKind::Idle | FocusPhaseKind::Paused => FocusCycleOutcome {
                phase_before,
                phase_after: self.phase,
                focus_block_completed: false,
                focused_ms_added: 0,
                resumed_from_sleep,
            },
        }
    }

    pub fn snapshot(&self, wall_now_ms: u64) -> FocusTimerSnapshot {
        let remaining_ms = match self.phase {
            FocusPhaseKind::Idle => 0,
            FocusPhaseKind::Paused => self.paused_remaining_ms,
            FocusPhaseKind::Focus | FocusPhaseKind::ShortBreak | FocusPhaseKind::LongBreak => {
                self.deadline_wall_ms.map(|d| d.saturating_sub(wall_now_ms)).unwrap_or(0)
            }
        };
        FocusTimerSnapshot {
            phase: self.phase,
            remaining_ms,
            completed_focus_blocks: self.completed_focus_blocks,
            total_focused_ms_today: self.total_focused_ms_today,
            periodic_ticks_per_second: 0,
            estimated_idle_cpu_pct: "0.0%",
        }
    }
}
