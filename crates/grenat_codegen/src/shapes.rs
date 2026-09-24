//! The shape of every heap type of a program (see `grenat_runtime::Shape`),
//! emitted as data of the module: the code refers to it by symbol, in
//! memory (JIT) as in an executable, and nothing needs to be built at load
//! time. Shapes are numbered in the order of [`index`].

use cranelift_module::{DataDescription, DataId, Module};
use grenat_runtime::layout;

use crate::structs::Structs;
use crate::ty::{Elem, StructId, Ty};

/// Element types of arrays, in shape order.
fn array_elems(structs: usize) -> impl Iterator<Item = Elem> {
    [Elem::Int, Elem::Float, Elem::Bool, Elem::Str, Elem::Unknown]
        .into_iter()
        .chain((0..structs).map(|i| Elem::Struct(StructId(i))))
}

/// Number of shapes of a program with `structs` structs.
pub(crate) fn count(structs: usize) -> usize {
    1 + structs + array_elems(structs).count()
}

/// Position of the shape of `ty` (a heap type).
pub(crate) fn index(ty: Ty, structs: usize) -> usize {
    match ty {
        Ty::Str => 0,
        Ty::Struct(id) => 1 + id.0,
        Ty::Array(elem) => 1 + structs + array_elems(structs).position(|e| e == elem).expect("an element type"),
        other => unreachable!("`{other:?}` is not an object"),
    }
}

/// Shape of what a slot holding `elem` points to, if anything.
fn slot(elem: Elem, structs: usize) -> Option<usize> {
    elem.ty().filter(|t| t.is_heap()).map(|t| index(t, structs))
}

/// Kind and slots of shape `i`.
fn describe(i: usize, structs: &Structs) -> (u64, Vec<Option<usize>>) {
    let n = structs.count();
    if i == 0 {
        return (grenat_runtime::STR, Vec::new());
    }
    if i <= n {
        let fields = &structs.get(StructId(i - 1)).fields;
        let slots = fields.iter().map(|(_, t)| slot(t.elem().expect("no array field"), n)).collect();
        return (grenat_runtime::RECORD, slots);
    }
    let elem = array_elems(n).nth(i - 1 - n).expect("an array shape");
    (grenat_runtime::ARRAY, vec![slot(elem, n)])
}

/// Declares and defines every shape; their data, in shape order.
pub(crate) fn emit(module: &mut impl Module, structs: &Structs) -> Result<Vec<DataId>, String> {
    let fail = |e: cranelift_module::ModuleError| e.to_string();
    let total = count(structs.count());
    let ids: Vec<DataId> =
        (0..total).map(|_| module.declare_anonymous_data(false, false).map_err(fail)).collect::<Result<_, _>>()?;
    for (i, id) in ids.iter().enumerate() {
        let (kind, slots) = describe(i, structs);
        let mut shape = DataDescription::new();
        let mut bytes = vec![0u8; layout::SHAPE_SIZE];
        bytes[layout::SHAPE_KIND as usize..][..8].copy_from_slice(&kind.to_ne_bytes());
        bytes[layout::SHAPE_COUNT as usize..][..8].copy_from_slice(&(slots.len() as u64).to_ne_bytes());
        // explicit zeros, not bss: the data holds relocations
        shape.define(bytes.into_boxed_slice());
        shape.set_align(8);
        if !slots.is_empty() {
            let table = module.declare_anonymous_data(false, false).map_err(fail)?;
            let mut data = DataDescription::new();
            data.define(vec![0u8; 8 * slots.len()].into_boxed_slice());
            data.set_align(8);
            for (j, target) in slots.iter().enumerate() {
                if let Some(target) = target {
                    let global = module.declare_data_in_data(ids[*target], &mut data);
                    data.write_data_addr((8 * j) as u32, global, 0);
                }
            }
            module.define_data(table, &data).map_err(fail)?;
            let global = module.declare_data_in_data(table, &mut shape);
            shape.write_data_addr(layout::SHAPE_SLOTS as u32, global, 0);
        }
        module.define_data(*id, &shape).map_err(fail)?;
    }
    Ok(ids)
}
