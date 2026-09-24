//! Printing values, for standalone programs: `puts`, `print` and `p` give
//! exactly what the interpreter gives.
//!
//! A value comes as its bits and a descriptor of its type: `I`, `F`, `B`,
//! `S`, `A` followed by the element's, `R` followed by `Name(field:desc,…)`.

use std::io::Write;

use crate::array::Arr;
use crate::format;
use crate::layout;
use crate::string::Str;

pub const PUTS: i64 = 0;
pub const PRINT: i64 = 1;
pub const INSPECT: i64 = 2;
pub const NEWLINE: i64 = 3;

/// Parsed descriptor.
enum Desc<'a> {
    Int,
    Float,
    Bool,
    Str,
    Array(Box<Desc<'a>>),
    Record(&'a str, Vec<(&'a str, Desc<'a>)>),
}

/// Parses the descriptor at the start of `text`; returns the rest.
fn parse(text: &str) -> (Desc<'_>, &str) {
    let (head, rest) = text.split_at(1);
    match head {
        "I" => (Desc::Int, rest),
        "F" => (Desc::Float, rest),
        "B" => (Desc::Bool, rest),
        "S" => (Desc::Str, rest),
        "A" => {
            let (elem, rest) = parse(rest);
            (Desc::Array(Box::new(elem)), rest)
        }
        "R" => {
            let open = rest.find('(').expect("a record descriptor");
            let name = &rest[..open];
            let mut rest = &rest[open + 1..];
            let mut fields = Vec::new();
            while !rest.starts_with(')') {
                let colon = rest.find(':').expect("a field");
                let field = &rest[..colon];
                let (desc, after) = parse(&rest[colon + 1..]);
                fields.push((field, desc));
                rest = after.strip_prefix(',').unwrap_or(after);
            }
            (Desc::Record(name, fields), &rest[1..])
        }
        other => unreachable!("unknown descriptor `{other}`"),
    }
}

/// `p`: the value as Grenat source (strings quoted).
unsafe fn inspect(out: &mut String, bits: u64, desc: &Desc) {
    // SAFETY (objects): `bits` is a live value of the described type
    match desc {
        Desc::Int => out.push_str(&(bits as i64).to_string()),
        Desc::Float => out.push_str(&format::float(f64::from_bits(bits))),
        Desc::Bool => out.push_str(if bits & 0xff != 0 { "true" } else { "false" }),
        Desc::Str => {
            out.push('"');
            out.extend(unsafe { Str::text(bits as *const Str) }.escape_debug());
            out.push('"');
        }
        Desc::Array(elem) => {
            out.push('[');
            for (i, item) in unsafe { Arr::items(bits as *const Arr) }.iter().enumerate() {
                if i > 0 {
                    out.push_str(", ");
                }
                unsafe { inspect(out, *item, elem) };
            }
            out.push(']');
        }
        Desc::Record(name, fields) => {
            out.push_str(name);
            out.push('(');
            for (i, (field, desc)) in fields.iter().enumerate() {
                if i > 0 {
                    out.push_str(", ");
                }
                out.push_str(field);
                out.push_str(": ");
                // SAFETY: field `i` of a live record
                let value = unsafe { *((bits as *const u8).add(layout::field(i) as usize) as *const u64) };
                unsafe { inspect(out, value, desc) };
            }
            out.push(')');
        }
    }
}

/// `print`: a string as it is, anything else inspected.
unsafe fn display(out: &mut String, bits: u64, desc: &Desc) {
    match desc {
        // SAFETY: a live string
        Desc::Str => out.push_str(unsafe { Str::text(bits as *const Str) }),
        other => unsafe { inspect(out, bits, other) },
    }
}

/// `puts`: displayed then a newline; an array, element by element.
unsafe fn puts(out: &mut String, bits: u64, desc: &Desc) {
    match desc {
        Desc::Array(elem) => {
            // SAFETY: a live array
            for item in unsafe { Arr::items(bits as *const Arr) } {
                unsafe { puts(out, *item, elem) };
            }
        }
        other => {
            unsafe { display(out, bits, other) };
            out.push('\n');
        }
    }
}

/// Prints `bits`, described by the `len` bytes at `desc`, to standard output.
///
/// # Safety
/// `desc` must be a descriptor, and `bits` a live value of that type.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn grenat_write(bits: u64, desc: *const u8, len: i64, mode: i64) {
    // SAFETY: a descriptor literal of the compiled code
    let desc = unsafe { std::str::from_utf8_unchecked(std::slice::from_raw_parts(desc, len as usize)) };
    // SAFETY: by contract
    let out = unsafe { text(bits, desc, mode) };
    let _ = std::io::stdout().lock().write_all(out.as_bytes());
}

/// What `grenat_write` prints.
///
/// # Safety
/// As [`grenat_write`].
pub unsafe fn text(bits: u64, desc: &str, mode: i64) -> String {
    let mut out = String::new();
    if mode == NEWLINE {
        out.push('\n');
        return out;
    }
    let (desc, _) = parse(desc);
    // SAFETY: `bits` is a live value of that type
    unsafe {
        match mode {
            PUTS => puts(&mut out, bits, &desc),
            PRINT => display(&mut out, bits, &desc),
            _ => {
                inspect(&mut out, bits, &desc);
                out.push('\n');
            }
        }
    }
    out
}
