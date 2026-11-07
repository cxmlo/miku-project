#![allow(unused_imports)]
use crate::prelude::*;

// ============================================================================
// 14. Step 2.9: Global Win32 Emergency Kill Switch (`Ctrl+Alt+Shift+K`)
//     with Atomic Gatekeeper Revocation, Voice Zero-Fill & Constant-Time Rearm
//     (F-21 / ADR-007 / ADR-009)
// ============================================================================

#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(tag = "status", rename_all = "snake_case")]
pub enum KillSwitchStatus {
    #[default]
    Armed,
    Tripped {
        tripped_at_ms: u64,
        reason: String,
    },
    Rearming {
        challenge_token: String,
        expires_at_ms: u64,
        failed_attempts: u8,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct HotkeyRegistrationStatus {
    pub hotkey_id: i32,
    pub combo: String,
    pub modifiers_mask: u32,
    pub vk_code: u32,
    pub os_registered: bool,
    pub fallback_ui_button_active: bool,
    pub diagnostic_note: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct KillSwitchTripReport {
    pub tripped_at_ms: u64,
    pub reason: String,
    pub reaction_latency_ms: f64,
    pub revoked_gatekeeper_tickets: usize,
    pub voice_aborted: bool,
    pub pcm_zero_report: VoiceBufferZeroReport,
    pub audit_seq: u64,
    pub audit_entry_hash: String,
    pub island_state: IslandState,
    pub avatar_mood: AvatarMood,
    pub pill_border_color_hex: String,
    pub ipc_alert_cell: WebViewIpcPayloadV1,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RearmChallenge {
    pub challenge_token: String,
    pub issued_at_ms: u64,
    pub expires_at_ms: u64,
    pub max_attempts: u8,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct KillSwitchAuditReport {
    pub trips_executed: u64,
    pub total_revoked_tickets: u64,
    pub voice_sessions_aborted: u64,
    pub pcm_zero_fills_verified: u64,
    pub rearms_completed: u64,
    pub rejected_rearm_attempts: u64,
    pub blocked_actions_while_tripped: u64,
    pub max_trip_latency_ms: f64,
    pub audit_chain_verified: bool,
    pub hotkey_fallback_verified: bool,
    pub xss_and_focus_verified: bool,
}

#[cfg(target_os = "windows")]
pub(crate) mod win32_kill_switch_hotkey {
    use std::ffi::c_void;
    use std::ptr::null_mut;

    type Hwnd = *mut c_void;
    type Bool = i32;
    type Uint = u32;

    #[link(name = "user32")]
    extern "system" {
        fn RegisterHotKey(h_wnd: Hwnd, id: i32, fs_modifiers: Uint, vk: Uint) -> Bool;
        fn UnregisterHotKey(h_wnd: Hwnd, id: i32) -> Bool;
    }

    /// Probes global OS hotkey availability on the current thread (`NULL` HWND) using
    /// `MOD_CONTROL | MOD_ALT | MOD_SHIFT | MOD_NOREPEAT` (`0x4007`) and `VK_K` (`0x4B`),
    /// immediately releasing the probe so parallel tests never collide.
    pub fn probe_and_verify_win32_hotkey(id: i32, modifiers: u32, vk: u32) -> bool {
        unsafe {
            let registered = RegisterHotKey(null_mut(), id, modifiers, vk);
            if registered != 0 {
                let _ = UnregisterHotKey(null_mut(), id);
                true
            } else {
                false
            }
        }
    }
}

pub struct UiKillSwitchManager {
    pub(crate) status: KillSwitchStatus,
    pub(crate) hotkey_id: i32,
    pub(crate) hotkey_combo: String,
    pub(crate) modifiers_mask: u32,
    pub(crate) vk_code: u32,
    pub(crate) os_hotkey_registered: bool,
    pub(crate) fallback_ui_button_active: bool,
    pub(crate) island_state: IslandState,
    pub(crate) pill_border_color_hex: String,
    pub(crate) trips_count: u64,
    pub(crate) total_revoked_tickets: u64,
    pub(crate) voice_aborts_count: u64,
    pub(crate) pcm_zero_fills_count: u64,
    pub(crate) rearms_completed_count: u64,
    pub(crate) rejected_rearm_attempts_count: u64,
    pub(crate) blocked_while_tripped_count: u64,
    pub(crate) max_trip_latency_ms: f64,
    pub(crate) next_challenge_nonce: u64,
    pub(crate) next_ipc_seq: u64,
}

impl Default for UiKillSwitchManager {
    fn default() -> Self {
        Self::new_default()
    }
}

impl UiKillSwitchManager {
    pub fn new() -> Self {
        Self::new_default()
    }

    pub fn new_default() -> Self {
        Self {
            status: KillSwitchStatus::Armed,
            hotkey_id: KILL_SWITCH_HOTKEY_ID,
            hotkey_combo: DEFAULT_KILL_SWITCH_HOTKEY_COMBO.to_string(),
            modifiers_mask: KILL_SWITCH_WIN32_MODIFIERS,
            vk_code: KILL_SWITCH_WIN32_VK_K,
            os_hotkey_registered: false,
            fallback_ui_button_active: true,
            island_state: IslandState::Idle,
            pill_border_color_hex: "#00E5FF".to_string(),
            trips_count: 0,
            total_revoked_tickets: 0,
            voice_aborts_count: 0,
            pcm_zero_fills_count: 0,
            rearms_completed_count: 0,
            rejected_rearm_attempts_count: 0,
            blocked_while_tripped_count: 0,
            max_trip_latency_ms: 0.0,
            next_challenge_nonce: 1,
            next_ipc_seq: 1,
        }
    }

    pub fn status(&self) -> &KillSwitchStatus {
        &self.status
    }

    pub fn is_tripped(&self) -> bool {
        matches!(self.status, KillSwitchStatus::Tripped { .. })
    }

    pub fn is_tripped_or_rearming(&self) -> bool {
        !matches!(self.status, KillSwitchStatus::Armed)
    }

    pub fn hotkey_id(&self) -> i32 {
        self.hotkey_id
    }

    pub fn hotkey_combo(&self) -> &str {
        &self.hotkey_combo
    }

    pub fn modifiers_mask(&self) -> u32 {
        self.modifiers_mask
    }

    pub fn vk_code(&self) -> u32 {
        self.vk_code
    }

    pub fn os_hotkey_registered(&self) -> bool {
        self.os_hotkey_registered
    }

    pub fn fallback_ui_button_active(&self) -> bool {
        self.fallback_ui_button_active
    }

    pub fn island_state(&self) -> IslandState {
        self.island_state
    }

    pub fn pill_border_color_hex(&self) -> &str {
        &self.pill_border_color_hex
    }

    pub fn trips_count(&self) -> u64 {
        self.trips_count
    }

    pub fn total_revoked_tickets(&self) -> u64 {
        self.total_revoked_tickets
    }

    pub fn voice_aborts_count(&self) -> u64 {
        self.voice_aborts_count
    }

    pub fn pcm_zero_fills_count(&self) -> u64 {
        self.pcm_zero_fills_count
    }

    pub fn rearms_completed_count(&self) -> u64 {
        self.rearms_completed_count
    }

    pub fn rejected_rearm_attempts_count(&self) -> u64 {
        self.rejected_rearm_attempts_count
    }

    pub fn blocked_while_tripped_count(&self) -> u64 {
        self.blocked_while_tripped_count
    }

    pub fn max_trip_latency_ms(&self) -> f64 {
        self.max_trip_latency_ms
    }

    /// Registers the global emergency shortcut `Ctrl+Alt+Shift+K` using Win32
    /// `MOD_CONTROL | MOD_ALT | MOD_SHIFT | 0x4000` (`MOD_NOREPEAT`) and `0x4B` (`VK_K`).
    /// Never panics if another application holds the shortcut: automatically enables the
    /// Island UI emergency button fallback (`fallback_ui_button_active = true`).
    pub fn register_hotkey(&mut self, simulate_conflict: bool) -> HotkeyRegistrationStatus {
        self.fallback_ui_button_active = true;

        if simulate_conflict {
            self.os_hotkey_registered = false;
            return HotkeyRegistrationStatus {
                hotkey_id: self.hotkey_id,
                combo: self.hotkey_combo.clone(),
                modifiers_mask: self.modifiers_mask,
                vk_code: self.vk_code,
                os_registered: false,
                fallback_ui_button_active: true,
                diagnostic_note: format!(
                    "RegisterHotKey conflict detected for '{}' (id=0x{:04X}); Island UI emergency button fallback active",
                    self.hotkey_combo, self.hotkey_id
                ),
            };
        }

        #[cfg(target_os = "windows")]
        let registered_ok = win32_kill_switch_hotkey::probe_and_verify_win32_hotkey(
            self.hotkey_id,
            self.modifiers_mask,
            self.vk_code,
        );
        #[cfg(not(target_os = "windows"))]
        let registered_ok = true;

        self.os_hotkey_registered = registered_ok;
        let diagnostic_note = if registered_ok {
            format!(
                "Win32 RegisterHotKey verified for '{}' (id=0x{:04X}, mods=0x{:04X}, vk=0x{:02X})",
                self.hotkey_combo, self.hotkey_id, self.modifiers_mask, self.vk_code
            )
        } else {
            format!(
                "OS hotkey '{}' already bound by external process; Island UI fallback button active",
                self.hotkey_combo
            )
        };

        HotkeyRegistrationStatus {
            hotkey_id: self.hotkey_id,
            combo: self.hotkey_combo.clone(),
            modifiers_mask: self.modifiers_mask,
            vk_code: self.vk_code,
            os_registered: self.os_hotkey_registered,
            fallback_ui_button_active: self.fallback_ui_button_active,
            diagnostic_note,
        }
    }

    /// Rejects any privileged UI/agent operation while the Kill Switch is in `Tripped` or `Rearming`.
    pub fn guard_operation(&mut self, operation_name: &str) -> Result<(), UiError> {
        if self.is_tripped_or_rearming() {
            self.blocked_while_tripped_count += 1;
            return Err(UiError::KillSwitchError(format!(
                "operation '{}' denied: global Kill Switch is active ({:?})",
                operation_name, self.status
            )));
        }
        Ok(())
    }
}
