//! The [`Shape`] of every heap type of a program, as the runtime needs them
//! to release objects.
//!
//! Compiled code does not embed their addresses (an executable is linked
//! before they exist): it reads them from a table of pointers, filled when
//! the code is loaded, in the order of [`index`].

use std::collections::HashMap;

use grenat_runtime::{Shape, Slot};

use crate::structs::Structs;
use crate::ty::{Elem, StructId, Ty};

/// Shapes are allocated once with `Box::into_raw` and never moved nor
/// mutated afterwards, so the pointers handed out stay valid until `drop`.
pub(crate) struct Shapes {
    string: *mut Shape,
    records: Vec<*mut Shape>,
    arrays: HashMap<Elem, *mut Shape>,
}

impl Shapes {
    pub fn build(structs: &Structs) -> Shapes {
        let string = Box::into_raw(Box::new(Shape::Str));
        let mut records = vec![std::ptr::null_mut(); structs.count()];
        for i in 0..structs.count() {
            record(StructId(i), structs, string, &mut records);
        }
        let arrays =
            array_elems(structs.count()).map(|e| (e, Box::into_raw(Box::new(Shape::Array(slot(e, string, &records)))))).collect();
        Shapes { string, records, arrays }
    }

    /// Every shape, in table order (see [`index`]).
    pub fn table(&self) -> Vec<*const Shape> {
        let arrays = array_elems(self.records.len()).map(|e| self.arrays[&e] as *const Shape);
        std::iter::once(self.string as *const Shape)
            .chain(self.records.iter().map(|r| *r as *const Shape))
            .chain(arrays)
            .collect()
    }

    /// Shape of objects of type `ty` (a heap type).
    pub fn of(&self, ty: Ty) -> *const Shape {
        match ty {
            Ty::Str => self.string,
            Ty::Struct(id) => self.records[id.0],
            Ty::Array(elem) => self.arrays[&elem],
            other => unreachable!("`{other:?}` is not an object"),
        }
    }
}

/// Element types of arrays, in table order.
fn array_elems(structs: usize) -> impl Iterator<Item = Elem> {
    [Elem::Int, Elem::Float, Elem::Bool, Elem::Str, Elem::Unknown]
        .into_iter()
        .chain((0..structs).map(|i| Elem::Struct(StructId(i))))
}

/// Number of shapes of a program with `structs` structs.
pub(crate) fn count(structs: usize) -> usize {
    1 + structs + array_elems(structs).count()
}

/// Position of the shape of `ty` in the table.
pub(crate) fn index(ty: Ty, structs: usize) -> usize {
    match ty {
        Ty::Str => 0,
        Ty::Struct(id) => 1 + id.0,
        Ty::Array(elem) => 1 + structs + array_elems(structs).position(|e| e == elem).expect("an element type"),
        other => unreachable!("`{other:?}` is not an object"),
    }
}

/// Builds the shape of a struct after those of the structs it contains
/// (structs are not recursive, see [`Structs`]).
fn record(id: StructId, structs: &Structs, string: *mut Shape, records: &mut Vec<*mut Shape>) -> *mut Shape {
    if !records[id.0].is_null() {
        return records[id.0];
    }
    let fields: Vec<Elem> = structs.get(id).fields.iter().map(|(_, t)| t.elem().expect("no array field")).collect();
    for field in &fields {
        if let Elem::Struct(inner) = field {
            record(*inner, structs, string, records);
        }
    }
    let slots = fields.into_iter().map(|e| slot(e, string, records)).collect();
    records[id.0] = Box::into_raw(Box::new(Shape::Record(slots)));
    records[id.0]
}

fn slot(elem: Elem, string: *mut Shape, records: &[*mut Shape]) -> Slot {
    match elem {
        Elem::Str => Slot::Heap(string),
        Elem::Struct(id) => Slot::Heap(records[id.0]),
        _ => Slot::Scalar,
    }
}

impl Drop for Shapes {
    fn drop(&mut self) {
        let all = std::iter::once(self.string).chain(self.records.iter().copied()).chain(self.arrays.values().copied());
        for shape in all {
            // SAFETY: allocated by `Box::into_raw` in `build`, freed exactly once
            drop(unsafe { Box::from_raw(shape) });
        }
    }
}
