//! Standalone Demo Mode domain (`miku --demo` / `miku demo` · `F-28`): CLI parser, in-RAM
//! `DemoSandboxGuard` (`0` disk writes, `0` API keys, mandatory `[DEMO]` badge), interactive
//! 6-spring controller, and live top-center Win32 layered demo runner.

mod controller;
pub mod sandbox_and_options;
pub mod win32_event_loop;
mod win32_ffi_and_paint;

pub use self::sandbox_and_options::*;
pub use self::win32_event_loop::*;
#[cfg(target_os = "windows")]
pub(crate) use self::win32_ffi_and_paint::*;

#[cfg(test)]
mod tests;
