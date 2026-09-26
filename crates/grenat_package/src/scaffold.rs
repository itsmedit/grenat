//! `grenat new`: a new package.

use std::path::Path;

/// Creates the package `name` in the new directory `dir`.
pub fn create(dir: &Path, name: &str) -> Result<(), String> {
    if !crate::manifest::valid_name(name) {
        return Err(format!("invalid package name `{name}`: use lowercase letters, digits and `_`"));
    }
    if dir.exists() {
        return Err(format!("{} already exists", dir.display()));
    }
    let files = [
        (crate::MANIFEST.to_string(), format!("[package]\nname = \"{name}\"\nversion = \"0.1.0\"\n\n[dependencies]\n")),
        (".gitignore".into(), ".grenat/\n".into()),
        (crate::facetfile::FACETFILE.to_string(), FACETFILE_TEMPLATE.to_string()),
        ("src/main.grn".into(), MAIN.into()),
        ("src/lib.grn".into(), LIB.into()),
        ("tests/lib_test.grn".into(), TEST.into()),
    ];
    for (path, text) in files {
        let path = dir.join(path);
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).map_err(|e| format!("cannot create {}: {e}", parent.display()))?;
        }
        std::fs::write(&path, text).map_err(|e| format!("cannot write {}: {e}", path.display()))?;
    }
    Ok(())
}

const MAIN: &str = "\
require \"./lib\"

def main(args: Array(String))
  puts greeting(args.first || \"world\")
end
";

const LIB: &str = "\
## A friendly greeting.
def greeting(name: String) -> String
  \"Hello, #{name}!\"
end
";

const TEST: &str = "\
require \"../src/lib\"

test \"greets by name\" do
  assert_equal \"Hello, Ada!\", greeting(\"Ada\")
end
";

/// Creates the facet (a library) `name` in the new directory `dir`.
pub fn create_facet(dir: &Path, name: &str) -> Result<(), String> {
    if !crate::manifest::valid_name(name) {
        return Err(format!("invalid facet name `{name}`: use lowercase letters, digits and `_`"));
    }
    if dir.exists() {
        return Err(format!("{} already exists", dir.display()));
    }
    let files = [
        (crate::MANIFEST.to_string(), format!("[package]\nname = \"{name}\"\nversion = \"0.1.0\"\n")),
        (crate::facetfile::FACETFILE.to_string(), FACETFILE_TEMPLATE.to_string()),
        (".gitignore".into(), ".grenat/\n".into()),
        (
            "README.md".into(),
            format!("# {name}\n\nA Grenat facet.\n\n```ruby\n# Facetfile\nfacet \"{name}\", \"~> 0.1\"\n```\n"),
        ),
        ("src/lib.grn".into(), LIB.into()),
        ("tests/lib_test.grn".into(), TEST.into()),
    ];
    for (path, text) in files {
        let path = dir.join(path);
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).map_err(|e| format!("cannot create {}: {e}", parent.display()))?;
        }
        std::fs::write(&path, text).map_err(|e| format!("cannot write {}: {e}", path.display()))?;
    }
    Ok(())
}

const FACETFILE_TEMPLATE: &str = "\
# The facets this facet uses (`setter add <name>`).
# source \"https://github.com/<org>/facets\"
";
