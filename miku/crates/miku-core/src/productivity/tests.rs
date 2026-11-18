use std::path::Path;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::Duration;

use super::{
    FocusPhaseKind, FocusPomodoroConfig, FocusPomodoroScheduler, ProductivityHub,
    QuickTaskPriority,
};

#[test]
fn test_focus_scheduler_uses_sleep_until_without_per_second_ticks_when_collapsed() {
    let cfg = FocusPomodoroConfig::default();
    let mut sched = FocusPomodoroScheduler::new(cfg);
    sched.set_island_collapsed(true);

    // Idle produces zero deadline & zero periodic wakeups
    assert_eq!(sched.next_sleep_until_plan(1_000_000), None);
    let idle_snap = sched.snapshot(1_000_000);
    assert_eq!(idle_snap.periodic_ticks_per_second, 0);
    assert_eq!(idle_snap.estimated_idle_cpu_pct, "0.0%");

    // Start 25-min focus block: single sleep_until straight to t + 1_500_000 ms, 0 Hz ticks
    let plan = sched.start_focus(1_000_000, 10_000, Some("task-0001".to_string()));
    assert_eq!(plan.periodic_ticks_per_second, 0);
    assert_eq!(plan.sleep_duration_ms, 25 * 60 * 1_000);
    assert_eq!(plan.target_wall_ms, 2_500_000);
    assert_eq!(sched.scheduled_wakeups_count(), 1);

    let queried = sched.next_sleep_until_plan(1_300_000).expect("active plan");
    assert_eq!(queried.target_wall_ms, 2_500_000);
    assert_eq!(queried.sleep_duration_ms, 1_200_000);
    assert_eq!(queried.periodic_ticks_per_second, 0);
}

#[test]
fn test_windows_sleep_suspend_reconciliation_completes_expired_focus_cleanly() {
    let mut sched = FocusPomodoroScheduler::new(FocusPomodoroConfig::default());
    sched.start_focus(10_000_000, 100_000, None);

    // Windows sleeps 40 minutes in wall clock while monotonic clock advances slowly
    let outcome = sched.reconcile_at_wall_and_monotonic(
        10_000_000 + 40 * 60 * 1_000,
        100_000 + 5_000,
    );
    assert!(outcome.resumed_from_sleep);
    assert!(outcome.focus_block_completed);
    assert_eq!(outcome.focused_ms_added, 25 * 60 * 1_000);
    // Break also expired during sleep -> returns cleanly to Idle without stale alarms
    assert_eq!(outcome.phase_after, FocusPhaseKind::Idle);
    assert_eq!(sched.next_sleep_until_plan(10_000_000 + 40 * 60 * 1_000), None);
}

#[test]
fn test_tokio_paused_sleep_until_executes_zero_intermediate_polls() {
    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_time()
        .build()
        .unwrap();

    rt.block_on(async {
        tokio::time::pause();
        let mut sched = FocusPomodoroScheduler::new(FocusPomodoroConfig::default());
        let plan = sched.start_focus(0, 0, None);
        let poll_counter = Arc::new(AtomicUsize::new(0));
        let counter_clone = Arc::clone(&poll_counter);

        let sleeper = tokio::spawn(async move {
            let deadline = tokio::time::Instant::now() + Duration::from_millis(plan.sleep_duration_ms);
            tokio::time::sleep_until(deadline).await;
            counter_clone.fetch_add(1, Ordering::SeqCst);
        });

        // Advance 24 minutes (still collapsed in idle sleep): task must have woken 0 times
        tokio::time::advance(Duration::from_secs(24 * 60)).await;
        tokio::task::yield_now().await;
        assert_eq!(poll_counter.load(Ordering::SeqCst), 0);

        // Advance final 60 seconds: wakes exactly once at target deadline
        tokio::time::advance(Duration::from_secs(60)).await;
        sleeper.await.unwrap();
        assert_eq!(poll_counter.load(Ordering::SeqCst), 1);

        let outcome = sched.reconcile_at_wall_and_monotonic(25 * 60 * 1_000, 25 * 60 * 1_000);
        assert!(outcome.focus_block_completed);
        assert_eq!(outcome.phase_after, FocusPhaseKind::ShortBreak);
    });
}

#[test]
fn test_quick_tasks_daily_summary_sanitization_and_atomic_persistence() {
    let mut hub = ProductivityHub::new(FocusPomodoroConfig::default());
    let t1 = hub
        .add_quick_task(
            "Revisar PR 42 <script>alert(1)</script>",
            "Checar Gatekeeper",
            QuickTaskPriority::High,
            1_000,
        )
        .unwrap();
    let _t2 = hub
        .add_quick_task("Responder correo", "Pendiente", QuickTaskPriority::Normal, 1_005)
        .unwrap();
    assert!(!t1.title.contains('<'));
    assert!(hub.mark_task_done(&t1.id, 2_000));
    hub.record_agent_run_completed();
    hub.scheduler_mut().start_focus(2_000, 2_000, Some(t1.id.clone()));
    hub.scheduler_mut()
        .reconcile_at_wall_and_monotonic(2_000 + 25 * 60 * 1_000, 2_000 + 25 * 60 * 1_000);

    let summary = hub.build_daily_summary("2026-11-13", 2_000 + 25 * 60 * 1_000);
    assert_eq!(summary.total_tasks, 2);
    assert_eq!(summary.completed_tasks, 1);
    assert_eq!(summary.pending_tasks, 1);
    assert_eq!(summary.focused_minutes, 25);
    assert_eq!(summary.completed_focus_blocks, 1);
    assert_eq!(summary.agent_runs_today, 1);

    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    let store_path = std::env::temp_dir().join(format!("miku_prod_4_6_{nanos}.json"));
    hub.save_atomic(&store_path).unwrap();
    let reloaded = ProductivityHub::load_from_disk(&store_path).unwrap();
    assert_eq!(reloaded.list_tasks().len(), 2);
    let _ = std::fs::remove_file(&store_path);
}

#[test]
fn test_productivity_modules_comply_with_sub_600_line_modular_budget() {
    let dir = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("src")
        .join("productivity");
    let mut files = 0usize;
    let mut max_lines = 0usize;
    for entry in std::fs::read_dir(&dir).unwrap() {
        let p = entry.unwrap().path();
        if p.extension().and_then(|e| e.to_str()) == Some("rs") {
            let lines = std::fs::read_to_string(&p).unwrap().lines().count();
            files += 1;
            max_lines = max_lines.max(lines);
            assert!(lines < 600, "module {} exceeded 600 lines ({lines})", p.display());
        }
    }
    println!("PRODUCTIVITY_4_6_MODULAR_AUDIT: files={files} max_lines={max_lines} (< 600)");
    assert_eq!(files, 4);
}
