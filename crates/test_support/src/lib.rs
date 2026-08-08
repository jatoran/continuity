#![warn(missing_docs)]
//! Shared test fixtures: golden buffers, fake clocks, and proptest
//! generators.
//!
//! Pulled in as a `dev-dependency` only.

#[cfg(feature = "native-harness")]
pub mod clock;
#[cfg(feature = "native-harness")]
pub mod editor_control_harness;
#[cfg(feature = "native-harness")]
pub mod fixtures;
#[cfg(feature = "native-harness")]
pub mod gen;
pub mod percentiles;
#[cfg(feature = "native-harness")]
pub mod win32_harness;

#[cfg(feature = "native-harness")]
pub use clock::FakeClock;
pub use continuity_test_fixtures::parity_corpus;
#[cfg(feature = "native-harness")]
pub use editor_control_harness::EditorControlHarness;
#[cfg(feature = "native-harness")]
pub use fixtures::{golden_markdown, hello_world};
pub use percentiles::{assert_within_budget, Percentiles};
#[cfg(feature = "native-harness")]
pub use win32_harness::Win32Harness;
