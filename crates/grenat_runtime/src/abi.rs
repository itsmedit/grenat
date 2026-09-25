//! Structures shared by compiled code and the programs that run it (the
//! interpreter, a standalone executable), with their layout.

use std::mem::offset_of;

use crate::poll::Poll;

/// The context of a native call, given to its trampoline.
#[repr(C)]
pub struct Context<'a> {
    /// Written by native code: 0, or a status (see [`status`](crate::status)).
    pub status: i64,
    /// Depth at which `StackOverflow` is raised.
    pub limit: i64,
    /// Written with an error status: the site where native code stopped.
    pub site: i64,
    /// Written by `exit`.
    pub exit_code: i64,
    /// The call's checkpoint, read by native code (its flag first).
    pub poll: Poll<'a>,
}

pub const STATUS_OFFSET: i32 = offset_of!(Context, status) as i32;
pub const LIMIT_OFFSET: i32 = offset_of!(Context, limit) as i32;
pub const SITE_OFFSET: i32 = offset_of!(Context, site) as i32;
pub const EXIT_CODE_OFFSET: i32 = offset_of!(Context, exit_code) as i32;
pub const POLL_OFFSET: i32 = offset_of!(Context, poll) as i32;

/// Uniform entry point of a compiled function: its arguments as 64-bit
/// slots, and the call's context; returns its result's bits.
pub type Trampoline = extern "C" fn(*const u64, *mut Context<'_>) -> u64;

/// A byte string of the image (not NUL-terminated).
#[repr(C)]
pub struct Bytes {
    pub ptr: *const u8,
    pub len: u64,
}

impl Bytes {
    /// # Safety
    /// `ptr` must point to `len` bytes of UTF-8 that live forever (data of the executable).
    pub unsafe fn text(&self) -> &'static str {
        // SAFETY: by contract
        unsafe { std::str::from_utf8_unchecked(std::slice::from_raw_parts(self.ptr, self.len as usize)) }
    }
}

/// Where a native program may stop with an error.
#[repr(C)]
pub struct SiteRecord {
    pub function: Bytes,
    /// Byte offsets of the function's definition in the source.
    pub function_start: u64,
    pub function_end: u64,
    /// Byte offsets in the source.
    pub start: u64,
    pub end: u64,
    /// For a value native code cannot represent: what it is (else empty).
    pub reason: Bytes,
}

/// A standalone program (`grenat build --native`), exported as `grenat_program`.
#[repr(C)]
pub struct Standalone {
    /// Trampoline of `main`.
    pub main: Trampoline,
    /// `main(args: Array(String))` rather than `main`.
    pub takes_args: u64,
    pub source: Bytes,
    /// The source's file table (`base path` lines), for error reports.
    pub files: Bytes,
    pub sites: *const SiteRecord,
    pub site_count: u64,
}

// SAFETY: immutable data of the executable
unsafe impl Sync for Standalone {}

/// Name of the exported [`Standalone`].
pub const STANDALONE_SYMBOL: &str = "grenat_program";
