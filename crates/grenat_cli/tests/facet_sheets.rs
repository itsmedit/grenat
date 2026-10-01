//! The official facet `sheets` (`facets/sheets`) as an application uses it:
//! its crate built once, the facet trusted in the application's Facetfile
//! (`native: true`) and installed, then Grenat programs that read and write
//! workbooks and CSV — run, checked and tested, errors included. The
//! facet's own Grenat tests and its crate's Rust tests pass too, the
//! README says what its examples print, and no test leaves files behind.

use std::path::{Path, PathBuf};
use std::process::{Command, Output};
use std::sync::OnceLock;

const FACET: &str = "sheets";

fn repository() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../..")
}

/// The facet's sources, in the repository.
fn source() -> PathBuf {
    repository().join("facets").join(FACET)
}

/// Where the facet's crate is built: a directory of the workspace's target.
fn target_dir() -> PathBuf {
    match std::env::var_os("CARGO_TARGET_DIR") {
        Some(dir) => Path::new(&dir).join("facet-sheets"),
        None => repository().join("target/facet-sheets"),
    }
}

/// The facet's library, built once per test binary from the repository's
/// crate (which depends on this checkout's `grenat_ext`).
fn library() -> &'static Path {
    static BUILT: OnceLock<PathBuf> = OnceLock::new();
    BUILT.get_or_init(|| {
        grenat_native::build::build(&source().join("native"), Some(&target_dir()))
            .unwrap_or_else(|e| panic!("the facet `sheets` cannot be built: {e}"))
    })
}

fn grenat_in(dir: &Path, args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_grenat"))
        .args(args)
        .current_dir(dir)
        .env("NO_COLOR", "1")
        .env_remove("ANTHROPIC_API_KEY")
        .output()
        .unwrap()
}

fn text(bytes: &[u8]) -> String {
    String::from_utf8_lossy(bytes).into_owned()
}

fn write(path: &Path, text: &str) {
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(path, text).unwrap();
}

fn copy_dir(from: &Path, to: &Path) {
    std::fs::create_dir_all(to).unwrap();
    for entry in std::fs::read_dir(from).unwrap() {
        let entry = entry.unwrap();
        let name = entry.file_name();
        if name == ".grenat" || name == "target" {
            continue;
        }
        if entry.path().is_dir() {
            copy_dir(&entry.path(), &to.join(&name));
        } else {
            std::fs::copy(entry.path(), to.join(&name)).unwrap();
        }
    }
}

/// A copy of the facet in `dir/sheets`, its library prebuilt for this
/// platform (so that installing it builds nothing).
fn facet_copy(dir: &Path) -> PathBuf {
    let facet = dir.join(FACET);
    copy_dir(&source(), &facet);
    let prebuilt = grenat_native::layout::prebuilt(FACET, &facet.join("native"));
    std::fs::create_dir_all(prebuilt.parent().unwrap()).unwrap();
    std::fs::copy(library(), &prebuilt).unwrap();
    facet
}

/// A test's scratch directory, removed when the test ends (passed or
/// failed): each holds a copy of the facet's library, megabytes.
struct Scratch(PathBuf);

impl Scratch {
    /// The empty directory of the test `name`.
    fn new(name: &str) -> Self {
        let dir = std::env::temp_dir().join(format!("grenat-cli-facet-sheets-{}-{name}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        Scratch(dir.canonicalize().unwrap())
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

/// An application using the facet, in a scratch directory of its own.
struct World {
    app: PathBuf,
    _dir: Scratch,
}

/// An application `app` using the facet `sheets` next to it, trusted and
/// installed; `main` is its `src/main.grn`.
fn world(name: &str, main: &str) -> World {
    let dir = Scratch::new(name);
    facet_copy(&dir.0);
    let app = dir.0.join("app");
    write(&app.join("grenat.toml"), "[package]\nname = \"app\"\n");
    write(&app.join("Facetfile"), "facet \"sheets\", path: \"../sheets\", native: true\n");
    write(&app.join("src/main.grn"), main);
    grenat_package::facets::install(&app, false).unwrap();
    World { app, _dir: dir }
}

/// Every function of the facet, from text to files and back.
const TOUR: &str = r#"require "sheets"

def main uses fs.read, fs.write
  rows = Sheets.parse_csv("name,city\n\"Doe, Jane\",Paris\nLinus,\"Port\nland\"\n")
  p rows
  p Sheets.parse_csv_records("a;b\n1;2\n", delimiter: ";")
  puts Sheets.to_csv(rows, delimiter: ";")
  Sheets.write_csv("people.csv", rows)
  p Sheets.read_csv("people.csv").trust! == rows
  people = Sheets.read_csv_records("people.csv").trust!
  p people.map { |person| person["city"] }

  Sheets.write("book.xlsx", [Worksheet(name: "People", rows: rows), Worksheet(name: "Codes", rows: [["007", "42"]])])
  p Sheets.names("book.xlsx")
  p Sheets.rows("book.xlsx", sheet: "Codes")
  p Sheets.records("book.xlsx").trust!.map { |person| person["name"] }
  Sheets.write_rows("typed.xlsx", Sheets.table(people, ["city", "name"]), name: "Cities", typed: true)
  p Sheets.rows("typed.xlsx", sheet: "Cities").trust!
  p sheets_format_csv([["raw", "call"]], "|")
end
"#;

#[test]
fn an_application_reads_and_writes_sheets_and_csv() {
    let world = world("tour", TOUR);
    let app = world.app.clone();
    let out = grenat_in(&app, &["check"]);
    assert_eq!(out.status.code(), Some(0), "{}", text(&out.stderr));
    let out = grenat_in(&app, &["run", "--log"]);
    let expected = "[[\"name\", \"city\"], [\"Doe, Jane\", \"Paris\"], [\"Linus\", \"Port\\nland\"]]\n\
                    [{\"a\" => \"1\", \"b\" => \"2\"}]\n\
                    name;city\nDoe, Jane;Paris\nLinus;\"Port\nland\"\n\n\
                    true\n\
                    [\"Paris\", \"Port\\nland\"]\n\
                    ~[\"People\", \"Codes\"]\n\
                    ~[[\"007\", \"42\"]]\n\
                    [\"Doe, Jane\", \"Linus\"]\n\
                    [[\"city\", \"name\"], [\"Paris\", \"Doe, Jane\"], [\"Port\\nland\", \"Linus\"]]\n\
                    \"raw|call\\n\"\n";
    assert_eq!(text(&out.stdout), expected, "{}", text(&out.stderr));
    let log = text(&out.stderr);
    for function in [
        "sheets_parse_csv",
        "sheets_parse_csv_records",
        "sheets_format_csv",
        "sheets_write_csv",
        "sheets_read_csv",
        "sheets_read_csv_records",
        "sheets_write_xlsx",
        "sheets_names",
        "sheets_rows",
        "sheets_records",
    ] {
        assert!(log.contains(&format!("[native] sheets: {function}\n")), "{function}: {log}");
    }
    let csv = std::fs::read_to_string(app.join("people.csv")).unwrap();
    assert_eq!(csv, "name,city\n\"Doe, Jane\",Paris\nLinus,\"Port\nland\"\n");
}

#[test]
fn errors_say_what_went_wrong_and_can_be_rescued() {
    let main = r#"require "sheets"

def main uses fs.read, fs.write
  begin
    Sheets.rows("book.xlsx", sheet: "Nope")
  rescue SheetError => e
    puts "sheet: SheetError: #{e.message}"
  end
  begin
    Sheets.names("missing.xlsx")
  rescue SheetError => e
    puts "file: SheetError: #{e.message}"
  end
  begin
    Sheets.write("none.xlsx", [])
  rescue SheetError => e
    puts "empty: SheetError: #{e.message}"
  end
  begin
    Sheets.write_rows("bad.xlsx", [["x"]], name: "a/b")
  rescue SheetError => e
    puts "name: SheetError: #{e.message}"
  end
  begin
    Sheets.parse_csv("a", delimiter: ";;")
  rescue CsvError => e
    puts "delimiter: CsvError: #{e.message}"
  end
  begin
    Sheets.parse_csv_records("a,a\n1,2\n")
  rescue CsvError => e
    puts "columns: CsvError: #{e.message}"
  end
  begin
    Sheets.read_csv_records("ragged.csv")
  rescue CsvError => e
    puts "ragged: CsvError: #{e.message}"
  end
  begin
    Sheets.read_csv("latin1.csv")
  rescue CsvError => e
    puts "latin1: CsvError: #{e.message}"
  end
  begin
    Sheets.parse_csv("a,\"unterminated\nb,c\nd,e\n")
  rescue CsvError => e
    puts "quote: CsvError: #{e.message}"
  end
  begin
    Sheets.read_csv_records("stray.csv")
  rescue CsvError => e
    puts "stray: CsvError: #{e.message}"
  end
  Sheets.rows("book.xlsx", sheet: "Nope")
end
"#;
    let world = world("errors", main);
    let app = world.app.clone();
    write(&app.join("ragged.csv"), "a,b\n1,2,3\n");
    std::fs::write(app.join("latin1.csv"), b"caf\xe9\n").unwrap();
    write(&app.join("stray.csv"), "x\n\"a\"b,c\n");
    let out = grenat_in(&app, &["run"]);
    // `book.xlsx` does not exist: the first attempt fails on the file
    let stdout = text(&out.stdout);
    let lines: Vec<&str> = stdout.lines().collect();
    assert_eq!(lines.len(), 10, "{stdout}\n{}", text(&out.stderr));
    assert!(lines[0].starts_with("sheet: SheetError: cannot read the workbook book.xlsx: "), "{stdout}");
    assert!(lines[1].starts_with("file: SheetError: cannot read the workbook missing.xlsx: "), "{stdout}");
    assert_eq!(lines[2], "empty: SheetError: a workbook has one sheet at least");
    assert!(lines[3].starts_with("name: SheetError: the sheet name `a/b`: "), "{stdout}");
    assert_eq!(
        lines[4],
        "delimiter: CsvError: a delimiter is one ASCII character, such as \",\", \";\" or \"\\t\": not \";;\""
    );
    assert_eq!(lines[5], "columns: CsvError: the column `a` appears twice in the header row");
    assert_eq!(lines[6], "ragged: CsvError: ragged.csv: row 2 has 3 cells, the header row 2");
    assert_eq!(lines[7], "latin1: CsvError: latin1.csv is not UTF-8 text (byte 3)");
    assert_eq!(lines[8], "quote: CsvError: line 1: a quoted cell is never closed");
    assert_eq!(
        lines[9],
        "stray: CsvError: stray.csv: line 2: a quoted cell is followed by 'b', where a delimiter or the end of the line must be"
    );
    // unrescued, the error ends the program
    let err = text(&out.stderr);
    assert_eq!(out.status.code(), Some(1), "{err}");
    assert!(err.contains("SheetError: cannot read the workbook book.xlsx"), "{err}");

    // with a workbook there, the missing sheet is named, and the sheets listed
    write(
        &app.join("src/make.grn"),
        "require \"sheets\"\n\ndef main uses fs.write\n  Sheets.write_rows(\"book.xlsx\", [[\"x\"]], name: \"Only\")\nend\n",
    );
    let out = grenat_in(&app, &["run", "src/make.grn"]);
    assert_eq!(out.status.code(), Some(0), "{}", text(&out.stderr));
    let out = grenat_in(&app, &["run"]);
    let first = text(&out.stdout).lines().next().unwrap_or_default().to_string();
    assert_eq!(first, "sheet: SheetError: book.xlsx has no sheet named `Nope`: its sheets are `Only`");
}

#[test]
fn the_checker_keeps_untrusted_rows_out_of_files_and_effects_declared() {
    let world = world("checker", "require \"sheets\"\n\ndef main\nend\n");
    let app = world.app.clone();
    let bad = r#"require "sheets"

def copy uses fs.read, fs.write
  rows = Sheets.read_csv("in.csv")
  Sheets.write_csv("out.csv", rows)
  Sheets.write("out.xlsx", [Worksheet(name: "S", rows: Sheets.parse_csv(Sheets.to_csv(rows)))])
end

def peek uses env
  Sheets.names("book.xlsx")
end

def save uses fs.read
  Sheets.write_csv("x.csv", [["a"]])
end

def wrong uses fs.read
  Sheets.parse_csv(42)
  Sheets.rows("book.xlsx", sheet: 1)
end
"#;
    write(&app.join("src/bad.grn"), bad);
    let out = grenat_in(&app, &["check", "src/bad.grn"]);
    let err = text(&out.stderr);
    assert_eq!(out.status.code(), Some(1), "{err}");
    assert!(
        err.contains("error[E0412]: an untrusted value reaches `write_csv` (effect `fs.write`) without validation"),
        "{err}"
    );
    // untrusted rows stay untrusted through pure functions
    assert!(
        err.contains("error[E0412]: an untrusted value reaches `write` (effect `fs.write`) without validation"),
        "{err}"
    );
    assert!(err.contains("error[E0300]: `peek` uses effect `fs.read` without declaring it"), "{err}");
    assert!(err.contains("error[E0300]: `save` uses effect `fs.write` without declaring it"), "{err}");
    assert_eq!(err.matches("error[E0200]").count(), 2, "{err}");

    // checked, or trusted, they may be written
    let good = r#"require "sheets"

def copy uses fs.read, fs.write
  rows = Sheets.read_csv("in.csv").check { |rs| rs.all? { |r| r.size == 2 } }
  Sheets.write_csv("out.csv", rows.value) if rows.ok?
  Sheets.write_rows("out.xlsx", Sheets.read_csv("in.csv").trust!)
end
"#;
    write(&app.join("src/good.grn"), good);
    let out = grenat_in(&app, &["check", "src/good.grn"]);
    assert_eq!(out.status.code(), Some(0), "{}", text(&out.stderr));
}

#[test]
fn an_application_tests_its_use_of_sheets() {
    let world = world("tests", "require \"sheets\"\n\ndef main\nend\n");
    let app = world.app.clone();
    let tests = r#"require "sheets"

test "a report round-trips through a workbook" do
  rows = Sheets.parse_csv("item,count\npens,12\nink,\n")
  Sheets.write_rows("report.xlsx", rows, name: "Stock", typed: true)
  assert_equal ["Stock"], Sheets.names("report.xlsx").trust!
  assert_equal rows, Sheets.rows("report.xlsx").trust!
  assert_equal ["12", ""], Sheets.records("report.xlsx").trust!.map { |r| r["count"] }
end

test "errors are raised as the facet's types" do
  assert_raises(SheetError) { Sheets.rows("report.xlsx", sheet: "Nope") }
  assert_raises(CsvError) { Sheets.read_csv("nowhere.csv") }
end
"#;
    write(&app.join("tests/report_test.grn"), tests);
    let out = grenat_in(&app, &["test"]);
    let err = text(&out.stderr);
    assert_eq!(out.status.code(), Some(0), "{err}");
    assert!(err.ends_with("2 passed, 0 failed\n"), "{err}");
}

#[test]
fn the_facet_must_be_trusted() {
    let world = world("trust", "require \"sheets\"\n\ndef main\n  p Sheets.parse_csv(\"a\")\nend\n");
    let app = world.app.clone();
    let out = grenat_in(&app, &["run"]);
    assert_eq!(text(&out.stdout), "[[\"a\"]]\n", "{}", text(&out.stderr));
    write(&app.join("Facetfile"), "facet \"sheets\", path: \"../sheets\"\n");
    let out = grenat_in(&app, &["run"]);
    assert_eq!(out.status.code(), Some(1));
    assert!(
        text(&out.stderr).contains(
            "add `native: true` to its line in the Facetfile: `facet \"sheets\", path: \"../sheets\", native: true`"
        ),
        "{}",
        text(&out.stderr)
    );
    let refused = grenat_package::facets::install(&app, false).unwrap_err();
    assert!(refused.contains("native: true"), "{refused}");
}

#[test]
fn the_facet_passes_its_own_grenat_tests() {
    let dir = Scratch::new("own");
    let facet = facet_copy(&dir.0);
    grenat_package::facets::install(&facet, false).unwrap();
    let out = grenat_in(&facet, &["test"]);
    let err = text(&out.stderr);
    assert_eq!(out.status.code(), Some(0), "{err}");
    assert!(err.ends_with("15 passed, 0 failed\n"), "{err}");
}

#[test]
fn the_facet_crate_passes_its_own_rust_tests() {
    // a temporary directory of their own, to see that they leave nothing in it
    let tmp = Scratch::new("crate-tmp");
    let cargo = std::env::var_os("CARGO").unwrap_or_else(|| "cargo".into());
    let out = Command::new(cargo)
        .args(["test", "--quiet", "--manifest-path"])
        .arg(source().join("native/Cargo.toml"))
        .arg("--target-dir")
        .arg(target_dir())
        .env("TMPDIR", &tmp.0)
        .output()
        .unwrap();
    assert!(out.status.success(), "{}\n{}", text(&out.stdout), text(&out.stderr));
    assert!(text(&out.stdout).contains(" 0 failed"), "{}", text(&out.stdout));
    let left: Vec<_> = std::fs::read_dir(&tmp.0)
        .unwrap()
        .map(|entry| entry.unwrap().file_name().to_string_lossy().into_owned())
        .filter(|name| name.starts_with("sheets-native-tests"))
        .collect();
    assert!(left.is_empty(), "the crate's tests left {left:?}");
}

#[test]
fn a_world_is_removed_when_its_test_ends() {
    let world = world("removed", "require \"sheets\"\n\ndef main\nend\n");
    let dir = world.app.parent().unwrap().to_path_buf();
    assert!(dir.join("sheets").is_dir() && dir.join("app/src/main.grn").is_file());
    drop(world);
    assert!(!dir.exists(), "{} is left behind", dir.display());
    let scratch = Scratch::new("removed-too");
    let dir = scratch.0.clone();
    drop(scratch);
    assert!(!dir.exists());
}

/// The README's examples, run: each printed line is a result the README
/// states (`# …`).
const README_EXAMPLES: &str = r#"require "sheets"

def main uses fs.read, fs.write
  Sheets.write("people.xlsx", [
    Worksheet(name: "People", rows: [["name", "age"], ["Ada", "36"]]),
    Worksheet(name: "Totals", rows: [["checked by Linus"]]),
  ])
  p Sheets.names("people.xlsx")
  p Sheets.rows("people.xlsx")
  p Sheets.records("stock.ods", sheet: "Stock").trust![0]
  p Sheets.parse_csv("name,note\n\"Doe, Jane\",\"said \"\"hi\"\"\"\n")
  records = Sheets.parse_csv_records("name;age\nAda;36\n", delimiter: ";")
  p records
  p Sheets.to_csv([["a", "b, c"]])
  p Sheets.table(records, ["name", "age"])
end
"#;

#[test]
fn the_readme_says_what_its_examples_print() {
    let world = world("readme", README_EXAMPLES);
    let app = world.app.clone();
    std::fs::copy(source().join("tests/fixtures/stock.ods"), app.join("stock.ods")).unwrap();
    let out = grenat_in(&app, &["run"]);
    assert_eq!(out.status.code(), Some(0), "{}", text(&out.stderr));
    let readme = std::fs::read_to_string(source().join("README.md")).unwrap();
    let printed = text(&out.stdout);
    assert_eq!(printed.lines().count(), 7, "{printed}");
    for line in printed.lines() {
        assert!(readme.contains(&format!("# {line}\n")), "the README does not say `# {line}`");
    }
    // the keys of a record come sorted, not in the columns' order (as the README says)
    let keys = "require \"sheets\"\n\ndef main\n  p Sheets.parse_csv_records(\"name,age,city\\nAda,36,London\\n\")[0].keys\nend\n";
    write(&app.join("src/keys.grn"), keys);
    let out = grenat_in(&app, &["run", "src/keys.grn"]);
    assert_eq!(text(&out.stdout), "[\"age\", \"city\", \"name\"]\n", "{}", text(&out.stderr));
}
