//! Releasing references: freeing objects whose count reaches zero, and
//! Perceus' drop-reuse, which hands a dying record's memory to a new one.

use crate::array::Arr;
use crate::record::Record;
use crate::shape::{ARRAY, RECORD, STR, Shape, Slot};
use crate::string::Str;

/// Adds a reference to the object at `bits`.
///
/// # Safety
/// `bits` must point to a live object.
pub(crate) unsafe fn dup(bits: u64) {
    // SAFETY: every object starts with its count
    unsafe { *(bits as *mut i64) += 1 }
}

/// Adds a reference to `obj`, which the caller will release.
///
/// # Safety
/// `obj` must point to a live object.
pub unsafe fn retain(obj: *mut u8) {
    // SAFETY: by contract
    unsafe { dup(obj as u64) }
}

/// Drops one reference to `obj`, freeing it (and releasing its children) at zero.
///
/// # Safety
/// `obj` must be a live object of shape `shape`, and the caller must own the reference.
pub unsafe fn release(obj: *mut u8, shape: *const Shape) {
    // SAFETY: by contract
    unsafe {
        let rc = obj as *mut i64;
        if *rc == 1 {
            grenat_free(obj, shape);
        } else {
            *rc -= 1;
        }
    }
}

/// # Safety
/// As [`release`], for the content of a slot.
pub(crate) unsafe fn release_slot(bits: u64, slot: Slot) {
    if !slot.is_null() {
        // SAFETY: a heap slot holds an owned reference to an object of that shape
        unsafe { release(bits as *mut u8, slot) }
    }
}

/// Releases the children of record `obj`, then returns its field count.
unsafe fn release_fields(obj: *mut u8, fields: &[Slot]) -> usize {
    // SAFETY: `obj` is a record with these fields; releasing a child never touches `obj`
    unsafe {
        let values = Record::fields(obj as *const u64, fields.len());
        for (bits, slot) in values.iter().zip(fields) {
            release_slot(*bits, *slot);
        }
    }
    fields.len()
}

/// Frees `obj`, whose last reference was just dropped by compiled code.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn grenat_free(obj: *mut u8, shape: *const Shape) {
    // SAFETY: the compiler passes the shape of the object's static type
    unsafe {
        let shape = &*shape;
        match shape.kind() {
            STR => Str::free(obj as *mut Str),
            ARRAY => Arr::free(obj as *mut Arr, shape.slots()[0]),
            _ => {
                let count = release_fields(obj, shape.slots());
                Record::free_memory(obj as *mut u64, count);
            }
        }
    }
}

/// Drops a reference to record `obj`. If it was the last one, its fields are
/// released and its memory is returned for reuse (still counted as live);
/// otherwise returns null.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn grenat_drop_reuse(obj: *mut u8, shape: *const Shape) -> *mut u8 {
    // SAFETY: a live record of that shape, reference owned by the caller
    unsafe {
        let rc = obj as *mut i64;
        if *rc != 1 {
            *rc -= 1;
            return std::ptr::null_mut();
        }
        debug_assert_eq!((*shape).kind(), RECORD, "only records are reused");
        release_fields(obj, (*shape).slots());
        obj
    }
}

/// Frees memory obtained by [`grenat_drop_reuse`] and never reused (an error
/// interrupted the construction). Null is ignored.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn grenat_free_token(token: *mut u8, shape: *const Shape) {
    if token.is_null() {
        return;
    }
    // SAFETY: a record of that shape whose fields are already released
    unsafe {
        debug_assert_eq!((*shape).kind(), RECORD, "only records are reused");
        Record::free_memory(token as *mut u64, (*shape).slots().len());
    }
}
