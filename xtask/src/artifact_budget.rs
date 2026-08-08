//! Release artifact size budgets shared by validation and packaging.

/// Maximum size of the stripped native Windows executable.
pub(crate) const WINDOWS_BINARY_SIZE_BUDGET_BYTES: u64 = 9 * 1024 * 1024;
