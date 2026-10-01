//! The native part of the facet `sheets`: spreadsheets and CSV for Grenat
//! programs, as Rust functions exported through `grenat_ext`.
//!
//! - [`workbook`]: reading `.xlsx`, `.xlsm`, `.xlsb`, `.xls` and `.ods`
//!   files (calamine): sheet names, rows of text, records;
//! - [`xlsx`]: writing `.xlsx` files (rust_xlsxwriter), as text or typed;
//! - [`csv_read`], [`csv_write`]: CSV text and files (the `csv` crate).
//!
//! What a function reads from a file is untrusted in Grenat (`~T`: it
//! declares `fs.read`); a function on text only (`pure`) gives back a
//! result as trusted as its arguments. Writing declares `fs.write`, which
//! no untrusted value reaches. Nothing here touches the network.
//!
//! Each exported function is named `sheets_…`: the raw layer, which the
//! facet's Grenat code (`src/lib.grn`, the module `Sheets`) wraps with
//! friendlier signatures.

pub mod cells;
pub mod csv_read;
pub mod csv_write;
pub mod delimiter;
pub mod number;
pub mod records;
pub mod workbook;
pub mod xlsx;

#[cfg(test)]
pub(crate) mod scratch;
