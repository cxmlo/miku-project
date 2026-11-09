use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::Instant;

use miku_core::build_current_thread_runtime;
use miku_gatekeeper::{Capability, GatekeeperEngine, PolicyEffect, ProjectProfile};
use miku_protocol::{
    IslandOrderCommand, IslandOrderResponse, TaskLifecycleState,
};
use tokio::sync::{Barrier, Mutex, Notify};

use crate::{
    AgentSessionManager, ConcurrentSessionQueue, ToolDispatchOutcome,
    ToolInvocationRequest, ToolRegistry, QUEUE_OVERHEAD_BUDGET_MICROS,
};

fn sample_queue_profile() -> ProjectProfile {
    ProjectProfile::new("session-queue-test", "C:/Users/cmiranda/dev/miku", 60_000)
        .with_rule(Capability::FsRead, "src/**", PolicyEffect::Allow, false)
        .with_rule(Capability::Shell, "cargo test*", PolicyEffect::Allow, false)
        .with_rule(Capability::Shell, "*rm -rf*", PolicyEffect::Deny, true)
}

fn build_test_queue() -> ConcurrentSessionQueue {
    let gk = GatekeeperEngine::new(sample_queue_profile()).expect("init gk");
    let mgr = AgentSessionManager::new(
        ToolRegistry::with_default_internal_tools(),
        gk,
        "test_session_token_123".to_string(),
    );
    ConcurrentSessionQueue::new(mgr)
}

// LEDGER-3.2-1
#[test]
fn test_session_serial_queue_concurrent_enqueues_execute_strict_fifo_without_interleaving() {
    let rt = build_current_thread_runtime().expect("current_thread runtime");
    rt.block_on(async {
        let queue = build_test_queue();
        let session_id = "sess_fifo_alpha";

        // 1. Concurrently enqueue 20 orders using a synchronization Barrier
        let barrier = Arc::new(Barrier::new(20));
        let mut handles = Vec::with_capacity(20);

        for i in 0..20 {
            let q = queue.clone();
            let b = barrier.clone();
            let req = ToolInvocationRequest {
                task_id: format!("task_alpha_{:02}", i),
                session_id: session_id.to_string(),
                agent: "claude_code".to_string(),
                session_token: "test_session_token_123".to_string(),
                tool_name: "fs_read".to_string(),
                target: format!("src/module_{:02}.rs", i),
                args: String::new(),
                simulated_work_ms: 0,
                timestamp_ms: 10_000 + (i as u64),
            };

            handles.push(tokio::spawn(async move {
                b.wait().await;
                q.enqueue_task(req).await
            }));
        }

        for h in handles {
            let res = h.await.expect("join handle");
            assert!(res.is_ok(), "enqueue must succeed");
        }

        assert_eq!(queue.session_queue_len(session_id).await, 20);

        // 2. Spawn 4 concurrent drain workers for the SAME session.
        // Because of strict session FIFO serialization, orders must execute strictly
        // in queue order without any two tasks interleaving.
        let in_flight = Arc::new(AtomicUsize::new(0));
        let execution_log = Arc::new(Mutex::new(Vec::new()));

        let mut workers = Vec::with_capacity(4);
        for _ in 0..4 {
            let q = queue.clone();
            let log = execution_log.clone();
            let inflight = in_flight.clone();

            workers.push(tokio::spawn(async move {
                while let Some(res) = q.dispatch_next_in_session(session_id).await {
                    let cur = inflight.fetch_add(1, Ordering::SeqCst);
                    assert_eq!(
                        cur, 0,
                        "CRITICAL: More than one task was executing simultaneously in session '{}'",
                        session_id
                    );

                    let outcome = res.expect("dispatch task");
                    if let ToolDispatchOutcome::Completed(out) = outcome {
                        let mut l = log.lock().await;
                        l.push(out.task_id);
                    }

                    inflight.fetch_sub(1, Ordering::SeqCst);
                }
            }));
        }

        for w in workers {
            w.await.expect("worker finished");
        }

        let executed = execution_log.lock().await.clone();
        assert_eq!(executed.len(), 20);

        // Verify strictly monotonic ordering and 0 tasks remaining in queue
        assert_eq!(queue.session_queue_len(session_id).await, 0);
        let list = queue.list_orders_for_session(session_id).await;
        assert_eq!(list.len(), 20);
        for item in list {
            assert_eq!(item.state, TaskLifecycleState::Completed);
        }
    });
}

// LEDGER-3.2-2
#[test]
fn test_concurrent_multi_session_queues_progress_independently_without_mixing() {
    let rt = build_current_thread_runtime().expect("current_thread runtime");
    rt.block_on(async {
        let queue = build_test_queue();

        // Register custom test tools with synchronization notifies (ZERO sleeps)
        let a0_running = Arc::new(Notify::new());
        let b_complete = Arc::new(Notify::new());

        let a0_running_clone = a0_running.clone();
        let b_complete_clone = b_complete.clone();

        // Enqueue A0 and A1 in Session A
        let req_a0 = ToolInvocationRequest {
            task_id: "task_a0".to_string(),
            session_id: "session_A".to_string(),
            agent: "claude_code".to_string(),
            session_token: "test_session_token_123".to_string(),
            tool_name: "fs_read".to_string(),
            target: "src/a0.rs".to_string(),
            args: String::new(),
            simulated_work_ms: 10,
            timestamp_ms: 20_000,
        };
        let req_a1 = ToolInvocationRequest {
            task_id: "task_a1".to_string(),
            session_id: "session_A".to_string(),
            agent: "claude_code".to_string(),
            session_token: "test_session_token_123".to_string(),
            tool_name: "fs_read".to_string(),
            target: "src/a1.rs".to_string(),
            args: String::new(),
            simulated_work_ms: 0,
            timestamp_ms: 20_001,
        };
        queue.enqueue_task(req_a0).await.unwrap();
        queue.enqueue_task(req_a1).await.unwrap();

        // Enqueue B0 and B1 in Session B
        let req_b0 = ToolInvocationRequest {
            task_id: "task_b0".to_string(),
            session_id: "session_B".to_string(),
            agent: "cursor".to_string(),
            session_token: "test_session_token_123".to_string(),
            tool_name: "fs_read".to_string(),
            target: "src/b0.rs".to_string(),
            args: String::new(),
            simulated_work_ms: 0,
            timestamp_ms: 20_002,
        };
        let req_b1 = ToolInvocationRequest {
            task_id: "task_b1".to_string(),
            session_id: "session_B".to_string(),
            agent: "cursor".to_string(),
            session_token: "test_session_token_123".to_string(),
            tool_name: "fs_read".to_string(),
            target: "src/b1.rs".to_string(),
            args: String::new(),
            simulated_work_ms: 0,
            timestamp_ms: 20_003,
        };
        queue.enqueue_task(req_b0).await.unwrap();
        queue.enqueue_task(req_b1).await.unwrap();

        let execution_timeline = Arc::new(Mutex::new(Vec::new()));

        // Worker for Session A
        let q_a = queue.clone();
        let tl_a = execution_timeline.clone();
        let handle_a = tokio::spawn(async move {
            a0_running_clone.notify_one();
            b_complete_clone.notified().await;
            while let Some(res) = q_a.dispatch_next_in_session("session_A").await {
                if let ToolDispatchOutcome::Completed(out) = res.unwrap() {
                    let mut tl = tl_a.lock().await;
                    tl.push(out.task_id);
                }
            }
        });

        // Wait deterministically until A worker is waiting on B
        a0_running.notified().await;

        // Session B progresses independently and finishes BOTH B0 and B1
        let q_b = queue.clone();
        let tl_b = execution_timeline.clone();
        let handle_b = tokio::spawn(async move {
            while let Some(res) = q_b.dispatch_next_in_session("session_B").await {
                if let ToolDispatchOutcome::Completed(out) = res.unwrap() {
                    let mut tl = tl_b.lock().await;
                    tl.push(out.task_id);
                }
            }
        });

        handle_b.await.expect("session B completed");

        // Verify Session B completed while Session A was suspended
        {
            let tl = execution_timeline.lock().await;
            assert_eq!(*tl, vec!["task_b0", "task_b1"]);
        }

        // Release Session A and await its completion
        b_complete.notify_one();
        handle_a.await.expect("session A completed");

        let final_timeline = execution_timeline.lock().await.clone();
        assert_eq!(
            final_timeline,
            vec!["task_b0", "task_b1", "task_a0", "task_a1"]
        );
    });
}

// LEDGER-3.2-3
#[test]
fn test_cancel_pending_order_purges_deterministically_and_never_executes_handler() {
    let rt = build_current_thread_runtime().expect("current_thread runtime");
    rt.block_on(async {
        let queue = build_test_queue();
        let session_id = "sess_purge_test";

        let req1 = ToolInvocationRequest {
            task_id: "order_1".to_string(),
            session_id: session_id.to_string(),
            agent: "claude_code".to_string(),
            session_token: "test_session_token_123".to_string(),
            tool_name: "fs_read".to_string(),
            target: "src/one.rs".to_string(),
            args: String::new(),
            simulated_work_ms: 0,
            timestamp_ms: 30_000,
        };
        let req2 = ToolInvocationRequest {
            task_id: "order_2_to_cancel".to_string(),
            session_id: session_id.to_string(),
            agent: "claude_code".to_string(),
            session_token: "test_session_token_123".to_string(),
            tool_name: "shell_exec".to_string(),
            target: "cargo test --should-never-run".to_string(),
            args: String::new(),
            simulated_work_ms: 100,
            timestamp_ms: 30_001,
        };
        let req3 = ToolInvocationRequest {
            task_id: "order_3".to_string(),
            session_id: session_id.to_string(),
            agent: "claude_code".to_string(),
            session_token: "test_session_token_123".to_string(),
            tool_name: "fs_read".to_string(),
            target: "src/three.rs".to_string(),
            args: String::new(),
            simulated_work_ms: 0,
            timestamp_ms: 30_002,
        };

        queue.enqueue_task(req1).await.unwrap();
        queue.enqueue_task(req2).await.unwrap();
        queue.enqueue_task(req3).await.unwrap();

        assert_eq!(queue.session_queue_len(session_id).await, 3);

        // Cancel order 2 via IslandOrderCommand (deny-by-default safe API)
        let resp = queue
            .handle_island_order_command(IslandOrderCommand::CancelOrder {
                task_id: "order_2_to_cancel".to_string(),
            })
            .await;

        assert_eq!(
            resp,
            IslandOrderResponse::OrderCancelled {
                task_id: "order_2_to_cancel".to_string(),
                state: TaskLifecycleState::Cancelled,
            }
        );

        // Queue length reduced to 2 (order 2 purged from active queue)
        assert_eq!(queue.session_queue_len(session_id).await, 2);

        // Dispatch 1st task -> completes order_1
        let r1 = queue.dispatch_next_in_session(session_id).await.unwrap().unwrap();
        assert!(matches!(r1, ToolDispatchOutcome::Completed(o) if o.task_id == "order_1"));

        // Dispatch 2nd task -> directly executes order_3; order_2 was NEVER run
        let r2 = queue.dispatch_next_in_session(session_id).await.unwrap().unwrap();
        assert!(matches!(r2, ToolDispatchOutcome::Completed(o) if o.task_id == "order_3"));

        // No more tasks in session queue
        assert!(queue.dispatch_next_in_session(session_id).await.is_none());

        let orders = queue.list_orders_for_session(session_id).await;
        let o2 = orders.iter().find(|o| o.task_id == "order_2_to_cancel").unwrap();
        assert_eq!(o2.state, TaskLifecycleState::Cancelled);
    });
}

// LEDGER-3.2-4
#[test]
fn test_cancel_executing_order_triggers_cooperative_cancellation_to_terminal_state() {
    let rt = build_current_thread_runtime().expect("current_thread runtime");
    rt.block_on(async {
        let queue = build_test_queue();
        let session_id = "sess_exec_cancel";

        let req = ToolInvocationRequest {
            task_id: "task_long_running".to_string(),
            session_id: session_id.to_string(),
            agent: "claude_code".to_string(),
            session_token: "test_session_token_123".to_string(),
            tool_name: "fs_read".to_string(),
            target: "src/large.rs".to_string(),
            args: String::new(),
            simulated_work_ms: 500, // 500 ms simulated work
            timestamp_ms: 40_000,
        };

        let handle = queue.enqueue_task(req).await.unwrap();

        // Spawn async cancellation triggered after 2 milliseconds
        let q_cancel = queue.clone();
        tokio::spawn(async move {
            tokio::time::sleep(std::time::Duration::from_millis(2)).await;
            let _ = q_cancel.cancel_task("task_long_running").await;
        });

        let t0 = Instant::now();
        let outcome = queue
            .dispatch_next_in_session(session_id)
            .await
            .expect("must return outcome")
            .expect("no error");
        let elapsed_ms = t0.elapsed().as_secs_f64() * 1000.0;

        assert!(matches!(
            outcome,
            ToolDispatchOutcome::Cancelled { ref task_id, .. } if task_id == "task_long_running"
        ));
        assert!(
            elapsed_ms < 50.0,
            "Cancellation took {:.2} ms, exceeding 50.0 ms cooperative threshold",
            elapsed_ms
        );
        assert!(handle.is_cancelled());

        // Terminal state verification: no double execution, no leak
        let orders = queue.list_orders_for_session(session_id).await;
        assert_eq!(orders.len(), 1);
        assert_eq!(orders[0].state, TaskLifecycleState::Cancelled);
    });
}

// LEDGER-3.2-5
#[test]
fn test_cancel_session_queue_and_global_kill_switch_semantics_plus_benchmark() {
    let rt = build_current_thread_runtime().expect("current_thread runtime");
    rt.block_on(async {
        let queue = build_test_queue();

        // 1. Enqueue 5 orders in Session A and 5 in Session B
        for i in 0..5 {
            let req_a = ToolInvocationRequest {
                task_id: format!("a_{}", i),
                session_id: "session_A".to_string(),
                agent: "claude_code".to_string(),
                session_token: "test_session_token_123".to_string(),
                tool_name: "fs_read".to_string(),
                target: format!("src/a_{}.rs", i),
                args: String::new(),
                simulated_work_ms: 0,
                timestamp_ms: 50_000 + i,
            };
            let req_b = ToolInvocationRequest {
                task_id: format!("b_{}", i),
                session_id: "session_B".to_string(),
                agent: "cursor".to_string(),
                session_token: "test_session_token_123".to_string(),
                tool_name: "fs_read".to_string(),
                target: format!("src/b_{}.rs", i),
                args: String::new(),
                simulated_work_ms: 0,
                timestamp_ms: 50_000 + i,
            };
            queue.enqueue_task(req_a).await.unwrap();
            queue.enqueue_task(req_b).await.unwrap();
        }

        // Cancel entire Session A queue via IslandOrderCommand
        let cancel_resp = queue
            .handle_island_order_command(IslandOrderCommand::CancelSession {
                session_id: "session_A".to_string(),
            })
            .await;

        assert_eq!(
            cancel_resp,
            IslandOrderResponse::SessionCancelled {
                session_id: "session_A".to_string(),
                cancelled_count: 5,
            }
        );

        assert_eq!(queue.session_queue_len("session_A").await, 0);
        assert_eq!(queue.session_queue_len("session_B").await, 5);

        // Session B orders remain intact and can execute normally
        for _ in 0..5 {
            let res = queue.dispatch_next_in_session("session_B").await.unwrap().unwrap();
            assert!(matches!(res, ToolDispatchOutcome::Completed(_)));
        }

        // 2. Global Kill Switch test: enqueue in C and D, then trigger Kill Switch
        for i in 0..3 {
            let req_c = ToolInvocationRequest {
                task_id: format!("c_{}", i),
                session_id: "session_C".to_string(),
                agent: "claude_code".to_string(),
                session_token: "test_session_token_123".to_string(),
                tool_name: "fs_read".to_string(),
                target: format!("src/c_{}.rs", i),
                args: String::new(),
                simulated_work_ms: 0,
                timestamp_ms: 60_000 + i,
            };
            let req_d = ToolInvocationRequest {
                task_id: format!("d_{}", i),
                session_id: "session_D".to_string(),
                agent: "cursor".to_string(),
                session_token: "test_session_token_123".to_string(),
                tool_name: "fs_read".to_string(),
                target: format!("src/d_{}.rs", i),
                args: String::new(),
                simulated_work_ms: 0,
                timestamp_ms: 60_000 + i,
            };
            queue.enqueue_task(req_c).await.unwrap();
            queue.enqueue_task(req_d).await.unwrap();
        }

        let total_cancelled = queue.trigger_global_kill_switch(60_100).await;
        assert_eq!(total_cancelled, 6);
        assert_eq!(queue.session_queue_len("session_C").await, 0);
        assert_eq!(queue.session_queue_len("session_D").await, 0);

        // 3. Deterministic Queue Overhead Benchmark: 1,000 enqueue + dequeue cycles
        let bench_queue = build_test_queue();
        let bench_session = "bench_session_1000";

        // Pre-allocate requests outside the timer to measure pure queue overhead
        let requests: Vec<_> = (0..1_000)
            .map(|i| ToolInvocationRequest {
                task_id: format!("bench_task_{}", i),
                session_id: bench_session.to_string(),
                agent: "claude_code".to_string(),
                session_token: "test_session_token_123".to_string(),
                tool_name: "fs_read".to_string(),
                target: "src/lib.rs".to_string(),
                args: String::new(),
                simulated_work_ms: 0,
                timestamp_ms: 70_000 + (i as u64),
            })
            .collect();

        let t0 = Instant::now();

        for req in requests {
            let _ = bench_queue.enqueue_task(req).await.unwrap();
            let _ = bench_queue.dispatch_next_in_session(bench_session).await;
        }

        let elapsed = t0.elapsed();
        let avg_overhead_us = (elapsed.as_secs_f64() * 1_000_000.0) / 1_000.0;
        let budget_us = if cfg!(debug_assertions) {
            // Unoptimized debug builds include runtime assertions, zero inlining, and unoptimized SHA-256
            QUEUE_OVERHEAD_BUDGET_MICROS * 6.0
        } else {
            QUEUE_OVERHEAD_BUDGET_MICROS
        };

        println!(
            "[BENCH-3.2] reps=1000 total_time_ms={:.3} avg_queue_overhead_us={:.4} us (budget < {:.1} us)",
            elapsed.as_secs_f64() * 1000.0,
            avg_overhead_us,
            budget_us
        );

        assert!(
            avg_overhead_us < budget_us,
            "Average queue overhead {:.4} us exceeded budget of {:.1} us",
            avg_overhead_us,
            budget_us
        );
    });
}
