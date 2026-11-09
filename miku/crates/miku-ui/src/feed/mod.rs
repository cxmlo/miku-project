//! Technical Feed domain (`F-14` / Micro-step 3.11):
//! Aggregates PRs, CI status, RSS/Atom feeds, and Repo Radar with conditional HTTP caching
//! (`ETag` / `If-None-Match` and `Last-Modified` / `If-Modified-Since`), `304 Not Modified`
//! zero-reparse & zero-UI-wake guarantees, GitHub `X-RateLimit-Remaining` enforcement,
//! per-source fault isolation, Gaming Mode & offline pause/resume, and Gatekeeper-authorized
//! link opening in Zen Browser.

pub mod engine;
pub mod types_and_cache;

pub use self::engine::*;
pub use self::types_and_cache::*;

#[cfg(test)]
mod tests;
