pub mod discovery_and_profile;
pub mod orchestrator;
pub mod protocol_drivers;
pub mod types_and_guards;

#[cfg(test)]
mod tests;

pub use discovery_and_profile::{
    build_isolated_launch_spec, detect_installed_browsers, discover_default_windows_browser,
    is_personal_profile_protected, resolve_isolated_profile_dir, BrowserDiscoveryReport,
    BrowserEngineKind, BrowserLaunchSpec,
};
pub use orchestrator::{
    BrowserApprovalOrigin, BrowserController, BrowserWorkflowOutcome, DownloadRequest,
    IslandBrowserCard,
};
pub use protocol_drivers::{
    BrowserBackend, CdpWebSocketClient, GeckoBiDiClient, LiveLoopbackDualProtocolBackend,
};
pub use types_and_guards::{
    extract_url_host, sanitize_browser_text, validate_browser_url, validate_download_target,
    BrowserError, BrowserPageSnapshot, BrowserProtocolKind, DownloadJailPolicy, DownloadReceipt,
    DANGEROUS_DOWNLOAD_EXTENSIONS, MAX_DOM_TEXT_EXCERPT_BYTES,
};
