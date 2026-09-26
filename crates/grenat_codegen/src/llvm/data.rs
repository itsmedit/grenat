//! Data objects as LLVM globals: bytes, with the addresses of functions and
//! other data objects written at their relocations.

use cranelift_module::{DataDeclaration, DataDescription, DataId, Init, ModuleRelocTarget};

use super::names::{Names, linkage};

pub(crate) fn global(
    names: &Names,
    id: DataId,
    decl: &DataDeclaration,
    data: &DataDescription,
) -> Result<String, String> {
    let bytes: Vec<u8> = match &data.init {
        Init::Bytes { contents } => contents.to_vec(),
        Init::Zeros { size } => vec![0; *size],
        Init::Uninitialized => return Err("a data object is not initialized".into()),
    };
    // (offset, the address written there)
    let mut relocs: Vec<(usize, String)> = Vec::new();
    for (offset, func) in &data.function_relocs {
        relocs.push((*offset as usize, target(names, &data.function_decls[*func])?));
    }
    for (offset, global, addend) in &data.data_relocs {
        let base = target(names, &data.data_decls[*global])?;
        let address = if *addend == 0 { base } else { format!("getelementptr (i8, ptr {base}, i64 {addend})") };
        relocs.push((*offset as usize, address));
    }
    relocs.sort_by_key(|(offset, _)| *offset);

    let (mut types, mut values) = (Vec::new(), Vec::new());
    let mut at = 0;
    for (offset, address) in relocs {
        if offset < at || offset + 8 > bytes.len() {
            return Err("overlapping or out-of-bounds relocations in a data object".into());
        }
        push_bytes(&bytes[at..offset], &mut types, &mut values);
        types.push("ptr".to_string());
        values.push(format!("ptr {address}"));
        at = offset + 8;
    }
    push_bytes(&bytes[at..], &mut types, &mut values);
    let kind = if decl.writable { "global" } else { "constant" };
    let align = data.align.unwrap_or(1).max(if types.iter().any(|t| t == "ptr") { 8 } else { 1 });
    Ok(format!(
        "{} = {}{kind} <{{ {} }}> <{{ {} }}>, align {align}\n",
        names.data(id),
        linkage(decl.linkage),
        types.join(", "),
        values.join(", ")
    ))
}

fn target(names: &Names, target: &ModuleRelocTarget) -> Result<String, String> {
    match target {
        ModuleRelocTarget::User { namespace, index } => {
            Ok(names.user(&cranelift_codegen::ir::UserExternalName { namespace: *namespace, index: *index }))
        }
        other => Err(format!("unsupported relocation target {other:?}")),
    }
}

fn push_bytes(bytes: &[u8], types: &mut Vec<String>, values: &mut Vec<String>) {
    if bytes.is_empty() {
        return;
    }
    types.push(format!("[{} x i8]", bytes.len()));
    let mut text = String::from("c\"");
    for &b in bytes {
        if b.is_ascii_graphic() && b != b'"' && b != b'\\' || b == b' ' {
            text.push(b as char);
        } else {
            text.push_str(&format!("\\{b:02X}"));
        }
    }
    text.push('"');
    values.push(format!("[{} x i8] {text}", bytes.len()));
}
