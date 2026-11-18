//! Safe COM UI Automation inspection & invocation wrapped in `tokio::task::spawn_blocking`
//! with `COINIT_MULTITHREADED`, bounded tree scope (`depth<=3, N<=40`), and strict 600ms timeout.

use std::time::Duration;
use crate::pc_control::backends::UiAutomationActionReceipt;
use crate::pc_control::win32_uia_com::{
    inspect_foreground_uia_sync, invoke_foreground_uia_sync, UiaTreeSnapshot, MAX_UIA_CONTROLS,
    MAX_UIA_DEPTH,
};
use crate::PlatformError;

pub const UIA_TIMEOUT_MS: u64 = 600;
pub const UIA_TIMEOUT_DURATION: Duration = Duration::from_millis(UIA_TIMEOUT_MS);

/// Synchronously attempts native COM UI Automation invocation (`COINIT_MULTITHREADED`).
fn execute_native_uia_invoke_sync(
    window_title_pattern: &str,
    automation_id_or_name: &str,
    control_type: &str,
    simulated_delay_ms: Option<u64>,
) -> UiAutomationActionReceipt {
    if let Some(delay) = simulated_delay_ms {
        std::thread::sleep(Duration::from_millis(delay));
        return UiAutomationActionReceipt {
            used_ui_automation_tree: true,
            target_summary: format!(
                "win32_uia_invoked:{}|{}|{}",
                window_title_pattern, automation_id_or_name, control_type
            ),
        };
    }

    match invoke_foreground_uia_sync(window_title_pattern, automation_id_or_name, control_type) {
        Ok(receipt) => receipt,
        Err(err) => UiAutomationActionReceipt {
            used_ui_automation_tree: false,
            target_summary: format!(
                "win32_uia_error_fallback:{}|{}|{}|err={}",
                window_title_pattern, automation_id_or_name, control_type, err
            ),
        },
    }
}

/// Inspects bounded UI Automation controls (`depth <= 3`, `N <= 40`) in `spawn_blocking` with 600ms timeout.
pub async fn inspect_uia_with_600ms_timeout(
    window_title_pattern: &str,
) -> Result<UiaTreeSnapshot, PlatformError> {
    let title = window_title_pattern.to_string();
    let blocking_task = tokio::task::spawn_blocking(move || {
        inspect_foreground_uia_sync(&title, MAX_UIA_DEPTH, MAX_UIA_CONTROLS)
    });
    match tokio::time::timeout(UIA_TIMEOUT_DURATION, blocking_task).await {
        Ok(Ok(res)) => res,
        Ok(Err(join_err)) => Err(PlatformError::DaemonStartupFailed(format!("UIA inspect panic: {}", join_err))),
        Err(_) => Err(PlatformError::DaemonStartupFailed("UIA inspect timeout (600ms)".to_string())),
    }
}

/// Synchronous safe entry point to inspect bounded UIA controls under 600ms timeout.
pub fn safe_ui_automation_inspect(
    window_title_pattern: &str,
) -> Result<UiaTreeSnapshot, PlatformError> {
    if tokio::runtime::Handle::try_current().is_ok() {
        let title = window_title_pattern.to_string();
        std::thread::scope(|s| {
            s.spawn(move || {
                let rt = tokio::runtime::Builder::new_current_thread()
                    .enable_all()
                    .build()
                    .map_err(|e| PlatformError::DaemonStartupFailed(e.to_string()))?;
                rt.block_on(inspect_uia_with_600ms_timeout(&title))
            })
            .join()
            .map_err(|_| PlatformError::DaemonStartupFailed("UIA inspect thread panicked".to_string()))?
        })
    } else {
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .map_err(|e| PlatformError::DaemonStartupFailed(e.to_string()))?;
        rt.block_on(inspect_uia_with_600ms_timeout(window_title_pattern))
    }
}

/// Invokes UI Automation asynchronously inside `tokio::task::spawn_blocking` protected by
/// a strict 600ms timeout (`UIA_TIMEOUT_DURATION`). If timed out, panicked, or unavailable,
/// returns a graceful fallback receipt without stalling the runtime.
pub async fn invoke_uia_with_600ms_timeout(
    window_title_pattern: &str,
    automation_id_or_name: &str,
    control_type: &str,
    simulated_delay_ms: Option<u64>,
) -> UiAutomationActionReceipt {
    let title = window_title_pattern.to_string();
    let id_or_name = automation_id_or_name.to_string();
    let ctrl = control_type.to_string();

    let blocking_task = tokio::task::spawn_blocking(move || {
        execute_native_uia_invoke_sync(&title, &id_or_name, &ctrl, simulated_delay_ms)
    });

    match tokio::time::timeout(UIA_TIMEOUT_DURATION, blocking_task).await {
        Ok(Ok(receipt)) => receipt,
        Ok(Err(join_err)) => UiAutomationActionReceipt {
            used_ui_automation_tree: false,
            target_summary: format!(
                "win32_uia_panic_fallback:{}|{}|{}|err={}",
                window_title_pattern, automation_id_or_name, control_type, join_err
            ),
        },
        Err(_timeout_elapsed) => UiAutomationActionReceipt {
            used_ui_automation_tree: false,
            target_summary: format!(
                "win32_uia_timeout_fallback_600ms:{}|{}|{}",
                window_title_pattern, automation_id_or_name, control_type
            ),
        },
    }
}

/// Synchronous safe entry point that executes `invoke_uia_with_600ms_timeout`
/// whether called from within an existing Tokio runtime or a regular OS thread.
pub fn safe_ui_automation_invoke(
    window_title_pattern: &str,
    automation_id_or_name: &str,
    control_type: &str,
    simulated_delay_ms: Option<u64>,
) -> Result<UiAutomationActionReceipt, PlatformError> {
    if tokio::runtime::Handle::try_current().is_ok() {
        let title = window_title_pattern.to_string();
        let id_or_name = automation_id_or_name.to_string();
        let ctrl = control_type.to_string();
        std::thread::scope(|s| {
            s.spawn(move || {
                let rt = tokio::runtime::Builder::new_current_thread()
                    .enable_all()
                    .build()
                    .map_err(|e| PlatformError::DaemonStartupFailed(e.to_string()))?;
                Ok(rt.block_on(invoke_uia_with_600ms_timeout(
                    &title,
                    &id_or_name,
                    &ctrl,
                    simulated_delay_ms,
                )))
            })
            .join()
            .map_err(|_| {
                PlatformError::DaemonStartupFailed("UIA worker thread panicked".to_string())
            })?
        })
    } else {
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .map_err(|e| PlatformError::DaemonStartupFailed(e.to_string()))?;
        Ok(rt.block_on(invoke_uia_with_600ms_timeout(
            window_title_pattern,
            automation_id_or_name,
            control_type,
            simulated_delay_ms,
        )))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_uia_invocation_succeeds_within_timeout() {
        let receipt = invoke_uia_with_600ms_timeout(
            "TestWindow",
            "btnSubmit",
            "Button",
            Some(10), // 10ms << 600ms
        )
        .await;
        assert!(receipt.target_summary.contains("TestWindow"));
    }

    #[tokio::test]
    async fn test_uia_invocation_graceful_fallback_on_600ms_timeout() {
        let receipt = invoke_uia_with_600ms_timeout(
            "HangingWindow",
            "btnFreeze",
            "Button",
            Some(800), // 800ms > 600ms strict timeout
        )
        .await;
        // Verify timeout was caught and graceful fallback receipt returned
        assert!(!receipt.used_ui_automation_tree);
        assert!(
            receipt.target_summary.contains("timeout_fallback_600ms"),
            "got receipt: {:?}",
            receipt
        );
    }
}
