//! Host of the executables built by `grenat build`.
//!
//! This static library is linked with a program's object file (see
//! `grenat_codegen::aot`) and provides `main`: it reads the program embedded
//! in the object, then runs it exactly as `grenat run` would, calling the
//! linked native code instead of compiling anything.

use std::ffi::{CStr, c_char, c_int};
use std::io::Write;

use grenat_codegen::aot::Image;

unsafe extern "C" {
    /// Emitted into the program's object file by `grenat_codegen::aot::object`.
    static grenat_image: Image;
}

/// # Safety
/// Called by the C runtime with the process's arguments.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn main(argc: c_int, argv: *const *const c_char) -> c_int {
    // SAFETY: `argc` valid C strings, as the C runtime passes them
    let arg = |i: c_int| unsafe { CStr::from_ptr(*argv.add(i as usize)) }.to_string_lossy().into_owned();
    let args: Vec<String> = (1..argc).map(arg).collect();
    // SAFETY: the image linked into this executable, never moved
    let image: &'static Image = unsafe { &*std::ptr::addr_of!(grenat_image) };
    // SAFETY: as above
    let sources = unsafe { grenat_driver::Sources::from_table(image.source(), image.files()) };

    // checked when it was built
    let status = match grenat_driver::parse(&sources, true) {
        Some(program) => {
            let options = grenat_interp::Options {
                log: grenat_driver::log_from_env(),
                jit: grenat_driver::native_from_env(),
                linked: Some(image),
                ..Default::default()
            };
            grenat_driver::execute(&sources, &program, args, options)
        }
        None => 1,
    };
    // the C runtime, not Rust's, ends the process: flush what Rust buffered
    let _ = std::io::stdout().flush();
    c_int::from(status)
}
