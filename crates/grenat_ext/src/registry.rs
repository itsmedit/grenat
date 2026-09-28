//! Every exported function registers itself (`inventory`, at load time);
//! the library's manifest lists them, with the structs they use. Here too
//! are the three C functions every library exports besides its entry
//! points: its ABI version, its manifest, and the free function.

use crate::abi::{ABI_VERSION, Buffer};
use crate::manifest::{Function, Manifest, Struct};

/// One exported function: its description, and the structs it mentions.
pub struct Export {
    pub describe: fn() -> Function,
    pub structs: fn(&mut Vec<Struct>),
}

inventory::collect!(Export);

/// The manifest of the functions exported by this library (by name).
pub fn manifest() -> Manifest {
    let exports: Vec<&Export> = inventory::iter::<Export>.into_iter().collect();
    let mut functions: Vec<Function> = exports.iter().map(|e| (e.describe)()).collect();
    functions.sort_by(|a, b| a.name.cmp(&b.name));
    let mut structs = Vec::new();
    for export in &exports {
        (export.structs)(&mut structs);
    }
    structs.sort_by(|a, b| a.name.cmp(&b.name));
    Manifest { abi: ABI_VERSION, functions, structs }
}

/// The ABI this library was built for.
#[unsafe(no_mangle)]
pub extern "C" fn grenat_ext_abi_version() -> u32 {
    ABI_VERSION
}

/// Writes the manifest's JSON into `out`.
///
/// # Safety
/// `out` is writable.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn grenat_ext_manifest(out: *mut Buffer) -> i32 {
    // SAFETY: no arguments; `out` is writable, as the caller guarantees
    unsafe { crate::runtime::entry(std::ptr::null(), 0, out, |_| crate::runtime::returned(manifest())) }
}

/// Frees a buffer this library handed out.
///
/// # Safety
/// `buffer` was written by this library (an entry point, the manifest), once.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn grenat_ext_free(buffer: Buffer) {
    // SAFETY: made by `Buffer::from_vec` in this library, as the caller guarantees
    drop(unsafe { buffer.into_vec() });
}
