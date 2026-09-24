//! Linking a program's object file with the host library into an executable,
//! with the system's C compiler driver (`cc`, or `$CC`).

use std::path::Path;
use std::process::Command;

/// System libraries the host library (Rust's standard library) needs.
#[cfg(target_os = "macos")]
const SYSTEM_LIBS: &[&str] = &["-liconv", "-lSystem", "-lc", "-lm", "-Wl,-dead_strip"];
/// Unused sections are dropped; on Linux, so is the debug information of the
/// standard library, which the linker would otherwise copy (macOS leaves it
/// in the object files).
#[cfg(target_os = "linux")]
const SYSTEM_LIBS: &[&str] =
    &["-lgcc_s", "-lutil", "-lrt", "-lpthread", "-lm", "-ldl", "-lc", "-Wl,--gc-sections", "-Wl,--strip-debug"];
#[cfg(not(any(target_os = "macos", target_os = "linux")))]
const SYSTEM_LIBS: &[&str] = &[];

pub fn link(object: &Path, host: &Path, output: &Path) -> Result<(), String> {
    let cc = std::env::var("CC").unwrap_or_else(|_| "cc".into());
    let result = Command::new(&cc)
        .arg(object)
        .arg(host)
        .args(SYSTEM_LIBS)
        .arg("-o")
        .arg(output)
        .output()
        .map_err(|e| format!("cannot run the linker `{cc}` ({e}): install a C toolchain, or set $CC"))?;
    if result.status.success() {
        Ok(())
    } else {
        Err(format!("the linker `{cc}` failed:\n{}", String::from_utf8_lossy(&result.stderr)))
    }
}
