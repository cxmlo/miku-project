//! Avatar domain: 22-asset SVG v7 catalog validator, 6-spring semi-implicit Euler physics engine
//! (`0.00 fps` settled sleep gate), production avatar runtime, and atomic `< 50 ms` hot-reload
//! (`ADR-002` / `F-02` / `F-03`).

pub mod catalog;
pub mod physics;
pub mod runtime_hot_reload;

pub use self::catalog::*;
pub use self::physics::*;
pub use self::runtime_hot_reload::*;

#[cfg(test)]
mod tests;
