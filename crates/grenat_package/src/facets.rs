//! Installing facets: the versions a `Facetfile` asks for, found in its
//! indexes, fetched into `.grenat/facets/`, pinned in `Facetfile.lock`.
//!
//! An index is a git repository with a `facets/<name>.toml` per facet
//! (`git = "<repository>"`); a facet's versions are its repository's tags
//! (`v1.2.3`). An entry may also name the facet's directory in the
//! repository (`path = "facets/html"`) and what its tags start with
//! (`tag_prefix = "html-v"`: the tags `html-v0.1.0`…), for a facet that
//! shares its repository. A facet's own `Facetfile` adds its facets, all
//! of them in one namespace: two requirements on a facet must agree on one
//! version.
//! A facet that ships Rust code is built too, and one that ships a bridge
//! asked to describe itself, if the application trusts it (see
//! [`crate::trust`], [`crate::native`], [`crate::bridge`]).

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
    let file =
        Facetfile::load(root)?.ok_or_else(|| format!("no {} in {}", crate::facetfile::FACETFILE, root.display()))?;
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
    // code outside Grenat: trusted by the application before anything is recorded, built or started
    let dirs: Vec<(String, PathBuf)> = facets.iter().map(|f| (f.name.clone(), absolute(root, &f.dir))).collect();
    crate::trust::check(root, &dirs)?;
    save(root, &facets)?;
    for (name, dir) in &dirs {
        crate::native::build(name, dir)?;
        crate::bridge::install(name, dir)?;
    }
    // the package's own native part or bridge, if it is such a facet
    let name = Manifest::load(root)?.name;
    crate::native::build(&name, root)?;
    crate::bridge::install(&name, root)?;
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
        Origin::Git { url, reference, path } => {
            let clone = facets_dir.join(format!("{name}-{}", slug(&reference.describe())));
            let pinned = locked.filter(|l| l.source == format!("git:{url}")).and_then(|l| l.commit.clone());
            let commit = git::fetch(url, reference, pinned.as_deref(), &root.join(&clone))?;
            let dir = inside(root, &clone, path.as_deref(), || {
                format!("facet `{name}`: {} of {url} (commit {})", reference.describe(), short(&commit))
            })?;
            let version = Manifest::load(&root.join(&dir)).ok().and_then(|m| Version::parse(&m.version));
            Ok(Installed { name: name.into(), version, source: format!("git:{url}"), commit: Some(commit), dir })
        }
        Origin::Index => {
            let entry = index_entry(root, name, sources)?;
            let tags = entry.versions()?;
            if tags.is_empty() {
                return Err(entry.no_version(name));
            }
            let preferred = locked.and_then(|l| l.version).filter(|v| requirements.iter().all(|r| r.matches(v)));
            let version = match preferred.filter(|v| tags.iter().any(|(t, ..)| t == v)) {
                Some(v) => v,
                None => highest(tags.iter().map(|(v, ..)| v), requirements).ok_or_else(|| {
                    let asked: Vec<String> = requirements.iter().map(ToString::to_string).collect();
                    format!("no version of `{name}` satisfies {}", asked.join(" and "))
                })?,
            };
            let (_, tag, commit) = tags.iter().find(|(v, ..)| *v == version).expect("chosen among them");
            let clone = facets_dir.join(format!("{name}-{version}"));
            if !root.join(&clone).join(".git").is_dir() {
                git::fetch(&entry.git, &Reference::Tag(tag.clone()), Some(commit), &root.join(&clone))?;
            }
            let dir = inside(root, &clone, entry.path.as_deref(), || {
                format!("facet `{name}`: {tag} of {} (commit {}), as {} says,", entry.git, short(commit), entry.shown)
            })?;
            Ok(Installed {
                name: name.into(),
                version: Some(version),
                source: format!("index:{}", entry.git),
                commit: Some(commit.clone()),
                dir,
            })
        }
    }
}

/// The facet's directory: the clone, or the directory `path` in it, which
/// must exist (`what` names the clone in the error).
fn inside(root: &Path, clone: &Path, path: Option<&Path>, what: impl Fn() -> String) -> Result<PathBuf, String> {
    let Some(path) = path else { return Ok(clone.to_path_buf()) };
    let dir = clone.join(path);
    if root.join(&dir).is_dir() { Ok(dir) } else { Err(format!("{} has no directory `{}`", what(), path.display())) }
}

/// A commit's first characters.
fn short(commit: &str) -> &str {
    commit.get(..12).unwrap_or(commit)
}

/// The latest version of `name` in the indexes of the package in `root`.
pub fn latest(root: &Path, name: &str) -> Result<Version, String> {
    let sources = Facetfile::load(root)?.map(|f| f.sources).unwrap_or_default();
    let entry = index_entry(root, name, &sources)?;
    entry.versions()?.into_iter().map(|(v, ..)| v).max().ok_or_else(|| entry.no_version(name))
}

/// A facet's entry in an index: `facets/<name>.toml`.
struct Entry {
    /// The facet's repository (`git = "…"`).
    git: String,
    /// Its directory in the repository (`path = "facets/html"`), or its root.
    path: Option<PathBuf>,
    /// What its version tags start with (`tag_prefix = "html-v"`); without
    /// one, `v1.2.3` (or `1.2.3`).
    tag_prefix: Option<String>,
    /// The entry, for errors: `facets/html.toml of <index>`.
    shown: String,
}

impl Entry {
    fn parse(table: &Table, shown: String) -> Result<Option<Entry>, String> {
        let text = |key: &str| -> Result<Option<String>, String> {
            match table.get(key) {
                None => Ok(None),
                Some(Value::String(s)) => Ok(Some(s.clone())),
                Some(_) => Err(format!("{shown}: `{key}` must be a string")),
            }
        };
        let Some(git) = text("git")? else { return Ok(None) };
        let path = text("path")?.map(|p| subdirectory(&p).map_err(|e| format!("{shown}: `path` {e}"))).transpose()?;
        let tag_prefix = text("tag_prefix")?;
        if let Some(prefix) = &tag_prefix
            && (prefix.is_empty() || prefix.chars().any(char::is_whitespace))
        {
            return Err(format!("{shown}: `tag_prefix` must be non-empty and without spaces (`{prefix}`)"));
        }
        Ok(Some(Entry { git, path, tag_prefix, shown }))
    }

    /// The facet's versions: (version, tag, commit), from the repository's
    /// tags with the entry's prefix (others ignored).
    fn versions(&self) -> Result<Vec<(Version, String, String)>, String> {
        Ok(git::tags(&self.git)?
            .into_iter()
            .filter_map(|(tag, commit)| Some((self.version(&tag)?, tag, commit)))
            .collect())
    }

    fn version(&self, tag: &str) -> Option<Version> {
        match &self.tag_prefix {
            None => Version::parse(tag),
            // the version right after the prefix: `html-v0.1.0`, not `html-vv0.1.0`
            Some(prefix) => tag
                .strip_prefix(prefix.as_str())
                .filter(|v| v.starts_with(|c: char| c.is_ascii_digit()))
                .and_then(Version::parse),
        }
    }

    fn no_version(&self, name: &str) -> String {
        let prefix = self.tag_prefix.as_deref().unwrap_or("v");
        format!("facet `{name}` has no version: no tag of {} such as {prefix}1.0.0 ({})", self.git, self.shown)
    }
}

/// A directory inside a repository, as an index entry or a `Facetfile`
/// writes it: relative, without `..`.
pub(crate) fn subdirectory(text: &str) -> Result<PathBuf, String> {
    use std::path::Component;
    let path = PathBuf::from(text);
    if text.trim().is_empty() {
        return Err("must name a directory (it is empty)".into());
    }
    for part in path.components() {
        match part {
            Component::Normal(_) | Component::CurDir => {}
            Component::ParentDir => return Err(format!("must stay inside the repository: no `..` (`{text}`)")),
            Component::RootDir | Component::Prefix(_) => {
                return Err(format!("must be relative to the repository's root (`{text}`)"));
            }
        }
    }
    Ok(path)
}

/// The entry of `name`, from the first index that lists it.
fn index_entry(root: &Path, name: &str, sources: &[String]) -> Result<Entry, String> {
    if sources.is_empty() {
        return Err(format!("facet `{name}` has no `path:` nor `git:`, and the Facetfile has no `source`"));
    }
    for source in sources {
        let dir = root.join(".grenat").join("index").join(slug(source));
        git::sync(source, &dir)?;
        let file = format!("facets/{name}.toml");
        let entry = dir.join(&file);
        if let Ok(text) = std::fs::read_to_string(&entry) {
            let table: Table =
                text.parse().map_err(|e: toml::de::Error| format!("{}: {}", entry.display(), e.message()))?;
            if let Some(entry) = Entry::parse(&table, format!("the index entry {file} of {source}"))? {
                return Ok(entry);
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

#[cfg(test)]
mod tests {
    use super::*;

    fn entry(text: &str) -> Result<Option<Entry>, String> {
        Entry::parse(&text.parse().unwrap(), "the entry".into())
    }

    #[test]
    fn the_official_entries_name_a_directory_and_a_prefix() {
        for (name, text) in [
            ("html", include_str!("../../../facets/index/facets/html.toml")),
            ("sheets", include_str!("../../../facets/index/facets/sheets.toml")),
        ] {
            let e = entry(text).unwrap().unwrap();
            assert_eq!(e.git, "https://github.com/itsmedit/grenat");
            assert_eq!(e.path, Some(PathBuf::from(format!("facets/{name}"))));
            assert_eq!(e.version(&format!("{name}-v0.1.0")).map(|v| v.to_string()).as_deref(), Some("0.1.0"));
            // Grenat's own releases are not the facet's versions
            assert_eq!(e.version("v0.1.5"), None);
            assert_eq!(e.version(&format!("{name}-vv0.1.0")), None);
        }
    }

    #[test]
    fn an_entry_is_checked() {
        assert!(entry("path = \"x\"\n").unwrap().is_none());
        let plain = entry("git = \"r\"\n").unwrap().unwrap();
        assert_eq!(plain.path, None);
        // without a prefix, as before: v1.2.0 or 1.2.0
        assert_eq!(plain.version("v1.2.0"), Version::parse("1.2.0"));
        assert_eq!(plain.version("1.2.0"), Version::parse("1.2.0"));
        let err = |text: &str| entry(text).err().unwrap();
        assert_eq!(err("git = \"r\"\npath = 3\n"), "the entry: `path` must be a string");
        assert_eq!(err("git = \"r\"\npath = \"\"\n"), "the entry: `path` must name a directory (it is empty)");
        assert_eq!(
            err("git = \"r\"\npath = \"a/../../b\"\n"),
            "the entry: `path` must stay inside the repository: no `..` (`a/../../b`)"
        );
        assert_eq!(
            err("git = \"r\"\npath = \"/a\"\n"),
            "the entry: `path` must be relative to the repository's root (`/a`)"
        );
        assert_eq!(
            err("git = \"r\"\ntag_prefix = \"\"\n"),
            "the entry: `tag_prefix` must be non-empty and without spaces (``)"
        );
    }
}
