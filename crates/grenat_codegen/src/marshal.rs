//! Conversions between [`Data`] and native values.

use grenat_runtime::{Arr, Record, Str};

use crate::data::Data;
use crate::shapes::Shapes;
use crate::structs::Structs;
use crate::ty::{Elem, Ty};

pub(crate) struct Marshal<'a> {
    pub structs: &'a Structs,
    pub shapes: &'a Shapes,
}

impl Marshal<'_> {
    /// `data` has exactly type `ty` (no conversion: the interpreter would
    /// compute with the value's own type).
    pub fn fits(&self, data: &Data, ty: Ty) -> bool {
        match (data, ty) {
            (Data::Int(_), Ty::Int) | (Data::Float(_), Ty::Float) | (Data::Bool(_), Ty::Bool) => true,
            (Data::Str(_), Ty::Str) => true,
            (Data::Array(items), Ty::Array(elem)) => {
                elem.ty().is_some_and(|t| items.iter().all(|item| self.fits(item, t))) || items.is_empty()
            }
            (Data::Record { ty: name, fields }, Ty::Struct(id)) => {
                let def = self.structs.get(id);
                *name == def.name
                    && fields.len() == def.fields.len()
                    && fields.iter().zip(&def.fields).all(|((n, d), (m, t))| n == m && self.fits(d, *t))
            }
            _ => false,
        }
    }

    /// A native value for `data`, which [`fits`](Self::fits) `ty`; objects
    /// are new, with a count of 1.
    pub fn write(&self, data: &Data, ty: Ty) -> u64 {
        match (data, ty) {
            (Data::Int(n), _) => *n as u64,
            (Data::Float(f), _) => f.to_bits(),
            (Data::Bool(b), _) => u64::from(*b),
            (Data::Str(s), _) => Str::new(s) as u64,
            (Data::Array(items), Ty::Array(elem)) => {
                let elem = elem.ty().unwrap_or(Ty::Int);
                Arr::new(items.iter().map(|item| self.write(item, elem)).collect()) as u64
            }
            (Data::Record { fields, .. }, Ty::Struct(id)) => {
                let def = self.structs.get(id);
                let record = Record::alloc(fields.len());
                for (i, ((_, value), (_, t))) in fields.iter().zip(&def.fields).enumerate() {
                    // SAFETY: a new record of `fields.len()` fields
                    unsafe { Record::set(record, i, self.write(value, *t)) };
                }
                record as u64
            }
            (data, ty) => unreachable!("{data:?} does not fit {ty:?}"),
        }
    }

    /// Reads a native value (without releasing it).
    pub fn read(&self, bits: u64, ty: Ty) -> Data {
        // SAFETY (all reads): `bits` is a live value of type `ty`, produced by native code
        match ty {
            Ty::Int => Data::Int(bits as i64),
            Ty::Float => Data::Float(f64::from_bits(bits)),
            Ty::Bool => Data::Bool(bits & 0xff != 0),
            Ty::Str => Data::Str(unsafe { Str::text(bits as *const Str) }.to_string()),
            Ty::Array(elem) => Data::Array(self.items(bits, elem)),
            Ty::Struct(id) => {
                let def = self.structs.get(id);
                let values = unsafe { Record::fields(bits as *const u64, def.fields.len()) };
                let fields = values.iter().zip(&def.fields).map(|(v, (n, t))| (n.clone(), self.read(*v, *t)));
                Data::Record { ty: def.name.clone(), fields: fields.collect() }
            }
        }
    }

    /// The elements of a native array.
    pub fn items(&self, bits: u64, elem: Elem) -> Vec<Data> {
        let elem = elem.ty().unwrap_or(Ty::Int);
        // SAFETY: a live array
        unsafe { Arr::items(bits as *const Arr) }.iter().map(|v| self.read(*v, elem)).collect()
    }

    /// Drops a reference held by the boundary.
    pub fn release(&self, bits: u64, ty: Ty) {
        if ty.is_heap() {
            // SAFETY: a live object of type `ty`, whose reference the caller owns
            unsafe { grenat_runtime::release(bits as *mut u8, self.shapes.of(ty)) }
        }
    }

    /// Adds a reference, kept by the boundary.
    pub fn retain(&self, bits: u64) {
        // SAFETY: a live object
        unsafe { grenat_runtime::retain(bits as *mut u8) }
    }
}
