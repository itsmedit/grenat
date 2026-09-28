//! The boundary between Grenat and a facet's library: C functions that
//! exchange bytes only (JSON, UTF-8), so that no Rust type — whose layout
//! changes from one compiler to the next — ever crosses it.
//!
//! A library exports:
//!
//! - `grenat_ext_abi_version() -> u32`: [`ABI_VERSION`], checked before anything else;
//! - `grenat_ext_manifest(out) -> status`: its [`Manifest`](crate::manifest::Manifest), as JSON;
//! - `grenat_ext_free(buffer)`: frees a buffer it handed out;
//! - one entry point per function, `grenat_ext_v1_<name>(args, len, out) -> status`:
//!   `args` is a JSON array of the arguments, owned by the caller; the
//!   library writes into `out` a buffer it owns — the result's JSON, or an
//!   error `{"type": "…", "message": "…"}` — which the caller gives back to
//!   `grenat_ext_free` once read.
//!
//! The status says which: [`STATUS_OK`], [`STATUS_ERROR`] (an `Err`, or
//! arguments that do not decode) or [`STATUS_PANIC`] (caught: a panic never
//! unwinds across the boundary).

/// The version of this ABI. A library built for another is refused.
pub const ABI_VERSION: u32 = 1;

pub const VERSION_SYMBOL: &str = "grenat_ext_abi_version";
pub const MANIFEST_SYMBOL: &str = "grenat_ext_manifest";
pub const FREE_SYMBOL: &str = "grenat_ext_free";
/// Entry points are named `grenat_ext_v1_<function>`.
pub const ENTRY_PREFIX: &str = "grenat_ext_v1_";

pub const STATUS_OK: i32 = 0;
pub const STATUS_ERROR: i32 = 1;
pub const STATUS_PANIC: i32 = 2;

pub type VersionFn = unsafe extern "C" fn() -> u32;
pub type ManifestFn = unsafe extern "C" fn(out: *mut Buffer) -> i32;
pub type EntryFn = unsafe extern "C" fn(args: *const u8, len: usize, out: *mut Buffer) -> i32;
pub type FreeFn = unsafe extern "C" fn(buffer: Buffer);

/// Bytes allocated by the library, freed by it (`grenat_ext_free`).
#[repr(C)]
#[derive(Debug)]
pub struct Buffer {
    pub ptr: *mut u8,
    pub len: usize,
    pub capacity: usize,
}

impl Buffer {
    pub fn empty() -> Buffer {
        Buffer { ptr: std::ptr::null_mut(), len: 0, capacity: 0 }
    }

    /// Hands `bytes` over: only [`Buffer::into_vec`], in this library, frees them.
    pub fn from_vec(bytes: Vec<u8>) -> Buffer {
        let mut bytes = std::mem::ManuallyDrop::new(bytes);
        Buffer { ptr: bytes.as_mut_ptr(), len: bytes.len(), capacity: bytes.capacity() }
    }

    /// The bytes, back to the vector they came from.
    ///
    /// # Safety
    /// `self` was made by [`Buffer::from_vec`] in this same library, and is
    /// not used again.
    pub unsafe fn into_vec(self) -> Vec<u8> {
        if self.ptr.is_null() {
            return Vec::new();
        }
        // SAFETY: the parts of a vector of this library, as the caller guarantees
        unsafe { Vec::from_raw_parts(self.ptr, self.len, self.capacity) }
    }

    /// The bytes, read in place (by the caller, before `grenat_ext_free`).
    ///
    /// # Safety
    /// `self` is a buffer the library filled and has not freed yet.
    pub unsafe fn as_slice(&self) -> &[u8] {
        if self.ptr.is_null() {
            return &[];
        }
        // SAFETY: `len` initialized bytes at `ptr`, as the caller guarantees
        unsafe { std::slice::from_raw_parts(self.ptr, self.len) }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_buffer_goes_and_comes_back() {
        let buffer = Buffer::from_vec(b"[1,2]".to_vec());
        // SAFETY: made just above, used once
        assert_eq!(unsafe { buffer.as_slice() }, b"[1,2]");
        assert_eq!(unsafe { buffer.into_vec() }, b"[1,2]");
        let empty = Buffer::empty();
        assert!(unsafe { empty.as_slice() }.is_empty());
        assert!(unsafe { empty.into_vec() }.is_empty());
    }

    #[test]
    fn the_entry_prefix_carries_the_version() {
        assert_eq!(ENTRY_PREFIX, format!("grenat_ext_v{ABI_VERSION}_"));
    }
}
