# grenat_ext

The SDK of **native facets** for [Grenat](https://github.com/itsmedit/grenat), an agentic
programming language: Rust code a facet (a Grenat library) ships, called by Grenat programs
as ordinary functions — as Ruby calls a gem's C code.

```rust
use grenat_ext::{GrenatType, export};
use serde::{Deserialize, Serialize};

/// A cell of a sheet.
#[derive(Serialize, Deserialize, GrenatType)]
pub struct Cell {
    pub row: i64,
    pub text: String,
}

/// Reads a sheet: one row per line, cells separated by commas.
#[export(effects = "fs.read")]
pub fn read_sheet(path: String) -> Result<Vec<Vec<String>>, std::io::Error> {
    let text = std::fs::read_to_string(path)?;
    Ok(text.lines().map(|line| line.split(',').map(str::to_string).collect()).collect())
}

/// The cell at `row` holding `text`.
#[export(pure)]
pub fn cell(row: i64, text: String) -> Cell {
    Cell { row, text }
}
```

## In a facet

The facet's crate is a `cdylib` (`native/Cargo.toml`), named in its `grenat.toml`:

```toml
# native/Cargo.toml
[lib]
crate-type = ["cdylib"]

[dependencies]
grenat_ext = "0.1"
serde = { version = "1", features = ["derive"] }
```

```toml
# grenat.toml
[package]
name = "sheets"
version = "0.1.0"

[native]
path = "native"
```

An application trusts the facet explicitly — native code runs outside Grenat's sandbox — and
`setter install` builds the crate, reads its manifest and writes the Grenat declarations:

```ruby
# Facetfile
facet "sheets", "~> 0.1", native: true

# generated:
#   native def read_sheet(path: String) -> ~Array(Array(String)) uses fs.read
#   native def cell(row: Int, text: String) -> Cell pure
```

Arguments and results cross as JSON through a versioned C ABI (version 1); `Err` and panics
become Grenat errors. The crate's documentation describes the ABI, the manifest a library
gives of itself, and the types that cross: <https://docs.rs/grenat_ext>.

## License

MIT or Apache-2.0, at your option.
