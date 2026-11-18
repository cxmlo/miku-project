#![allow(
    unknown_lints,
    clippy::too_many_arguments,
    clippy::manual_range_contains,
    clippy::needless_range_loop,
    clippy::large_enum_variant,
    clippy::chunks_exact_to_as_chunks,
    clippy::type_complexity,
    clippy::invisible_characters
)]

//! `miku-ui` — Native Win32 Island, Mode C WebView2 Host, 6-Spring Avatar Runtime,
//! Pre-Execution Gatekeeper Interceptor, Append-Only JSONL Audit Store, PTT/Wake-Word Voice Loop,
//! Global Kill Switch (`Ctrl+Alt+Shift+K`), Idle Suspension Coordinator, and Standalone Demo Mode.
//!
//! Micro-step 2.12 (Principle 8 · Modular Boundaries):
//! This crate root acts as an isomorphic public facade (`pub mod` + `pub use`) over 8 cohesive
//! domain modules (`win32`, `avatar`, `webview`, `gatekeeper`, `audit`, `voice`, `power`, `demo`),
//! where every source file in `miku/crates/miku-ui/src/` (including `main.rs`) is strictly `< 600` lines.

pub use miku_gatekeeper::{
    AuditEntry, CanonicalPathJail, Capability, DenyReason, GatekeeperEngine, GatekeeperVerdict,
    JailError, PermissionTicket, PolicyEffect, PolicyRule, ProjectProfile, TamperError,
    TamperEvidentAuditLog, GENESIS_AUDIT_HASH,
};

pub mod audit;
pub mod avatar;
pub mod custom_ui;
pub mod demo;
pub mod feed;
pub mod gatekeeper;
pub mod power;
pub mod vision;
pub mod voice;
pub mod webview;
pub mod win32;

pub use crate::audit::{
    create_unique_isolated_audit_temp_dir, format_canonical_windows_audit_log_dir,
    resolve_isolated_audit_log_dir, run_persistent_jsonl_audit_store_benchmark, AuditViewerFilter,
    AuditViewerPage, AuditViewerRow, JsonlChainVerificationReport,
    PersistentJsonlAuditBenchmarkReport, PersistentJsonlAuditStore, SafeWebViewAuditViewer,
};
pub use crate::avatar::{
    resolve_avatar_assets_dir, run_avatar_pack_hot_reload_audit, sha256_hex_bytes,
    validate_avatar_asset_catalog, AvatarCatalogAuditReport, AvatarPackAuditReport,
    AvatarPackDescriptor, AvatarPackHotReloader, AvatarPackSnapshot, HotReloadMetrics,
    MultiSpringAvatarPhysics, ProductionAvatarRuntime, SpringChannelState, V7AccessoriesSection,
    V7AccessoryItem, V7AvatarTokensSection, V7DesignTokens, V7DesignTokensDocument,
    V7IslandCompactTokens, V7IslandExpandedTokens, V7IslandSection, V7RigDocument, V7RigIdleConfig,
    V7RigLayer, V7RigMeta, V7RigPhysics, V7RigSmoothingConfig, V7RigSpringConfig, V7ScaleEntry,
    V7ScaleSection,
};
pub use crate::custom_ui::{
    resolve_custom_ui_base_dir, resolve_user_ui_override_dir, validate_custom_ui_tree,
    CustomUiResolver, CustomUiTreeReport, ResolvedUiAsset, REQUIRED_CUSTOM_UI_FILES,
};
pub use crate::demo::{
    parse_demo_cli_args, run_interactive_demo_session, DemoCliOptions, DemoInteractionPhase,
    DemoInteractiveController, DemoSandboxGuard, DemoSessionReport,
};
pub use crate::feed::{
    parse_github_json_items, parse_rss_or_atom_items, sanitize_feed_diagnostic,
    ConditionalValidators, FeedHttpRequest, FeedHttpResponse, FeedPauseReason, FeedSyncSummary,
    FeedTransport, FeedTransportError, IslandTechFeedController, MockFeedTransport,
    MockZenBrowserLauncher, RateLimitState, TechFeedItem, TechFeedSourceConfig,
    TechFeedSourceKind, ZenBrowserLauncher, ZenBrowserOpenOutcome,
};
pub use crate::gatekeeper::{
    default_ui_gatekeeper_profile, run_kill_switch_audit, run_ui_gatekeeper_interceptor_audit,
    ConfirmationModalState, GatekeeperInterceptOutcome, HotkeyRegistrationStatus,
    ImpactSummaryCard, KillSwitchAuditReport, KillSwitchStatus, KillSwitchTripReport,
    RearmChallenge, RiskSeverityLevel, UiGatekeeperAuditReport, UiGatekeeperInterceptor,
    UiKillSwitchManager,
};
pub use crate::power::{
    run_idle_suspension_bank05_audit, BudgetCheckItem, IdlePowerState, IdleSuspensionBank05AuditReport,
    IdleSuspensionCoordinator, IdleSuspensionTransitionReceipt, Phase2BudgetCertificationMatrix,
    WakeFromIdleReceipt, WakeStimulusKind,
};
pub use crate::vision::types_and_codec::{
    DEFAULT_VISION_MAX_DIMENSION_PX, DEFAULT_VISION_MAX_OUTPUT_BYTES,
    DEFAULT_VISION_PREVIEW_MAX_BYTES, DEFAULT_VISION_PREVIEW_MAX_HEIGHT_PX,
    DEFAULT_VISION_PREVIEW_MAX_WIDTH_PX, PNG_MAGIC_BYTES,
};
pub use crate::vision::{
    compress_rgba_to_bounded_png, decode_png_ihdr_dimensions, downscale_rgba_box, encode_rgba_png,
    zeroize_slice, BoundedPngFrame, DeterministicFixtureOcrBackend, DeterministicFixtureVisionBackend,
    LocalOcrBackend, LocalWindowsOcrBackend, OcrLine, OcrResult, OcrWord, RawRgbaFrame,
    ScreenCaptureBackend, ScreenVisionController, TurnVisionPayload, VisionCaptureOutcome,
    VisionCapturePolicy, VisionCaptureTarget, VisionConfirmationPreview,
    Win32GdiScreenVisionBackend,
};
pub use crate::voice::{
    build_groq_multipart_body, decode_riff_wav_pcm16_mono, encode_riff_wav_pcm16_mono,
    infer_language_from_text, is_loopback_host, parse_groq_endpoint,
    parse_groq_transcription_json, redact_sensitive_voice_diagnostic,
    redact_sensitive_voice_diagnostic_with_secrets, run_voice_interaction_loop_audit,
    validate_voice_secret_alias, AudioDuckingController, AudioDuckingReceipt, AudioDuckingState,
    AudioRestoreReceipt, DeterministicFixtureCaptureDevice, DuckingRecoveryRecord,
    GroqBillingEstimate, GroqEndpointTarget, GroqSttConfig, GroqSttTranscript, GroqSttTransport,
    GroqWhisperHttpClient, GroqWhisperModel, InMemoryVoiceSecretVault, IslandMediaController,
    LocalSapiRecognizer, LocalSttTranscript, MediaPlaybackStatus, MediaTrackMetadata,
    MediaTransportCommand, OptionalWakeWordConfig, PttAudioCaptureStream, PttPipelineOutcome,
    PttSttOrchestrator, SapiAudioRecognizer, SapiRecognitionResult, SimulatedAudioMediaBackend,
    SttEngineError, SttLanguage, SttModelError, SystemAudioMediaBackend, SystemAudioSnapshot,
    VoiceAgentSessionOrder, VoiceBufferZeroReport, VoiceCancelReceipt, VoiceCancellationReason,
    VoiceInteractionAuditReport, VoiceInteractionController, VoiceInteractionState,
    VoicePcmRingBuffer, VoicePowerPolicy, VoiceSecretAliasStore, VoiceSessionOrderQueue,
    VoiceSoundwaveIndicator, VoiceSttEngine, VoiceSttSettings, VoiceTranscriptEvent,
    VoiceTriggerMode, VoiceUtteranceCompletionReport, WinMmSystemAudioMediaBackend,
    WinMmWaveInCaptureProbe, WindowsCredentialAliasStore, ZeroizingBytes, AUDIO_VOLUME_EPSILON,
    DEFAULT_AUDIO_DUCKING_TARGET_LEVEL, DEFAULT_GROQ_STT_TIMEOUT_MS, GROQ_API_HOST,
    GROQ_DEFAULT_KEY_ALIAS, GROQ_MIN_BILLED_SECONDS_PER_REQUEST, GROQ_MULTIPART_BOUNDARY,
    GROQ_STT_DEFAULT_ENDPOINT, MAX_STT_WAV_UPLOAD_BYTES, MIN_SPEECH_RMS_AMPLITUDE,
};
pub use crate::webview::{
    build_overview_scene_bundle, compute_accessory_slot_transform,
    compose_avatar_layer_stack, compose_avatar_layer_stack_from_rig, compute_global_cursor_sample,
    escape_html_entities_for_text_content, evaluate_navigation_target,
    format_canonical_windows_webview2_data_dir, live_child_msedgewebview2_pids,
    probe_edge_webview2_runtime, query_current_process_private_ram_mb,
    resolve_isolated_webview2_user_data_dir, run_five_cycle_webview2_ttl_recovery_audit,
    run_ipc_bridge_stress_audit, run_overview_scene_audit, run_win32_com_webview2_cycle,
    sanitize_for_text_content, validate_ipc_sender_origin, validate_webview_csp_policy,
    AccessorySlotTransform, AvatarExpressionPose, AvatarLayerComposeNode, AvatarLayerNode,
    AvatarMood, ExpandTransitionReport, FiveCycleWebView2AuditReport, GlobalCursorSample,
    GlobalCursorTracker, IpcBridgeAuditReport, IslandHitRect,
    IsolatedWebView2ProcessDriver, NavigationVerdict, OverviewSceneAuditReport,
    OverviewSceneBundle, OverviewSceneController, UiInteractionKind, WebView2HostStateMachine,
    WebView2IpcBridge, WebView2LifecycleDriver, WebView2LifecycleState, WebView2ProcessDriver,
    LiveWebView2Config, LiveWebView2Event, LiveWebView2Window, LiveWindowKind, WindowRectPx,
    WebView2RuntimeProbe, WebView2SecurityPolicy, WebViewIpcEnvelopeV1, WebViewIpcPayloadV1,
    Win32ComLifecycleTelemetry, Win32WebView2ComDriver,
};
pub use crate::win32::{
    compute_top_center_pill_anchor, handle_wm_mouseactivate,
    recompute_anchor_on_dpi_or_display_change, run_native_pill_verification_cycle,
    scale_px_for_dpi, select_target_monitor, verify_foreground_focus_invariant,
    HoverTransitionResult, JsonlTamperDiagnostic, MonitorDescriptor, NativePillCycleMetrics,
    PillAnchorGeometry, PillHoverStateMachine, PillVisualMode, UiError, VirtualRect,
    Win32PillStyleConfig, ACTIVE_AUDIT_JSONL_FILENAME, AUDIT_ISOLATED_SUBDIR_WINDOWS,
    AVATAR_EYES_CENTER_ANCHOR, AVATAR_HEAD_TOP_ANCHOR, AVATAR_NECK_ANCHOR, AVATAR_VIEWBOX_SIZE,
    DEFAULT_CONFIRMATION_MODAL_TTL_MS, DEFAULT_IDLE_SUSPENSION_TTL_MS,
    DEFAULT_KILL_SWITCH_HOTKEY_COMBO, DEFAULT_MAX_AUDIT_FILE_BYTES,
    DEFAULT_MAX_ROTATED_AUDIT_FILES, DEFAULT_PTT_HOTKEY_COMBO, DEFAULT_VOICE_SILENCE_TIMEOUT_MS,
    DEFAULT_WAKE_WORD_PHRASE, DEFAULT_WEBVIEW2_WARM_TTL_MS, DEMO_BADGE_PREFIX,
    DEMO_DEFAULT_DURATION_MS, DEMO_MAX_STARTUP_LATENCY_MS, DEMO_PTT_HOTKEY_ID,
    EXPANDED_CORNER_RADIUS_PX, EXPANDED_DEFAULT_HEIGHT_PX, EXPANDED_DEFAULT_WIDTH_PX,
    EXPECTED_AVATAR_SVG_COUNT, EXPECTED_AVATAR_SVG_REL_PATHS, EXPECTED_RIG_SPRING_COUNT,
    EXPECTED_RIG_SPRING_NAMES, FIVE_CYCLE_STRESS_REPS, HOVER_AVATAR_BODY_PX, HOVER_AVATAR_SCALE_K,
    HOVER_CORNER_RADIUS_PX, HOVER_HYSTERESIS_PX, HOVER_PEEK_HEIGHT_PX, HOVER_PEEK_WIDTH_PX,
    IDLE_AVATAR_BODY_PX, IDLE_AVATAR_SCALE_K, IDLE_COMPACT_HEIGHT_PX, IDLE_COMPACT_WIDTH_PX,
    IDLE_CORNER_RADIUS_PX, KILL_SWITCH_ALERT_COLOR_HEX, KILL_SWITCH_HOTKEY_ID,
    KILL_SWITCH_WIN32_MODIFIERS, KILL_SWITCH_WIN32_VK_K, MAX_10K_VERIFY_LATENCY_MS,
    MAX_AUDIT_APPEND_LATENCY_MS, MAX_AUDIT_VIEWER_PAGE_SIZE, MAX_AVATAR_HOT_RELOAD_LATENCY_MS,
    MAX_DEFERRED_GAMING_EVENTS, MAX_EXPANDED_WEBVIEW2_RAM_MB, MAX_EXPAND_VISIBLE_LATENCY_MS,
    MAX_GAMING_CPU_BUDGET_PCT, MAX_GATEKEEPER_INTERCEPT_LATENCY_MS, MAX_HOVER_RESPONSE_MS,
    MAX_IDLE_POST_TTL_RAM_MB, MAX_IPC_MESSAGE_BYTES, MAX_IPC_QUEUE_DEPTH, MAX_IPC_RTT_BUDGET_MS,
    MAX_KILL_SWITCH_LATENCY_MS, MAX_OVERVIEW_BUNDLE_BYTES, MAX_OVERVIEW_EVENT_FEED,
    MAX_REARM_ATTEMPTS, MAX_SINGLE_SVG_ASSET_BYTES, MAX_SOUNDWAVE_UPDATE_LATENCY_MS,
    MAX_VOICE_CANCEL_LATENCY_MS, MAX_WARM_WAKE_LATENCY_MS, MA_NOACTIVATE_RESULT,
    MIKU_JSONRPC_V2, MIKU_WEBVIEW_CSP_HEADER, MIN_PTT_HOLD_DURATION_MS, OVERVIEW_AVATAR_BODY_PX,
    OVERVIEW_AVATAR_SCALE_K, OVERVIEW_CORNER_RADIUS_PX, OVERVIEW_EXPANDED_HEIGHT_PX,
    OVERVIEW_EXPANDED_WIDTH_PX, OVERVIEW_VIRTUAL_INDEX_URL, REARM_CHALLENGE_TTL_MS,
    REQUIRED_BASE_STYLE, REQUIRED_EX_STYLE, RIG_MAX_DT_SEC, RIG_SLEEP_EPSILON_DEFAULT,
    SOUNDWAVE_BAND_COUNT, STANDARD_DPI, SWP_NOACTIVATE_FLAG, SWP_NOMOVE_FLAG, SWP_NOSIZE_FLAG,
    SWP_NOZORDER_FLAG, SWP_SHOWWINDOW_FLAG, SW_SHOWNOACTIVATE_CMD, TARGET_IDLE_CPU_BUDGET_PCT,
    TEN_CYCLE_STABILITY_REPS, TOP_MARGIN_DEFAULT_PX, VOICE_CHANNELS,
    VOICE_PCM_RING_CAPACITY_SAMPLES, VOICE_SAMPLE_RATE_HZ, WEBVIEW2_ISOLATED_SUBDIR_WINDOWS,
    WEBVIEW2_LOCAL_VIRTUAL_ORIGIN, WIN32_MOD_ALT, WIN32_MOD_CONTROL, WIN32_MOD_NOREPEAT,
    WIN32_MOD_SHIFT, WIN32_VK_ESCAPE, WIN32_VK_SPACE, WM_DISPLAYCHANGE_MSG, WM_DPICHANGED_MSG,
    WM_MOUSEACTIVATE_MSG, WS_EX_APPWINDOW_FORBIDDEN, WS_EX_LAYERED_STYLE,
    WS_EX_NOACTIVATE_STYLE, WS_EX_TOOLWINDOW_STYLE, WS_EX_TOPMOST_STYLE, WS_POPUP_STYLE,
    WS_VISIBLE_STYLE,
};

#[allow(unused_imports)]
pub(crate) mod prelude {
    pub(crate) use super::{
        build_groq_multipart_body, build_overview_scene_bundle, compose_avatar_layer_stack,
        compose_avatar_layer_stack_from_rig, compress_rgba_to_bounded_png,
        compute_accessory_slot_transform, compute_top_center_pill_anchor,
        create_unique_isolated_audit_temp_dir, decode_png_ihdr_dimensions,
        decode_riff_wav_pcm16_mono, default_ui_gatekeeper_profile, downscale_rgba_box,
        encode_riff_wav_pcm16_mono, encode_rgba_png, escape_html_entities_for_text_content,
        evaluate_navigation_target, format_canonical_windows_audit_log_dir,
        format_canonical_windows_webview2_data_dir, handle_wm_mouseactivate, infer_language_from_text, is_loopback_host,
        live_child_msedgewebview2_pids, parse_demo_cli_args, parse_github_json_items,
        parse_groq_endpoint, parse_groq_transcription_json, parse_rss_or_atom_items,
        probe_edge_webview2_runtime, query_current_process_private_ram_mb,
        recompute_anchor_on_dpi_or_display_change, redact_sensitive_voice_diagnostic,
        redact_sensitive_voice_diagnostic_with_secrets, resolve_avatar_assets_dir,
        resolve_isolated_audit_log_dir, resolve_isolated_webview2_user_data_dir,
        run_avatar_pack_hot_reload_audit, run_five_cycle_webview2_ttl_recovery_audit,
        run_idle_suspension_bank05_audit, run_interactive_demo_session,
        run_ipc_bridge_stress_audit, run_kill_switch_audit, run_native_pill_verification_cycle,
        run_overview_scene_audit, run_persistent_jsonl_audit_store_benchmark,
        run_ui_gatekeeper_interceptor_audit, run_voice_interaction_loop_audit,
        run_win32_com_webview2_cycle, sanitize_feed_diagnostic, sanitize_for_text_content,
        scale_px_for_dpi, select_target_monitor, sha256_hex_bytes, validate_avatar_asset_catalog,
        validate_ipc_sender_origin, validate_voice_secret_alias, validate_webview_csp_policy,
        verify_foreground_focus_invariant, zeroize_slice, AccessorySlotTransform,
        AudioDuckingController, AudioDuckingReceipt, AudioDuckingState, AudioRestoreReceipt,
        AuditViewerFilter, AuditViewerPage, AuditViewerRow, AvatarCatalogAuditReport,
        AvatarExpressionPose, AvatarLayerComposeNode, AvatarLayerNode, AvatarMood,
        AvatarPackAuditReport, AvatarPackDescriptor, AvatarPackHotReloader, AvatarPackSnapshot,
        BoundedPngFrame, BudgetCheckItem, ConditionalValidators, ConfirmationModalState,
        DemoCliOptions, DemoInteractionPhase, DemoInteractiveController, DemoSandboxGuard,
        DemoSessionReport, DeterministicFixtureCaptureDevice, DeterministicFixtureOcrBackend,
        DeterministicFixtureVisionBackend, DuckingRecoveryRecord, ExpandTransitionReport,
        FeedHttpRequest, FeedHttpResponse, FeedPauseReason, FeedSyncSummary, FeedTransport,
        FeedTransportError, FiveCycleWebView2AuditReport, GatekeeperInterceptOutcome,
        GroqBillingEstimate, GroqEndpointTarget, GroqSttConfig, GroqSttTranscript,
        GroqSttTransport, GroqWhisperHttpClient, GroqWhisperModel, HotReloadMetrics,
        HotkeyRegistrationStatus, HoverTransitionResult, IdlePowerState,
        IdleSuspensionBank05AuditReport, IdleSuspensionCoordinator, IdleSuspensionTransitionReceipt,
        ImpactSummaryCard, InMemoryVoiceSecretVault, IpcBridgeAuditReport, IslandMediaController,
        IslandTechFeedController, IsolatedWebView2ProcessDriver, JsonlChainVerificationReport,
        JsonlTamperDiagnostic, KillSwitchAuditReport, KillSwitchStatus, KillSwitchTripReport,
        LocalOcrBackend, LocalSapiRecognizer, LocalSttTranscript, LocalWindowsOcrBackend,
        MediaPlaybackStatus, MediaTrackMetadata, MediaTransportCommand, MockFeedTransport,
        MockZenBrowserLauncher, MonitorDescriptor, MultiSpringAvatarPhysics, NativePillCycleMetrics,
        NavigationVerdict, OcrLine, OcrResult, OcrWord, OptionalWakeWordConfig,
        OverviewSceneAuditReport, OverviewSceneBundle, OverviewSceneController,
        PersistentJsonlAuditBenchmarkReport, PersistentJsonlAuditStore,
        Phase2BudgetCertificationMatrix, PillAnchorGeometry, PillHoverStateMachine,
        PillVisualMode, ProductionAvatarRuntime, PttAudioCaptureStream, PttPipelineOutcome,
        PttSttOrchestrator, RateLimitState, RawRgbaFrame, RearmChallenge, RiskSeverityLevel,
        SafeWebViewAuditViewer, SapiAudioRecognizer, SapiRecognitionResult, ScreenCaptureBackend,
        ScreenVisionController, SimulatedAudioMediaBackend, SpringChannelState, SttEngineError,
        SttLanguage, SttModelError, SystemAudioMediaBackend, SystemAudioSnapshot, TechFeedItem,
        TechFeedSourceConfig, TechFeedSourceKind, TurnVisionPayload, UiError,
        UiGatekeeperAuditReport, UiGatekeeperInterceptor, UiInteractionKind, UiKillSwitchManager,
        V7AccessoriesSection, V7AccessoryItem, V7AvatarTokensSection, V7DesignTokens,
        V7DesignTokensDocument, V7IslandCompactTokens, V7IslandExpandedTokens, V7IslandSection,
        V7RigDocument, V7RigIdleConfig, V7RigLayer, V7RigMeta, V7RigPhysics, V7RigSmoothingConfig,
        V7RigSpringConfig, V7ScaleEntry, V7ScaleSection, VirtualRect, VisionCaptureOutcome,
        VisionCapturePolicy, VisionCaptureTarget, VisionConfirmationPreview,
        VoiceAgentSessionOrder, VoiceBufferZeroReport, VoiceCancelReceipt, VoiceCancellationReason,
        VoiceInteractionAuditReport, VoiceInteractionController, VoiceInteractionState,
        VoicePcmRingBuffer, VoicePowerPolicy, VoiceSecretAliasStore, VoiceSessionOrderQueue,
        VoiceSoundwaveIndicator, VoiceSttEngine, VoiceSttSettings, VoiceTranscriptEvent,
        VoiceTriggerMode, VoiceUtteranceCompletionReport, WakeFromIdleReceipt, WakeStimulusKind, WebView2HostStateMachine, WebView2IpcBridge,
        WebView2LifecycleDriver, WebView2LifecycleState, WebView2ProcessDriver,
        WebView2RuntimeProbe, WebView2SecurityPolicy, WebViewIpcEnvelopeV1, WebViewIpcPayloadV1,
        Win32ComLifecycleTelemetry, Win32GdiScreenVisionBackend, Win32PillStyleConfig,
        Win32WebView2ComDriver, WinMmSystemAudioMediaBackend, WinMmWaveInCaptureProbe,
        WindowsCredentialAliasStore, ZenBrowserLauncher, ZenBrowserOpenOutcome, ZeroizingBytes,
        ACTIVE_AUDIT_JSONL_FILENAME, AUDIO_VOLUME_EPSILON, AUDIT_ISOLATED_SUBDIR_WINDOWS,
        AVATAR_EYES_CENTER_ANCHOR, AVATAR_HEAD_TOP_ANCHOR, AVATAR_NECK_ANCHOR, AVATAR_VIEWBOX_SIZE,
        DEFAULT_AUDIO_DUCKING_TARGET_LEVEL, DEFAULT_CONFIRMATION_MODAL_TTL_MS,
        DEFAULT_GROQ_STT_TIMEOUT_MS, DEFAULT_IDLE_SUSPENSION_TTL_MS,
        DEFAULT_KILL_SWITCH_HOTKEY_COMBO, DEFAULT_MAX_AUDIT_FILE_BYTES,
        DEFAULT_MAX_ROTATED_AUDIT_FILES, DEFAULT_PTT_HOTKEY_COMBO,
        DEFAULT_VISION_MAX_DIMENSION_PX, DEFAULT_VISION_MAX_OUTPUT_BYTES,
        DEFAULT_VISION_PREVIEW_MAX_BYTES, DEFAULT_VISION_PREVIEW_MAX_HEIGHT_PX,
        DEFAULT_VISION_PREVIEW_MAX_WIDTH_PX, DEFAULT_VOICE_SILENCE_TIMEOUT_MS,
        DEFAULT_WAKE_WORD_PHRASE, DEFAULT_WEBVIEW2_WARM_TTL_MS, DEMO_BADGE_PREFIX,
        DEMO_DEFAULT_DURATION_MS, DEMO_MAX_STARTUP_LATENCY_MS, DEMO_PTT_HOTKEY_ID,
        EXPANDED_CORNER_RADIUS_PX, EXPANDED_DEFAULT_HEIGHT_PX, EXPANDED_DEFAULT_WIDTH_PX,
        EXPECTED_AVATAR_SVG_COUNT, EXPECTED_AVATAR_SVG_REL_PATHS, EXPECTED_RIG_SPRING_COUNT,
        EXPECTED_RIG_SPRING_NAMES, FIVE_CYCLE_STRESS_REPS, GROQ_API_HOST,
        GROQ_DEFAULT_KEY_ALIAS, GROQ_MIN_BILLED_SECONDS_PER_REQUEST, GROQ_MULTIPART_BOUNDARY,
        GROQ_STT_DEFAULT_ENDPOINT, HOVER_AVATAR_BODY_PX, HOVER_AVATAR_SCALE_K,
        HOVER_CORNER_RADIUS_PX, HOVER_HYSTERESIS_PX, HOVER_PEEK_HEIGHT_PX, HOVER_PEEK_WIDTH_PX,
        IDLE_AVATAR_BODY_PX, IDLE_AVATAR_SCALE_K, IDLE_COMPACT_HEIGHT_PX, IDLE_COMPACT_WIDTH_PX,
        IDLE_CORNER_RADIUS_PX, KILL_SWITCH_ALERT_COLOR_HEX, KILL_SWITCH_HOTKEY_ID,
        KILL_SWITCH_WIN32_MODIFIERS, KILL_SWITCH_WIN32_VK_K, MAX_10K_VERIFY_LATENCY_MS,
        MAX_AUDIT_APPEND_LATENCY_MS, MAX_AUDIT_VIEWER_PAGE_SIZE, MAX_AVATAR_HOT_RELOAD_LATENCY_MS,
        MAX_DEFERRED_GAMING_EVENTS, MAX_EXPANDED_WEBVIEW2_RAM_MB, MAX_EXPAND_VISIBLE_LATENCY_MS,
        MAX_GAMING_CPU_BUDGET_PCT, MAX_GATEKEEPER_INTERCEPT_LATENCY_MS, MAX_HOVER_RESPONSE_MS,
        MAX_IDLE_POST_TTL_RAM_MB, MAX_IPC_MESSAGE_BYTES, MAX_IPC_QUEUE_DEPTH,
        MAX_IPC_RTT_BUDGET_MS, MAX_KILL_SWITCH_LATENCY_MS, MAX_OVERVIEW_BUNDLE_BYTES,
        MAX_OVERVIEW_EVENT_FEED, MAX_REARM_ATTEMPTS, MAX_SINGLE_SVG_ASSET_BYTES,
        MAX_SOUNDWAVE_UPDATE_LATENCY_MS, MAX_STT_WAV_UPLOAD_BYTES, MAX_VOICE_CANCEL_LATENCY_MS,
        MAX_WARM_WAKE_LATENCY_MS, MA_NOACTIVATE_RESULT, MIKU_JSONRPC_V2, MIKU_WEBVIEW_CSP_HEADER,
        MIN_PTT_HOLD_DURATION_MS, MIN_SPEECH_RMS_AMPLITUDE, OVERVIEW_AVATAR_BODY_PX,
        OVERVIEW_AVATAR_SCALE_K, OVERVIEW_CORNER_RADIUS_PX, OVERVIEW_EXPANDED_HEIGHT_PX,
        OVERVIEW_EXPANDED_WIDTH_PX, OVERVIEW_VIRTUAL_INDEX_URL, PNG_MAGIC_BYTES,
        REARM_CHALLENGE_TTL_MS, REQUIRED_BASE_STYLE, REQUIRED_EX_STYLE, RIG_MAX_DT_SEC,
        RIG_SLEEP_EPSILON_DEFAULT, SOUNDWAVE_BAND_COUNT, STANDARD_DPI, SWP_NOACTIVATE_FLAG,
        SWP_NOMOVE_FLAG, SWP_NOSIZE_FLAG, SWP_NOZORDER_FLAG, SWP_SHOWWINDOW_FLAG,
        SW_SHOWNOACTIVATE_CMD, TARGET_IDLE_CPU_BUDGET_PCT, TEN_CYCLE_STABILITY_REPS,
        TOP_MARGIN_DEFAULT_PX, VOICE_CHANNELS, VOICE_PCM_RING_CAPACITY_SAMPLES,
        VOICE_SAMPLE_RATE_HZ, WEBVIEW2_ISOLATED_SUBDIR_WINDOWS, WEBVIEW2_LOCAL_VIRTUAL_ORIGIN,
        WIN32_MOD_ALT, WIN32_MOD_CONTROL, WIN32_MOD_NOREPEAT, WIN32_MOD_SHIFT, WIN32_VK_ESCAPE,
        WIN32_VK_SPACE, WM_DISPLAYCHANGE_MSG, WM_DPICHANGED_MSG, WM_MOUSEACTIVATE_MSG,
        WS_EX_APPWINDOW_FORBIDDEN, WS_EX_LAYERED_STYLE, WS_EX_NOACTIVATE_STYLE,
        WS_EX_TOOLWINDOW_STYLE, WS_EX_TOPMOST_STYLE, WS_POPUP_STYLE, WS_VISIBLE_STYLE,
    };
    pub(crate) use miku_gatekeeper::{
        AuditEntry, CanonicalPathJail, Capability, DenyReason, GatekeeperEngine, GatekeeperVerdict,
        JailError, PermissionTicket, PolicyEffect, PolicyRule, ProjectProfile, TamperError,
        TamperEvidentAuditLog, GENESIS_AUDIT_HASH,
    };
    pub(crate) use miku_protocol::{
        constant_time_token_eq, sha256_hex, IslandState, MikuLinkMessageV1, PermissionDecisionKind,
        MAX_AUTH_FRAME_BYTES, MIKU_PROTOCOL_VERSION_V1, SUPPORTED_PROTOCOL_VERSIONS,
    };
    pub(crate) use serde::{Deserialize, Serialize};
    pub(crate) use std::collections::{BTreeMap, VecDeque};
    pub(crate) use std::fs::{self, OpenOptions};
    pub(crate) use std::io::{BufRead, BufReader, Write};
    pub(crate) use std::path::{Path, PathBuf};
    pub(crate) use std::sync::atomic::{AtomicU64, Ordering};
    pub(crate) use std::time::{Instant, SystemTime, UNIX_EPOCH};
}


#[cfg(test)]
mod tests_modular_facade;
