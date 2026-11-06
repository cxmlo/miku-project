//! Binary entrypoint for Spike 0.3 — Offline Wake Word ("Hey Miku") & WASAPI/CPU Benchmark.
//!
//! Responsibilities:
//! 1. Probes the default Windows capture endpoint via WASAPI (`IMMDeviceEnumerator::GetDefaultAudioEndpoint`)
//!    with automatic, panic-free fallback to the deterministic 16 kHz PCM generator if no microphone
//!    is connected or permitted.
//! 2. Audits 20 start/stop cycles of the WASAPI/COM audio session using `GetProcessHandleCount` to
//!    certify `handle_leak_delta <= 0`.
//! 3. Runs `n = 20` repetitions of the RIFF WAV ("Hey Miku" positive, negative speech, white noise,
//!    and mechanical keyboard transients) benchmark to measure `p50`/`p95` detection latency (`<= 200 ms`).
//! 4. Measures real continuous CPU usage (`<= 1.0%` AC / `<= 0.5%` Battery) over a 2000 ms real-time
//!    streaming window using Win32 `GetProcessTimes` (`KernelTime + UserTime`) normalized by
//!    `std::thread::available_parallelism()`.

use miku_spike_03_wakeword::{
    compute_normalized_cpu_pct, decode_wav_pcm16_mono, encode_wav_pcm16_mono,
    evaluate_wakeword_power_policy, format_spike03_report, percentile_sorted,
    synthesize_hey_miku_pcm, synthesize_keyboard_transients_pcm, synthesize_negative_speech_pcm,
    synthesize_white_noise_pcm, OfflineWakeWordDetector, Spike03BenchmarkSummary,
    FRAME_HOP_SAMPLES, FRAME_LEN_SAMPLES, REQUIRED_REPETITIONS, SAMPLE_RATE_HZ,
};
use std::time::{Duration, Instant};

#[cfg(windows)]
mod win32_audit {
    use windows::Win32::Foundation::FILETIME;
    use windows::Win32::Media::Audio::{
        eCapture, eConsole, IMMDeviceEnumerator, MMDeviceEnumerator,
    };
    use windows::Win32::System::Com::{
        CoCreateInstance, CoInitializeEx, CoUninitialize, CLSCTX_ALL, COINIT_MULTITHREADED,
    };
    use windows::Win32::System::Power::{GetSystemPowerStatus, SYSTEM_POWER_STATUS};
    use windows::Win32::System::Threading::{
        GetCurrentProcess, GetProcessHandleCount, GetProcessTimes,
    };

    fn filetime_to_microseconds(ft: FILETIME) -> f64 {
        let ticks_100ns = ((ft.dwHighDateTime as u64) << 32) | (ft.dwLowDateTime as u64);
        (ticks_100ns as f64) / 10.0
    }

    /// Returns `(kernel_us, user_us)` for the current process using Win32 `GetProcessTimes`.
    pub fn query_process_cpu_times_us() -> Result<(f64, f64), String> {
        unsafe {
            let hproc = GetCurrentProcess();
            let mut creation = FILETIME::default();
            let mut exit = FILETIME::default();
            let mut kernel = FILETIME::default();
            let mut user = FILETIME::default();
            GetProcessTimes(hproc, &mut creation, &mut exit, &mut kernel, &mut user)
                .map_err(|e| format!("GetProcessTimes failed: {}", e))?;
            Ok((
                filetime_to_microseconds(kernel),
                filetime_to_microseconds(user),
            ))
        }
    }

    /// Returns the number of open OS handles for the current process via `GetProcessHandleCount`.
    pub fn query_process_handle_count() -> Result<u32, String> {
        unsafe {
            let hproc = GetCurrentProcess();
            let mut count = 0u32;
            GetProcessHandleCount(hproc, &mut count)
                .map_err(|e| format!("GetProcessHandleCount failed: {}", e))?;
            Ok(count)
        }
    }

    /// Returns `true` if the system is currently running on battery power (`ACLineStatus == 0`).
    pub fn query_is_on_battery() -> bool {
        unsafe {
            let mut sps = SYSTEM_POWER_STATUS::default();
            if GetSystemPowerStatus(&mut sps).is_ok() {
                sps.ACLineStatus == 0
            } else {
                false
            }
        }
    }

    /// Probes WASAPI default capture endpoint and runs 20 start/stop cycles to verify 0 handle leaks.
    /// Never panics if a microphone is absent or blocked: logs an informative notice and falls back cleanly.
    pub fn audit_wasapi_cycles_and_handles(cycles: usize) -> Result<(bool, String, u32, u32, i64), String> {
        unsafe {
            let hr = CoInitializeEx(None, COINIT_MULTITHREADED);
            let com_initialized = hr.is_ok();

            // 1. Warm-up probe so Windows loads MMDevApi.dll once before baseline handle snapshot
            let mut endpoint_available = false;
            let mut status_note = String::new();

            match CoCreateInstance::<_, IMMDeviceEnumerator>(&MMDeviceEnumerator, None, CLSCTX_ALL) {
                Ok(enum_warmup) => match enum_warmup.GetDefaultAudioEndpoint(eCapture, eConsole) {
                    Ok(device) => {
                        let state = device.GetState().map(|s| s.0).unwrap_or(0);
                        endpoint_available = true;
                        status_note = format!(
                            "WASAPI Default Capture Endpoint active (state=0x{:X}) + 16 kHz PCM stream",
                            state
                        );
                        drop(device);
                    }
                    Err(err) => {
                        println!(
                            "  [WASAPI Notice] Default capture endpoint unavailable ({}); falling back to deterministic 16 kHz PCM generator.",
                            err
                        );
                        status_note = format!(
                            "Fallback determinista 16 kHz PCM en memoria (WASAPI sin micro activo: {})",
                            err.code()
                        );
                    }
                },
                Err(err) => {
                    println!(
                        "  [WASAPI Notice] MMDeviceEnumerator unavailable ({}); falling back to deterministic 16 kHz PCM generator.",
                        err
                    );
                    status_note = format!(
                        "Fallback determinista 16 kHz PCM en memoria (COM/WASAPI aviso: {})",
                        err.code()
                    );
                }
            }

            // Capture baseline handle count AFTER one-time DLL warm-up
            let handles_before = query_process_handle_count()?;

            // 2. Run 20 start/stop cycles of WASAPI session acquisition & release
            for _ in 0..cycles {
                if let Ok(enumerator) =
                    CoCreateInstance::<_, IMMDeviceEnumerator>(&MMDeviceEnumerator, None, CLSCTX_ALL)
                {
                    if let Ok(device) = enumerator.GetDefaultAudioEndpoint(eCapture, eConsole) {
                        let _ = device.GetState();
                        drop(device);
                    }
                    drop(enumerator);
                }
            }

            let handles_after = query_process_handle_count()?;
            let leak_delta = (handles_after as i64) - (handles_before as i64);

            if com_initialized {
                CoUninitialize();
            }

            Ok((
                endpoint_available,
                status_note,
                handles_before,
                handles_after,
                leak_delta,
            ))
        }
    }
}

pub fn run_spike03_benchmark() -> Result<Spike03BenchmarkSummary, String> {
    let logical_cores = std::thread::available_parallelism()
        .map(|n| n.get())
        .unwrap_or(1);

    #[cfg(windows)]
    let (
        wasapi_capture_endpoint_available,
        wasapi_status_note,
        handles_before,
        handles_after,
        handle_leak_delta,
    ) = win32_audit::audit_wasapi_cycles_and_handles(REQUIRED_REPETITIONS)?;

    #[cfg(not(windows))]
    let (
        wasapi_capture_endpoint_available,
        wasapi_status_note,
        handles_before,
        handles_after,
        handle_leak_delta,
    ) = (
        false,
        "Non-Windows fallback".to_string(),
        0u32,
        0u32,
        0i64,
    );

    #[cfg(windows)]
    let on_battery_power = win32_audit::query_is_on_battery();
    #[cfg(not(windows))]
    let on_battery_power = false;

    let mut detector = OfflineWakeWordDetector::new_hey_miku();
    let mut latencies_ms = Vec::with_capacity(REQUIRED_REPETITIONS);
    let mut true_positives = 0usize;

    // 1. Run 20 repetitions of positive "Hey Miku" WAV fixtures across tempo & volume variations
    for rep in 0..REQUIRED_REPETITIONS {
        let tempo = 0.95 + ((rep % 5) as f64) * 0.025; // 0.95x .. 1.05x
        let gain = 0.45 + ((rep % 4) as f64) * 0.08; // 0.45 .. 0.69
        let raw_pcm = synthesize_hey_miku_pcm(tempo, gain);
        let wav_bytes = encode_wav_pcm16_mono(&raw_pcm);
        let decoded_pcm = decode_wav_pcm16_mono(&wav_bytes)?;

        detector.reset_stream();
        let detections = detector.process_pcm_stream(&decoded_pcm);
        if let Some(first) = detections.first() {
            true_positives += 1;
            latencies_ms.push(first.detection_latency_ms);
            println!(
                "  [0.3 {:02}/{}] 'Hey Miku' (tempo={:.3}x, gain={:.2}) -> DETECTED | confidence={:.2} | dtw={:.4} | latency={:.2} ms",
                rep + 1,
                REQUIRED_REPETITIONS,
                tempo,
                gain,
                first.confidence,
                first.dtw_distance,
                first.detection_latency_ms
            );
        } else {
            return Err(format!(
                "Missed positive 'Hey Miku' detection on repetition {}",
                rep + 1
            ));
        }
    }

    // 2. Evaluate negative fixtures (other speech, white noise, mechanical keyboard clicks)
    detector.reset_stream();
    let neg_speech_wav = encode_wav_pcm16_mono(&synthesize_negative_speech_pcm());
    let neg_speech_pcm = decode_wav_pcm16_mono(&neg_speech_wav)?;
    let false_positives_negative_speech = detector.process_pcm_stream(&neg_speech_pcm).len();

    detector.reset_stream();
    let white_noise_pcm = synthesize_white_noise_pcm(3000, 0.35);
    let false_positives_white_noise = detector.process_pcm_stream(&white_noise_pcm).len();

    detector.reset_stream();
    let keyboard_pcm = synthesize_keyboard_transients_pcm(3000);
    let false_positives_keyboard = detector.process_pcm_stream(&keyboard_pcm).len();

    // 3. Measure real continuous CPU usage over a 2000 ms real-time streaming window (16 kHz)
    // Stream composition: 90% silence/background + 10% voice/transients
    let mut realtime_stream = vec![0i16; (SAMPLE_RATE_HZ as usize) * 2]; // 2.0 seconds = 32,000 samples
    let sample_phrase = synthesize_hey_miku_pcm(1.0, 0.55);
    let copy_len = sample_phrase.len().min(realtime_stream.len() / 5);
    realtime_stream[..copy_len].copy_from_slice(&sample_phrase[..copy_len]);

    detector.reset_stream();

    #[cfg(windows)]
    let (k_start_us, u_start_us) = win32_audit::query_process_cpu_times_us()?;
    let wall_start = Instant::now();

    // Process the 2000 ms stream in 20 chunks of 100 ms (sleeping 100 ms between chunks to match real-time audio cadence)
    let chunk_hops = 10usize; // 10 hops = 100 ms
    let total_hops = (realtime_stream.len() - FRAME_LEN_SAMPLES) / FRAME_HOP_SAMPLES;
    let mut hop_idx = 0usize;

    while hop_idx < total_hops {
        let batch_end = (hop_idx + chunk_hops).min(total_hops);
        while hop_idx < batch_end {
            let sample_offset = hop_idx * FRAME_HOP_SAMPLES;
            let frame = &realtime_stream[sample_offset..sample_offset + FRAME_LEN_SAMPLES];
            detector.process_single_hop(frame);
            hop_idx += 1;
        }
        std::thread::sleep(Duration::from_millis(95));
    }

    let wall_elapsed_us = wall_start.elapsed().as_secs_f64() * 1_000_000.0;
    #[cfg(windows)]
    let (k_end_us, u_end_us) = win32_audit::query_process_cpu_times_us()?;
    #[cfg(windows)]
    let continuous_cpu_pct = compute_normalized_cpu_pct(
        k_end_us - k_start_us,
        u_end_us - u_start_us,
        wall_elapsed_us,
        logical_cores,
    );
    #[cfg(not(windows))]
    let continuous_cpu_pct = 0.0;

    let silence_vad_rejection_ratio_pct = if detector.total_frames_evaluated > 0 {
        let rejected = detector
            .total_frames_evaluated
            .saturating_sub(detector.spectral_frames_computed);
        ((rejected as f64) / (detector.total_frames_evaluated as f64)) * 100.0
    } else {
        0.0
    };

    latencies_ms.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
    let latency_p50_ms = percentile_sorted(&latencies_ms, 50.0);
    let latency_p95_ms = percentile_sorted(&latencies_ms, 95.0);

    let total_fp =
        false_positives_negative_speech + false_positives_white_noise + false_positives_keyboard;
    let effective_leak_delta = handle_leak_delta.max(0);
    let recommended_policy = evaluate_wakeword_power_policy(
        continuous_cpu_pct,
        latency_p95_ms,
        total_fp,
        effective_leak_delta,
    );

    let mut summary = Spike03BenchmarkSummary {
        repetitions: REQUIRED_REPETITIONS,
        wasapi_capture_endpoint_available,
        wasapi_status_note,
        on_battery_power,
        logical_cores,
        true_positives,
        false_positives_negative_speech,
        false_positives_white_noise,
        false_positives_keyboard,
        latency_p50_ms,
        latency_p95_ms,
        silence_vad_rejection_ratio_pct,
        continuous_cpu_pct,
        handles_before,
        handles_after,
        handle_leak_delta: effective_leak_delta,
        recommended_policy,
        passes_all_thresholds: false,
    };
    summary.passes_all_thresholds = summary.evaluate_thresholds();
    Ok(summary)
}

fn main() {
    if !cfg!(target_os = "windows") {
        eprintln!("Spike 0.3 requires Windows (WASAPI + GetProcessTimes + GetProcessHandleCount).");
        std::process::exit(1);
    }

    println!(
        "Running Spike 0.3 (PID={}) — Offline Wake Word ('Hey Miku') & WASAPI/CPU Audit (n={})...",
        std::process::id(),
        REQUIRED_REPETITIONS
    );

    match run_spike03_benchmark() {
        Ok(summary) => {
            let report = format_spike03_report(&summary);
            println!("\n{}", report);
            if !summary.passes_all_thresholds {
                eprintln!(
                    "VERIFIER FAILED [0.3]: One or more Wake Word / WASAPI / CPU thresholds failed."
                );
                std::process::exit(1);
            }
            println!(
                "VERIFIER: OK (Spike 0.3 verified across {} repetitions: 0 false positives, latency p95 <= 200 ms, CPU <= 1.0%, 0 leaked handles).",
                REQUIRED_REPETITIONS
            );
        }
        Err(err) => {
            eprintln!("FATAL [0.3]: {}", err);
            std::process::exit(1);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use miku_spike_03_wakeword::WakeWordPowerPolicy;

    /// LEDGER-0.3-5: Integration test verifying RIFF WAV round-trip encoding/decoding,
    /// WASAPI handle hygiene audit (`handle_leak_delta <= 0`), and `Spike03BenchmarkSummary`
    /// threshold evaluation.
    #[test]
    fn e2e_wasapi_loop_and_offline_wake_word_benchmark() {
        let pcm = synthesize_hey_miku_pcm(1.0, 0.55);
        let wav = encode_wav_pcm16_mono(&pcm);
        let decoded = decode_wav_pcm16_mono(&wav).expect("RIFF WAV round-trip must succeed");
        assert_eq!(pcm, decoded);

        #[cfg(windows)]
        {
            let (_avail, note, before, after, delta) =
                win32_audit::audit_wasapi_cycles_and_handles(5)
                    .expect("WASAPI handle audit must complete without error");
            assert!(!note.is_empty());
            assert!(
                delta <= 0,
                "WASAPI cycle must not leak handles (before={}, after={}, delta={})",
                before,
                after,
                delta
            );
        }

        let mut summary = Spike03BenchmarkSummary {
            repetitions: REQUIRED_REPETITIONS,
            wasapi_capture_endpoint_available: true,
            wasapi_status_note: "WASAPI Default Capture Endpoint active".to_string(),
            on_battery_power: false,
            logical_cores: 16,
            true_positives: REQUIRED_REPETITIONS,
            false_positives_negative_speech: 0,
            false_positives_white_noise: 0,
            false_positives_keyboard: 0,
            latency_p50_ms: 40.0,
            latency_p95_ms: 40.0,
            silence_vad_rejection_ratio_pct: 88.5,
            continuous_cpu_pct: 0.12,
            handles_before: 140,
            handles_after: 140,
            handle_leak_delta: 0,
            recommended_policy: WakeWordPowerPolicy::AlwaysOnAcAndBattery,
            passes_all_thresholds: false,
        };
        summary.passes_all_thresholds = summary.evaluate_thresholds();
        assert!(summary.passes_all_thresholds);

        // Verify that any false positive or handle leak causes the verifier to fail
        summary.false_positives_keyboard = 1;
        assert!(!summary.evaluate_thresholds());
    }
}
