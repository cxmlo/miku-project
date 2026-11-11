#![allow(unused_imports)]
use crate::prelude::*;
use std::fs;

/// Target multimedia volume scalar (`20%` = `0.20`) applied while Miku is listening (`F-15`).
pub const DEFAULT_AUDIO_DUCKING_TARGET_LEVEL: f32 = 0.20;
/// Tolerance for floating-point volume comparisons (`0.0001`).
pub const AUDIO_VOLUME_EPSILON: f32 = 1e-4;

/// Exact snapshot of the system multimedia output volume and mute register prior to ducking.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct SystemAudioSnapshot {
    pub volume_scalar: f32,
    pub raw_waveout_volume_u32: u32,
    pub muted: bool,
}

impl SystemAudioSnapshot {
    pub fn from_scalar(volume_scalar: f32, muted: bool) -> Self {
        let clamped = volume_scalar.clamp(0.0, 1.0);
        let ch = (clamped * 65535.0).round() as u32 & 0xFFFF;
        Self { volume_scalar: clamped, raw_waveout_volume_u32: ch | (ch << 16), muted }
    }

    pub fn from_raw_waveout_u32(raw_waveout_volume_u32: u32, muted: bool) -> Self {
        let left = (raw_waveout_volume_u32 & 0xFFFF) as f32 / 65535.0;
        let right = ((raw_waveout_volume_u32 >> 16) & 0xFFFF) as f32 / 65535.0;
        Self { volume_scalar: ((left + right) * 0.5).clamp(0.0, 1.0), raw_waveout_volume_u32, muted }
    }
}

/// Playback status of the current multimedia session shown in the Island (`F-15`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MediaPlaybackStatus {
    Playing,
    Paused,
    Stopped,
}

/// Metadata for the currently playing track displayed in the Island (`F-15`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MediaTrackMetadata {
    pub title: String,
    pub artist: String,
    pub album: String,
    pub status: MediaPlaybackStatus,
    pub duration_ms: u64,
    pub position_ms: u64,
}

impl MediaTrackMetadata {
    pub fn format_island_badge(&self) -> String {
        let tag = match self.status {
            MediaPlaybackStatus::Playing => "PLAYING",
            MediaPlaybackStatus::Paused => "PAUSED",
            MediaPlaybackStatus::Stopped => "STOPPED",
        };
        if self.artist.trim().is_empty() {
            format!("[{}] {}", tag, self.title.trim())
        } else {
            format!("[{}] {} — {}", tag, self.title.trim(), self.artist.trim())
        }
    }

    pub fn apply_transport(&mut self, command: MediaTransportCommand) {
        match command {
            MediaTransportCommand::Play => self.status = MediaPlaybackStatus::Playing,
            MediaTransportCommand::Pause => self.status = MediaPlaybackStatus::Paused,
            MediaTransportCommand::TogglePlayPause => {
                self.status = match self.status {
                    MediaPlaybackStatus::Playing => MediaPlaybackStatus::Paused,
                    MediaPlaybackStatus::Paused | MediaPlaybackStatus::Stopped => MediaPlaybackStatus::Playing,
                };
            }
            MediaTransportCommand::Stop => {
                self.status = MediaPlaybackStatus::Stopped;
                self.position_ms = 0;
            }
            MediaTransportCommand::NextTrack | MediaTransportCommand::PreviousTrack => {
                self.position_ms = 0;
            }
        }
    }
}

/// Media transport commands supported by the Island media bar (`F-15`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MediaTransportCommand {
    Play,
    Pause,
    TogglePlayPause,
    NextTrack,
    PreviousTrack,
    Stop,
}

impl MediaTransportCommand {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Play => "play",
            Self::Pause => "pause",
            Self::TogglePlayPause => "toggle_play_pause",
            Self::NextTrack => "next_track",
            Self::PreviousTrack => "previous_track",
            Self::Stop => "stop",
        }
    }
}

/// Portable backend trait abstracting system multimedia volume ducking and media key transport.
pub trait SystemAudioMediaBackend: Send {
    fn read_volume_snapshot(&self) -> Result<SystemAudioSnapshot, VoiceInteractionError>;
    fn apply_volume_snapshot(&mut self, s: &SystemAudioSnapshot) -> Result<(), VoiceInteractionError>;
    fn query_current_track(&self) -> Result<Option<MediaTrackMetadata>, VoiceInteractionError>;
    fn dispatch_media_command(&mut self, cmd: MediaTransportCommand) -> Result<(), VoiceInteractionError>;
}

/// Deterministic in-memory backend for unit and integration testing of ducking and media controls.
#[derive(Debug, Clone)]
pub struct SimulatedAudioMediaBackend {
    current_snapshot: SystemAudioSnapshot,
    current_track: Option<MediaTrackMetadata>,
    applied_snapshots: Vec<SystemAudioSnapshot>,
    dispatched_commands: Vec<MediaTransportCommand>,
    pub fail_on_read_volume: bool,
    pub fail_on_apply_volume: bool,
    pub fail_on_media_command: bool,
}

impl SimulatedAudioMediaBackend {
    pub fn new(initial_volume: f32, muted: bool) -> Self {
        Self {
            current_snapshot: SystemAudioSnapshot::from_scalar(initial_volume, muted),
            current_track: None,
            applied_snapshots: Vec::new(),
            dispatched_commands: Vec::new(),
            fail_on_read_volume: false,
            fail_on_apply_volume: false,
            fail_on_media_command: false,
        }
    }

    pub fn with_track(mut self, track: MediaTrackMetadata) -> Self {
        self.current_track = Some(track);
        self
    }

    pub fn set_current_track(&mut self, track: Option<MediaTrackMetadata>) { self.current_track = track; }
    pub fn current_snapshot(&self) -> SystemAudioSnapshot { self.current_snapshot }
    pub fn applied_snapshots(&self) -> &[SystemAudioSnapshot] { &self.applied_snapshots }
    pub fn dispatched_commands(&self) -> &[MediaTransportCommand] { &self.dispatched_commands }
}

impl SystemAudioMediaBackend for SimulatedAudioMediaBackend {
    fn read_volume_snapshot(&self) -> Result<SystemAudioSnapshot, VoiceInteractionError> {
        if self.fail_on_read_volume {
            return Err(VoiceInteractionError::InvalidTransition {
                from: "AudioVolumeRead".to_string(),
                attempted: "simulated read_volume_snapshot failure".to_string(),
            });
        }
        Ok(self.current_snapshot)
    }

    fn apply_volume_snapshot(&mut self, s: &SystemAudioSnapshot) -> Result<(), VoiceInteractionError> {
        if self.fail_on_apply_volume {
            return Err(VoiceInteractionError::InvalidTransition {
                from: "AudioVolumeApply".to_string(),
                attempted: "simulated apply_volume_snapshot failure".to_string(),
            });
        }
        self.current_snapshot = *s;
        self.applied_snapshots.push(*s);
        Ok(())
    }

    fn query_current_track(&self) -> Result<Option<MediaTrackMetadata>, VoiceInteractionError> {
        Ok(self.current_track.clone())
    }

    fn dispatch_media_command(&mut self, cmd: MediaTransportCommand) -> Result<(), VoiceInteractionError> {
        if self.fail_on_media_command {
            return Err(VoiceInteractionError::InvalidTransition {
                from: "MediaTransport".to_string(),
                attempted: "simulated dispatch_media_command failure".to_string(),
            });
        }
        if let Some(track) = &mut self.current_track {
            track.apply_transport(cmd);
        }
        self.dispatched_commands.push(cmd);
        Ok(())
    }
}

/// Production Windows multimedia backend backed by `winmm.dll` and `user32.dll`.
#[derive(Debug, Clone)]
pub struct WinMmSystemAudioMediaBackend {
    shadow_snapshot: SystemAudioSnapshot,
    current_track: Option<MediaTrackMetadata>,
    allow_os_media_keys: bool,
}

impl Default for WinMmSystemAudioMediaBackend {
    fn default() -> Self { Self::new() }
}

impl WinMmSystemAudioMediaBackend {
    pub fn new() -> Self {
        let initial = Self::query_os_waveout_volume()
            .unwrap_or_else(|| SystemAudioSnapshot::from_scalar(0.80, false));
        Self { shadow_snapshot: initial, current_track: None, allow_os_media_keys: false }
    }

    pub fn with_os_media_keys_enabled(mut self, enabled: bool) -> Self {
        self.allow_os_media_keys = enabled;
        self
    }

    pub fn set_track_metadata(&mut self, track: Option<MediaTrackMetadata>) {
        self.current_track = track;
    }

    #[cfg(target_os = "windows")]
    fn query_os_waveout_volume() -> Option<SystemAudioSnapshot> {
        type WinMmResult = u32;
        type WinDword = u32;
        #[link(name = "winmm")]
        extern "system" { fn waveOutGetVolume(hwo: usize, pdw_volume: *mut WinDword) -> WinMmResult; }
        let mut raw_vol: WinDword = 0;
        let rc = unsafe { waveOutGetVolume(0xFFFF_FFFF, &mut raw_vol) };
        (rc == 0).then(|| SystemAudioSnapshot::from_raw_waveout_u32(raw_vol, false))
    }

    #[cfg(not(target_os = "windows"))]
    fn query_os_waveout_volume() -> Option<SystemAudioSnapshot> { None }

    #[cfg(target_os = "windows")]
    fn apply_os_waveout_volume(raw_u32: u32) -> bool {
        type WinMmResult = u32;
        type WinDword = u32;
        #[link(name = "winmm")]
        extern "system" { fn waveOutSetVolume(hwo: usize, dw_volume: WinDword) -> WinMmResult; }
        unsafe { waveOutSetVolume(0xFFFF_FFFF, raw_u32) == 0 }
    }

    #[cfg(not(target_os = "windows"))]
    fn apply_os_waveout_volume(_raw_u32: u32) -> bool { false }

    #[cfg(target_os = "windows")]
    fn send_os_virtual_media_key(command: MediaTransportCommand) {
        type WinByte = u8;
        type WinDword = u32;
        #[link(name = "user32")]
        extern "system" { fn keybd_event(b_vk: WinByte, b_scan: WinByte, dw_flags: WinDword, dw_extra: usize); }
        let vk: WinByte = match command {
            MediaTransportCommand::Play | MediaTransportCommand::Pause | MediaTransportCommand::TogglePlayPause => 0xB3,
            MediaTransportCommand::NextTrack => 0xB0,
            MediaTransportCommand::PreviousTrack => 0xB1,
            MediaTransportCommand::Stop => 0xB2,
        };
        unsafe {
            keybd_event(vk, 0, 0x0001, 0);
            keybd_event(vk, 0, 0x0001 | 0x0002, 0);
        }
    }

    #[cfg(not(target_os = "windows"))]
    fn send_os_virtual_media_key(_command: MediaTransportCommand) {}
}

impl SystemAudioMediaBackend for WinMmSystemAudioMediaBackend {
    fn read_volume_snapshot(&self) -> Result<SystemAudioSnapshot, VoiceInteractionError> {
        Ok(Self::query_os_waveout_volume().unwrap_or(self.shadow_snapshot))
    }

    fn apply_volume_snapshot(&mut self, s: &SystemAudioSnapshot) -> Result<(), VoiceInteractionError> {
        let effective_raw = if s.muted { 0 } else { s.raw_waveout_volume_u32 };
        let _ = Self::apply_os_waveout_volume(effective_raw);
        self.shadow_snapshot = *s;
        Ok(())
    }

    fn query_current_track(&self) -> Result<Option<MediaTrackMetadata>, VoiceInteractionError> {
        Ok(self.current_track.clone())
    }

    fn dispatch_media_command(&mut self, cmd: MediaTransportCommand) -> Result<(), VoiceInteractionError> {
        if self.allow_os_media_keys {
            Self::send_os_virtual_media_key(cmd);
        }
        if let Some(track) = &mut self.current_track {
            track.apply_transport(cmd);
        }
        Ok(())
    }
}

/// On-disk crash-recovery record written before attenuating system volume (`F-15` / `miku doctor`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct DuckingRecoveryRecord {
    pub original_snapshot: SystemAudioSnapshot,
    pub ducked_volume: f32,
    pub engaged_at_ms: u64,
}

/// Finite-state machine for audio ducking (`Normal` <-> `Ducked`).
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub enum AudioDuckingState {
    Normal,
    Ducked {
        original: SystemAudioSnapshot,
        ducked_volume: f32,
        engaged_at_ms: u64,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct AudioDuckingReceipt {
    pub original_snapshot: SystemAudioSnapshot,
    pub ducked_volume: f32,
    pub engaged_at_ms: u64,
    pub was_already_ducked: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct AudioRestoreReceipt {
    pub restored_snapshot: SystemAudioSnapshot,
    pub previous_ducked_volume: f32,
    pub duck_duration_ms: u64,
}

/// Deterministic Audio Ducking Controller with RAII `Drop` restoration and crash recovery (`F-15`).
pub struct AudioDuckingController {
    backend: Box<dyn SystemAudioMediaBackend>,
    state: AudioDuckingState,
    target_duck_level: f32,
    recovery_file_path: Option<PathBuf>,
    duck_transitions_count: u64,
    restore_transitions_count: u64,
}

impl AudioDuckingController {
    pub fn new(backend: Box<dyn SystemAudioMediaBackend>) -> Self {
        Self {
            backend,
            state: AudioDuckingState::Normal,
            target_duck_level: DEFAULT_AUDIO_DUCKING_TARGET_LEVEL,
            recovery_file_path: None,
            duck_transitions_count: 0,
            restore_transitions_count: 0,
        }
    }

    pub fn new_default_winmm() -> Self { Self::new(Box::new(WinMmSystemAudioMediaBackend::new())) }
    pub fn with_recovery_file(mut self, path: PathBuf) -> Self { self.recovery_file_path = Some(path); self }
    pub const fn state(&self) -> &AudioDuckingState { &self.state }
    pub const fn is_ducked(&self) -> bool { matches!(self.state, AudioDuckingState::Ducked { .. }) }
    pub const fn target_duck_level(&self) -> f32 { self.target_duck_level }
    pub const fn duck_transitions_count(&self) -> u64 { self.duck_transitions_count }
    pub const fn restore_transitions_count(&self) -> u64 { self.restore_transitions_count }
    pub fn current_backend_snapshot(&self) -> Result<SystemAudioSnapshot, VoiceInteractionError> {
        self.backend.read_volume_snapshot()
    }

    pub fn engage_ducking(&mut self, now_ms: u64) -> Result<AudioDuckingReceipt, VoiceInteractionError> {
        if let AudioDuckingState::Ducked { original, ducked_volume, engaged_at_ms } = self.state {
            return Ok(AudioDuckingReceipt {
                original_snapshot: original,
                ducked_volume,
                engaged_at_ms,
                was_already_ducked: true,
            });
        }

        let original = self.backend.read_volume_snapshot()?;
        let ducked_volume = if original.muted || original.volume_scalar <= self.target_duck_level {
            original.volume_scalar
        } else {
            self.target_duck_level
        };

        self.persist_recovery_record(&DuckingRecoveryRecord {
            original_snapshot: original,
            ducked_volume,
            engaged_at_ms: now_ms,
        });

        let target_snapshot = if original.muted || (original.volume_scalar - ducked_volume).abs() <= AUDIO_VOLUME_EPSILON {
            original
        } else {
            SystemAudioSnapshot::from_scalar(ducked_volume, original.muted)
        };

        if let Err(err) = self.backend.apply_volume_snapshot(&target_snapshot) {
            self.clear_recovery_record();
            return Err(err);
        }

        self.state = AudioDuckingState::Ducked { original, ducked_volume, engaged_at_ms: now_ms };
        self.duck_transitions_count = self.duck_transitions_count.saturating_add(1);
        Ok(AudioDuckingReceipt {
            original_snapshot: original,
            ducked_volume,
            engaged_at_ms: now_ms,
            was_already_ducked: false,
        })
    }

    pub fn restore_original_volume(
        &mut self,
        now_ms: u64,
    ) -> Result<Option<AudioRestoreReceipt>, VoiceInteractionError> {
        let (original, ducked_volume, engaged_at_ms) = match self.state {
            AudioDuckingState::Normal => return Ok(None),
            AudioDuckingState::Ducked { original, ducked_volume, engaged_at_ms } => {
                (original, ducked_volume, engaged_at_ms)
            }
        };

        self.state = AudioDuckingState::Normal;
        let apply_res = self.backend.apply_volume_snapshot(&original);
        self.clear_recovery_record();
        apply_res?;

        self.restore_transitions_count = self.restore_transitions_count.saturating_add(1);
        Ok(Some(AudioRestoreReceipt {
            restored_snapshot: original,
            previous_ducked_volume: ducked_volume,
            duck_duration_ms: now_ms.saturating_sub(engaged_at_ms),
        }))
    }

    pub fn recover_interrupted_ducking_from_disk(
        &mut self,
    ) -> Result<Option<SystemAudioSnapshot>, VoiceInteractionError> {
        let Some(path) = &self.recovery_file_path else { return Ok(None) };
        if !path.exists() {
            return Ok(None);
        }
        let raw = fs::read_to_string(path).map_err(|e| VoiceInteractionError::InvalidTransition {
            from: "DuckingCrashRecovery".to_string(),
            attempted: format!("failed to read recovery journal: {}", e),
        })?;
        let record: DuckingRecoveryRecord = serde_json::from_str(&raw).map_err(|e| {
            VoiceInteractionError::InvalidTransition {
                from: "DuckingCrashRecovery".to_string(),
                attempted: format!("invalid recovery journal JSON: {}", e),
            }
        })?;
        self.backend.apply_volume_snapshot(&record.original_snapshot)?;
        self.state = AudioDuckingState::Normal;
        self.clear_recovery_record();
        self.restore_transitions_count = self.restore_transitions_count.saturating_add(1);
        Ok(Some(record.original_snapshot))
    }

    fn persist_recovery_record(&self, record: &DuckingRecoveryRecord) {
        if let Some(path) = &self.recovery_file_path {
            if let Some(parent) = path.parent() {
                let _ = fs::create_dir_all(parent);
            }
            if let Ok(json) = serde_json::to_string(record) {
                let _ = fs::write(path, json);
            }
        }
    }

    fn clear_recovery_record(&self) {
        if let Some(path) = &self.recovery_file_path {
            let _ = fs::remove_file(path);
        }
    }
}

impl Drop for AudioDuckingController {
    fn drop(&mut self) {
        if let AudioDuckingState::Ducked { original, .. } = self.state {
            let _ = self.backend.apply_volume_snapshot(&original);
            self.state = AudioDuckingState::Normal;
            self.clear_recovery_record();
        }
    }
}

/// Optional Wake Word configuration (`F-16` / `BUD-05` / `BUD-06`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct OptionalWakeWordConfig {
    pub enabled_by_user: bool,
    pub power_policy: VoicePowerPolicy,
    pub phrase: String,
}

impl Default for OptionalWakeWordConfig {
    fn default() -> Self {
        Self {
            enabled_by_user: false,
            power_policy: VoicePowerPolicy::default_laptop_safe(),
            phrase: "Hey Miku".to_string(),
        }
    }
}

impl OptionalWakeWordConfig {
    pub const fn is_listening_allowed(&self) -> bool {
        self.enabled_by_user && self.power_policy.is_wake_word_enabled()
    }
    pub const fn active_idle_audio_streams(&self) -> usize {
        if self.enabled_by_user && self.power_policy.is_wake_word_enabled() { 1 } else { 0 }
    }
    pub const fn estimated_idle_cpu_pct(&self) -> f64 { 0.0 }
}

/// Island current track & media transport controller (`F-15`).
pub struct IslandMediaController {
    backend: Box<dyn SystemAudioMediaBackend>,
    last_rendered_badge: Option<String>,
}

impl IslandMediaController {
    pub fn new(backend: Box<dyn SystemAudioMediaBackend>) -> Self {
        Self { backend, last_rendered_badge: None }
    }

    pub fn backend(&self) -> &dyn SystemAudioMediaBackend { self.backend.as_ref() }
    pub fn last_rendered_badge(&self) -> Option<&str> { self.last_rendered_badge.as_deref() }

    pub fn sync_current_track_to_island(
        &mut self,
        bridge: &mut WebView2IpcBridge,
    ) -> Result<Option<WebViewIpcPayloadV1>, VoiceInteractionError> {
        let Some(track) = self.backend.query_current_track()? else {
            self.last_rendered_badge = None;
            return Ok(None);
        };
        let badge_raw = track.format_island_badge();
        let payload = sanitize_for_text_content("miku-island-media-track", &badge_raw);
        let env = WebViewIpcEnvelopeV1::new_event(
            bridge.origin_token(),
            "media.track.updated",
            payload.clone(),
        );
        let raw_json = serde_json::to_string(&env)
            .map_err(|e| VoiceInteractionError::IpcSerializationError(e.to_string()))?;
        let _ = bridge
            .receive_from_webview(&raw_json)
            .map_err(|e| VoiceInteractionError::IpcSerializationError(e.to_string()))?;
        self.last_rendered_badge = Some(badge_raw);
        Ok(Some(payload))
    }

    pub fn execute_transport_command(
        &mut self,
        command: MediaTransportCommand,
        now_ms: u64,
        gatekeeper: &mut GatekeeperEngine,
        bridge: Option<&mut WebView2IpcBridge>,
    ) -> Result<Option<MediaTrackMetadata>, VoiceInteractionError> {
        let verdict = gatekeeper.evaluate_request(
            "miku-island-media",
            Capability::Input,
            "media:transport_control",
            command.as_str(),
            now_ms,
        );
        if !matches!(verdict, GatekeeperVerdict::Allow { .. }) {
            return Err(VoiceInteractionError::InvalidTransition {
                from: "IslandMediaTransport".to_string(),
                attempted: format!("media command '{}' denied by Gatekeeper", command.as_str()),
            });
        }
        self.backend.dispatch_media_command(command)?;
        if let Some(b) = bridge {
            let _ = self.sync_current_track_to_island(b)?;
        }
        self.backend.query_current_track()
    }
}
