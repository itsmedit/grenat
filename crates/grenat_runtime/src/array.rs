//! Mutable arrays of value slots.
//!
//! An array is a reference: every holder sees its mutations, exactly like the
//! interpreter's arrays. Compiled code reads and writes elements inline (see
//! [`layout`](crate::layout)); growing goes through these functions.

use std::mem::ManuallyDrop;

use crate::live;
use crate::release::{dup, release_slot};
use crate::shape::Slot;

#[repr(C)]
pub struct Arr {
    rc: i64,
    len: i64,
    cap: i64,
    ptr: *mut u64,
}

impl Arr {
    /// A new array owning `items` (their references are transferred), count 1.
    pub fn new(items: Vec<u64>) -> *mut Arr {
        let mut items = ManuallyDrop::new(items);
        let a = Arr { rc: 1, len: items.len() as i64, cap: items.capacity() as i64, ptr: items.as_mut_ptr() };
        live::allocated();
        Box::into_raw(Box::new(a))
    }

    /// The elements of `a`.
    ///
    /// # Safety
    /// `a` must point to a live array.
    pub unsafe fn items<'a>(a: *const Arr) -> &'a [u64] {
        // SAFETY: the buffer holds `len` initialized slots
        unsafe { std::slice::from_raw_parts((*a).ptr, (*a).len as usize) }
    }

    /// # Safety
    /// `a` must be live; `f` must leave every slot initialized.
    unsafe fn update(a: *mut Arr, f: impl FnOnce(&mut Vec<u64>)) {
        // SAFETY: (ptr, len, cap) come from a `Vec<u64>` given up by `new` or a previous `update`
        unsafe {
            let a = &mut *a;
            let mut items = Vec::from_raw_parts(a.ptr, a.len as usize, a.cap as usize);
            f(&mut items);
            let mut items = ManuallyDrop::new(items);
            (a.ptr, a.len, a.cap) = (items.as_mut_ptr(), items.len() as i64, items.capacity() as i64);
        }
    }

    /// Releases the elements, then frees the array.
    ///
    /// # Safety
    /// `a` must be live, with no reference left; `elem` must describe its elements.
    pub(crate) unsafe fn free(a: *mut Arr, elem: Slot) {
        // SAFETY: allocated by `new`; the elements are released exactly once
        unsafe {
            let a = Box::from_raw(a);
            let items = Vec::from_raw_parts(a.ptr, a.len as usize, a.cap as usize);
            for bits in items {
                release_slot(bits, elem);
            }
        }
        live::freed();
    }
}

// ── Functions called by compiled code ─────────────────────────

pub(crate) extern "C" fn grenat_array_new(capacity: i64) -> *mut Arr {
    Arr::new(Vec::with_capacity(capacity.max(0) as usize))
}

/// Appends `bits`, whose reference (if any) is transferred to the array.
pub(crate) unsafe extern "C" fn grenat_array_push(a: *mut Arr, bits: u64) {
    // SAFETY: live array
    unsafe { Arr::update(a, |items| items.push(bits)) }
}

/// A new array with the elements of `a` then `b`; `heap`: the elements are objects.
pub(crate) unsafe extern "C" fn grenat_array_concat(a: *const Arr, b: *const Arr, heap: i64) -> *mut Arr {
    // SAFETY: live arrays; each copied object gains a reference
    unsafe {
        let items: Vec<u64> = Arr::items(a).iter().chain(Arr::items(b)).copied().collect();
        if heap != 0 {
            items.iter().for_each(|&bits| dup(bits));
        }
        Arr::new(items)
    }
}

/// A shallow copy of `a` (`dup`, or the snapshot iterated by `each`).
pub(crate) unsafe extern "C" fn grenat_array_copy(a: *const Arr, heap: i64) -> *mut Arr {
    // SAFETY: live array; each copied object gains a reference
    unsafe {
        let items = Arr::items(a).to_vec();
        if heap != 0 {
            items.iter().for_each(|&bits| dup(bits));
        }
        Arr::new(items)
    }
}
