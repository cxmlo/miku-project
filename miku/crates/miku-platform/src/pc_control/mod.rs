pub mod backends;
pub mod controller;
pub mod coords;
pub mod types_and_guards;
pub mod win32_input;
pub mod win32_uia;
pub mod win32_uia_com;

#[cfg(test)]
mod tests;

pub use backends::{
    InputStep, PcControlBackend, SandboxPcControlBackend, ShellExecOutput,
    UiAutomationActionReceipt, WindowsNativePcControlBackend,
};
pub use controller::{KillSwitchAbortReport, PcControlController, PcControlOutcome};
pub use coords::{
    desktop_virtual_to_sendinput_abs, normalized_1000_to_virtual, normalized_coords_to_virtual,
    pixel_to_virtual, scale_mouse_click_coords, screen_coords_to_virtual,
    virtual_to_normalized_coords, virtual_to_screen_pixel, virtual_to_sendinput_abs,
    CaptureTransform,
};
pub use types_and_guards::{
    matches_immutable_shell_denylist, PcActionExecutionResult, PcControlAction,
    IMMUTABLE_SHELL_DENY_PATTERNS,
};
pub use win32_input::{
    is_sensitive_window_metadata, parse_modifier_vk, parse_virtual_key,
    send_mouse_absolute_click, send_unicode_text, send_virtual_key_down, send_virtual_key_tap,
    send_virtual_key_up, verify_input_target_safe, KeyComboGuard, KEYEVENTF_KEYUP, VK_CONTROL,
    VK_LWIN, VK_MENU, VK_RWIN, VK_SHIFT,
};
pub use win32_uia::{
    inspect_uia_with_600ms_timeout, invoke_uia_with_600ms_timeout, safe_ui_automation_inspect,
    safe_ui_automation_invoke, UIA_TIMEOUT_DURATION, UIA_TIMEOUT_MS,
};
pub use win32_uia_com::{
    inspect_foreground_uia_sync, invoke_foreground_uia_sync, UiaControlNode, UiaTreeSnapshot,
    MAX_UIA_CONTROLS, MAX_UIA_DEPTH,
};
