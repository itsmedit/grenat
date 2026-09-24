//! Ahead-of-time compilation (`grenat build`).
//!
//! [`object`] emits the compiled functions into an object file, together with
//! an [`Image`] exported as `grenat_image`: the program's source, the names
//! of the compiled functions, their trampolines and the table of shapes. The
//! object is linked with the host library (`grenat_host`), which parses the
//! source again at startup, runs it, and calls the linked code through
//! [`Native::link`]: nothing is compiled at run time.

use cranelift_module::{DataDescription, DataId, Linkage, Module};
use cranelift_object::{ObjectBuilder, ObjectModule};
use grenat_ast::Program;
use grenat_runtime::Shape;

use crate::eligibility::select;
use crate::emit::{emit, isa};
use crate::native::{Native, Report, Trampoline};
use crate::shapes::Shapes;
use crate::structs::Structs;

/// Name of the exported [`Image`].
pub const IMAGE_SYMBOL: &str = "grenat_image";

/// What an executable built by `grenat build` holds, as laid out in memory.
#[repr(C)]
pub struct Image {
    source: *const u8,
    source_len: u64,
    /// Compiled function names, one per line, in the order of `functions`.
    names: *const u8,
    names_len: u64,
    functions: *const *const u8,
    function_count: u64,
    /// Pointers to shapes, written once at startup by [`Native::link`].
    shapes: *mut *const u8,
    shape_count: u64,
}

// SAFETY: an image is immutable, except its table of shapes, which
// `Native::link` fills once before any native code runs.
unsafe impl Sync for Image {}

impl Image {
    /// The program's source.
    ///
    /// # Safety
    /// `self` must be an image emitted by [`object`] and linked into this executable.
    pub unsafe fn source(&self) -> &str {
        // SAFETY: UTF-8 bytes copied from a `&str` by `object`
        unsafe { std::str::from_utf8_unchecked(std::slice::from_raw_parts(self.source, self.source_len as usize)) }
    }

    /// # Safety
    /// As [`Image::source`].
    unsafe fn names(&self) -> Vec<&str> {
        // SAFETY: as `source`
        let names = unsafe { std::str::from_utf8_unchecked(std::slice::from_raw_parts(self.names, self.names_len as usize)) };
        names.lines().collect()
    }
}

/// An object file, and what it compiled.
pub struct Object {
    pub bytes: Vec<u8>,
    pub report: Report,
}

/// The object file of `program` (parsed from `source`, and checked).
pub fn object(program: &Program, source: &str) -> Result<Object, String> {
    let fail = |e: cranelift_module::ModuleError| e.to_string();
    let structs = Structs::from_program(program);
    let (selected, interpreted) = select(program, &structs);
    let report = Report { compiled: selected.iter().map(|c| c.def.name.name.clone()).collect(), interpreted };
    let builder =
        ObjectBuilder::new(isa(true)?, "grenat_program", cranelift_module::default_libcall_names()).map_err(fail)?;
    let mut module = ObjectModule::new(builder);
    let emitted = emit(&mut module, &selected, &structs)?;

    let names = selected.iter().map(|c| c.def.name.name.as_str()).collect::<Vec<_>>().join("\n");
    let source_id = bytes(&mut module, source.as_bytes())?;
    let names_id = bytes(&mut module, names.as_bytes())?;

    let functions = module.declare_anonymous_data(false, false).map_err(fail)?;
    let mut table = DataDescription::new();
    // explicit zeros: a zero-filled (bss) section cannot hold relocations
    table.define(vec![0; 8 * emitted.trampolines.len().max(1)].into_boxed_slice());
    // pointers: the linkers require them aligned
    table.set_align(8);
    for (i, id) in emitted.trampolines.iter().enumerate() {
        let func = module.declare_func_in_data(*id, &mut table);
        table.write_function_addr((i * 8) as u32, func);
    }
    module.define_data(functions, &table).map_err(fail)?;

    let image = module.declare_data(IMAGE_SYMBOL, Linkage::Export, false, false).map_err(fail)?;
    let mut data = DataDescription::new();
    let mut contents = vec![0u8; 64];
    let lengths = [source.len(), names.len(), emitted.trampolines.len(), crate::shapes::count(structs.count())];
    for (i, len) in lengths.into_iter().enumerate() {
        contents[16 * i + 8..16 * i + 16].copy_from_slice(&(len as u64).to_ne_bytes());
    }
    data.define(contents.into_boxed_slice());
    data.set_align(8);
    for (i, id) in [source_id, names_id, functions, emitted.shapes].into_iter().enumerate() {
        let global = module.declare_data_in_data(id, &mut data);
        data.write_data_addr((16 * i) as u32, global, 0);
    }
    module.define_data(image, &data).map_err(fail)?;

    let bytes = module.finish().emit().map_err(|e| e.to_string())?;
    Ok(Object { bytes, report })
}

/// Read-only bytes, NUL-terminated (a data object is never empty).
fn bytes(module: &mut ObjectModule, content: &[u8]) -> Result<DataId, String> {
    let id = module.declare_anonymous_data(false, false).map_err(|e| e.to_string())?;
    let mut data = DataDescription::new();
    data.define([content, &[0]].concat().into_boxed_slice());
    module.define_data(id, &data).map_err(|e| e.to_string())?;
    Ok(id)
}

impl Native {
    /// The native code linked into this executable, for `program`.
    ///
    /// # Safety
    /// `image` must be the image of this executable, and `program` parsed
    /// from its source. Call it once: it fills the table of shapes.
    pub unsafe fn link(program: &Program, image: &Image) -> Result<Native, String> {
        let structs = Structs::from_program(program);
        let (selected, interpreted) = select(program, &structs);
        let compiled: Vec<&str> = selected.iter().map(|c| c.def.name.name.as_str()).collect();
        // SAFETY: by contract
        let linked = unsafe { image.names() };
        if linked != compiled || image.function_count as usize != compiled.len() {
            return Err(format!("the executable was built by another version of Grenat ({linked:?} ≠ {compiled:?})"));
        }
        let shapes = Shapes::build(&structs);
        let pointers = shapes.table();
        if image.shape_count as usize != pointers.len() {
            return Err("the executable was built by another version of Grenat (shapes)".into());
        }
        // SAFETY: the image's writable table, of that size, read by no code yet;
        // its functions are trampolines emitted by `object`
        let trampolines = unsafe {
            std::ptr::copy_nonoverlapping(pointers.as_ptr(), image.shapes as *mut *const Shape, pointers.len());
            std::slice::from_raw_parts(image.functions, compiled.len())
                .iter()
                .map(|f| std::mem::transmute::<*const u8, Trampoline>(*f))
                .collect()
        };
        let report = Report { compiled: compiled.iter().map(|s| s.to_string()).collect(), interpreted };
        Ok(Native::new(&selected, trampolines, report, structs, shapes, Box::new(())))
    }
}
