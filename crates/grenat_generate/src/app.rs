//! `grenat new --app`: an application, and the file that lists its parts.

use std::path::Path;

use crate::names::Names;
use crate::templates;
use crate::write::{Change, Writer};

/// The file that requires the parts of an application.
pub const APP_FILE: &str = "src/app.grn";

/// Creates the application `name` in the new directory `dir`.
pub fn create_app(dir: &Path, name: &str) -> Result<Vec<Change>, String> {
    let names = Names::new(name).map_err(|e| e.replace("name", "application name"))?;
    if dir.exists() {
        return Err(format!("{} already exists", dir.display()));
    }
    let mut writer = Writer::new(dir);
    let manifest = format!("[package]\nname = \"{name}\"\nversion = \"0.1.0\"\nmain = \"{APP_FILE}\"\n\n[dependencies]\n");
    writer.create(grenat_package::MANIFEST, &manifest)?;
    writer.create(grenat_package::facetfile::FACETFILE, "# The facets this application uses (`setter add <name>`).\n")?;
    writer.create(".gitignore", ".grenat/\ndb/*.db\n")?;
    writer.create("README.md", &templates::fill(templates::README, &names))?;
    writer.create("src/config.grn", templates::CONFIG)?;
    writer.create(APP_FILE, templates::APP)?;
    writer.create("tests/app_test.grn", templates::APP_TEST)?;
    writer.create("db/.keep", "")?;
    Ok(writer.changes)
}

/// `app` with `require "<target>"` after its last `require` (or first),
/// unless it has it already.
pub(crate) fn with_require(app: &str, target: &str) -> String {
    let line = format!("require \"{target}\"");
    let lines: Vec<&str> = app.lines().collect();
    if lines.iter().any(|l| l.trim() == line) {
        return app.to_string();
    }
    let at = lines.iter().rposition(|l| l.starts_with("require ")).map_or(0, |i| i + 1);
    let mut out: Vec<&str> = lines[..at].to_vec();
    out.push(&line);
    out.extend(&lines[at..]);
    out.join("\n") + "\n"
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn requires_are_added_after_the_last_one() {
        let app = "# app\nrequire \"./config\"\n\nget \"/\" do |req|\n  1\nend\n";
        let once = with_require(app, "./agents/triage");
        assert_eq!(once, "# app\nrequire \"./config\"\nrequire \"./agents/triage\"\n\nget \"/\" do |req|\n  1\nend\n");
        assert_eq!(with_require(&once, "./agents/triage"), once);
        assert_eq!(with_require("puts 1\n", "./x"), "require \"./x\"\nputs 1\n");
    }
}
