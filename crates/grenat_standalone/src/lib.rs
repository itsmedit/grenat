//! Entry point of the executables built by `grenat build --native`.
//!
//! This static library is linked with a standalone program's object file
//! (see `grenat_codegen::standalone`) and provides `main`: it runs the
//! program's compiled `main`, then turns its status into an exit code or an
//! error message, like `grenat run` would. Nothing else: no parser, no
//! interpreter.

use std::ffi::{CStr, c_char, c_int};
use std::io::{IsTerminal, Write};

use grenat_ast::{Diagnostic, Span};
use grenat_runtime::abi::{Context, SiteRecord, Standalone};
use grenat_runtime::{Arr, Poll, Str, status};

unsafe extern "C" {
    /// Emitted into the program's object file by `grenat_codegen::standalone`.
    static grenat_program: Standalone;
}

/// Stack of the thread running the program, as the interpreter's.
const STACK: usize = 512 * 1024 * 1024;
/// Recursion limit, as the interpreter's (`StackOverflow` beyond).
const MAX_DEPTH: i64 = 20_000;

/// # Safety
/// Called by the C runtime with the process's arguments.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn main(argc: c_int, argv: *const *const c_char) -> c_int {
    // SAFETY: `argc` valid C strings, as the C runtime passes them
    let arg = |i: c_int| unsafe { CStr::from_ptr(*argv.add(i as usize)) }.to_string_lossy().into_owned();
    let args: Vec<String> = (1..argc).map(arg).collect();
    // SAFETY: the program linked into this executable, never moved
    let program: &'static Standalone = unsafe { &*std::ptr::addr_of!(grenat_program) };
    let code = std::thread::Builder::new()
        .stack_size(STACK)
        .spawn(move || run(program, args))
        .expect("starting the program")
        .join()
        .unwrap_or(101);
    // the C runtime, not Rust's, ends the process: flush what Rust buffered
    let _ = std::io::stdout().flush();
    code
}

fn run(program: &Standalone, args: Vec<String>) -> c_int {
    let mut slots = Vec::new();
    if program.takes_args != 0 {
        slots.push(Arr::new(args.iter().map(|a| Str::new(a) as u64).collect()) as u64);
    }
    // nothing cancels a standalone program
    let never = || false;
    let mut ctx = Context { status: 0, limit: MAX_DEPTH, site: 0, exit_code: 0, poll: Poll::new(&never) };
    (program.main)(slots.as_ptr(), &mut ctx);
    let _ = std::io::stdout().flush();
    match ctx.status {
        0 => 0,
        status::EXIT => ctx.exit_code.clamp(0, 255) as c_int,
        status => {
            report(program, &ctx, status);
            1
        }
    }
}

/// The error, rendered as `grenat run` renders a runtime error.
fn report(program: &Standalone, ctx: &Context, status: i64) {
    // SAFETY: data of the executable, emitted by `grenat_codegen::standalone`
    let (site, source, path) = unsafe {
        let sites = std::slice::from_raw_parts(program.sites, program.site_count as usize);
        (&sites[ctx.site as usize], program.source.text(), program.path.text())
    };
    let SiteRecord { function, function_start, function_end, start, end, reason } = site;
    // SAFETY: as above
    let (function, reason) = unsafe { (function.text(), reason.text()) };
    let message = match status::error(status) {
        Some((ty, message)) => format!("{ty}: {message}"),
        None => format!("NativeError: {reason} cannot be represented in a native program"),
    };
    let span = |start: u64, end: u64| Span { start: start as u32, end: end as u32 };
    let diagnostic = Diagnostic::new(span(*start, *end), message)
        .with_note(span(*function_start, *function_end), format!("in `{function}`"));
    let color = std::io::stderr().is_terminal() && std::env::var_os("NO_COLOR").is_none();
    eprint!("{}", grenat_report::render(path, source, &diagnostic, color));
}
