# sheets

Spreadsheets and CSV for Grenat programs: read `.xlsx`, `.xlsm`, `.xlsb`,
`.xls` and `.ods` workbooks, write `.xlsx` workbooks, parse and write CSV.

`sheets` is a **native facet**: its work is done by Rust code it ships
(`native/`, a crate built on `grenat_ext`, using
[calamine](https://crates.io/crates/calamine),
[rust_xlsxwriter](https://crates.io/crates/rust_xlsxwriter) and
[csv](https://crates.io/crates/csv)). That code runs outside Grenat's
sandbox, so the application trusts it explicitly in its `Facetfile`:

```ruby
# Facetfile
facet "sheets", "~> 0.1", native: true                          # from an index
facet "sheets", path: "../grenat/facets/sheets", native: true   # or from a checkout
```

`setter install` then builds the crate (`cargo build --release`: a Rust
toolchain is needed) and writes the declarations of its functions. Without
`native: true`, the install is refused, and says how to trust the facet.

```ruby
require "sheets"

def main uses fs.read, fs.write
  orders = Sheets.records("orders.xlsx", sheet: "2026").trust!
  big = orders.select { |o| o["total"].to_f > 1000.0 }
  Sheets.write_csv("big_orders.csv", Sheets.table(big, ["id", "customer", "total"]))
end
```

## Trust and effects

- **What comes from a file is untrusted** (`~T`): `Sheets.names`, `rows`,
  `records`, `read_csv` and `read_csv_records` declare `fs.read`, and their
  results must be checked (`.check { … }`) or trusted (`.trust!`) before
  they reach a command, a request, a human or a file.
- **Functions on text are pure**: `parse_csv`, `parse_csv_records` and
  `to_csv` have no effect, and their result is as trusted as their
  arguments — rows parsed from an untrusted text are untrusted.
- **Writing declares `fs.write`**, which refuses untrusted rows (E0412 when
  checking, `TaintError` when running).
- No function touches the network. Paths are the program's, relative to
  its working directory.

## Workbooks

Cells are read as text, as the sheet shows them: numbers in their usual
form (`"36"`, `"2.5"`, never `"36.0"`), booleans as `"true"` and `"false"`,
dates as `"2024-01-15"` (`"2024-01-15 10:30:00"` with a time, `"10:30:00"`
for a time alone), durations as `"27:15:00"`, errors as `"#DIV/0!"`, empty
cells as `""`; a formula gives the value the file saved for it. A sheet's
rows are its used range — from its first non-empty row and column to its
last — every row as wide as the widest.

| Function | Returns |
|---|---|
| `Sheets.names(path)` | `~Array(String)`: the sheets, in order |
| `Sheets.rows(path, sheet: nil)` | `~Array(Array(String))`: the rows of a sheet (the first unless named) |
| `Sheets.records(path, sheet: nil)` | `~Array(Hash(String, String))`: one hash per row, named by the first row |
| `Sheets.write(path, sheets, typed: false)` | writes `Array(Worksheet)` as an `.xlsx` file |
| `Sheets.write_rows(path, rows, name: "Sheet1", typed: false)` | writes one sheet |

```ruby
Sheets.names("people.xlsx")                     # ~["People", "Totals"]
Sheets.rows("people.xlsx")                      # ~[["name", "age"], ["Ada", "36"]]
Sheets.records("stock.ods", sheet: "Stock")     # ~[{"item" => "pens", "count" => "12"}]

Sheets.write("report.xlsx", [
  Worksheet(name: "People", rows: [["name", "age"], ["Ada", "36"]]),
  Worksheet(name: "Notes", rows: [["checked by Linus"]]),
])
Sheets.write_rows("codes.xlsx", [["code", "price"], ["007", "1.5"]], typed: true)
```

**Text unless typed.** By default every cell is written as text, exactly as
given: `"007"` stays `007`, and `"=SUM(A1:A9)"` is text, never a formula —
a file written from data cannot run anything when it is opened. With
`typed: true`, a cell that is a number is written as a number (which a
spreadsheet sums and sorts): exactly the cells whose number reads back as
the same text, such as `"42"`, `"-3.5"` or `"0.25"`; `"007"`, `"1.50"`,
`"1e3"`, `"+1"` and numbers with more digits than a number keeps stay text.
Either way, reading the file back gives the same rows.

A workbook has one sheet at least; a sheet's name has at most 31
characters and none of `[]:*?/\`, and names are unique; Excel's limits
(1,048,576 rows of 16,384 cells, 32,767 characters a cell) hold. Each of
these, a missing file or sheet, or a file that is no workbook raises a
`SheetError`.

## CSV

Quoted cells may hold delimiters, quotes (doubled: `""`) and line breaks;
rows may differ in length; blank lines are skipped; a UTF-8 byte order mark
(as Excel writes one) is dropped. The delimiter is one ASCII character —
`","` unless given, `";"`, `"\t"`, `"|"` — never a quote or a line break.
When writing, a cell is quoted when it must be, and each row ends with
`\n`.

| Function | Returns |
|---|---|
| `Sheets.parse_csv(text, delimiter: ",")` | `Array(Array(String))` (pure) |
| `Sheets.parse_csv_records(text, delimiter: ",")` | `Array(Hash(String, String))` (pure) |
| `Sheets.read_csv(path, delimiter: ",")` | `~Array(Array(String))` |
| `Sheets.read_csv_records(path, delimiter: ",")` | `~Array(Hash(String, String))` |
| `Sheets.to_csv(rows, delimiter: ",")` | `String`: the CSV text (pure) |
| `Sheets.write_csv(path, rows, delimiter: ",")` | writes the CSV file |
| `Sheets.table(records, columns)` | `Array(Array(String))`: a header row, then each record's cells |

```ruby
Sheets.parse_csv("name,note\n\"Doe, Jane\",\"said \"\"hi\"\"\"\n")
# [["name", "note"], ["Doe, Jane", "said \"hi\""]]
Sheets.parse_csv_records("name;age\nAda;36\n", delimiter: ";")
# [{"name" => "Ada", "age" => "36"}]
Sheets.to_csv([["a", "b, c"]])                  # "a,\"b, c\"\n"
Sheets.write_csv("out.tsv", rows, delimiter: "\t")
Sheets.table(records, ["name", "age"])          # [["name", "age"], ["Ada", "36"]]
```

A file that is not UTF-8, a bad delimiter, or records whose table is
ambiguous raise a `CsvError`.

**Records** (for workbooks and CSV alike): the first row names the columns,
and each name appears once; a row with fewer cells gets `""` for the
missing ones, one with more (non-empty) cells is an error, and a row whose
cells are all empty is skipped.

**CSV and spreadsheets.** A CSV file holds no types: a spreadsheet that
opens one may read a cell such as `=1+1` as a formula. Rows written to a
file are trusted rows, but if they come from people you do not know and the
file is meant for a spreadsheet, write an `.xlsx` (whose text cells are
never formulas) or check the cells first.

## The native functions

`Sheets` wraps ten native functions, which a program may also call
directly; `setter install` declares them in `.grenat/native/native.grn`:

```ruby
struct Worksheet
  name: String
  rows: Array(Array(String))
end

native def sheets_names(path: String) -> ~Array(String) uses fs.read
native def sheets_rows(path: String, sheet: String?) -> ~Array(Array(String)) uses fs.read
native def sheets_records(path: String, sheet: String?) -> ~Array(Hash(String, String)) uses fs.read
native def sheets_write_xlsx(path: String, sheets: Array(Worksheet), typed: Bool) uses fs.write
native def sheets_parse_csv(text: String, delimiter: String) -> Array(Array(String)) pure
native def sheets_parse_csv_records(text: String, delimiter: String) -> Array(Hash(String, String)) pure
native def sheets_read_csv(path: String, delimiter: String) -> ~Array(Array(String)) uses fs.read
native def sheets_read_csv_records(path: String, delimiter: String) -> ~Array(Hash(String, String)) uses fs.read
native def sheets_format_csv(rows: Array(Array(String)), delimiter: String) -> String pure
native def sheets_write_csv(path: String, rows: Array(Array(String)), delimiter: String) uses fs.write
```

## Developing

```sh
cd facets/sheets
setter install                                  # builds native/, declares its functions
grenat test                                     # the Grenat tests (tests/*.grn)
cargo test --manifest-path native/Cargo.toml    # the Rust tests
```

The crate is not a member of Grenat's workspace (its `Cargo.toml` has a
`[workspace]` of its own, and its own `Cargo.lock`). Grenat's test suite
builds it once and runs all of the above
(`crates/grenat_cli/tests/facet_sheets.rs`).
