//! `grenat.toml`: a package's name, entry points and dependencies.
//!
//! ```toml
//! [package]
//! name = "support"
//! version = "0.1.0"
//! main = "src/main.grn"   # the default; `lib` defaults to "src/lib.grn"
//!
//! [dependencies]
//! utils = { path = "../utils" }
//! http = { git = "https://github.com/grenat-lang/http", tag = "v0.2.0" }
//! ```

use std::path::{Path, PathBuf};

use toml::{Table, Value};

#[derive(Debug, Clone, PartialEq)]
pub struct Manifest {
    pub name: String,
    pub version: String,
    /// The program run by `grenat run` (relative to the package).
    pub main: PathBuf,
    /// What `require "<name>"` loads (relative to the package).
    pub lib: PathBuf,
    pub dependencies: Vec<Dependency>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Dependency {
    pub name: String,
    pub source: Source,
}

#[derive(Debug, Clone, PartialEq)]
pub enum Source {
    /// Relative to the package that declares it.
    Path(PathBuf),
    Git {
        url: String,
        reference: Reference,
    },
}

/// What to check out of a git repository.
#[derive(Debug, Clone, PartialEq)]
pub enum Reference {
    /// The remote's default branch.
    Default,
    Branch(String),
    Tag(String),
    Rev(String),
}

impl Reference {
    /// As written in the lock file.
    pub fn describe(&self) -> String {
        match self {
            Reference::Default => "default".into(),
            Reference::Branch(b) => format!("branch={b}"),
            Reference::Tag(t) => format!("tag={t}"),
            Reference::Rev(r) => format!("rev={r}"),
        }
    }
}

impl Manifest {
    /// The `grenat.toml` of the package in `dir`.
    pub fn load(dir: &Path) -> Result<Manifest, String> {
        let path = dir.join(crate::MANIFEST);
        let text = std::fs::read_to_string(&path).map_err(|e| format!("cannot read {}: {e}", path.display()))?;
        Manifest::parse(&text).map_err(|e| format!("{}: {e}", path.display()))
    }

    pub fn parse(text: &str) -> Result<Manifest, String> {
        let table: Table = text.parse().map_err(|e: toml::de::Error| e.message().to_string())?;
        let package = table.get("package").and_then(Value::as_table).ok_or("missing the `[package]` table")?;
        let field = |name: &str| -> Result<Option<String>, String> {
            match package.get(name) {
                None => Ok(None),
                Some(Value::String(s)) => Ok(Some(s.clone())),
                Some(_) => Err(format!("`package.{name}` must be a string")),
            }
        };
        let name = field("name")?.ok_or("missing `package.name`")?;
        if !valid_name(&name) {
            return Err(format!("invalid package name `{name}`: use lowercase letters, digits and `_`"));
        }
        let mut dependencies = Vec::new();
        if let Some(deps) = table.get("dependencies") {
            let deps = deps.as_table().ok_or("`dependencies` must be a table")?;
            for (dep, spec) in deps {
                dependencies.push(dependency(dep, spec)?);
            }
        }
        Ok(Manifest {
            name,
            version: field("version")?.unwrap_or_else(|| "0.1.0".into()),
            main: field("main")?.unwrap_or_else(|| "src/main.grn".into()).into(),
            lib: field("lib")?.unwrap_or_else(|| "src/lib.grn".into()).into(),
            dependencies,
        })
    }

    pub fn dependency(&self, name: &str) -> Option<&Dependency> {
        self.dependencies.iter().find(|d| d.name == name)
    }
}

fn dependency(name: &str, spec: &Value) -> Result<Dependency, String> {
    if !valid_name(name) {
        return Err(format!("invalid dependency name `{name}`"));
    }
    let spec =
        spec.as_table().ok_or_else(|| format!("dependency `{name}`: expected `{{ path = … }}` or `{{ git = … }}`"))?;
    let string = |key: &str| spec.get(key).and_then(Value::as_str).map(str::to_string);
    for key in spec.keys() {
        if !["path", "git", "branch", "tag", "rev"].contains(&key.as_str()) {
            return Err(format!("dependency `{name}`: unknown key `{key}`"));
        }
    }
    let source = match (string("path"), string("git")) {
        (Some(path), None) => Source::Path(path.into()),
        (None, Some(url)) => {
            let reference = match (string("branch"), string("tag"), string("rev")) {
                (None, None, None) => Reference::Default,
                (Some(b), None, None) => Reference::Branch(b),
                (None, Some(t), None) => Reference::Tag(t),
                (None, None, Some(r)) => Reference::Rev(r),
                _ => return Err(format!("dependency `{name}`: give one of `branch`, `tag` or `rev`")),
            };
            Source::Git { url, reference }
        }
        _ => return Err(format!("dependency `{name}`: give either `path` or `git`")),
    };
    Ok(Dependency { name: name.to_string(), source })
}

pub(crate) fn valid_name(name: &str) -> bool {
    name.starts_with(|c: char| c.is_ascii_lowercase())
        && name.chars().all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_')
}
