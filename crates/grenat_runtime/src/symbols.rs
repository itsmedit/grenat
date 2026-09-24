//! The functions compiled code calls, by symbol name.

use crate::array::*;
use crate::record::*;
use crate::release::*;
use crate::string::*;

/// Every runtime function with its address, for the JIT's symbol lookup.
pub fn symbols() -> Vec<(&'static str, *const u8)> {
    macro_rules! table {
        ($($f:ident),* $(,)?) => { vec![$((stringify!($f), $f as *const u8)),*] };
    }
    table![
        grenat_free,
        grenat_drop_reuse,
        grenat_free_token,
        grenat_record_alloc,
        grenat_array_new,
        grenat_array_push,
        grenat_array_concat,
        grenat_array_copy,
        grenat_str_from,
        grenat_str_concat,
        grenat_str_add_owned,
        grenat_str_push_bytes,
        grenat_str_push_str,
        grenat_str_push_int,
        grenat_str_push_float,
        grenat_str_push_bool,
        grenat_str_eq,
        grenat_str_cmp,
        grenat_str_length,
        grenat_str_char_at,
        grenat_str_upcase,
        grenat_str_downcase,
        grenat_str_reverse,
        grenat_str_strip,
        grenat_str_repeat,
        grenat_str_includes,
        grenat_str_starts_with,
        grenat_str_ends_with,
        grenat_str_to_i,
        grenat_str_to_f,
    ]
}
