//! Installing facets: the versions a `Facetfile` asks for, found in its
//! indexes, fetched into `.grenat/facets/`, pinned in `Facetfile.lock`.
//!
//! An index is a git repository with a `facets/<name>.toml` per facet
//! (`git = "<repository>"`); a facet's versions are its repository's tags
//! (`v1.2.3`). A facet's own `Facetfile` adds its facets, all of them in
//! one namespace: two requirements on a facet must agree on one version.

use std::collections::{BTreeMap, HashMap};
use std::path::{Path, PathBuf};

use toml::{Table, Value};

use crate::facetfile::{FacetDecl, Facetfile, Origin};
use crate::git;
use crate::manifest::{Manifest, Reference};
use crate::version::{Requirement, Version, highest};

/// The lock file of facets, next to the `Facetfile`.
pub const FACET_LOCK: &str = "Facetfile.lock";

/// A facet as installed.
#[derive(Debug, Clone, PartialEq)]
pub struct Installed {
    pub name: String,
    pub version: Option<Version>,
    /// `index:<repository>`, `git:<repository>` or `path:<directory>`.
    pub source: String,
    pub commit: Option<String>,
    /// Where it is (relative to the root package, unless absolute).
    pub dir: PathBuf,
}

/// Installs the facets of the package in `root`; `update` ignores the lock
/// (the latest versions allowed). Returns them, sorted by name.
pub fn install(root: &Path, update: bool) -> Result<Vec<Installed>, String> {
    let file = Facetfile::load(root)?.ok_or_else(|| format!("no {} in {}", crate::facetfile::FACETFILE, root.display()))?;
    let locked: HashMap<String, Installed> =
        if update { HashMap::new() } else { installed(root)?.into_iter().map(|i| (i.name.clone(), i)).collect() };
    let mut queue: Vec<(FacetDecl, PathBuf, Vec<String>)> =
        file.facets.iter().map(|d| (d.clone(), root.to_path_buf(), file.sources.clone())).collect();
    let mut requirements: HashMap<String, Vec<Requirement>> = HashMap::new();
    let mut origins: HashMap<String, Origin> = HashMap::new();
    let mut chosen: BTreeMap<String, Installed> = BTreeMap::new();
    while let Some((decl, base, sources)) = queue.pop() {
        let origin = match &decl.origin {
            Origin::Path(p) => Origin::Path(base.join(p)),
            other => other.clone(),
        };
        match origins.get(&decl.name) {
            Some(known) if *known != origin => return Err(format!("facet `{}` comes from two places", decl.name)),
            _ => origins.insert(decl.name.clone(), origin.clone()),
        };
        let reqs = requirements.entry(decl.name.clone()).or_default();
        reqs.push(decl.requirement.clone());
        let reqs = reqs.clone();
        if let Some(done) = chosen.get(&decl.name)
            && done.version.is_none_or(|v| reqs.iter().all(|r| r.matches(&v)))
        {
            continue;
        }
        let facet = fetch(root, &decl.name, &origin, &reqs, &sources, locked.get(&decl.name))?;
        let dir = absolute(root, &facet.dir);
        check_name(&dir, &decl.name)?;
        if let Some(inner) = Facetfile::load(&dir)? {
            let inner_sources = if inner.sources.is_empty() { sources.clone() } else { inner.sources.clone() };
            queue.extend(inner.facets.into_iter().map(|d| (d, dir.clone(), inner_sources.clone())));
        }
        chosen.insert(decl.name.clone(), facet);
    }
    let facets: Vec<Installed> = chosen.into_values().collect();
    save(root, &facets)?;
    Ok(facets)
}

/// The facets `Facetfile.lock` records (none without one).
pub fn installed(root: &Path) -> Result<Vec<Installed>, String> {
    let path = root.join(FACET_LOCK);
    let text = match std::fs::read_to_string(&path) {
        Ok(text) => text,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(e) => return Err(format!("cannot read {}: {e}", path.display())),
    };
    let table: Table = text.parse().map_err(|e: toml::de::Error| format!("{}: {}", path.display(), e.message()))?;
    let mut facets = Vec::new();
    for entry in table.get("facet").and_then(Value::as_array).cloned().unwrap_or_default() {
        let get = |key: &str| entry.get(key).and_then(Value::as_str).map(str::to_string);
        let (Some(name), Some(source), Some(dir)) = (get("name"), get("source"), get("dir")) else {
            return Err(format!("{}: each `[[facet]]` needs `name`, `source` and `dir`", path.display()));
        };
        facets.push(Installed {
            name,
            version: get("version").as_deref().and_then(Version::parse),
            source,
            commit: get("commit"),
            dir: dir.into(),
        });
    }
    Ok(facets)
}

fn save(root: &Path, facets: &[Installed]) -> Result<(), String> {
    let quote = |s: &str| Value::String(s.to_string()).to_string();
    let mut text = String::from("# Written by setter: the facets installed, and exactly which.\n");
    for f in facets {
        text.push_str(&format!("\n[[facet]]\nname = {}\n", quote(&f.name)));
        if let Some(v) = &f.version {
            text.push_str(&format!("version = {}\n", quote(&v.to_string())));
        }
        text.push_str(&format!("source = {}\n", quote(&f.source)));
        if let Some(c) = &f.commit {
            text.push_str(&format!("commit = {}\n", quote(c)));
        }
        text.push_str(&format!("dir = {}\n", quote(&f.dir.to_string_lossy())));
    }
    let path = root.join(FACET_LOCK);
    std::fs::write(&path, text).map_err(|e| format!("cannot write {}: {e}", path.display()))
}

/// Fetches the facet `name` (unless already there): its directory, relative to `root`.
fn fetch(
    root: &Path,
    name: &str,
    origin: &Origin,
    requirements: &[Requirement],
    sources: &[String],
    locked: Option<&Installed>,
) -> Result<Installed, String> {
    let facets_dir = PathBuf::from(".grenat").join("facets");
    match origin {
        Origin::Path(dir) => {
            let dir = dir.canonicalize().map_err(|e| format!("facet `{name}`: cannot find {}: {e}", dir.display()))?;
            let version = Manifest::load(&dir).ok().and_then(|m| Version::parse(&m.version));
            // relative to the root package: the lock file is shared
            let dir = relative(&root.canonicalize().unwrap_or(root.to_path_buf()), &dir);
            Ok(Installed { name: name.into(), version, source: format!("path:{}", dir.display()), commit: None, dir })
        }
        Origin::Git { url, reference } => {
            let dir = facets_dir.join(format!("{name}-{}", slug(&reference.describe())));
            let pinned = locked.filter(|l| l.source == format!("git:{url}")).and_then(|l| l.commit.clone());
            let commit = git::fetch(url, reference, pinned.as_deref(), &root.join(&dir))?;
            let version = Manifest::load(&root.join(&dir)).ok().and_then(|m| Version::parse(&m.version));
            Ok(Installed { name: name.into(), version, source: format!("git:{url}"), commit: Some(commit), dir })
        }
        Origin::Index => {
            let url = index_entry(root, name, sources)?;
            let tags: Vec<(Version, String, String)> =
                git::tags(&url)?.into_iter().filter_map(|(tag, commit)| Some((Version::parse(&tag)?, tag, commit))).collect();
            let preferred = locked.and_then(|l| l.version).filter(|v| requirements.iter().all(|r| r.matches(v)));
            let version = match preferred.filter(|v| tags.iter().any(|(t, ..)| t == v)) {
                Some(v) => v,
                None => highest(tags.iter().map(|(v, ..)| v), requirements).ok_or_else(|| {
                    let asked: Vec<String> = requirements.iter().map(ToString::to_string).collect();
                    format!("no version of `{name}` satisfies {}", asked.join(" and "))
                })?,
            };
            let (_, tag, commit) = tags.iter().find(|(v, ..)| *v == version).expect("chosen among them");
            let dir = facets_dir.join(format!("{name}-{version}"));
            if !root.join(&dir).join(".git").is_dir() {
                git::fetch(&url, &Reference::Tag(tag.clone()), Some(commit), &root.join(&dir))?;
            }
            Ok(Installed {
                name: name.into(),
                version: Some(version),
                source: format!("index:{url}"),
                commit: Some(commit.clone()),
                dir,
            })
        }
    }
}

/// The latest version of `name` in the indexes of the package in `root`.
pub fn latest(root: &Path, name: &str) -> Result<Version, String> {
    let sources = Facetfile::load(root)?.map(|f| f.sources).unwrap_or_default();
    let url = index_entry(root, name, &sources)?;
    git::tags(&url)?
        .iter()
        .filter_map(|(tag, _)| Version::parse(tag))
        .max()
        .ok_or_else(|| format!("facet `{name}` has no version (no tag such as v1.0.0)"))
}

/// The repository of `name`, from the first index that lists it.
fn index_entry(root: &Path, name: &str, sources: &[String]) -> Result<String, String> {
    if sources.is_empty() {
        return Err(format!("facet `{name}` has no `path:` nor `git:`, and the Facetfile has no `source`"));
    }
    for source in sources {
        let dir = root.join(".grenat").join("index").join(slug(source));
        git::sync(source, &dir)?;
        let entry = dir.join("facets").join(format!("{name}.toml"));
        if let Ok(text) = std::fs::read_to_string(&entry) {
            let table: Table = text.parse().map_err(|e: toml::de::Error| format!("{}: {}", entry.display(), e.message()))?;
            if let Some(url) = table.get("git").and_then(Value::as_str) {
                return Ok(url.to_string());
            }
        }
    }
    Err(format!("facet `{name}` is in none of the indexes ({})", sources.join(", ")))
}

fn check_name(dir: &Path, name: &str) -> Result<(), String> {
    let manifest = Manifest::load(dir)?;
    if manifest.name == name {
        Ok(())
    } else {
        Err(format!("the facet in {} is named `{}`, not `{name}`", dir.display(), manifest.name))
    }
}

fn absolute(root: &Path, dir: &Path) -> PathBuf {
    if dir.is_absolute() { dir.to_path_buf() } else { root.join(dir) }
}

/// `to`, relative to `from` (both absolute).
fn relative(from: &Path, to: &Path) -> PathBuf {
    let (from, to): (Vec<_>, Vec<_>) = (from.components().collect(), to.components().collect());
    let common = from.iter().zip(&to).take_while(|(a, b)| a == b).count();
    let mut path = PathBuf::new();
    for _ in common..from.len() {
        path.push("..");
    }
    for part in &to[common..] {
        path.push(part);
    }
    if path.as_os_str().is_empty() { PathBuf::from(".") } else { path }
}

/// A name safe for a directory.
fn slug(text: &str) -> String {
    text.chars().map(|c| if c.is_ascii_alphanumeric() || c == '.' || c == '-' { c } else { '_' }).collect()
}
