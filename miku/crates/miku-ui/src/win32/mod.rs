//! Win32 native layered pill domain: DPI scaling, multi-monitor geometry, 6px hover hysteresis FSM,
//! zero-focus-stealing (`WS_EX_LAYERED | WS_EX_TOPMOST | WS_EX_TOOLWINDOW | WS_EX_NOACTIVATE`),
//! and native verification cycle (`ADR-001` / `ADR-011` / `F-01`).

pub mod constants_and_errors;
pub mod geometry_and_styles;
pub mod hover_fsm;
pub mod native_window;

pub use self::constants_and_errors::*;
pub use self::geometry_and_styles::*;
pub use self::hover_fsm::*;
pub use self::native_window::*;

#[cfg(test)]
mod tests;
