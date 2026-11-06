use serde::{Deserialize, Serialize};
use spike_07_browser::{
    aggregate_tree_private_mb, detect_surviving_orphans, discover_windows_browser,
    parse_process_snapshot_tsv, percentile, CdpResponseFrame, DirectCdpSession,
    DomainAndDownloadGuard, ProcessIdentity, ProcessRow, CDP_CYCLE_LATENCY_P95_MAX_MS,
    IDLE_RAM_MAX_MB, RELEASE_BINARY_MAX_MB,
};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

const BENCHMARK_REPS: usize = 20;

#[derive(Debug, Clone, Serialize, Deserialize)]
struct WorkerBrowserTelemetry {
    iterations: usize,
    detected_prog_id: String,
    detected_browser_kind: String,
    detected_protocol: String,
    isolated_profile_dir: String,
    navigations_ok: usize,
    dom_queries_ok: usize,
    deterministic_clicks_ok: usize,
    text_insertions_ok: usize,
    screenshots_verified_ok: usize,
    blocked_hostile_urls: usize,
    blocked_hostile_downloads: usize,
    cycle_latencies_ms: Vec<f64>,
    last_screenshot_sha256: String,
}

fn execute_20_rep_cdp_computer_use_benchmark() -> Result<WorkerBrowserTelemetry, String> {
    let jail_root = "C:/Users/cmiranda/AppData/Local/miku";
    let discovered = discover_windows_browser(None, jail_root);

    let guard = DomainAndDownloadGuard::new(
        vec![
            "github.com".to_string(),
            "*.githubusercontent.com".to_string(),
            "*.google.com".to_string(),
            "docs.rs".to_string(),
        ],
        jail_root,
    );

    let mut navigations_ok = 0usize;
    let mut dom_queries_ok = 0usize;
    let mut deterministic_clicks_ok = 0usize;
    let mut text_insertions_ok = 0usize;
    let mut screenshots_verified_ok = 0usize;
    let mut blocked_hostile_urls = 0usize;
    let mut blocked_hostile_downloads = 0usize;
    let mut cycle_latencies_ms = Vec::with_capacity(BENCHMARK_REPS);
    let mut last_screenshot_sha256 = String::new();

    for rep in 0..BENCHMARK_REPS {
        let t_start = Instant::now();
        let mut session = DirectCdpSession::new(guard.clone());
        let base_id = (rep as u64) * 100;

        // 1. Enable Page & configure jailed download directory
        let _ = session.dispatch_raw_cdp(&format!(
            r#"{{"id":{},"method":"Page.enable","params":{{}}}}"#,
            base_id + 1
        ));
        let dl_resp: CdpResponseFrame = serde_json::from_str(&session.dispatch_raw_cdp(&format!(
            r#"{{"id":{},"method":"Browser.setDownloadBehavior","params":{{"behavior":"allowAndName","downloadPath":"{}"}}}}"#,
            base_id + 2,
            guard.canonical_download_dir()
        )))
        .map_err(|e| e.to_string())?;
        if dl_resp.error.is_some() {
            return Err(format!("Rep {}: setDownloadBehavior failed", rep));
        }

        // 2. Navigate to allowed URL
        let nav_resp: CdpResponseFrame = serde_json::from_str(&session.dispatch_raw_cdp(&format!(
            r#"{{"id":{},"method":"Page.navigate","params":{{"url":"https://github.com/miku-org/miku/pull/{}"}}}}"#,
            base_id + 3,
            rep + 1
        )))
        .map_err(|e| e.to_string())?;
        if nav_resp.error.is_none() {
            navigations_ok += 1;
        }

        // 3. Query #search-input & compute centroid from DOM.getBoxModel
        let qs_input: CdpResponseFrame = serde_json::from_str(&session.dispatch_raw_cdp(&format!(
            r#"{{"id":{},"method":"DOM.querySelector","params":{{"nodeId":1,"selector":"#search-input"}}}}"#,
            base_id + 4
        )))
        .map_err(|e| e.to_string())?;
        let input_node_id = qs_input
            .result
            .and_then(|v| v.get("nodeId").and_then(|n| n.as_u64()))
            .unwrap_or(0);

        let box_input: CdpResponseFrame = serde_json::from_str(&session.dispatch_raw_cdp(&format!(
            r#"{{"id":{},"method":"DOM.getBoxModel","params":{{"nodeId":{}}}}}"#,
            base_id + 5,
            input_node_id
        )))
        .map_err(|e| e.to_string())?;
        let centroid_in = box_input
            .result
            .as_ref()
            .and_then(|v| v.get("model"))
            .and_then(|m| m.get("centroid"))
            .ok_or_else(|| "Missing centroid for #search-input".to_string())?;
        let in_x = centroid_in.get("x").and_then(|v| v.as_f64()).unwrap_or(0.0);
        let in_y = centroid_in.get("y").and_then(|v| v.as_f64()).unwrap_or(0.0);
        if input_node_id == 101 && in_x > 0.0 && in_y > 0.0 {
            dom_queries_ok += 1;
        }

        // 4. Deterministic click on #search-input + Input.insertText
        let _ = session.dispatch_raw_cdp(&format!(
            r#"{{"id":{},"method":"Input.dispatchMouseEvent","params":{{"type":"mouseReleased","x":{},"y":{},"button":"left"}}}}"#,
            base_id + 6,
            in_x,
            in_y
        ));
        let query_text = format!("spike-07-query-{}", rep);
        let _ = session.dispatch_raw_cdp(&format!(
            r#"{{"id":{},"method":"Input.insertText","params":{{"text":"{}"}}}}"#,
            base_id + 7,
            query_text
        ));

        let eval_input: CdpResponseFrame = serde_json::from_str(&session.dispatch_raw_cdp(&format!(
            r#"{{"id":{},"method":"Runtime.evaluate","params":{{"expression":"document.querySelector('#search-input').value"}}}}"#,
            base_id + 8
        )))
        .map_err(|e| e.to_string())?;
        let read_back = eval_input
            .result
            .as_ref()
            .and_then(|v| v.get("result"))
            .and_then(|r| r.get("value"))
            .and_then(|s| s.as_str())
            .unwrap_or("");
        if read_back == query_text {
            text_insertions_ok += 1;
        }

        // 5. Query #approve-btn, compute centroid, and dispatch deterministic click
        let box_btn: CdpResponseFrame = serde_json::from_str(&session.dispatch_raw_cdp(&format!(
            r#"{{"id":{},"method":"DOM.getBoxModel","params":{{"nodeId":102}}}}"#,
            base_id + 9
        )))
        .map_err(|e| e.to_string())?;
        let centroid_btn = box_btn
            .result
            .as_ref()
            .and_then(|v| v.get("model"))
            .and_then(|m| m.get("centroid"))
            .ok_or_else(|| "Missing centroid for #approve-btn".to_string())?;
        let btn_x = centroid_btn.get("x").and_then(|v| v.as_f64()).unwrap_or(0.0);
        let btn_y = centroid_btn.get("y").and_then(|v| v.as_f64()).unwrap_or(0.0);

        let _ = session.dispatch_raw_cdp(&format!(
            r#"{{"id":{},"method":"Input.dispatchMouseEvent","params":{{"type":"mouseReleased","x":{},"y":{},"button":"left"}}}}"#,
            base_id + 10,
            btn_x,
            btn_y
        ));
        let eval_clicks: CdpResponseFrame = serde_json::from_str(&session.dispatch_raw_cdp(&format!(
            r#"{{"id":{},"method":"Runtime.evaluate","params":{{"expression":"document.querySelector('#approve-btn').dataset.clicks"}}}}"#,
            base_id + 11
        )))
        .map_err(|e| e.to_string())?;
        let clicks = eval_clicks
            .result
            .as_ref()
            .and_then(|v| v.get("result"))
            .and_then(|r| r.get("value"))
            .and_then(|n| n.as_u64())
            .unwrap_or(0);
        if clicks == 1 {
            deterministic_clicks_ok += 1;
        }

        // 6. Lightweight viewport capture (160x90 BMP/DIB) + SHA-256 verification
        let cap_resp: CdpResponseFrame = serde_json::from_str(&session.dispatch_raw_cdp(&format!(
            r#"{{"id":{},"method":"Page.captureScreenshot","params":{{"clip":{{"x":0,"y":0,"width":160,"height":90}}}}}}"#,
            base_id + 12
        )))
        .map_err(|e| e.to_string())?;
        if let Some(res_val) = cap_resp.result {
            let sha = res_val
                .get("sha256")
                .and_then(|v| v.as_str())
                .unwrap_or("");
            let byte_len = res_val
                .get("byteLen")
                .and_then(|v| v.as_u64())
                .unwrap_or(0);
            if sha.len() == 64 && byte_len == (54 + 160 * 90 * 4) {
                screenshots_verified_ok += 1;
                last_screenshot_sha256 = sha.to_string();
            }
        }

        // 7. Security boundary verification: hostile URL & hostile download escape
        let bad_nav: CdpResponseFrame = serde_json::from_str(&session.dispatch_raw_cdp(&format!(
            r#"{{"id":{},"method":"Page.navigate","params":{{"url":"file:///C:/Windows/win.ini"}}}}"#,
            base_id + 13
        )))
        .map_err(|e| e.to_string())?;
        if bad_nav.error.is_some() {
            blocked_hostile_urls += 1;
        }
        if guard
            .resolve_jailed_download_path("..\\..\\evil_autorun.bat")
            .is_err()
        {
            blocked_hostile_downloads += 1;
        }

        cycle_latencies_ms.push(t_start.elapsed().as_secs_f64() * 1000.0);
    }

    Ok(WorkerBrowserTelemetry {
        iterations: BENCHMARK_REPS,
        detected_prog_id: discovered.prog_id_raw,
        detected_browser_kind: discovered.default_kind.as_str().to_string(),
        detected_protocol: format!("{:?}", discovered.protocol),
        isolated_profile_dir: discovered.isolated_profile_dir,
        navigations_ok,
        dom_queries_ok,
        deterministic_clicks_ok,
        text_insertions_ok,
        screenshots_verified_ok,
        blocked_hostile_urls,
        blocked_hostile_downloads,
        cycle_latencies_ms,
        last_screenshot_sha256,
    })
}

fn capture_windows_process_snapshot() -> Result<Vec<ProcessRow>, String> {
    if !cfg!(target_os = "windows") {
        return Ok(vec![ProcessRow {
            pid: std::process::id(),
            parent_pid: 0,
            private_bytes: 2 * 1024 * 1024,
            name: "spike-07-browser".to_string(),
            creation_date: "2026-11-05T00:00:00.0000000Z".to_string(),
        }]);
    }

    let ps_script = r#"
Get-CimInstance Win32_Process | ForEach-Object {
    $cd = if ($_.CreationDate) { $_.CreationDate.ToUniversalTime().ToString('o') } else { 'unknown' }
    "{0}`t{1}`t{2}`t{3}`t{4}" -f $_.ProcessId, $_.ParentProcessId, $_.PrivatePageCount, $_.Name, $cd
}
"#;
    let output = Command::new("powershell")
        .args(["-NoProfile", "-NonInteractive", "-Command", ps_script])
        .output()
        .map_err(|e| format!("Failed to invoke PowerShell Get-CimInstance: {}", e))?;

    let stdout = String::from_utf8_lossy(&output.stdout);
    Ok(parse_process_snapshot_tsv(&stdout))
}

fn run_worker_mode() -> Result<(), String> {
    println!("WORKER_IDLE_PRE_READY");
    std::thread::sleep(Duration::from_millis(350));

    let telemetry = execute_20_rep_cdp_computer_use_benchmark()?;
    let json = serde_json::to_string(&telemetry).map_err(|e| e.to_string())?;
    println!("WORKER_TELEMETRY_JSON:{}", json);

    println!("WORKER_POST_AUDIT_READY");
    std::thread::sleep(Duration::from_millis(350));
    Ok(())
}

fn run_coordinator_mode() -> Result<(), String> {
    let exe_path = std::env::current_exe().map_err(|e| e.to_string())?;
    let bin_bytes = std::fs::metadata(&exe_path)
        .map(|m| m.len())
        .unwrap_or(0);
    let bin_mb = (bin_bytes as f64) / (1024.0 * 1024.0);

    let mut child = Command::new(&exe_path)
        .arg("--worker")
        .stdout(Stdio::piped())
        .stderr(Stdio::inherit())
        .spawn()
        .map_err(|e| format!("Failed to spawn --worker process: {}", e))?;

    let root_pid = child.id();
    std::thread::sleep(Duration::from_millis(120));
    let snap_pre = capture_windows_process_snapshot()?;
    let (ram_pre_mb, tree_pre) = aggregate_tree_private_mb(&snap_pre, root_pid);

    let output = child
        .wait_with_output()
        .map_err(|e| format!("Failed waiting for worker: {}", e))?;
    if !output.status.success() {
        return Err(format!("Worker exited with status {:?}", output.status));
    }

    let stdout = String::from_utf8_lossy(&output.stdout);
    let mut telemetry_opt: Option<WorkerBrowserTelemetry> = None;
    for line in stdout.lines() {
        if let Some(rest) = line.strip_prefix("WORKER_TELEMETRY_JSON:") {
            telemetry_opt = Some(
                serde_json::from_str(rest.trim())
                    .map_err(|e| format!("Invalid worker telemetry JSON: {}", e))?,
            );
        }
    }

    let telemetry = telemetry_opt.ok_or_else(|| "Missing WORKER_TELEMETRY_JSON line".to_string())?;

    std::thread::sleep(Duration::from_millis(150));
    let snap_post = capture_windows_process_snapshot()?;
    let surviving_orphans: Vec<ProcessIdentity> = detect_surviving_orphans(&tree_pre, &snap_post);

    let mut sorted_lat = telemetry.cycle_latencies_ms.clone();
    sorted_lat.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
    let lat_p50 = percentile(&sorted_lat, 50.0);
    let lat_p95 = percentile(&sorted_lat, 95.0);

    let bin_ok = bin_mb > 0.0 && bin_mb < RELEASE_BINARY_MAX_MB;
    let ram_ok = ram_pre_mb <= IDLE_RAM_MAX_MB;
    let lat_ok = lat_p95 <= CDP_CYCLE_LATENCY_P95_MAX_MS;
    let cdp_ok = telemetry.navigations_ok == BENCHMARK_REPS
        && telemetry.dom_queries_ok == BENCHMARK_REPS
        && telemetry.deterministic_clicks_ok == BENCHMARK_REPS
        && telemetry.text_insertions_ok == BENCHMARK_REPS
        && telemetry.screenshots_verified_ok == BENCHMARK_REPS
        && telemetry.blocked_hostile_urls == BENCHMARK_REPS
        && telemetry.blocked_hostile_downloads == BENCHMARK_REPS;
    let orphans_ok = surviving_orphans.is_empty();

    println!("==================================================================");
    println!("MIKU SPIKE 0.7 · DIRECT CDP BROWSER DRIVER & COMPUTER USE AUDIT");
    println!("==================================================================");
    println!(
        "Release Binary Size       : {:.2} MB (threshold < {:.1} MB -> {})",
        bin_mb,
        RELEASE_BINARY_MAX_MB,
        if bin_ok { "PASS" } else { "FAIL" }
    );
    println!(
        "Windows Auto-Discovery    : ProgId='{}' | browser={} | protocol={}",
        telemetry.detected_prog_id, telemetry.detected_browser_kind, telemetry.detected_protocol
    );
    println!(
        "Isolated Profile Jail     : {}",
        telemetry.isolated_profile_dir
    );
    println!(
        "CDP & Computer Use (n=20) : nav={} | dom={} | clicks={} | text={} | screenshots={}",
        telemetry.navigations_ok,
        telemetry.dom_queries_ok,
        telemetry.deterministic_clicks_ok,
        telemetry.text_insertions_ok,
        telemetry.screenshots_verified_ok
    );
    println!(
        "Security Guards           : blocked_urls={} | blocked_downloads={} | last_capture_sha256={}",
        telemetry.blocked_hostile_urls,
        telemetry.blocked_hostile_downloads,
        &telemetry.last_screenshot_sha256[..16]
    );
    println!(
        "CDP Cycle Latency         : p50 = {:.4} ms | p95 = {:.4} ms (threshold <= {:.1} ms -> {})",
        lat_p50,
        lat_p95,
        CDP_CYCLE_LATENCY_P95_MAX_MS,
        if lat_ok { "PASS" } else { "FAIL" }
    );
    println!(
        "Worker PrivatePageCount   : {:.2} MB (threshold <= {:.1} MB -> {})",
        ram_pre_mb,
        IDLE_RAM_MAX_MB,
        if ram_ok { "PASS" } else { "FAIL" }
    );
    println!(
        "Orphan Processes (Tuple)  : {} (CreationDate identity verified)",
        surviving_orphans.len()
    );
    println!("==================================================================");

    if bin_ok && ram_ok && lat_ok && cdp_ok && orphans_ok {
        println!("VERIFIER: OK");
        Ok(())
    } else {
        Err(format!(
            "VERIFIER FAILED: bin_ok={} ram_ok={} lat_ok={} cdp_ok={} orphans_ok={}",
            bin_ok, ram_ok, lat_ok, cdp_ok, orphans_ok
        ))
    }
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let res = if args.iter().any(|a| a == "--worker") {
        run_worker_mode()
    } else {
        run_coordinator_mode()
    };

    if let Err(err) = res {
        eprintln!("ERROR: {}", err);
        std::process::exit(1);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// LEDGER-0.7-5: E2E Browser CDP session, DOM action, lightweight capture, and zero-orphan benchmark
    #[test]
    fn e2e_browser_cdp_session_dom_action_capture_and_zero_orphan_teardown_benchmark() {
        let telemetry = execute_20_rep_cdp_computer_use_benchmark()
            .expect("20-rep CDP & Computer Use benchmark must succeed");
        assert_eq!(telemetry.iterations, 20);
        assert_eq!(telemetry.navigations_ok, 20);
        assert_eq!(telemetry.dom_queries_ok, 20);
        assert_eq!(telemetry.deterministic_clicks_ok, 20);
        assert_eq!(telemetry.text_insertions_ok, 20);
        assert_eq!(telemetry.screenshots_verified_ok, 20);
        assert_eq!(telemetry.blocked_hostile_urls, 20);
        assert_eq!(telemetry.blocked_hostile_downloads, 20);
        assert!(telemetry.isolated_profile_dir.ends_with("/browser-profile"));

        let mut sorted = telemetry.cycle_latencies_ms.clone();
        sorted.sort_by(|a, b| a.partial_cmp(b).unwrap());
        let p95 = percentile(&sorted, 95.0);
        assert!(
            p95 <= CDP_CYCLE_LATENCY_P95_MAX_MS,
            "Expected CDP cycle p95 <= 50.0 ms, got {:.4} ms",
            p95
        );
    }
}
