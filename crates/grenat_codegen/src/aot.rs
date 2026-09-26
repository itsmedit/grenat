//! Ahead-of-time compilation (`grenat build`).
//!
//! [`object`] emits the compiled functions into an object file, together with
//! an [`Image`] exported as `grenat_image`: the program's source, the names
//! of the compiled functions, their trampolines and their shapes. The
//! object is linked with the host library (`grenat_host`), which parses the
//! source again at startup, runs it, and calls the linked code through
//! [`Native::link`]: nothing is compiled at run time.

use cranelift_module::{Linkage, Module};
use grenat_ast::Program;
use grenat_runtime::Shape;

use crate::Backend;
use crate::eligibility::select;
use crate::emit::emit;
use crate::infer::Target;
use crate::native::{Native, Report, Trampoline};
use crate::object_data::{Strings, Word, new_record, record};
use crate::structs::Structs;

/// Name of the exported [`Image`].
pub const IMAGE_SYMBOL: &str = "grenat_image";

/// What an executable built by `grenat build` holds, as laid out in memory.
#[repr(C)]
pub struct Image {
    source: *const u8,
    source_len: u64,
    /// The source's file table (`grenat_report::Sources::table`).
    files: *const u8,
    files_len: u64,
    /// Compiled function names, one per line, in the order of `functions`.
    names: *const u8,
    names_len: u64,
    functions: *const *const u8,
    function_count: u64,
    /// The shapes, in shape order (see [`shapes`](crate::shapes)).
    shapes: *const *const Shape,
    shape_count: u64,
}

// SAFETY: an image is immutable data of the executable.
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

    /// The file table of [`source`](Self::source).
    ///
    /// # Safety
    /// As [`Image::source`].
    pub unsafe fn files(&self) -> &str {
        // SAFETY: as `source`
        unsafe { std::str::from_utf8_unchecked(std::slice::from_raw_parts(self.files, self.files_len as usize)) }
    }

    /// # Safety
    /// As [`Image::source`].
    unsafe fn names(&self) -> Vec<&str> {
        // SAFETY: as `source`
        let names =
            unsafe { std::str::from_utf8_unchecked(std::slice::from_raw_parts(self.names, self.names_len as usize)) };
        names.lines().collect()
    }
}

/// An object file, and what it compiled.
pub struct Object {
    pub bytes: Vec<u8>,
    pub report: Report,
}

/// The object file of `program` (parsed from `source`, whose file table is
/// `files`, and checked).
pub fn object(program: &Program, source: &str, files: &str, backend: Backend) -> Result<Object, String> {
    let structs = Structs::from_program(program);
    let (selected, interpreted) = select(program, &structs, Target::Hosted);
    let report = Report { compiled: selected.iter().map(|c| c.def.name.name.clone()).collect(), interpreted };
    let (bytes, ()) = backend.object(|mut module| image(&mut module, &selected, &structs, source, files))?;
    Ok(Object { bytes, report })
}

/// The compiled functions and the [`Image`] describing them.
fn image(
    module: &mut impl Module,
    selected: &[crate::eligibility::Compiled],
    structs: &Structs,
    source: &str,
    files: &str,
) -> Result<(), String> {
    let fail = |e: cranelift_module::ModuleError| e.to_string();
    let emitted = emit(module, selected, structs)?;

    let names = selected.iter().map(|c| c.def.name.name.as_str()).collect::<Vec<_>>().join("\n");
    let mut strings = Strings::default();
    let functions: Vec<Word> = emitted.trampolines.iter().map(|id| Word::Function(*id)).collect();
    let functions = new_record(module, &functions)?;
    let shapes: Vec<Word> = emitted.shapes.iter().map(|id| Word::Data(*id)).collect();
    let shapes = new_record(module, &shapes)?;

    let image = module.declare_data(IMAGE_SYMBOL, Linkage::Export, false, false).map_err(fail)?;
    let [source_ptr, source_len] = strings.words(module, source)?;
    let [files_ptr, files_len] = strings.words(module, files)?;
    let [names_ptr, names_len] = strings.words(module, &names)?;
    let words = [
        source_ptr,
        source_len,
        files_ptr,
        files_len,
        names_ptr,
        names_len,
        Word::Data(functions),
        Word::Number(emitted.trampolines.len() as u64),
        Word::Data(shapes),
        Word::Number(emitted.shapes.len() as u64),
    ];
    record(module, image, &words)
}

impl Native {
    /// The native code linked into this executable, for `program`.
    ///
    /// # Safety
    /// `image` must be the image of this executable, and `program` parsed
    /// from its source.
    pub unsafe fn link(program: &Program, image: &Image) -> Result<Native, String> {
        let structs = Structs::from_program(program);
        let (selected, interpreted) = select(program, &structs, Target::Hosted);
        let compiled: Vec<&str> = selected.iter().map(|c| c.def.name.name.as_str()).collect();
        // SAFETY: by contract
        let linked = unsafe { image.names() };
        if linked != compiled || image.function_count as usize != compiled.len() {
            return Err(format!("the executable was built by another version of Grenat ({linked:?} ≠ {compiled:?})"));
        }
        if image.shape_count as usize != crate::shapes::count(structs.count()) {
            return Err("the executable was built by another version of Grenat (shapes)".into());
        }
        // SAFETY: tables of the image, of those sizes; its functions are
        // trampolines emitted by `object`
        let (shapes, trampolines) = unsafe {
            let shapes = std::slice::from_raw_parts(image.shapes, image.shape_count as usize).to_vec();
            let trampolines = std::slice::from_raw_parts(image.functions, compiled.len())
                .iter()
                .map(|f| std::mem::transmute::<*const u8, Trampoline>(*f))
                .collect();
            (shapes, trampolines)
        };
        let report = Report { compiled: compiled.iter().map(|s| s.to_string()).collect(), interpreted };
        Ok(Native::new(&selected, trampolines, report, structs, shapes, Box::new(())))
    }
}
