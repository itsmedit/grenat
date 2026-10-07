//! A scan: the files under a pattern's base that match it, each with its
//! [`Stamp`] — and those refused.
//!
//! Directories are gone through as deep as the pattern needs, never through
//! a link. A link to a file is followed only when its target is inside the
//! base (both resolved): one leading out of the watched directory
//! (`inbox/a.pdf -> /etc/passwd`) is refused, as is a name that is not
//! UTF-8 (no program could name it back). What cannot be read below the
//! base is left out; the base itself unreadable fails the scan.

use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::time::SystemTime;

use crate::Glob;

/// What tells one state of a file from the next without reading it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Stamp {
    pub size: u64,
    pub modified: Option<SystemTime>,
}

impl Stamp {
    fn of(metadata: &fs::Metadata) -> Stamp {
        Stamp { size: metadata.len(), modified: metadata.modified().ok() }
    }
}

/// A file that matches, by its path as the pattern writes it (`inbox/a.pdf`).
#[derive(Debug, Clone, PartialEq)]
pub struct Found {
    pub path: PathBuf,
    pub stamp: Stamp,
}

/// A path that matches but is never handed over, and why.
#[derive(Debug, Clone, PartialEq)]
pub struct Refused {
    pub path: PathBuf,
    pub why: String,
}

#[derive(Debug, Default)]
pub struct Scan {
    pub found: Vec<Found>,
    pub refused: Vec<Refused>,
}

/// Scans the files `glob` matches.
pub fn scan(glob: &Glob) -> io::Result<Scan> {
    let base = glob.base();
    let inside = fs::canonicalize(base)?;
    if !inside.is_dir() {
        return Err(io::Error::new(io::ErrorKind::NotADirectory, format!("{} is not a directory", base.display())));
    }
    let mut out = Scan::default();
    let mut names = Vec::new();
    walk(glob, base, &inside, &mut names, &mut out, true)?;
    out.found.sort_by(|a, b| a.path.cmp(&b.path));
    Ok(out)
}

fn walk(glob: &Glob, dir: &Path, inside: &Path, names: &mut Vec<String>, out: &mut Scan, top: bool) -> io::Result<()> {
    let entries = match fs::read_dir(dir) {
        Ok(entries) => entries,
        Err(e) if top => return Err(e),
        Err(_) => return Ok(()),
    };
    for entry in entries.flatten() {
        // `a.pdf` for `*.pdf`, not `./a.pdf`
        let path = if dir == Path::new(".") { PathBuf::from(entry.file_name()) } else { dir.join(entry.file_name()) };
        let Ok(kind) = entry.file_type() else { continue };
        let Some(name) = entry.file_name().to_str().map(str::to_string) else {
            if kind.is_file() || kind.is_symlink() {
                let why = "its name is not UTF-8".to_string();
                out.refused.push(Refused { path, why });
            }
            continue;
        };
        names.push(name);
        let relative: Vec<&str> = names.iter().map(String::as_str).collect();
        if kind.is_dir() {
            if glob.may_enter(&relative) {
                walk(glob, &path, inside, names, out, false)?;
            }
        } else if glob.matches_names(&relative) {
            if kind.is_symlink() {
                match fs::canonicalize(&path) {
                    Ok(target) if !target.starts_with(inside) => {
                        let why = "a link leading out of the watched directory".to_string();
                        out.refused.push(Refused { path, why });
                    }
                    Ok(target) => {
                        // a link to a directory is never gone through
                        if let Ok(metadata) = fs::metadata(&target)
                            && metadata.is_file()
                        {
                            out.found.push(Found { path, stamp: Stamp::of(&metadata) });
                        }
                    }
                    // dangling: nothing to read
                    Err(_) => {}
                }
            } else if kind.is_file()
                && let Ok(metadata) = entry.metadata()
            {
                out.found.push(Found { path, stamp: Stamp::of(&metadata) });
            }
        }
        names.pop();
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("grenat-watch-scan-{}-{name}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn found(glob: &str) -> Vec<String> {
        let scan = scan(&Glob::parse(glob).unwrap()).unwrap();
        scan.found.iter().map(|f| f.path.to_string_lossy().into_owned()).collect()
    }

    #[test]
    fn matching_files_with_their_stamps() {
        let dir = temp("match");
        fs::create_dir_all(dir.join("a/b")).unwrap();
        fs::create_dir_all(dir.join(".cache")).unwrap();
        for f in ["x.pdf", "y.txt", "a/z.pdf", "a/b/w.pdf", ".cache/h.pdf", ".hidden.pdf"] {
            fs::write(dir.join(f), f).unwrap();
        }
        let d = dir.display();
        assert_eq!(found(&format!("{d}/*.pdf")), [format!("{d}/x.pdf")]);
        assert_eq!(
            found(&format!("{d}/**/*.pdf")),
            [format!("{d}/a/b/w.pdf"), format!("{d}/a/z.pdf"), format!("{d}/x.pdf")]
        );
        assert_eq!(found(&format!("{d}/*/*.pdf")), [format!("{d}/a/z.pdf")]);
        assert_eq!(found(&format!("{d}/.cache/*.pdf")), [format!("{d}/.cache/h.pdf")]);
        let scan = scan(&Glob::parse(&format!("{d}/x.pdf")).unwrap()).unwrap();
        assert_eq!(scan.found[0].stamp.size, 5);
        assert!(scan.found[0].stamp.modified.is_some());
    }

    #[test]
    fn a_missing_base_fails_the_scan() {
        let dir = temp("missing");
        assert!(scan(&Glob::parse(&format!("{}/nope/*.pdf", dir.display())).unwrap()).is_err());
        fs::write(dir.join("file"), "x").unwrap();
        assert!(scan(&Glob::parse(&format!("{}/file/*.pdf", dir.display())).unwrap()).is_err());
    }

    #[cfg(unix)]
    #[test]
    fn links_stay_inside() {
        let dir = temp("links");
        let outside = temp("links-outside");
        fs::write(outside.join("secret.pdf"), "secret").unwrap();
        fs::create_dir_all(dir.join("inbox/sub")).unwrap();
        fs::write(dir.join("inbox/real.pdf"), "real").unwrap();
        fs::write(dir.join("inbox/sub/deep.pdf"), "deep").unwrap();
        std::os::unix::fs::symlink(outside.join("secret.pdf"), dir.join("inbox/escape.pdf")).unwrap();
        std::os::unix::fs::symlink(dir.join("inbox/real.pdf"), dir.join("inbox/alias.pdf")).unwrap();
        std::os::unix::fs::symlink(dir.join("inbox/../inbox/real.pdf"), dir.join("inbox/dotted.pdf")).unwrap();
        std::os::unix::fs::symlink(dir.join("inbox/gone"), dir.join("inbox/dangling.pdf")).unwrap();
        std::os::unix::fs::symlink(&outside, dir.join("inbox/out")).unwrap();
        let glob = Glob::parse(&format!("{}/inbox/**/*.pdf", dir.display())).unwrap();
        let result = scan(&glob).unwrap();
        let names: Vec<_> =
            result.found.iter().map(|f| f.path.file_name().unwrap().to_string_lossy().into_owned()).collect();
        assert_eq!(names, ["alias.pdf", "dotted.pdf", "real.pdf", "deep.pdf"]);
        assert_eq!(result.refused.len(), 1, "{:?}", result.refused);
        assert!(result.refused[0].path.ends_with("escape.pdf"));
        assert!(result.refused[0].why.contains("leading out"));
    }
}
