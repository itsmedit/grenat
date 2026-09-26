//! LLVM names of the module's functions and data objects.

use cranelift_codegen::ir::{ExternalName, Function, UserExternalName};
use cranelift_module::{DataId, FuncId, Linkage, ModuleDeclarations};

pub(crate) struct Names<'d> {
    declarations: &'d ModuleDeclarations,
}

impl<'d> Names<'d> {
    pub(crate) fn new(declarations: &'d ModuleDeclarations) -> Names<'d> {
        Names { declarations }
    }

    pub(crate) fn function(&self, id: FuncId) -> String {
        match &self.declarations.get_function_decl(id).name {
            Some(name) => quote(name),
            None => quote(&format!("grenat_anonymous_function_{}", id.as_u32())),
        }
    }

    pub(crate) fn data(&self, id: DataId) -> String {
        match &self.declarations.get_data_decl(id).name {
            Some(name) => quote(name),
            None => quote(&format!("grenat_anonymous_data_{}", id.as_u32())),
        }
    }

    /// The name of what `name` refers to in `func`.
    pub(crate) fn external(&self, func: &Function, name: &ExternalName) -> Result<String, String> {
        match name {
            ExternalName::User(reference) => Ok(self.user(&func.params.user_named_funcs()[*reference])),
            other => Err(format!("unsupported external name {other:?}")),
        }
    }

    /// Namespace 0: functions, 1: data (as `cranelift_module` declares them).
    pub(crate) fn user(&self, name: &UserExternalName) -> String {
        if name.namespace == 0 {
            self.function(FuncId::from_u32(name.index))
        } else {
            self.data(DataId::from_u32(name.index))
        }
    }
}

/// `@"name"`: any name is valid quoted.
fn quote(name: &str) -> String {
    let mut out = String::from("@\"");
    for b in name.bytes() {
        if b == b'"' || b == b'\\' || !b.is_ascii_graphic() {
            out.push_str(&format!("\\{b:02X}"));
        } else {
            out.push(b as char);
        }
    }
    out.push('"');
    out
}

/// The LLVM linkage of a definition.
pub(crate) fn linkage(linkage: Linkage) -> &'static str {
    match linkage {
        Linkage::Local => "internal ",
        Linkage::Hidden => "hidden ",
        Linkage::Import | Linkage::Preemptible | Linkage::Export => "",
    }
}
