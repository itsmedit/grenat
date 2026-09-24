//! Immutable UTF-8 strings.
//!
//! A string is never modified once another reference to it exists: the only
//! in-place update, [`grenat_str_add_owned`], requires a count of 1 (the
//! reuse of a uniquely owned value, as in Perceus' "functional but in place").

use std::mem::ManuallyDrop;

use crate::format;
use crate::live;

#[repr(C)]
pub struct Str {
    rc: i64,
    len: i64,
    cap: i64,
    ptr: *mut u8,
}

impl Str {
    /// A new string, with a count of 1.
    pub fn new(text: &str) -> *mut Str {
        Str::from_vec(text.as_bytes().to_vec())
    }

    fn from_vec(bytes: Vec<u8>) -> *mut Str {
        let mut bytes = ManuallyDrop::new(bytes);
        let s = Str { rc: 1, len: bytes.len() as i64, cap: bytes.capacity() as i64, ptr: bytes.as_mut_ptr() };
        live::allocated();
        Box::into_raw(Box::new(s))
    }

    /// The text of `s`.
    ///
    /// # Safety
    /// `s` must point to a live string.
    pub unsafe fn text<'a>(s: *const Str) -> &'a str {
        // SAFETY: the buffer holds `len` initialized bytes of valid UTF-8 (every
        // constructor copies a `&str`, every operation preserves validity)
        unsafe { std::str::from_utf8_unchecked(std::slice::from_raw_parts((*s).ptr, (*s).len as usize)) }
    }

    /// Appends `text` in place.
    ///
    /// # Safety
    /// `s` must be live, and uniquely referenced by the caller.
    unsafe fn push(s: *mut Str, text: &str) {
        // SAFETY: (ptr, len, cap) come from a `Vec<u8>` given up by `from_vec` or a previous `push`
        unsafe {
            let s = &mut *s;
            let mut bytes = Vec::from_raw_parts(s.ptr, s.len as usize, s.cap as usize);
            bytes.extend_from_slice(text.as_bytes());
            let mut bytes = ManuallyDrop::new(bytes);
            (s.ptr, s.len, s.cap) = (bytes.as_mut_ptr(), bytes.len() as i64, bytes.capacity() as i64);
        }
    }

    /// # Safety
    /// `s` must be live, with no reference left.
    pub(crate) unsafe fn free(s: *mut Str) {
        // SAFETY: `s` was allocated by `from_vec`, its buffer by a `Vec<u8>`
        unsafe {
            let s = Box::from_raw(s);
            drop(Vec::from_raw_parts(s.ptr, s.len as usize, s.cap as usize));
        }
        live::freed();
    }
}

fn boolean(b: bool) -> i64 {
    i64::from(b)
}

// ── Functions called by compiled code ─────────────────────────
//
// Pointer arguments are live strings, borrowed unless stated otherwise.
// A null result means "not representable natively": the caller deoptimizes.

#[unsafe(no_mangle)]
pub unsafe extern "C" fn grenat_str_from(bytes: *const u8, len: i64) -> *mut Str {
    // SAFETY: literal bytes owned by the compiled code, valid UTF-8
    Str::new(unsafe { std::str::from_utf8_unchecked(std::slice::from_raw_parts(bytes, len as usize)) })
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn grenat_str_concat(a: *const Str, b: *const Str) -> *mut Str {
    // SAFETY: live strings
    let (a, b) = unsafe { (Str::text(a), Str::text(b)) };
    let mut out = String::with_capacity(a.len() + b.len());
    out.push_str(a);
    out.push_str(b);
    Str::from_vec(out.into_bytes())
}

/// `a + b` where the caller owns `a` and gives it up: appends in place when
/// no one else holds `a`, otherwise copies.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn grenat_str_add_owned(a: *mut Str, b: *const Str) -> *mut Str {
    // SAFETY: live strings; with a count of 1, `a` is not `b` (which is referenced elsewhere)
    unsafe {
        if (*a).rc == 1 {
            Str::push(a, Str::text(b));
            a
        } else {
            // other references remain: the count cannot reach zero here
            (*a).rc -= 1;
            grenat_str_concat(a, b)
        }
    }
}

/// Appends literal bytes to a string being built (an interpolation): `s` is unique.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn grenat_str_push_bytes(s: *mut Str, bytes: *const u8, len: i64) {
    // SAFETY: `s` unique; literal bytes owned by the compiled code, valid UTF-8
    unsafe { Str::push(s, std::str::from_utf8_unchecked(std::slice::from_raw_parts(bytes, len as usize))) }
}

/// Appends to a string being built (an interpolation): `s` is unique.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn grenat_str_push_str(s: *mut Str, t: *const Str) {
    // SAFETY: `s` unique, `t` live
    unsafe { Str::push(s, Str::text(t)) }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn grenat_str_push_int(s: *mut Str, n: i64) {
    // SAFETY: `s` unique
    unsafe { Str::push(s, &n.to_string()) }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn grenat_str_push_float(s: *mut Str, f: f64) {
    // SAFETY: `s` unique
    unsafe { Str::push(s, &format::float(f)) }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn grenat_str_push_bool(s: *mut Str, b: i64) {
    // SAFETY: `s` unique
    unsafe { Str::push(s, if b != 0 { "true" } else { "false" }) }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn grenat_str_eq(a: *const Str, b: *const Str) -> i64 {
    // SAFETY: live strings
    boolean(unsafe { Str::text(a) == Str::text(b) })
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn grenat_str_cmp(a: *const Str, b: *const Str) -> i64 {
    // SAFETY: live strings
    unsafe { Str::text(a).cmp(Str::text(b)) as i64 }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn grenat_str_length(s: *const Str) -> i64 {
    // SAFETY: live string
    unsafe { Str::text(s).chars().count() as i64 }
}

/// `s[i]`: the character at `i` (negative: from the end), null when out of range.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn grenat_str_char_at(s: *const Str, i: i64) -> *mut Str {
    // SAFETY: live string
    let text = unsafe { Str::text(s) };
    let count = text.chars().count() as i64;
    let i = if i < 0 { i + count } else { i };
    if !(0..count).contains(&i) {
        return std::ptr::null_mut();
    }
    let c = text.chars().nth(i as usize).expect("index checked");
    Str::new(c.encode_utf8(&mut [0; 4]))
}

/// A method returning a new string.
unsafe fn map(s: *const Str, f: impl FnOnce(&str) -> String) -> *mut Str {
    // SAFETY: live string
    Str::from_vec(f(unsafe { Str::text(s) }).into_bytes())
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn grenat_str_upcase(s: *const Str) -> *mut Str {
    unsafe { map(s, str::to_uppercase) }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn grenat_str_downcase(s: *const Str) -> *mut Str {
    unsafe { map(s, str::to_lowercase) }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn grenat_str_reverse(s: *const Str) -> *mut Str {
    unsafe { map(s, |t| t.chars().rev().collect()) }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn grenat_str_strip(s: *const Str) -> *mut Str {
    unsafe { map(s, |t| t.trim().to_string()) }
}

/// `s * n`; null for a negative `n` (a `TypeError` in the interpreter).
#[unsafe(no_mangle)]
pub unsafe extern "C" fn grenat_str_repeat(s: *const Str, n: i64) -> *mut Str {
    if n < 0 {
        return std::ptr::null_mut();
    }
    unsafe { map(s, |t| t.repeat(n as usize)) }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn grenat_str_includes(s: *const Str, t: *const Str) -> i64 {
    // SAFETY: live strings
    boolean(unsafe { Str::text(s).contains(Str::text(t)) })
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn grenat_str_starts_with(s: *const Str, t: *const Str) -> i64 {
    // SAFETY: live strings
    boolean(unsafe { Str::text(s).starts_with(Str::text(t)) })
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn grenat_str_ends_with(s: *const Str, t: *const Str) -> i64 {
    // SAFETY: live strings
    boolean(unsafe { Str::text(s).ends_with(Str::text(t)) })
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn grenat_str_to_i(s: *const Str) -> i64 {
    // SAFETY: live string
    unsafe { Str::text(s).trim().parse().unwrap_or(0) }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn grenat_str_to_f(s: *const Str) -> f64 {
    // SAFETY: live string
    unsafe { Str::text(s).trim().parse().unwrap_or(0.0) }
}
