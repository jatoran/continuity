//! C-compatible public value types.

use std::ffi::c_void;

/// ABI major supported by this library.
pub const CONTINUITY_ENGINE_ABI_MAJOR: u16 = 1;
/// ABI minor supported by this library.
pub const CONTINUITY_ENGINE_ABI_MINOR: u16 = 0;
/// Canonical SDK major version.
pub const CONTINUITY_ENGINE_SDK_MAJOR: u16 = 0;
/// Canonical SDK minor version.
pub const CONTINUITY_ENGINE_SDK_MINOR: u16 = 1;
/// Canonical SDK patch version.
pub const CONTINUITY_ENGINE_SDK_PATCH: u16 = 0;
/// UTF-16 input and output capability.
pub const CONTINUITY_ENGINE_CAP_UTF16: u64 = 1 << 0;
/// Post-mutation callback capability.
pub const CONTINUITY_ENGINE_CAP_CALLBACK: u64 = 1 << 1;
/// Multi-cursor editing capability.
pub const CONTINUITY_ENGINE_CAP_MULTI_CURSOR: u64 = 1 << 2;
/// Branching undo capability.
pub const CONTINUITY_ENGINE_CAP_BRANCHING_UNDO: u64 = 1 << 3;

/// Status returned by every fallible ABI function.
#[repr(i32)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ContinuityEngineStatus {
    /// Operation completed.
    Ok = 0,
    /// A required pointer was null.
    NullPointer = 1,
    /// Input was not valid UTF-8.
    InvalidUtf8 = 2,
    /// Input was not valid UTF-16.
    InvalidUtf16 = 3,
    /// A source position was invalid.
    InvalidPosition = 4,
    /// The handle was called from a thread other than its creator.
    WrongThread = 5,
    /// An API call was attempted from inside its change callback.
    ReentrantCall = 6,
    /// The requested ABI major is unsupported.
    UnsupportedAbi = 7,
    /// The engine rejected the operation.
    EngineError = 8,
    /// A Rust panic was contained at the ABI boundary.
    Panic = 9,
}

/// ABI and feature negotiation record.
#[repr(C)]
#[derive(Clone, Copy, Debug)]
pub struct ContinuityEngineCapabilities {
    /// Size of this struct in bytes.
    pub struct_size: u32,
    /// Supported ABI major.
    pub abi_major: u16,
    /// Supported ABI minor.
    pub abi_minor: u16,
    /// SDK release major.
    pub sdk_major: u16,
    /// SDK release minor.
    pub sdk_minor: u16,
    /// SDK release patch.
    pub sdk_patch: u16,
    /// Reserved; callers initialize to zero and ignore.
    pub reserved: u16,
    /// Bitwise capability flags.
    pub flags: u64,
}

/// Zero-based source position using UTF-8 bytes within a line.
#[repr(C)]
#[derive(Clone, Copy, Debug)]
pub struct ContinuityEnginePosition {
    /// Zero-based source line.
    pub line: u32,
    /// Zero-based UTF-8 byte offset within the line.
    pub byte_in_line: u32,
}

/// One source edit delta in absolute UTF-8 bytes.
#[repr(C)]
#[derive(Clone, Copy, Debug)]
pub struct ContinuityEngineDelta {
    /// Absolute source byte offset.
    pub at: usize,
    /// Removed UTF-8 byte count.
    pub removed_bytes: usize,
    /// Inserted UTF-8 byte count.
    pub inserted_bytes: usize,
}

/// Rust-allocated UTF-8 byte buffer.
#[repr(C)]
#[derive(Clone, Copy, Debug)]
pub struct ContinuityEngineString {
    /// Buffer data, not NUL-terminated.
    pub data: *mut u8,
    /// Buffer length in bytes.
    pub len: usize,
}

/// Rust-allocated UTF-16 code-unit buffer.
#[repr(C)]
#[derive(Clone, Copy, Debug)]
pub struct ContinuityEngineUtf16String {
    /// Buffer data, not NUL-terminated.
    pub data: *mut u16,
    /// Buffer length in code units.
    pub len: usize,
}

/// Opaque engine handle owned by its creating thread.
pub struct ContinuityEngineHandle(pub(crate) crate::handle::Handle);

/// Post-mutation callback. Calls into the same handle are rejected while it runs.
pub type ContinuityEngineChangeCallback =
    Option<unsafe extern "C" fn(user_data: *mut c_void, revision: u64)>;
