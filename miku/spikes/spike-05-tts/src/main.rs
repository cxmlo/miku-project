use miku_spike_05_tts::{
    aggregate_process_tree, build_voice_pack_and_manifest, canonical_bilingual_tts_phrases,
    detect_orphans_by_identity, parse_powershell_process_tsv, percentile_f64,
    verify_and_parse_voice_pack, verify_spike_05_report, Language, OnDemandTtsSession,
    ProcessIdentity, ProcessRow, Spike05AuditReport, TtsError, VoicePackManifest,
    BENCHMARK_VOICE_SLAB_BYTES, DEFAULT_MAX_VOICE_PACK_BYTES, DEFAULT_SAMPLE_RATE_HZ,
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
struct WorkerActiveTtsPayload {
    es_count: usize,
    en_count: usize,
    total_chunks: usize,
    underrun_chunks: usize,
    max_ttfa_ms: f64,
    max_rtf: f64,
    allocated_bytes: usize,
}

fn capture_windows_process_snapshot() -> Result<Vec<ProcessRow>, String> {
    let ps_script = "Get-CimInstance Win32_Process | ForEach-Object { $cd = if ($_.CreationDate) { $_.CreationDate.ToUniversalTime().ToString('o') } else { 'unknown' }; \"$($_.ProcessId)`t$($_.ParentProcessId)`t$($_.PrivatePageCount)`t$cd`t$($_.Name)\" }";
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

fn run_worker_mode(voice_pack_path: &Path, manifest_path: &Path) -> Result<(), String> {
    let manifest_raw = fs::read_to_string(manifest_path)
        .map_err(|e| format!("Worker failed to read manifest: {}", e))?;
    let manifest: VoicePackManifest = serde_json::from_str(&manifest_raw)
        .map_err(|e| format!("Worker failed to parse manifest JSON: {}", e))?;

    let mut session = OnDemandTtsSession::new(1_000, DEFAULT_MAX_VOICE_PACK_BYTES);

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
            "LOAD_AND_STREAM_TTS" => {
                let raw_pack = fs::read(voice_pack_path)
                    .map_err(|e| format!("Worker failed to read voice pack file: {}", e))?;
                let allocated_bytes = session
                    .load_verified(&raw_pack, &manifest, 1_000)
                    .map_err(|e| format!("Worker SHA-256 / voice pack load error: {:?}", e))?;
                // Drop temporary file buffer so only session.voice_bank (48 MB) remains committed
                drop(raw_pack);

                let mut es_count = 0usize;
                let mut en_count = 0usize;
                let mut total_chunks = 0usize;
                let mut underrun_chunks = 0usize;
                let mut max_ttfa_ms = 0.0f64;
                let mut max_rtf = 0.0f64;

                for (phrase, lang) in canonical_bilingual_tts_phrases() {
                    let (summary, _chunks) = session
                        .synthesize_streaming_utterance(
                            phrase,
                            lang,
                            DEFAULT_SAMPLE_RATE_HZ,
                            1_100,
                        )
                        .map_err(|e| format!("Worker synthesis error: {:?}", e))?;

                    match lang {
                        Language::Es => es_count += 1,
                        Language::En => en_count += 1,
                    }
                    total_chunks += summary.chunk_count;
                    underrun_chunks += summary.underrun_chunks;
                    if summary.ttfa_ms > max_ttfa_ms {
                        max_ttfa_ms = summary.ttfa_ms;
                    }
                    if summary.rtf > max_rtf {
                        max_rtf = summary.rtf;
                    }
                }

                let payload = WorkerActiveTtsPayload {
                    es_count,
                    en_count,
                    total_chunks,
                    underrun_chunks,
                    max_ttfa_ms,
                    max_rtf,
                    allocated_bytes,
                };
                let json = serde_json::to_string(&payload)
                    .map_err(|e| format!("Worker JSON serialize error: {}", e))?;
                println!("ACTIVE_PEAK|{}", json);
                std::io::stdout().flush().ok();
            }
            "EXPIRE_TTL" => {
                let unloaded = session.poll_ttl(2_600);
                if !unloaded || session.allocated_voice_bytes() != 0 {
                    return Err("Worker failed to unload voice bank on TTL expiry".to_string());
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

fn prepare_voice_pack_on_disk(
    dir: &Path,
) -> Result<(PathBuf, PathBuf, VoicePackManifest), String> {
    fs::create_dir_all(dir).map_err(|e| format!("Failed to create fixture dir: {}", e))?;
    let pack_path = dir.join("miku-voice-es-en-v1.mktts");
    let manifest_path = dir.join("miku-voice-es-en-v1.manifest.json");

    let (raw_bytes, manifest) =
        build_voice_pack_and_manifest("miku-voice-es-en-v1", BENCHMARK_VOICE_SLAB_BYTES);
    fs::write(&pack_path, &raw_bytes)
        .map_err(|e| format!("Failed to write voice pack file: {}", e))?;

    let manifest_json = serde_json::to_string_pretty(&manifest)
        .map_err(|e| format!("Failed to serialize manifest: {}", e))?;
    fs::write(&manifest_path, manifest_json)
        .map_err(|e| format!("Failed to write manifest file: {}", e))?;

    Ok((pack_path, manifest_path, manifest))
}

pub fn run_coordinator_benchmark(repetitions: usize) -> Result<Spike05AuditReport, String> {
    let current_exe =
        env::current_exe().map_err(|e| format!("Failed to resolve current_exe: {}", e))?;
    let exe_meta =
        fs::metadata(&current_exe).map_err(|e| format!("Failed to stat current_exe: {}", e))?;
    let binary_size_mb = exe_meta.len() as f64 / (1024.0 * 1024.0);

    let fixture_dir = env::temp_dir().join("miku_spike_05_tts_artifacts");
    let (pack_path, manifest_path, manifest) = prepare_voice_pack_on_disk(&fixture_dir)?;

    let disk_bytes =
        fs::read(&pack_path).map_err(|e| format!("Failed to read voice pack from disk: {}", e))?;
    let sha256_verified_ok =
        verify_and_parse_voice_pack(&disk_bytes, &manifest, DEFAULT_MAX_VOICE_PACK_BYTES).is_ok();

    let mut corrupted = disk_bytes;
    let mid = corrupted.len() / 2;
    corrupted[mid] ^= 0xFF;
    let tamper_res =
        verify_and_parse_voice_pack(&corrupted, &manifest, DEFAULT_MAX_VOICE_PACK_BYTES);
    let sha256_tamper_rejected = matches!(tamper_res, Err(TtsError::IntegrityMismatch { .. }));
    drop(corrupted);

    let mut idle_pre_samples_mb = Vec::with_capacity(repetitions);
    let mut active_peak_samples_mb = Vec::with_capacity(repetitions);
    let mut post_ttl_samples_mb = Vec::with_capacity(repetitions);
    let mut ttfa_samples_ms = Vec::with_capacity(repetitions);
    let mut rtf_samples = Vec::with_capacity(repetitions);

    let mut es_utterances_synthesized = 0usize;
    let mut en_utterances_synthesized = 0usize;
    let mut total_chunks_streamed = 0usize;
    let mut underrun_chunks_total = 0usize;
    let mut orphan_count_total = 0usize;

    for rep in 0..repetitions {
        let mut child = Command::new(&current_exe)
            .args([
                "--worker",
                "--voice-pack",
                &pack_path.to_string_lossy(),
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

        // 2. Trigger SHA-256 verification + 48 MB voice bank load + streaming TTS synthesis
        writeln!(child_stdin, "LOAD_AND_STREAM_TTS").map_err(|e| {
            format!(
                "Rep {} failed sending LOAD_AND_STREAM_TTS: {}",
                rep + 1,
                e
            )
        })?;
        child_stdin.flush().ok();

        line.clear();
        reader
            .read_line(&mut line)
            .map_err(|e| format!("Rep {} failed reading ACTIVE_PEAK: {}", rep + 1, e))?;
        let trimmed = line.trim();
        let json_part = trimmed
            .strip_prefix("ACTIVE_PEAK|")
            .ok_or_else(|| format!("Rep {} unexpected ACTIVE_PEAK line: '{}'", rep + 1, trimmed))?;
        let payload: WorkerActiveTtsPayload = serde_json::from_str(json_part)
            .map_err(|e| format!("Rep {} invalid ACTIVE_PEAK JSON: {}", rep + 1, e))?;

        let snap_active = capture_windows_process_snapshot()?;
        let tree_active = aggregate_process_tree(root_pid, &snap_active)?;
        let active_mb = tree_active.total_private_bytes as f64 / (1024.0 * 1024.0);
        active_peak_samples_mb.push(active_mb);

        let mut tracked_identities: Vec<ProcessIdentity> = tree_active.identities.clone();

        es_utterances_synthesized += payload.es_count;
        en_utterances_synthesized += payload.en_count;
        total_chunks_streamed += payload.total_chunks;
        underrun_chunks_total += payload.underrun_chunks;
        ttfa_samples_ms.push(payload.max_ttfa_ms);
        rtf_samples.push(payload.max_rtf);

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

        // 4. Clean shutdown + taskkill /T /F + 150 ms Windows kernel settle + orphan audit with CreationDate
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
            "  [Rep {:02}/{}] idle={:.2} MB | active_peak={:.2} MB | post_ttl={:.2} MB | TTFA={:.3} ms | RTF={:.5} | chunks={} | orphans={}",
            rep + 1,
            repetitions,
            idle_mb,
            active_mb,
            post_ttl_mb,
            payload.max_ttfa_ms,
            payload.max_rtf,
            payload.total_chunks,
            orphans.len()
        );
    }

    let _ = fs::remove_dir_all(&fixture_dir);

    idle_pre_samples_mb.sort_by(|a, b| a.partial_cmp(b).unwrap());
    active_peak_samples_mb.sort_by(|a, b| a.partial_cmp(b).unwrap());
    post_ttl_samples_mb.sort_by(|a, b| a.partial_cmp(b).unwrap());
    ttfa_samples_ms.sort_by(|a, b| a.partial_cmp(b).unwrap());
    rtf_samples.sort_by(|a, b| a.partial_cmp(b).unwrap());

    let mut report = Spike05AuditReport {
        step_id: "0.5".to_string(),
        binary_size_mb,
        sha256_verified_ok,
        sha256_tamper_rejected,
        repetitions,
        sample_rate_hz: DEFAULT_SAMPLE_RATE_HZ,
        es_utterances_synthesized,
        en_utterances_synthesized,
        total_chunks_streamed,
        underrun_chunks_total,
        ttfa_p50_ms: percentile_f64(&ttfa_samples_ms, 50.0),
        ttfa_p95_ms: percentile_f64(&ttfa_samples_ms, 95.0),
        rtf_p50: percentile_f64(&rtf_samples, 50.0),
        rtf_p95: percentile_f64(&rtf_samples, 95.0),
        idle_pre_ram_p50_mb: percentile_f64(&idle_pre_samples_mb, 50.0),
        idle_pre_ram_p95_mb: percentile_f64(&idle_pre_samples_mb, 95.0),
        active_peak_ram_p50_mb: percentile_f64(&active_peak_samples_mb, 50.0),
        active_peak_ram_p95_mb: percentile_f64(&active_peak_samples_mb, 95.0),
        post_ttl_ram_p50_mb: percentile_f64(&post_ttl_samples_mb, 50.0),
        post_ttl_ram_p95_mb: percentile_f64(&post_ttl_samples_mb, 95.0),
        orphan_count_total,
        verifier_ok: false,
    };

    verify_spike_05_report(&report)?;
    report.verifier_ok = true;
    Ok(report)
}

fn main() {
    let args: Vec<String> = env::args().collect();
    if args.iter().any(|a| a == "--worker") {
        let mut pack_path: Option<PathBuf> = None;
        let mut manifest_path: Option<PathBuf> = None;
        let mut i = 1;
        while i < args.len() {
            if args[i] == "--voice-pack" && i + 1 < args.len() {
                pack_path = Some(PathBuf::from(&args[i + 1]));
                i += 2;
            } else if args[i] == "--manifest" && i + 1 < args.len() {
                manifest_path = Some(PathBuf::from(&args[i + 1]));
                i += 2;
            } else {
                i += 1;
            }
        }
        let vp = pack_path.expect("--worker requires --voice-pack <path>");
        let mf = manifest_path.expect("--worker requires --manifest <path>");
        if let Err(e) = run_worker_mode(&vp, &mf) {
            eprintln!("WORKER ERROR: {}", e);
            std::process::exit(2);
        }
        return;
    }

    println!("==================================================================");
    println!(" MIKU SPIKE 0.5 · STREAMING OFFLINE ES/EN TTS + SHA-256 + TTL");
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
                "Streaming Utterances (24kHz) : ES={} | EN={} | Total Chunks={} | Underruns={}",
                report.es_utterances_synthesized,
                report.en_utterances_synthesized,
                report.total_chunks_streamed,
                report.underrun_chunks_total
            );
            println!(
                "First-Chunk Latency (TTFA)   : p50={:.3} ms | p95={:.3} ms (<= 250.0 ms)",
                report.ttfa_p50_ms, report.ttfa_p95_ms
            );
            println!(
                "Real-Time Factor (RTF)       : p50={:.5} | p95={:.5} (< 0.25)",
                report.rtf_p50, report.rtf_p95
            );
            println!(
                "Idle Pre-Load RAM            : p50={:.2} MB | p95={:.2} MB (<= 30.0 MB)",
                report.idle_pre_ram_p50_mb, report.idle_pre_ram_p95_mb
            );
            println!(
                "Active Peak RAM (Loaded)     : p50={:.2} MB | p95={:.2} MB (> 45.0 MB)",
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
    fn e2e_tts_streaming_ttfa_rtf_and_post_ttl_ram_recovery_audit() {
        let temp_dir = env::temp_dir().join("miku_spike_05_unit_e2e_test");
        let (pack_path, manifest_path, manifest) =
            prepare_voice_pack_on_disk(&temp_dir).expect("Must write voice pack and manifest");

        let raw = fs::read(&pack_path).expect("Must read voice pack");
        let manifest_from_disk: VoicePackManifest =
            serde_json::from_str(&fs::read_to_string(&manifest_path).unwrap()).unwrap();
        assert_eq!(
            manifest.expected_sha256_hex,
            manifest_from_disk.expected_sha256_hex
        );

        let mut session = OnDemandTtsSession::new(1_000, DEFAULT_MAX_VOICE_PACK_BYTES);
        let allocated = session
            .load_verified(&raw, &manifest_from_disk, 100)
            .expect("Must verify SHA-256 and allocate 48 MB voice bank");
        assert!(allocated >= BENCHMARK_VOICE_SLAB_BYTES);

        for (phrase, lang) in canonical_bilingual_tts_phrases() {
            let (summary, chunks) = session
                .synthesize_streaming_utterance(phrase, lang, DEFAULT_SAMPLE_RATE_HZ, 200)
                .expect("Must synthesize streaming chunks");
            assert!(summary.ttfa_ms <= 250.0);
            assert!(summary.rtf < 0.25);
            assert_eq!(summary.underrun_chunks, 0);
            assert!(!chunks.is_empty());
        }

        assert!(session.poll_ttl(1_500));
        assert_eq!(session.allocated_voice_bytes(), 0);
        let _ = fs::remove_dir_all(&temp_dir);
    }
}
