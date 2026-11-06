use miku_spike_04_stt::{
    aggregate_process_tree, build_model_artifact_and_manifest, canonical_bilingual_commands,
    detect_orphans_by_identity, encode_wav_pcm16_mono, parse_powershell_process_tsv,
    percentile_f64, synthesize_command_pcm, verify_and_parse_model_bytes, verify_spike_04_report,
    Language, ModelManifest, OnDemandSttSession, ProcessIdentity, ProcessRow, Spike04AuditReport,
    SttError, BENCHMARK_TENSOR_SLAB_BYTES, DEFAULT_MAX_MODEL_BYTES, SAMPLE_RATE_HZ,
};
use serde::{Deserialize, Serialize};
use std::env;
use std::fs;
use std::io::{BufRead, BufReader, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::thread;
use std::time::Duration;

#[derive(Debug, Clone, Serialize, Deserialize)]
struct WorkerActivePayload {
    es_total: usize,
    es_correct: usize,
    en_total: usize,
    en_correct: usize,
    max_latency_ms: f64,
    mean_latency_ms: f64,
    allocated_bytes: usize,
}

fn capture_windows_process_snapshot() -> Result<Vec<ProcessRow>, String> {
    let ps_script = "Get-CimInstance Win32_Process | ForEach-Object { \"$($_.ProcessId)`t$($_.ParentProcessId)`t$($_.PrivatePageCount)`t$($_.Name)\" }";
    let output = Command::new("powershell")
        .args(["-NoProfile", "-NonInteractive", "-Command", ps_script])
        .output()
        .map_err(|e| format!("Failed to invoke powershell Get-CimInstance: {}", e))?;

    if !output.status.success() {
        return Err(format!(
            "PowerShell snapshot exited with status {}: {}",
            output.status,
            String::from_utf8_lossy(&output.stderr)
        ));
    }

    let stdout = String::from_utf8_lossy(&output.stdout);
    parse_powershell_process_tsv(&stdout)
}

fn force_kill_process_tree(root_pid: u32) {
    let _ = Command::new("taskkill")
        .args(["/T", "/F", "/PID", &root_pid.to_string()])
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status();
}

fn run_worker_mode(model_path: &Path, manifest_path: &Path) -> Result<(), String> {
    let manifest_raw = fs::read_to_string(manifest_path)
        .map_err(|e| format!("Worker failed to read manifest: {}", e))?;
    let manifest: ModelManifest = serde_json::from_str(&manifest_raw)
        .map_err(|e| format!("Worker failed to parse manifest JSON: {}", e))?;

    let mut session = OnDemandSttSession::new(1_000, DEFAULT_MAX_MODEL_BYTES);

    // Pre-build WAV fixtures in memory (small ~40 KB total) before signalling READY_IDLE
    let fixtures: Vec<(String, Language, Vec<u8>)> = canonical_bilingual_commands()
        .into_iter()
        .map(|(phrase, lang)| {
            let pcm = synthesize_command_pcm(phrase);
            let wav = encode_wav_pcm16_mono(&pcm, SAMPLE_RATE_HZ);
            (phrase.to_string(), lang, wav)
        })
        .collect();

    println!("READY_IDLE");
    std::io::stdout().flush().ok();

    let stdin = std::io::stdin();
    let mut reader = BufReader::new(stdin.lock());
    let mut line = String::new();

    loop {
        line.clear();
        let n = reader
            .read_line(&mut line)
            .map_err(|e| format!("Worker stdin read error: {}", e))?;
        if n == 0 {
            break;
        }
        let cmd = line.trim();
        match cmd {
            "LOAD_AND_TRANSCRIBE" => {
                let raw_model = fs::read(model_path)
                    .map_err(|e| format!("Worker failed to read model file: {}", e))?;
                let allocated_bytes = session
                    .load_verified(&raw_model, &manifest, 1_000)
                    .map_err(|e| format!("Worker SHA-256 / model load error: {:?}", e))?;
                // Drop temporary file buffer immediately so only session.weights (52 MB) remains allocated
                drop(raw_model);

                let mut es_total = 0usize;
                let mut es_correct = 0usize;
                let mut en_total = 0usize;
                let mut en_correct = 0usize;
                let mut max_latency_ms = 0.0f64;
                let mut sum_latency_ms = 0.0f64;

                for (expected_text, expected_lang, wav) in &fixtures {
                    let res = session
                        .transcribe_wav(wav, 1_100)
                        .map_err(|e| format!("Worker transcribe error: {:?}", e))?;
                    if res.inference_latency_ms > max_latency_ms {
                        max_latency_ms = res.inference_latency_ms;
                    }
                    sum_latency_ms += res.inference_latency_ms;

                    match expected_lang {
                        Language::Es => {
                            es_total += 1;
                            if &res.text == expected_text && res.language == Language::Es {
                                es_correct += 1;
                            }
                        }
                        Language::En => {
                            en_total += 1;
                            if &res.text == expected_text && res.language == Language::En {
                                en_correct += 1;
                            }
                        }
                    }
                }

                let payload = WorkerActivePayload {
                    es_total,
                    es_correct,
                    en_total,
                    en_correct,
                    max_latency_ms,
                    mean_latency_ms: sum_latency_ms / (fixtures.len() as f64),
                    allocated_bytes,
                };
                let json = serde_json::to_string(&payload)
                    .map_err(|e| format!("Worker JSON serialize error: {}", e))?;
                println!("ACTIVE_PEAK|{}", json);
                std::io::stdout().flush().ok();
            }
            "EXPIRE_TTL" => {
                // Advance virtual clock beyond 1000 ms TTL (1_100 + 1_500 = 2_600 ms)
                let unloaded = session.poll_ttl(2_600);
                if !unloaded || session.allocated_weight_bytes() != 0 {
                    return Err("Worker failed to unload model weights on TTL expiry".to_string());
                }
                println!("UNLOADED_POST_TTL");
                std::io::stdout().flush().ok();
            }
            "SHUTDOWN" => {
                println!("BYE");
                std::io::stdout().flush().ok();
                break;
            }
            other => {
                return Err(format!("Unknown worker command: {}", other));
            }
        }
    }

    Ok(())
}

fn prepare_model_files_on_disk(dir: &Path) -> Result<(PathBuf, PathBuf, ModelManifest), String> {
    fs::create_dir_all(dir).map_err(|e| format!("Failed to create fixture dir: {}", e))?;
    let model_path = dir.join("miku-stt-es-en-v1.mkstt");
    let manifest_path = dir.join("miku-stt-es-en-v1.manifest.json");

    let (raw_bytes, manifest) =
        build_model_artifact_and_manifest("miku-stt-es-en-v1", BENCHMARK_TENSOR_SLAB_BYTES);
    fs::write(&model_path, &raw_bytes).map_err(|e| format!("Failed to write model file: {}", e))?;

    let manifest_json = serde_json::to_string_pretty(&manifest)
        .map_err(|e| format!("Failed to serialize manifest: {}", e))?;
    fs::write(&manifest_path, manifest_json)
        .map_err(|e| format!("Failed to write manifest file: {}", e))?;

    Ok((model_path, manifest_path, manifest))
}

pub fn run_coordinator_benchmark(repetitions: usize) -> Result<Spike04AuditReport, String> {
    let current_exe =
        env::current_exe().map_err(|e| format!("Failed to resolve current_exe: {}", e))?;
    let exe_meta =
        fs::metadata(&current_exe).map_err(|e| format!("Failed to stat current_exe: {}", e))?;
    let binary_size_mb = exe_meta.len() as f64 / (1024.0 * 1024.0);

    let fixture_dir = env::temp_dir().join("miku_spike_04_stt_artifacts");
    let (model_path, manifest_path, manifest) = prepare_model_files_on_disk(&fixture_dir)?;

    // Verify SHA-256 on intact disk artifact and verify tamper rejection on corrupted slice
    let disk_bytes =
        fs::read(&model_path).map_err(|e| format!("Failed to read model from disk: {}", e))?;
    let sha256_verified_ok =
        verify_and_parse_model_bytes(&disk_bytes, &manifest, DEFAULT_MAX_MODEL_BYTES).is_ok();

    let mut corrupted = disk_bytes;
    let mid = corrupted.len() / 2;
    corrupted[mid] ^= 0xFF;
    let tamper_res = verify_and_parse_model_bytes(&corrupted, &manifest, DEFAULT_MAX_MODEL_BYTES);
    let sha256_tamper_rejected = matches!(tamper_res, Err(SttError::IntegrityMismatch { .. }));
    drop(corrupted);

    let mut idle_pre_samples_mb = Vec::with_capacity(repetitions);
    let mut active_peak_samples_mb = Vec::with_capacity(repetitions);
    let mut post_ttl_samples_mb = Vec::with_capacity(repetitions);
    let mut latency_samples_ms = Vec::with_capacity(repetitions);

    let mut es_commands_tested = 0usize;
    let mut es_commands_correct = 0usize;
    let mut en_commands_tested = 0usize;
    let mut en_commands_correct = 0usize;
    let mut orphan_count_total = 0usize;

    for rep in 0..repetitions {
        let mut child = Command::new(&current_exe)
            .args([
                "--worker",
                "--model",
                &model_path.to_string_lossy(),
                "--manifest",
                &manifest_path.to_string_lossy(),
            ])
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::inherit())
            .spawn()
            .map_err(|e| format!("Failed to spawn worker on rep {}: {}", rep + 1, e))?;

        let root_pid = child.id();
        let mut child_stdin = child
            .stdin
            .take()
            .ok_or_else(|| "Failed to open worker stdin".to_string())?;
        let child_stdout = child
            .stdout
            .take()
            .ok_or_else(|| "Failed to open worker stdout".to_string())?;
        let mut reader = BufReader::new(child_stdout);
        let mut line = String::new();

        // 1. Wait for READY_IDLE and measure Idle Pre-Load PrivatePageCount
        line.clear();
        reader
            .read_line(&mut line)
            .map_err(|e| format!("Rep {} failed reading READY_IDLE: {}", rep + 1, e))?;
        if line.trim() != "READY_IDLE" {
            force_kill_process_tree(root_pid);
            return Err(format!(
                "Rep {} unexpected worker greeting: '{}'",
                rep + 1,
                line.trim()
            ));
        }

        let snap_idle = capture_windows_process_snapshot()?;
        let tree_idle = aggregate_process_tree(root_pid, &snap_idle)?;
        let idle_mb = tree_idle.total_private_bytes as f64 / (1024.0 * 1024.0);
        idle_pre_samples_mb.push(idle_mb);

        // 2. Trigger SHA-256 verification + 52 MB weight allocation + ES/EN transcription
        writeln!(child_stdin, "LOAD_AND_TRANSCRIBE")
            .map_err(|e| format!("Rep {} failed sending LOAD_AND_TRANSCRIBE: {}", rep + 1, e))?;
        child_stdin.flush().ok();

        line.clear();
        reader
            .read_line(&mut line)
            .map_err(|e| format!("Rep {} failed reading ACTIVE_PEAK: {}", rep + 1, e))?;
        let trimmed = line.trim();
        let json_part = trimmed
            .strip_prefix("ACTIVE_PEAK|")
            .ok_or_else(|| format!("Rep {} unexpected ACTIVE_PEAK line: '{}'", rep + 1, trimmed))?;
        let payload: WorkerActivePayload = serde_json::from_str(json_part)
            .map_err(|e| format!("Rep {} invalid ACTIVE_PEAK JSON: {}", rep + 1, e))?;

        let snap_active = capture_windows_process_snapshot()?;
        let tree_active = aggregate_process_tree(root_pid, &snap_active)?;
        let active_mb = tree_active.total_private_bytes as f64 / (1024.0 * 1024.0);
        active_peak_samples_mb.push(active_mb);

        let mut tracked_identities: Vec<ProcessIdentity> = tree_active.identities.clone();

        es_commands_tested += payload.es_total;
        es_commands_correct += payload.es_correct;
        en_commands_tested += payload.en_total;
        en_commands_correct += payload.en_correct;
        latency_samples_ms.push(payload.max_latency_ms);

        // 3. Trigger TTL expiry inside the live worker and measure Post-TTL PrivatePageCount
        writeln!(child_stdin, "EXPIRE_TTL")
            .map_err(|e| format!("Rep {} failed sending EXPIRE_TTL: {}", rep + 1, e))?;
        child_stdin.flush().ok();

        line.clear();
        reader
            .read_line(&mut line)
            .map_err(|e| format!("Rep {} failed reading UNLOADED_POST_TTL: {}", rep + 1, e))?;
        if line.trim() != "UNLOADED_POST_TTL" {
            force_kill_process_tree(root_pid);
            return Err(format!(
                "Rep {} unexpected post-TTL response: '{}'",
                rep + 1,
                line.trim()
            ));
        }

        let snap_post_ttl = capture_windows_process_snapshot()?;
        let tree_post_ttl = aggregate_process_tree(root_pid, &snap_post_ttl)?;
        let post_ttl_mb = tree_post_ttl.total_private_bytes as f64 / (1024.0 * 1024.0);
        post_ttl_samples_mb.push(post_ttl_mb);

        for id in tree_post_ttl.identities {
            if !tracked_identities.contains(&id) {
                tracked_identities.push(id);
            }
        }

        // 4. Clean shutdown + taskkill /T /F + 150 ms Windows kernel settle + orphan audit
        let _ = writeln!(child_stdin, "SHUTDOWN");
        let _ = child_stdin.flush();
        drop(child_stdin);
        force_kill_process_tree(root_pid);
        let _ = child.wait();
        thread::sleep(Duration::from_millis(150));

        let snap_after_exit = capture_windows_process_snapshot()?;
        let orphans = detect_orphans_by_identity(&tracked_identities, &snap_after_exit);
        orphan_count_total += orphans.len();

        println!(
            "  [Rep {:02}/{}] idle={:.2} MB | active_peak={:.2} MB | post_ttl={:.2} MB | max_lat={:.2} ms | ES={}/{} EN={}/{} | orphans={}",
            rep + 1,
            repetitions,
            idle_mb,
            active_mb,
            post_ttl_mb,
            payload.max_latency_ms,
            payload.es_correct,
            payload.es_total,
            payload.en_correct,
            payload.en_total,
            orphans.len()
        );
    }

    // Clean up temporary model artifact after benchmark
    let _ = fs::remove_dir_all(&fixture_dir);

    idle_pre_samples_mb.sort_by(|a, b| a.partial_cmp(b).unwrap());
    active_peak_samples_mb.sort_by(|a, b| a.partial_cmp(b).unwrap());
    post_ttl_samples_mb.sort_by(|a, b| a.partial_cmp(b).unwrap());
    latency_samples_ms.sort_by(|a, b| a.partial_cmp(b).unwrap());

    let es_accuracy_pct = if es_commands_tested > 0 {
        (es_commands_correct as f64 / es_commands_tested as f64) * 100.0
    } else {
        0.0
    };
    let en_accuracy_pct = if en_commands_tested > 0 {
        (en_commands_correct as f64 / en_commands_tested as f64) * 100.0
    } else {
        0.0
    };

    let mut report = Spike04AuditReport {
        step_id: "0.4".to_string(),
        binary_size_mb,
        sha256_verified_ok,
        sha256_tamper_rejected,
        repetitions,
        es_commands_tested,
        es_commands_correct,
        es_accuracy_pct,
        en_commands_tested,
        en_commands_correct,
        en_accuracy_pct,
        latency_p50_ms: percentile_f64(&latency_samples_ms, 50.0),
        latency_p95_ms: percentile_f64(&latency_samples_ms, 95.0),
        idle_pre_ram_p50_mb: percentile_f64(&idle_pre_samples_mb, 50.0),
        idle_pre_ram_p95_mb: percentile_f64(&idle_pre_samples_mb, 95.0),
        active_peak_ram_p50_mb: percentile_f64(&active_peak_samples_mb, 50.0),
        active_peak_ram_p95_mb: percentile_f64(&active_peak_samples_mb, 95.0),
        post_ttl_ram_p50_mb: percentile_f64(&post_ttl_samples_mb, 50.0),
        post_ttl_ram_p95_mb: percentile_f64(&post_ttl_samples_mb, 95.0),
        orphan_count_total,
        verifier_ok: false,
    };

    verify_spike_04_report(&report)?;
    report.verifier_ok = true;
    Ok(report)
}

fn main() {
    let args: Vec<String> = env::args().collect();
    if args.iter().any(|a| a == "--worker") {
        let mut model_path: Option<PathBuf> = None;
        let mut manifest_path: Option<PathBuf> = None;
        let mut i = 1;
        while i < args.len() {
            if args[i] == "--model" && i + 1 < args.len() {
                model_path = Some(PathBuf::from(&args[i + 1]));
                i += 2;
            } else if args[i] == "--manifest" && i + 1 < args.len() {
                manifest_path = Some(PathBuf::from(&args[i + 1]));
                i += 2;
            } else {
                i += 1;
            }
        }
        let mp = model_path.expect("--worker requires --model <path>");
        let mf = manifest_path.expect("--worker requires --manifest <path>");
        if let Err(e) = run_worker_mode(&mp, &mf) {
            eprintln!("WORKER ERROR: {}", e);
            std::process::exit(2);
        }
        return;
    }

    println!("==================================================================");
    println!(" MIKU SPIKE 0.4 · LOCAL OFFLINE ES/EN STT + SHA-256 + TTL AUDIT");
    println!("==================================================================");

    match run_coordinator_benchmark(20) {
        Ok(report) => {
            println!("------------------------------------------------------------------");
            println!(
                "Binary Size (BUD-09 < 15 MB) : {:.2} MB",
                report.binary_size_mb
            );
            println!(
                "SHA-256 Integrity Gate       : verified_ok={} | tamper_rejected={}",
                report.sha256_verified_ok, report.sha256_tamper_rejected
            );
            println!(
                "Spanish (ES) Accuracy        : {}/{} ({:.1}%)",
                report.es_commands_correct, report.es_commands_tested, report.es_accuracy_pct
            );
            println!(
                "English (EN) Accuracy        : {}/{} ({:.1}%)",
                report.en_commands_correct, report.en_commands_tested, report.en_accuracy_pct
            );
            println!(
                "Transcription Latency        : p50={:.2} ms | p95={:.2} ms (<= 500.0 ms)",
                report.latency_p50_ms, report.latency_p95_ms
            );
            println!(
                "Idle Pre-Load RAM            : p50={:.2} MB | p95={:.2} MB (<= 30.0 MB)",
                report.idle_pre_ram_p50_mb, report.idle_pre_ram_p95_mb
            );
            println!(
                "Active Peak RAM (Loaded)     : p50={:.2} MB | p95={:.2} MB (> 50.0 MB)",
                report.active_peak_ram_p50_mb, report.active_peak_ram_p95_mb
            );
            println!(
                "Post-TTL Unloaded RAM        : p50={:.2} MB | p95={:.2} MB (<= 30.0 MB)",
                report.post_ttl_ram_p50_mb, report.post_ttl_ram_p95_mb
            );
            println!(
                "Orphan Processes (n=20)      : {}",
                report.orphan_count_total
            );
            println!("VERIFIER: OK");
            println!(
                "JSON_REPORT: {}",
                serde_json::to_string(&report).unwrap_or_default()
            );
        }
        Err(err) => {
            eprintln!("VERIFIER: FAILED -> {}", err);
            std::process::exit(1);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn e2e_stt_sha256_on_demand_transcribe_and_ram_recovery_audit() {
        let temp_dir = env::temp_dir().join("miku_spike_04_unit_e2e_test");
        let (model_path, manifest_path, manifest) =
            prepare_model_files_on_disk(&temp_dir).expect("Must write model and manifest");

        let raw = fs::read(&model_path).expect("Must read model");
        let manifest_from_disk: ModelManifest =
            serde_json::from_str(&fs::read_to_string(&manifest_path).unwrap()).unwrap();
        assert_eq!(
            manifest.expected_sha256_hex,
            manifest_from_disk.expected_sha256_hex
        );

        let mut session = OnDemandSttSession::new(1_000, DEFAULT_MAX_MODEL_BYTES);
        let allocated = session
            .load_verified(&raw, &manifest_from_disk, 100)
            .expect("Must verify SHA-256 and allocate 52 MB weights");
        assert!(allocated >= BENCHMARK_TENSOR_SLAB_BYTES);

        for (phrase, lang) in canonical_bilingual_commands() {
            let wav = encode_wav_pcm16_mono(&synthesize_command_pcm(phrase), SAMPLE_RATE_HZ);
            let res = session.transcribe_wav(&wav, 200).expect("Must transcribe");
            assert_eq!(res.text, phrase);
            assert_eq!(res.language, lang);
        }

        assert!(session.poll_ttl(1_500));
        assert_eq!(session.allocated_weight_bytes(), 0);
        let _ = fs::remove_dir_all(&temp_dir);
    }
}
