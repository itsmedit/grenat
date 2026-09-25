//! What `setter` does, command by command.

use std::path::{Path, PathBuf};
use std::process::Command;

use grenat_package::facetfile::{FACETFILE, Facetfile};
use grenat_package::facets::{Installed, install as install_facets, installed, latest};
use grenat_package::version::Requirement;
use grenat_package::{Manifest, find_package};

/// The package of the current directory.
fn root() -> Result<PathBuf, String> {
    match find_package(Path::new("."))? {
        Some(package) => Ok(package.root),
        None => Err(format!("no {} here or above: run it in a package", grenat_package::MANIFEST)),
    }
}

pub fn init() -> Result<String, String> {
    let path = root()?.join(FACETFILE);
    if path.exists() {
        return Err(format!("{} already exists", path.display()));
    }
    std::fs::write(&path, "# The facets this package uses (`setter add <name>`).\n# source \"https://github.com/<org>/facets\"\n")
        .map_err(|e| e.to_string())?;
    Ok(format!("✓ created {}", path.display()))
}

pub fn new(name: &str) -> Result<String, String> {
    grenat_package::create_facet(Path::new(name), name)?;
    Ok(format!("✓ created the facet `{name}`\n  cd {name} && git init && grenat test && setter publish"))
}

/// `add <name> [requirement] [--path dir | --git url [--tag t]]`.
pub fn add(args: &[String]) -> Result<String, String> {
    let root = root()?;
    let name = &args[0];
    let mut requirement: Option<String> = None;
    let mut options: Vec<String> = Vec::new();
    let mut rest = args[1..].iter();
    while let Some(arg) = rest.next() {
        match arg.as_str() {
            flag @ ("--path" | "--git" | "--tag" | "--branch" | "--rev") => {
                let value = rest.next().ok_or_else(|| format!("{flag} expects a value"))?;
                options.push(format!("{}: {}", &flag[2..], quote(value)));
            }
            other if requirement.is_none() => {
                Requirement::parse(other)?;
                requirement = Some(other.to_string());
            }
            other => return Err(format!("unexpected `{other}`")),
        }
    }
    let path = root.join(FACETFILE);
    let text = std::fs::read_to_string(&path).unwrap_or_default();
    if Facetfile::parse(&text)?.facets.iter().any(|f| &f.name == name) {
        return Err(format!("facet `{name}` is already in the Facetfile"));
    }
    // from the indexes, and no version given: compatible with the latest
    let requirement = match (requirement, options.is_empty()) {
        (Some(r), _) => Some(r),
        (None, true) => Some(Requirement::compatible_with(&latest(&root, name)?).to_string()),
        (None, false) => None,
    };
    let mut line = format!("facet {}", quote(name));
    if let Some(r) = &requirement {
        line.push_str(&format!(", {}", quote(r)));
    }
    for option in &options {
        line.push_str(&format!(", {option}"));
    }
    let separator = if text.is_empty() || text.ends_with('\n') { "" } else { "\n" };
    std::fs::write(&path, format!("{text}{separator}{line}\n")).map_err(|e| e.to_string())?;
    let facets = install_facets(&root, false)?;
    Ok(format!("✓ added `{line}`\n{}", describe(&facets)))
}

pub fn install(update: bool) -> Result<String, String> {
    let facets = install_facets(&root()?, update)?;
    Ok(describe(&facets))
}

pub fn list() -> Result<String, String> {
    let facets = installed(&root()?)?;
    if facets.is_empty() {
        return Ok("no facet installed".into());
    }
    Ok(describe(&facets))
}

/// Tags the current version (`v1.2.3`): what indexes list as a version.
pub fn publish() -> Result<String, String> {
    let root = root()?;
    let manifest = Manifest::load(&root)?;
    let tag = format!("v{}", manifest.version);
    if grenat_package::version::Version::parse(&manifest.version).is_none() {
        return Err(format!("version `{}` is not a version such as 1.2.3", manifest.version));
    }
    if !root.join(".git").exists() {
        return Err("this facet is not a git repository: `git init` and commit it first".into());
    }
    let status = git(&root, &["status", "--porcelain"])?;
    if !status.trim().is_empty() {
        return Err("commit your changes first: the tag names the last commit".into());
    }
    if git(&root, &["tag", "--list", &tag])?.trim() == tag {
        return Err(format!("{tag} is already tagged: raise the version in grenat.toml"));
    }
    git(&root, &["tag", "-a", &tag, "-m", &format!("{} {}", manifest.name, manifest.version)])?;
    Ok(format!(
        "✓ tagged {tag}\n  git push origin {tag}\n  then, once, list the facet in an index: facets/{}.toml with git = \"<this repository's URL>\"",
        manifest.name
    ))
}

fn describe(facets: &[Installed]) -> String {
    let mut lines = vec![format!("✓ {} facet(s):", facets.len())];
    for f in facets {
        let version = f.version.map(|v| v.to_string()).unwrap_or_else(|| "-".into());
        lines.push(format!("  {} {version} ({})", f.name, f.source));
    }
    lines.join("\n")
}

fn quote(text: &str) -> String {
    format!("\"{}\"", text.replace('\\', "\\\\").replace('"', "\\\""))
}

fn git(dir: &Path, args: &[&str]) -> Result<String, String> {
    let out = Command::new("git").args(args).current_dir(dir).output().map_err(|e| format!("cannot run git: {e}"))?;
    if out.status.success() {
        Ok(String::from_utf8_lossy(&out.stdout).into_owned())
    } else {
        Err(String::from_utf8_lossy(&out.stderr).trim().to_string())
    }
}
