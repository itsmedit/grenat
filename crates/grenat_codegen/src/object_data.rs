//! Read-only data of an object file: byte strings, tables of pointers,
//! records mixing numbers and pointers.

use std::collections::HashMap;

use cranelift_module::{DataDescription, DataId, FuncId, Module};

/// A field of a record: a number, or the address of a function or of data.
pub(crate) enum Word {
    Number(u64),
    Function(FuncId),
    Data(DataId),
}

/// Defines a read-only record of 64-bit words (with relocations for addresses).
pub(crate) fn record(module: &mut impl Module, id: DataId, words: &[Word]) -> Result<(), String> {
    let mut data = DataDescription::new();
    let mut bytes = vec![0u8; 8 * words.len().max(1)];
    for (i, word) in words.iter().enumerate() {
        if let Word::Number(n) = word {
            bytes[8 * i..8 * i + 8].copy_from_slice(&n.to_ne_bytes());
        }
    }
    // explicit bytes, not bss: a zero-filled section cannot hold relocations
    data.define(bytes.into_boxed_slice());
    // addresses: the linkers require them aligned
    data.set_align(8);
    for (i, word) in words.iter().enumerate() {
        match word {
            Word::Function(func) => {
                let func = module.declare_func_in_data(*func, &mut data);
                data.write_function_addr((8 * i) as u32, func);
            }
            Word::Data(target) => {
                let global = module.declare_data_in_data(*target, &mut data);
                data.write_data_addr((8 * i) as u32, global, 0);
            }
            Word::Number(_) => {}
        }
    }
    module.define_data(id, &data).map_err(|e| e.to_string())
}

/// A new read-only record.
pub(crate) fn new_record(module: &mut impl Module, words: &[Word]) -> Result<DataId, String> {
    let id = module.declare_anonymous_data(false, false).map_err(|e| e.to_string())?;
    record(module, id, words)?;
    Ok(id)
}

/// Read-only byte strings, NUL-terminated (a data object is never empty),
/// each defined once.
#[derive(Default)]
pub(crate) struct Strings {
    defined: HashMap<String, DataId>,
}

impl Strings {
    pub fn get(&mut self, module: &mut impl Module, text: &str) -> Result<DataId, String> {
        if let Some(id) = self.defined.get(text) {
            return Ok(*id);
        }
        let id = module.declare_anonymous_data(false, false).map_err(|e| e.to_string())?;
        let mut data = DataDescription::new();
        data.define([text.as_bytes(), &[0]].concat().into_boxed_slice());
        module.define_data(id, &data).map_err(|e| e.to_string())?;
        self.defined.insert(text.to_string(), id);
        Ok(id)
    }

    /// The two words of a `Bytes` (address, length).
    pub fn words(&mut self, module: &mut impl Module, text: &str) -> Result<[Word; 2], String> {
        Ok([Word::Data(self.get(module, text)?), Word::Number(text.len() as u64)])
    }
}
