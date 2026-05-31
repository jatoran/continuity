#![warn(missing_docs)]
//! Shared test fixtures: golden buffers, fake clocks, and proptest
//! generators.
//!
//! Pulled in as a `dev-dependency` only.

pub mod clock;
pub mod fixtures;
pub mod gen;
pub mod percentiles;
pub mod win32_harness;

pub use clock::FakeClock;
pub use fixtures::{golden_markdown, hello_world};
pub use percentiles::{assert_within_budget, Percentiles};
pub use win32_harness::Win32Harness;
