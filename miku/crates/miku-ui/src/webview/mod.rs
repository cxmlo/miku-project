//! WebView2 domain: Mode C on-demand lifecycle host with warm TTL (`30_000 ms`), isolated
//! `%LOCALAPPDATA%\miku\webview2` data folder, JSON-RPC v1 IPC bridge with mandatory `textContent`
//! XSS neutralization, and Overview scene (`https://miku.local/index.html`) (`ADR-001` / `ADR-011`).

pub mod host_fsm;
pub mod ipc_bridge;
pub mod ipc_envelope;
pub mod overview_bundle;
pub mod overview_controller;
pub mod security_and_driver;

pub use self::host_fsm::*;
pub use self::ipc_bridge::*;
pub use self::ipc_envelope::*;
pub use self::overview_bundle::*;
pub use self::overview_controller::*;
pub use self::security_and_driver::*;

#[cfg(test)]
mod tests_host_and_ipc;
#[cfg(test)]
mod tests_overview;
