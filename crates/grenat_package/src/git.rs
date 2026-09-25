//! Fetching git dependencies, with the `git` command.

use std::path::Path;
use std::process::Command;

use crate::manifest::Reference;

/// Clones `url` into `dir` (replacing it) and checks out `commit`, or else
/// `reference`; returns the commit checked out.
pub(crate) fn fetch(url: &str, reference: &Reference, commit: Option<&str>, dir: &Path) -> Result<String, String> {
    if dir.exists() {
        std::fs::remove_dir_all(dir).map_err(|e| format!("cannot remove {}: {e}", dir.display()))?;
    }
    if let Some(parent) = dir.parent() {
        std::fs::create_dir_all(parent).map_err(|e| format!("cannot create {}: {e}", parent.display()))?;
    }
    let target = dir.to_string_lossy().into_owned();
    let mut clone = vec!["clone", "--quiet"];
    if let (None, Reference::Branch(name) | Reference::Tag(name)) = (commit, reference) {
        clone.extend(["--depth", "1", "--branch", name]);
    }
    clone.extend([url, target.as_str()]);
    git(&clone, None)?;
    let checkout = match (commit, reference) {
        (Some(commit), _) => Some(commit),
        (None, Reference::Rev(rev)) => Some(rev.as_str()),
        _ => None,
    };
    if let Some(what) = checkout {
        git(&["checkout", "--quiet", what], Some(dir))?;
    }
    head(dir)
}

/// The commit checked out in `dir`.
pub(crate) fn head(dir: &Path) -> Result<String, String> {
    git(&["rev-parse", "HEAD"], Some(dir)).map(|out| out.trim().to_string())
}

fn git(args: &[&str], dir: Option<&Path>) -> Result<String, String> {
    let mut command = Command::new("git");
    command.args(args).env("GIT_TERMINAL_PROMPT", "0");
    if let Some(dir) = dir {
        command.current_dir(dir);
    }
    let out = command.output().map_err(|e| format!("cannot run git: {e}"))?;
    if out.status.success() {
        Ok(String::from_utf8_lossy(&out.stdout).into_owned())
    } else {
        Err(format!("`git {}` failed: {}", args.join(" "), String::from_utf8_lossy(&out.stderr).trim()))
    }
}

/// The tags of the repository at `url`: (tag, commit), a tag object
/// resolved to the commit it names.
pub(crate) fn tags(url: &str) -> Result<Vec<(String, String)>, String> {
    let listing = git(&["ls-remote", "--tags", url], None)?;
    let mut tags: Vec<(String, String)> = Vec::new();
    for line in listing.lines() {
        let Some((commit, reference)) = line.split_once('\t') else { continue };
        let Some(tag) = reference.strip_prefix("refs/tags/") else { continue };
        match tag.strip_suffix("^{}") {
            // an annotated tag: the commit it points to
            Some(tag) => match tags.iter_mut().find(|(t, _)| t == tag) {
                Some(entry) => entry.1 = commit.to_string(),
                None => tags.push((tag.to_string(), commit.to_string())),
            },
            None if !tags.iter().any(|(t, _)| t == tag) => tags.push((tag.to_string(), commit.to_string())),
            None => {}
        }
    }
    Ok(tags)
}

/// A copy of the repository at `url` in `dir`, brought up to date.
pub(crate) fn sync(url: &str, dir: &Path) -> Result<(), String> {
    if dir.join(".git").is_dir() {
        return git(&["pull", "--quiet", "--ff-only"], Some(dir)).map(drop);
    }
    if let Some(parent) = dir.parent() {
        std::fs::create_dir_all(parent).map_err(|e| format!("cannot create {}: {e}", parent.display()))?;
    }
    git(&["clone", "--quiet", "--depth", "1", url, &dir.to_string_lossy()], None).map(drop)
}
