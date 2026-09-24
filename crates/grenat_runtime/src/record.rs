//! Struct records: a count followed by one slot per field, immutable once built.

use std::alloc::{Layout, alloc_zeroed, dealloc};

use crate::layout;
use crate::live;

pub struct Record;

impl Record {
    fn layout(fields: usize) -> Layout {
        Layout::array::<u64>(fields + 1).expect("record size")
    }

    /// A new record with a count of 1 and zeroed fields, which the caller fills.
    pub fn alloc(fields: usize) -> *mut u64 {
        // SAFETY: the layout has a non-zero size (the count)
        let r = unsafe { alloc_zeroed(Record::layout(fields)) } as *mut u64;
        assert!(!r.is_null(), "out of memory");
        // SAFETY: freshly allocated, at least one word
        unsafe { r.write(1) };
        live::allocated();
        r
    }

    /// The fields of `r`.
    ///
    /// # Safety
    /// `r` must point to a live record of `count` fields.
    pub unsafe fn fields<'a>(r: *const u64, count: usize) -> &'a [u64] {
        // SAFETY: the fields follow the count
        unsafe { std::slice::from_raw_parts(r.byte_add(layout::field(0) as usize), count) }
    }

    /// Stores field `index` of a record being built.
    ///
    /// # Safety
    /// `r` must be a record of more than `index` fields, not yet shared.
    pub unsafe fn set(r: *mut u64, index: usize, bits: u64) {
        // SAFETY: in bounds by contract
        unsafe { r.byte_add(layout::field(index) as usize).write(bits) }
    }

    /// Frees the memory of `r`, whose fields are already released.
    ///
    /// # Safety
    /// `r` must have been allocated by [`Record::alloc`] with `fields` fields.
    pub(crate) unsafe fn free_memory(r: *mut u64, fields: usize) {
        // SAFETY: same layout as the allocation
        unsafe { dealloc(r as *mut u8, Record::layout(fields)) };
        live::freed();
    }
}

pub(crate) extern "C" fn grenat_record_alloc(fields: i64) -> *mut u64 {
    Record::alloc(fields as usize)
}
