//! Spike 0.1 (0.1a + 0.1b): Real child-process tree private memory (`PrivatePageCount`)
//! and 4 real UI lifecycle modes (`Win32` top-center pill + `WebView2` host).
//!
//! Strict Rules:
//! 1. Zero hardcoded RAM, CPU, or latency numbers exist in this library or binary.
//! 2. Step 0.1a measures the process tree rooted at the spawned child (`root_pid = child.id()`).
//!    It is labeled `"MEDIDO (referencia de proceso de prueba, sin veredicto de producto)"`.
//! 3. Step 0.1b measures the 4 real UI modes (`A`, `B`, `C`, `D`) using a Coordinator + Worker
//!    (`--worker --mode <A|B|C|D>`) architecture so each repetition spawns a clean worker
//!    process tree (`root_pid = child.id()`), runs a real Win32 message pump, and uses a
//!    per-repetition isolated User Data Directory (UDD) cleaned up after termination.
//! 4. Single latency threshold across step card, code, and decision rule:
//!    `hover -> visible p95 <= 150.0 ms`, idle private RAM `<= 30.0 MB`.

use std::collections::{HashMap, HashSet};

pub const REQUIRED_REPETITIONS: usize = 20;
pub const HOVER_TO_VISIBLE_P95_THRESHOLD_MS: f64 = 150.0;
pub const IDLE_PRIVATE_RAM_THRESHOLD_MB: f64 = 30.0;
pub const GRANDCHILD_POLL_INTERVAL_MS: u64 = 25;
pub const GRANDCHILD_POLL_TIMEOUT_MS: u64 = 3000;
pub const POST_KILL_SETTLE_DELAY_MS: u64 = 150;

pub const PILL_WIDTH_PX: i32 = 220;
pub const PILL_HEIGHT_PX: i32 = 36;
pub const EXPANDED_WIDTH_PX: i32 = 460;
pub const EXPANDED_HEIGHT_PX: i32 = 380;
pub const TOP_MARGIN_PX: i32 = 8;

pub const WS_POPUP_FLAG: u32 = 0x8000_0000;
pub const WS_VISIBLE_FLAG: u32 = 0x1000_0000;
pub const WS_EX_TOPMOST_FLAG: u32 = 0x0000_0008;
pub const WS_EX_TOOLWINDOW_FLAG: u32 = 0x0000_0080;
pub const WS_EX_NOACTIVATE_FLAG: u32 = 0x0800_0000;

pub const STEP_01A_REFERENCE_LABEL: &str =
    "MEDIDO (referencia de proceso de prueba, sin veredicto de producto)";

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct ProcessIdentity {
    pub pid: u32,
    pub creation_date: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProcessNode {
    pub pid: u32,
    pub parent_pid: u32,
    pub name: String,
    pub private_bytes: u64,
    pub creation_date: String,
}

impl ProcessNode {
    pub fn identity(&self) -> ProcessIdentity {
        ProcessIdentity {
            pid: self.pid,
            creation_date: self.creation_date.clone(),
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct TreeIterationSample {
    pub repetition: usize,
    pub root_pid: u32,
    pub tree_identities: Vec<ProcessIdentity>,
    pub process_count: usize,
    pub total_private_bytes: u64,
    pub elapsed_ms: f64,
    pub orphan_count_after_close: usize,
}

impl TreeIterationSample {
    pub fn total_private_mb(&self) -> f64 {
        bytes_to_megabytes(self.total_private_bytes)
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct TreeBenchmarkSummary {
    pub label: String,
    pub inspector_pid: u32,
    pub repetitions: usize,
    pub median_private_mb: f64,
    pub p95_private_mb: f64,
    pub median_elapsed_ms: f64,
    pub p95_elapsed_ms: f64,
    pub min_process_count: usize,
    pub max_orphan_count: usize,
    pub reference_status: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum UiLifecycleMode {
    ModeANativeOnly,
    ModeBWebViewAlwaysAlive,
    ModeCWebViewPrewarmedTtl,
    ModeDWebViewOnDemand,
}

impl UiLifecycleMode {
    pub fn all() -> [UiLifecycleMode; 4] {
        [
            UiLifecycleMode::ModeANativeOnly,
            UiLifecycleMode::ModeBWebViewAlwaysAlive,
            UiLifecycleMode::ModeCWebViewPrewarmedTtl,
            UiLifecycleMode::ModeDWebViewOnDemand,
        ]
    }

    pub fn code(&self) -> &'static str {
        match self {
            UiLifecycleMode::ModeANativeOnly => "A",
            UiLifecycleMode::ModeBWebViewAlwaysAlive => "B",
            UiLifecycleMode::ModeCWebViewPrewarmedTtl => "C",
            UiLifecycleMode::ModeDWebViewOnDemand => "D",
        }
    }

    pub fn from_code(code: &str) -> Option<UiLifecycleMode> {
        match code.trim().to_ascii_uppercase().as_str() {
            "A" => Some(UiLifecycleMode::ModeANativeOnly),
            "B" => Some(UiLifecycleMode::ModeBWebViewAlwaysAlive),
            "C" => Some(UiLifecycleMode::ModeCWebViewPrewarmedTtl),
            "D" => Some(UiLifecycleMode::ModeDWebViewOnDemand),
            _ => None,
        }
    }

    pub fn display_name(&self) -> &'static str {
        match self {
            UiLifecycleMode::ModeANativeOnly => "0.1b-A: Pastilla nativa Win32 (sin WebView2)",
            UiLifecycleMode::ModeBWebViewAlwaysAlive => {
                "0.1b-B: WebView2 siempre vivo (oculto en reposo)"
            }
            UiLifecycleMode::ModeCWebViewPrewarmedTtl => {
                "0.1b-C: WebView2 precalentado con TTL tras inactividad"
            }
            UiLifecycleMode::ModeDWebViewOnDemand => {
                "0.1b-D: WebView2 bajo demanda (crear al expandir, destruir al colapsar)"
            }
        }
    }

    pub fn requires_webview2(&self) -> bool {
        !matches!(self, UiLifecycleMode::ModeANativeOnly)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct IslandWindowGeometry {
    pub x: i32,
    pub y: i32,
    pub width: i32,
    pub height: i32,
    pub style_flags: u32,
    pub ex_style_flags: u32,
}

pub fn compute_island_geometry(
    monitor_width: i32,
    _monitor_height: i32,
    scale_factor: f64,
    expanded: bool,
) -> IslandWindowGeometry {
    let safe_scale = if scale_factor <= 0.1 { 1.0 } else { scale_factor };
    let logical_w = if expanded { EXPANDED_WIDTH_PX } else { PILL_WIDTH_PX };
    let logical_h = if expanded { EXPANDED_HEIGHT_PX } else { PILL_HEIGHT_PX };
    let width = ((logical_w as f64) * safe_scale).round() as i32;
    let height = ((logical_h as f64) * safe_scale).round() as i32;
    let y = ((TOP_MARGIN_PX as f64) * safe_scale).round() as i32;
    let x = ((monitor_width - width) / 2).max(0);

    IslandWindowGeometry {
        x,
        y,
        width,
        height,
        style_flags: WS_POPUP_FLAG | WS_VISIBLE_FLAG,
        ex_style_flags: WS_EX_TOPMOST_FLAG | WS_EX_TOOLWINDOW_FLAG | WS_EX_NOACTIVATE_FLAG,
    }
}

pub fn build_isolated_udd_path(
    base_local_app_data: &str,
    mode: UiLifecycleMode,
    repetition: usize,
    coordinator_pid: u32,
) -> String {
    let trimmed = base_local_app_data.trim_end_matches(['\\', '/']);
    format!(
        "{}\\miku\\spike-01b\\mode_{}_rep_{}_pid_{}",
        trimmed,
        mode.code(),
        repetition,
        coordinator_pid
    )
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ModeCTtlPhase {
    ColdIdle,
    ExpandedActive { last_activity_ms: u64 },
    WarmHiddenWaitingTtl { collapsed_at_ms: u64 },
    CollapsedColdAfterTtl,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ModeCTtlController {
    pub ttl_ms: u64,
    pub phase: ModeCTtlPhase,
    pub webview_alive: bool,
}

impl ModeCTtlController {
    pub fn new(ttl_ms: u64) -> Self {
        Self {
            ttl_ms,
            phase: ModeCTtlPhase::ColdIdle,
            webview_alive: false,
        }
    }

    pub fn on_hover_expand(&mut self, now_ms: u64) {
        self.webview_alive = true;
        self.phase = ModeCTtlPhase::ExpandedActive {
            last_activity_ms: now_ms,
        };
    }

    pub fn on_collapse_to_idle(&mut self, now_ms: u64) {
        if self.webview_alive {
            self.phase = ModeCTtlPhase::WarmHiddenWaitingTtl {
                collapsed_at_ms: now_ms,
            };
        } else {
            self.phase = ModeCTtlPhase::ColdIdle;
        }
    }

    /// Advances the clock and returns `true` if the TTL just expired and the WebView2
    /// instance must be destroyed immediately.
    pub fn poll_timer(&mut self, now_ms: u64) -> bool {
        if let ModeCTtlPhase::WarmHiddenWaitingTtl { collapsed_at_ms } = self.phase {
            if now_ms.saturating_sub(collapsed_at_ms) >= self.ttl_ms {
                self.webview_alive = false;
                self.phase = ModeCTtlPhase::CollapsedColdAfterTtl;
                return true;
            }
        }
        false
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct ModeIterationSample {
    pub mode: UiLifecycleMode,
    pub repetition: usize,
    pub worker_root_pid: u32,
    pub idle_tree_identities: Vec<ProcessIdentity>,
    pub expanded_tree_identities: Vec<ProcessIdentity>,
    pub idle_process_count: usize,
    pub expanded_process_count: usize,
    pub expanded_webview_child_count: usize,
    pub idle_private_bytes: u64,
    pub expanded_private_bytes: u64,
    pub hover_to_visible_ms: f64,
    pub post_collapse_private_bytes: u64,
    pub orphan_count_after_close: usize,
}

impl ModeIterationSample {
    pub fn idle_private_mb(&self) -> f64 {
        bytes_to_megabytes(self.idle_private_bytes)
    }

    pub fn expanded_private_mb(&self) -> f64 {
        bytes_to_megabytes(self.expanded_private_bytes)
    }

    pub fn post_collapse_private_mb(&self) -> f64 {
        bytes_to_megabytes(self.post_collapse_private_bytes)
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct ModeBenchmarkSummary {
    pub mode: UiLifecycleMode,
    pub repetitions: usize,
    pub idle_p50_mb: f64,
    pub idle_p95_mb: f64,
    pub expanded_p50_mb: f64,
    pub expanded_p95_mb: f64,
    pub post_collapse_p50_mb: f64,
    pub hover_p50_ms: f64,
    pub hover_p95_ms: f64,
    pub max_orphan_count: usize,
    pub meets_idle_ram_budget: bool,
    pub meets_latency_budget: bool,
    pub passes_all_budgets: bool,
    pub verdict_label: String,
}

#[derive(Debug, Clone, PartialEq)]
pub enum ModeBenchmarkState {
    PendingLocal { mode: UiLifecycleMode },
    Measured(ModeBenchmarkSummary),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AdrLifecycleRecommendation {
    pub recommended_mode: UiLifecycleMode,
    pub adr_001_status: &'static str,
    pub adr_005_decision: String,
}

pub fn bytes_to_megabytes(bytes: u64) -> f64 {
    (bytes as f64) / (1024.0 * 1024.0)
}

pub fn collect_process_tree(
    root_pid: u32,
    snapshot: &[ProcessNode],
) -> Result<(Vec<ProcessIdentity>, u64), String> {
    let by_pid: HashMap<u32, &ProcessNode> = snapshot.iter().map(|n| (n.pid, n)).collect();
    if !by_pid.contains_key(&root_pid) {
        return Err(format!("root_pid {} not found in OS process snapshot", root_pid));
    }

    let mut children_map: HashMap<u32, Vec<u32>> = HashMap::new();
    for node in snapshot {
        if node.pid != node.parent_pid {
            children_map.entry(node.parent_pid).or_default().push(node.pid);
        }
    }

    let mut visited_pids = HashSet::new();
    let mut ordered_identities = Vec::new();
    let mut stack = vec![root_pid];
    let mut total_private_bytes: u64 = 0;

    while let Some(pid) = stack.pop() {
        if !visited_pids.insert(pid) {
            continue;
        }
        let node = by_pid
            .get(&pid)
            .ok_or_else(|| format!("PID {} disappeared while traversing snapshot", pid))?;
        ordered_identities.push(node.identity());
        total_private_bytes = total_private_bytes.saturating_add(node.private_bytes);

        if let Some(children) = children_map.get(&pid) {
            let mut sorted = children.clone();
            sorted.sort_unstable();
            for child_pid in sorted.into_iter().rev() {
                if !visited_pids.contains(&child_pid) {
                    stack.push(child_pid);
                }
            }
        }
    }

    Ok((ordered_identities, total_private_bytes))
}

pub fn count_webview2_Descendants(root_pid: u32, snapshot: &[ProcessNode]) -> usize {
    let Ok((identities, _)) = collect_process_tree(root_pid, snapshot) else {
        return 0;
    };
    let pid_set: HashSet<u32> = identities.into_iter().map(|id| id.pid).collect();
    snapshot
        .iter()
        .filter(|n| {
            n.pid != root_pid
                && pid_set.contains(&n.pid)
                && n.name.to_ascii_lowercase().contains("msedgewebview2")
        })
        .count()
}

pub fn count_surviving_identities_after_close(
    spawned_tree_identities: &[ProcessIdentity],
    post_close_snapshot: &[ProcessNode],
) -> usize {
    let alive_identities: HashSet<ProcessIdentity> =
        post_close_snapshot.iter().map(|n| n.identity()).collect();
    spawned_tree_identities
        .iter()
        .filter(|id| alive_identities.contains(*id))
        .count()
}

pub fn compute_median(values: &[f64]) -> Result<f64, String> {
    if values.is_empty() {
        return Err("cannot compute median of empty slice".to_string());
    }
    let mut sorted = values.to_vec();
    sorted.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
    let mid = sorted.len() / 2;
    if sorted.len() % 2 == 1 {
        Ok(sorted[mid])
    } else {
        Ok((sorted[mid - 1] + sorted[mid]) / 2.0)
    }
}

pub fn compute_p95(values: &[f64]) -> Result<f64, String> {
    if values.is_empty() {
        return Err("cannot compute p95 of empty slice".to_string());
    }
    let mut sorted = values.to_vec();
    sorted.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
    let rank = ((sorted.len() as f64) * 0.95).ceil() as usize;
    let idx = rank.saturating_sub(1).min(sorted.len() - 1);
    Ok(sorted[idx])
}

pub fn parse_powershell_process_csv(raw_tsv: &str) -> Result<Vec<ProcessNode>, String> {
    let mut nodes = Vec::new();
    for (line_idx, raw_line) in raw_tsv.lines().enumerate() {
        let line = raw_line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let cols: Vec<&str> = line
            .split('\t')
            .map(|c| c.trim().trim_matches('"'))
            .collect();
        if cols.len() < 4 {
            continue;
        }
        if cols[0].eq_ignore_ascii_case("ProcessId") {
            continue;
        }
        let pid = cols[0].parse::<u32>().map_err(|e| {
            format!("invalid ProcessId '{}' at line {}: {}", cols[0], line_idx + 1, e)
        })?;
        let parent_pid = cols[1].parse::<u32>().unwrap_or(0);
        let name = cols[2].to_string();
        let private_bytes = cols[3].parse::<u64>().unwrap_or(0);
        let creation_date = if cols.len() >= 5 && !cols[4].is_empty() {
            cols[4].to_string()
        } else {
            "unknown-creation-time".to_string()
        };

        nodes.push(ProcessNode {
            pid,
            parent_pid,
            name,
            private_bytes,
            creation_date,
        });
    }

    if nodes.is_empty() {
        return Err("OS process snapshot parsed 0 processes".to_string());
    }
    Ok(nodes)
}

pub fn summarize_01a_samples(
    label: &str,
    inspector_pid: u32,
    samples_with_snapshots: &[(TreeIterationSample, Vec<ProcessNode>)],
) -> Result<TreeBenchmarkSummary, String> {
    if samples_with_snapshots.len() != REQUIRED_REPETITIONS {
        return Err(format!(
            "Verifier error: expected exactly {} repetitions, got {}",
            REQUIRED_REPETITIONS,
            samples_with_snapshots.len()
        ));
    }

    let mut private_mb_samples = Vec::with_capacity(REQUIRED_REPETITIONS);
    let mut elapsed_ms_samples = Vec::with_capacity(REQUIRED_REPETITIONS);
    let mut max_orphan_count = 0usize;
    let mut min_process_count = usize::MAX;

    for (idx, (sample, snapshot)) in samples_with_snapshots.iter().enumerate() {
        if sample.root_pid == inspector_pid {
            return Err(format!(
                "Verifier error in repetition {}: root_pid ({}) equals inspector_pid ({})",
                idx + 1,
                sample.root_pid,
                inspector_pid
            ));
        }
        if sample.tree_identities.is_empty() || sample.process_count < 2 {
            return Err(format!(
                "Verifier error in repetition {}: expected tree with child + grandchild (process_count >= 2), got {}",
                idx + 1,
                sample.process_count
            ));
        }

        let snapshot_identities: HashSet<ProcessIdentity> =
            snapshot.iter().map(|n| n.identity()).collect();
        for identity in &sample.tree_identities {
            if !snapshot_identities.contains(identity) {
                return Err(format!(
                    "Verifier error in repetition {}: ProcessIdentity (pid={}, creation_date={}) not found in OS snapshot",
                    idx + 1,
                    identity.pid,
                    identity.creation_date
                ));
            }
        }

        let (recomputed_ids, recomputed_bytes) =
            collect_process_tree(sample.root_pid, snapshot)?;
        if recomputed_bytes != sample.total_private_bytes
            || recomputed_ids.len() != sample.process_count
        {
            return Err(format!(
                "Verifier error in repetition {}: sample bytes/count mismatch against OS snapshot",
                idx + 1
            ));
        }

        if sample.total_private_bytes == 0 {
            return Err(format!(
                "Verifier error in repetition {}: total_private_bytes is 0",
                idx + 1
            ));
        }

        private_mb_samples.push(sample.total_private_mb());
        elapsed_ms_samples.push(sample.elapsed_ms);
        if sample.orphan_count_after_close > max_orphan_count {
            max_orphan_count = sample.orphan_count_after_close;
        }
        if sample.process_count < min_process_count {
            min_process_count = sample.process_count;
        }
    }

    Ok(TreeBenchmarkSummary {
        label: label.to_string(),
        inspector_pid,
        repetitions: REQUIRED_REPETITIONS,
        median_private_mb: compute_median(&private_mb_samples)?,
        p95_private_mb: compute_p95(&private_mb_samples)?,
        median_elapsed_ms: compute_median(&elapsed_ms_samples)?,
        p95_elapsed_ms: compute_p95(&elapsed_ms_samples)?,
        min_process_count,
        max_orphan_count,
        reference_status: STEP_01A_REFERENCE_LABEL.to_string(),
    })
}

pub fn summarize_01b_mode_samples(
    mode: UiLifecycleMode,
    coordinator_pid: u32,
    samples: &[ModeIterationSample],
) -> Result<ModeBenchmarkSummary, String> {
    if samples.len() != REQUIRED_REPETITIONS {
        return Err(format!(
            "Verifier error for {}: expected {} repetitions, got {}",
            mode.display_name(),
            REQUIRED_REPETITIONS,
            samples.len()
        ));
    }

    let mut idle_mb = Vec::with_capacity(REQUIRED_REPETITIONS);
    let mut expanded_mb = Vec::with_capacity(REQUIRED_REPETITIONS);
    let mut post_collapse_mb = Vec::with_capacity(REQUIRED_REPETITIONS);
    let mut hover_ms = Vec::with_capacity(REQUIRED_REPETITIONS);
    let mut max_orphan_count = 0usize;

    for (idx, s) in samples.iter().enumerate() {
        if s.mode != mode {
            return Err(format!(
                "Verifier error in repetition {}: mode mismatch ({:?} != {:?})",
                idx + 1,
                s.mode,
                mode
            ));
        }
        if s.worker_root_pid == coordinator_pid {
            return Err(format!(
                "Verifier error in repetition {}: worker_root_pid ({}) equals coordinator_pid",
                idx + 1,
                s.worker_root_pid
            ));
        }
        if s.idle_private_bytes == 0 || s.expanded_private_bytes == 0 {
            return Err(format!(
                "Verifier error in repetition {}: zero private bytes recorded",
                idx + 1
            ));
        }
        if mode.requires_webview2() && s.expanded_webview_child_count == 0 {
            return Err(format!(
                "Verifier error in repetition {} ({}): expected >= 1 msedgewebview2.exe child when expanded, got 0",
                idx + 1,
                mode.code()
            ));
        }

        // For Mode C and Mode D, post-collapse / post-TTL represents the steady-state rest RAM.
        let effective_rest_mb = match mode {
            UiLifecycleMode::ModeCWebViewPrewarmedTtl | UiLifecycleMode::ModeDWebViewOnDemand => {
                s.post_collapse_private_mb()
            }
            UiLifecycleMode::ModeANativeOnly | UiLifecycleMode::ModeBWebViewAlwaysAlive => {
                s.idle_private_mb()
            }
        };

        idle_mb.push(effective_rest_mb);
        expanded_mb.push(s.expanded_private_mb());
        post_collapse_mb.push(s.post_collapse_private_mb());
        hover_ms.push(s.hover_to_visible_ms);
        if s.orphan_count_after_close > max_orphan_count {
            max_orphan_count = s.orphan_count_after_close;
        }
    }

    let idle_p50_mb = compute_median(&idle_mb)?;
    let idle_p95_mb = compute_p95(&idle_mb)?;
    let expanded_p50_mb = compute_median(&expanded_mb)?;
    let expanded_p95_mb = compute_p95(&expanded_mb)?;
    let post_collapse_p50_mb = compute_median(&post_collapse_mb)?;
    let hover_p50_ms = compute_median(&hover_ms)?;
    let hover_p95_ms = compute_p95(&hover_ms)?;

    let meets_idle_ram_budget = idle_p50_mb <= IDLE_PRIVATE_RAM_THRESHOLD_MB;
    let meets_latency_budget = hover_p95_ms <= HOVER_TO_VISIBLE_P95_THRESHOLD_MS;
    let passes_all_budgets =
        meets_idle_ram_budget && meets_latency_budget && max_orphan_count == 0;

    let verdict_label = if passes_all_budgets {
        "CUMPLE (<= 30 MB reposo, p95 <= 150 ms, 0 huérfanos)".to_string()
    } else {
        let mut reasons = Vec::new();
        if !meets_idle_ram_budget {
            reasons.push(format!("RAM reposo {:.2} MB > 30.0 MB", idle_p50_mb));
        }
        if !meets_latency_budget {
            reasons.push(format!("latencia p95 {:.2} ms > 150.0 ms", hover_p95_ms));
        }
        if max_orphan_count > 0 {
            reasons.push(format!("{} huérfanos", max_orphan_count));
        }
        format!("NO CUMPLE ({})", reasons.join(", "))
    };

    Ok(ModeBenchmarkSummary {
        mode,
        repetitions: REQUIRED_REPETITIONS,
        idle_p50_mb,
        idle_p95_mb,
        expanded_p50_mb,
        expanded_p95_mb,
        post_collapse_p50_mb,
        hover_p50_ms,
        hover_p95_ms,
        max_orphan_count,
        meets_idle_ram_budget,
        meets_latency_budget,
        passes_all_budgets,
        verdict_label,
    })
}

pub fn select_recommended_lifecycle_mode(
    summaries: &[ModeBenchmarkSummary],
) -> Result<AdrLifecycleRecommendation, String> {
    let by_mode: HashMap<UiLifecycleMode, &ModeBenchmarkSummary> =
        summaries.iter().map(|s| (s.mode, s)).collect();

    if let Some(mode_d) = by_mode.get(&UiLifecycleMode::ModeDWebViewOnDemand) {
        if mode_d.passes_all_budgets {
            return Ok(AdrLifecycleRecommendation {
                recommended_mode: UiLifecycleMode::ModeDWebViewOnDemand,
                adr_001_status: "Fijado",
                adr_005_decision: format!(
                    "Modo D (WebView2 bajo demanda estricto): reposo {:.2} MB (<= 30 MB) y latencia p95 {:.2} ms (<= 150 ms)",
                    mode_d.idle_p50_mb, mode_d.hover_p95_ms
                ),
            });
        }
    }

    if let Some(mode_c) = by_mode.get(&UiLifecycleMode::ModeCWebViewPrewarmedTtl) {
        if mode_c.passes_all_budgets {
            return Ok(AdrLifecycleRecommendation {
                recommended_mode: UiLifecycleMode::ModeCWebViewPrewarmedTtl,
                adr_001_status: "Fijado",
                adr_005_decision: format!(
                    "Modo C (WebView2 precalentado con TTL): reposo post-TTL {:.2} MB (<= 30 MB) y latencia p95 {:.2} ms (<= 150 ms)",
                    mode_c.idle_p50_mb, mode_c.hover_p95_ms
                ),
            });
        }
    }

    if let Some(mode_a) = by_mode.get(&UiLifecycleMode::ModeANativeOnly) {
        if mode_a.passes_all_budgets {
            return Ok(AdrLifecycleRecommendation {
                recommended_mode: UiLifecycleMode::ModeANativeOnly,
                adr_001_status: "Fijado (Fallback Nativo)",
                adr_005_decision: format!(
                    "Modo A (Island nativa Win32, WebView2 solo para ajustes): reposo {:.2} MB y latencia p95 {:.2} ms",
                    mode_a.idle_p50_mb, mode_a.hover_p95_ms
                ),
            });
        }
    }

    Err("Ninguno de los modos (D, C, A) cumplió simultáneamente <= 30 MB y p95 <= 150 ms".to_string())
}

pub fn initial_01b_pending_states() -> Vec<ModeBenchmarkState> {
    UiLifecycleMode::all()
        .into_iter()
        .map(|mode| ModeBenchmarkState::PendingLocal { mode })
        .collect()
}

pub fn format_01b_mode_row(state: &ModeBenchmarkState) -> String {
    match state {
        ModeBenchmarkState::PendingLocal { mode } => format!(
            "| {:<60} | {:<18} | {:<18} | {:<18} | {:<18} | {:<10} | {:<45} |",
            mode.display_name(),
            "PENDIENTE-LOCAL",
            "PENDIENTE-LOCAL",
            "PENDIENTE-LOCAL",
            "PENDIENTE-LOCAL",
            "PENDIENTE",
            "PENDIENTE-LOCAL"
        ),
        ModeBenchmarkState::Measured(s) => format!(
            "| {:<60} | {:>6.2} / {:>6.2} MB | {:>6.2} / {:>6.2} MB | {:>6.2} / {:>6.2} ms | {:>15.2} MB | {:>10} | {:<45} |",
            s.mode.display_name(),
            s.idle_p50_mb,
            s.idle_p95_mb,
            s.expanded_p50_mb,
            s.expanded_p95_mb,
            s.hover_p50_ms,
            s.hover_p95_ms,
            s.post_collapse_p50_mb,
            s.max_orphan_count,
            s.verdict_label
        ),
    }
}

pub fn format_benchmark_report(
    summary_01a: &TreeBenchmarkSummary,
    modes_01b: &[ModeBenchmarkState],
    recommendation: Option<&AdrLifecycleRecommendation>,
) -> String {
    let mut out = String::new();
    out.push_str("========================================================================================================================================================================\n");
    out.push_str(" MIKU DESKTOP ORCHESTRATOR · SPIKE 0.1 (0.1a + 0.1b) · WIN32 ISLAND + WEBVIEW2 LIFECYCLE BENCHMARK\n");
    out.push_str(" Metric: OS Committed Private Memory (PrivatePageCount, MB) on root_pid = child.id() | Latency threshold: p95 <= 150.0 ms | Idle RAM threshold: <= 30.0 MB\n");
    out.push_str("========================================================================================================================================================================\n");
    out.push_str(&format!(
        " [0.1a Baseline] {:<45} | Inspector PID: {} | Repetitions: n={} | PrivatePageCount p50/p95: {:.2} / {:.2} MB | Min procs: {} | Orphans: {} | {}\n",
        summary_01a.label,
        summary_01a.inspector_pid,
        summary_01a.repetitions,
        summary_01a.median_private_mb,
        summary_01a.p95_private_mb,
        summary_01a.min_process_count,
        summary_01a.max_orphan_count,
        summary_01a.reference_status
    ));
    out.push_str("------------------------------------------------------------------------------------------------------------------------------------------------------------------------\n");
    out.push_str(&format!(
        "| {:<60} | {:<18} | {:<18} | {:<18} | {:<18} | {:<10} | {:<45} |\n",
        "Modo 0.1b (root_pid = worker.id(), n=20)",
        "Reposo p50/p95",
        "Expandido p50/p95",
        "Hover->Vis p50/p95",
        "Post-Colapso p50",
        "Huérfanos",
        "Veredicto de Producto (ADR-001 / ADR-005)"
    ));
    out.push_str("|--------------------------------------------------------------|--------------------|--------------------|--------------------|--------------------|------------|-----------------------------------------------|\n");
    for state in modes_01b {
        out.push_str(&format_01b_mode_row(state));
        out.push('\n');
    }
    out.push_str("========================================================================================================================================================================\n");
    if let Some(rec) = recommendation {
        out.push_str(&format!(
            " RECOMENDACIÓN AUTOMÁTICA ADR-001 ({}) / ADR-005: {}\n",
            rec.adr_001_status, rec.adr_005_decision
        ));
        out.push_str("========================================================================================================================================================================\n");
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    // --- 5 Tests from 0.1a ---

    #[test]
    fn aggregates_private_bytes_for_spawned_child_tree_excluding_inspector() {
        let inspector_pid = 9999;
        let child_root_pid = 100;
        let snapshot = vec![
            ProcessNode {
                pid: inspector_pid,
                parent_pid: 1,
                name: "miku-spike-01-ui.exe".to_string(),
                private_bytes: 50 * 1024 * 1024,
                creation_date: "20261105010000.000000+000".to_string(),
            },
            ProcessNode {
                pid: child_root_pid,
                parent_pid: inspector_pid,
                name: "cmd.exe".to_string(),
                private_bytes: 4 * 1024 * 1024,
                creation_date: "20261105010001.000000+000".to_string(),
            },
            ProcessNode {
                pid: 101,
                parent_pid: child_root_pid,
                name: "ping.exe".to_string(),
                private_bytes: 6 * 1024 * 1024,
                creation_date: "20261105010001.500000+000".to_string(),
            },
        ];

        let (identities, total_bytes) =
            collect_process_tree(child_root_pid, &snapshot).expect("tree must resolve");
        assert_eq!(identities.len(), 2);
        assert!(!identities.iter().any(|id| id.pid == inspector_pid));
        assert!((bytes_to_megabytes(total_bytes) - 10.0).abs() < 1e-6);
    }

    #[test]
    fn computes_median_and_p95_over_20_samples() {
        let mut samples = Vec::new();
        for i in 1..=20 {
            samples.push(i as f64);
        }
        let median = compute_median(&samples).expect("median");
        let p95 = compute_p95(&samples).expect("p95");
        assert!((median - 10.5).abs() < 1e-6);
        assert!((p95 - 19.0).abs() < 1e-6);
    }

    #[test]
    fn detects_surviving_spawned_identities_and_ignores_recycled_pids() {
        let spawned_identities = vec![
            ProcessIdentity {
                pid: 200,
                creation_date: "20261105010001.000000+000".to_string(),
            },
            ProcessIdentity {
                pid: 201,
                creation_date: "20261105010002.000000+000".to_string(),
            },
        ];

        let post_close_with_orphan_and_recycled = vec![
            ProcessNode {
                pid: 200,
                parent_pid: 1,
                name: "svchost.exe".to_string(),
                private_bytes: 8 * 1024 * 1024,
                creation_date: "20261105010009.999999+000".to_string(),
            },
            ProcessNode {
                pid: 201,
                parent_pid: 1,
                name: "ping.exe".to_string(),
                private_bytes: 2 * 1024 * 1024,
                creation_date: "20261105010002.000000+000".to_string(),
            },
        ];
        assert_eq!(
            count_surviving_identities_after_close(
                &spawned_identities,
                &post_close_with_orphan_and_recycled
            ),
            1
        );
    }

    #[test]
    fn verifier_fails_if_sample_identity_missing_from_os_snapshot_or_missing_grandchild() {
        let inspector_pid = 9000;
        let real_snapshot = vec![
            ProcessNode {
                pid: 300,
                parent_pid: inspector_pid,
                name: "cmd.exe".to_string(),
                private_bytes: 4 * 1024 * 1024,
                creation_date: "20261105010001.000000+000".to_string(),
            },
            ProcessNode {
                pid: 301,
                parent_pid: 300,
                name: "ping.exe".to_string(),
                private_bytes: 2 * 1024 * 1024,
                creation_date: "20261105010001.100000+000".to_string(),
            },
        ];

        let mut bad_samples = Vec::new();
        for rep in 1..=REQUIRED_REPETITIONS {
            let forged_sample = TreeIterationSample {
                repetition: rep,
                root_pid: 300,
                tree_identities: vec![
                    ProcessIdentity {
                        pid: 300,
                        creation_date: "20261105010001.000000+000".to_string(),
                    },
                    ProcessIdentity {
                        pid: 99999,
                        creation_date: "20261105010001.999999+000".to_string(),
                    },
                ],
                process_count: 2,
                total_private_bytes: 6 * 1024 * 1024,
                elapsed_ms: 40.0,
                orphan_count_after_close: 0,
            };
            bad_samples.push((forged_sample, real_snapshot.clone()));
        }

        let err = summarize_01a_samples("forged", inspector_pid, &bad_samples).unwrap_err();
        assert!(err.contains("99999"));
    }

    #[test]
    fn labels_01a_without_product_verdict_and_keeps_01b_pending_local_without_numbers() {
        let pending = initial_01b_pending_states();
        assert_eq!(pending.len(), 4);
        for state in &pending {
            let formatted = format_01b_mode_row(state);
            assert!(formatted.contains("PENDIENTE-LOCAL"));
            assert!(!formatted.contains(" MB"));
            assert!(!formatted.contains(" ms"));
        }
    }

    // --- 4 New Tests for 0.1b ---

    #[test]
    fn computes_top_center_island_geometry_without_focus_steal_flags() {
        let pill_100 = compute_island_geometry(1920, 1080, 1.0, false);
        assert_eq!(pill_100.width, 220);
        assert_eq!(pill_100.height, 36);
        assert_eq!(pill_100.x, (1920 - 220) / 2);
        assert_eq!(pill_100.y, 8);
        assert_eq!(
            pill_100.ex_style_flags & WS_EX_NOACTIVATE_FLAG,
            WS_EX_NOACTIVATE_FLAG
        );
        assert_eq!(
            pill_100.ex_style_flags & WS_EX_TOPMOST_FLAG,
            WS_EX_TOPMOST_FLAG
        );
        assert_eq!(
            pill_100.ex_style_flags & WS_EX_TOOLWINDOW_FLAG,
            WS_EX_TOOLWINDOW_FLAG
        );

        let exp_125 = compute_island_geometry(2560, 1440, 1.25, true);
        assert_eq!(exp_125.width, 575);
        assert_eq!(exp_125.height, 475);
        assert_eq!(exp_125.x, (2560 - 575) / 2);
        assert_eq!(exp_125.y, 10);

        let udd_b1 = build_isolated_udd_path(
            "C:\\Users\\user\\AppData\\Local",
            UiLifecycleMode::ModeBWebViewAlwaysAlive,
            1,
            4242,
        );
        let udd_b2 = build_isolated_udd_path(
            "C:\\Users\\user\\AppData\\Local",
            UiLifecycleMode::ModeBWebViewAlwaysAlive,
            2,
            4242,
        );
        assert_ne!(udd_b1, udd_b2);
        assert!(udd_b1.ends_with("miku\\spike-01b\\mode_B_rep_1_pid_4242"));
    }

    #[test]
    fn aggregates_webview2_process_tree_including_msedgewebview2_children() {
        let coordinator_pid = 1000;
        let worker_pid = 2000;
        let snapshot = vec![
            ProcessNode {
                pid: coordinator_pid,
                parent_pid: 1,
                name: "miku-spike-01-ui.exe".to_string(),
                private_bytes: 12 * 1024 * 1024,
                creation_date: "20261105020000.000000+000".to_string(),
            },
            ProcessNode {
                pid: worker_pid,
                parent_pid: coordinator_pid,
                name: "miku-spike-01-ui.exe".to_string(),
                private_bytes: 4 * 1024 * 1024,
                creation_date: "20261105020001.000000+000".to_string(),
            },
            ProcessNode {
                pid: 2001,
                parent_pid: worker_pid,
                name: "msedgewebview2.exe".to_string(),
                private_bytes: 28 * 1024 * 1024,
                creation_date: "20261105020001.100000+000".to_string(),
            },
            ProcessNode {
                pid: 2002,
                parent_pid: 2001,
                name: "msedgewebview2.exe".to_string(),
                private_bytes: 34 * 1024 * 1024,
                creation_date: "20261105020001.200000+000".to_string(),
            },
            ProcessNode {
                pid: 2003,
                parent_pid: 2001,
                name: "msedgewebview2.exe".to_string(),
                private_bytes: 18 * 1024 * 1024,
                creation_date: "20261105020001.300000+000".to_string(),
            },
        ];

        let (identities, total_bytes) =
            collect_process_tree(worker_pid, &snapshot).expect("worker tree");
        assert_eq!(identities.len(), 4);
        assert_eq!(count_webview2_Descendants(worker_pid, &snapshot), 3);
        assert!((bytes_to_megabytes(total_bytes) - 84.0).abs() < 1e-6);
    }

    #[test]
    fn mode_c_ttl_state_machine_destroys_webview2_on_expiry_and_verifies_zero_orphans() {
        let mut fsm = ModeCTtlController::new(250);
        assert_eq!(fsm.phase, ModeCTtlPhase::ColdIdle);
        assert!(!fsm.webview_alive);

        fsm.on_hover_expand(1000);
        assert!(fsm.webview_alive);
        assert_eq!(
            fsm.phase,
            ModeCTtlPhase::ExpandedActive {
                last_activity_ms: 1000
            }
        );

        fsm.on_collapse_to_idle(1200);
        assert!(fsm.webview_alive);
        assert!(!fsm.poll_timer(1350)); // Only 150 ms elapsed (< 250 ms TTL)
        assert!(fsm.webview_alive);

        assert!(fsm.poll_timer(1450)); // 250 ms elapsed -> triggers destruction
        assert!(!fsm.webview_alive);
        assert_eq!(fsm.phase, ModeCTtlPhase::CollapsedColdAfterTtl);
    }

    #[test]
    fn evaluates_4_modes_against_product_thresholds_30mb_idle_and_150ms_p95() {
        let coordinator_pid = 500;
        let build_samples = |mode: UiLifecycleMode,
                             idle_mb: u64,
                             exp_mb: u64,
                             post_mb: u64,
                             hover_ms: f64|
         -> Vec<ModeIterationSample> {
            (1..=REQUIRED_REPETITIONS)
                .map(|rep| ModeIterationSample {
                    mode,
                    repetition: rep,
                    worker_root_pid: 1000 + rep as u32,
                    idle_tree_identities: vec![ProcessIdentity {
                        pid: 1000 + rep as u32,
                        creation_date: "20261105020000.000000+000".to_string(),
                    }],
                    expanded_tree_identities: vec![ProcessIdentity {
                        pid: 1000 + rep as u32,
                        creation_date: "20261105020000.000000+000".to_string(),
                    }],
                    idle_process_count: 1,
                    expanded_process_count: if mode.requires_webview2() { 5 } else { 1 },
                    expanded_webview_child_count: if mode.requires_webview2() { 4 } else { 0 },
                    idle_private_bytes: idle_mb * 1024 * 1024,
                    expanded_private_bytes: exp_mb * 1024 * 1024,
                    hover_to_visible_ms: hover_ms,
                    post_collapse_private_bytes: post_mb * 1024 * 1024,
                    orphan_count_after_close: 0,
                })
                .collect()
        };

        // Scenario 1: Mode D meets p95 <= 150 ms (e.g., 115 ms) -> Mode D wins
        let sum_a = summarize_01b_mode_samples(
            UiLifecycleMode::ModeANativeOnly,
            coordinator_pid,
            &build_samples(UiLifecycleMode::ModeANativeOnly, 4, 5, 4, 2.0),
        )
        .unwrap();
        let sum_b = summarize_01b_mode_samples(
            UiLifecycleMode::ModeBWebViewAlwaysAlive,
            coordinator_pid,
            &build_samples(UiLifecycleMode::ModeBWebViewAlwaysAlive, 68, 85, 68, 12.0),
        )
        .unwrap();
        let sum_c = summarize_01b_mode_samples(
            UiLifecycleMode::ModeCWebViewPrewarmedTtl,
            coordinator_pid,
            &build_samples(UiLifecycleMode::ModeCWebViewPrewarmedTtl, 65, 82, 5, 14.0),
        )
        .unwrap();
        let sum_d_fast = summarize_01b_mode_samples(
            UiLifecycleMode::ModeDWebViewOnDemand,
            coordinator_pid,
            &build_samples(UiLifecycleMode::ModeDWebViewOnDemand, 4, 82, 4, 115.0),
        )
        .unwrap();

        assert!(sum_a.passes_all_budgets);
        assert!(!sum_b.passes_all_budgets); // Fails <= 30 MB idle
        assert!(sum_c.passes_all_budgets); // Post-TTL collapse is 5 MB <= 30 MB, hover 14 ms <= 150 ms
        assert!(sum_d_fast.passes_all_budgets);

        let rec1 = select_recommended_lifecycle_mode(&[
            sum_a.clone(),
            sum_b.clone(),
            sum_c.clone(),
            sum_d_fast,
        ])
        .unwrap();
        assert_eq!(rec1.recommended_mode, UiLifecycleMode::ModeDWebViewOnDemand);

        // Scenario 2: Mode D cold start is > 150 ms (e.g., 210 ms) -> falls back to Mode C
        let sum_d_slow = summarize_01b_mode_samples(
            UiLifecycleMode::ModeDWebViewOnDemand,
            coordinator_pid,
            &build_samples(UiLifecycleMode::ModeDWebViewOnDemand, 4, 82, 4, 210.0),
        )
        .unwrap();
        assert!(!sum_d_slow.passes_all_budgets);

        let rec2 =
            select_recommended_lifecycle_mode(&[sum_a, sum_b, sum_c, sum_d_slow]).unwrap();
        assert_eq!(
            rec2.recommended_mode,
            UiLifecycleMode::ModeCWebViewPrewarmedTtl
        );
    }
}
