//! A library built for another version of Grenat's native ABI.

#[unsafe(no_mangle)]
pub extern "C" fn grenat_ext_abi_version() -> u32 {
    0
}
