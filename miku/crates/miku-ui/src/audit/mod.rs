//! Audit domain: append-only JSONL persistent store (`%LOCALAPPDATA%\miku\audit\audit.jsonl`),
//! transactional size rotation (`audit.1.jsonl`, `audit.2.jsonl`), `< 50 ms` SHA-256 chain verifier,
//! and XSS-safe WebView2 audit viewer (`ADR-007` / `F-09`).

mod append_and_verify;
pub mod paths_and_store;
pub mod viewer_and_bench;

pub(crate) use self::append_and_verify::*;
pub use self::paths_and_store::*;
pub use self::viewer_and_bench::*;

#[cfg(test)]
mod tests;
