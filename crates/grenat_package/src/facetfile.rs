//! The `Facetfile`: which facets (libraries) a package uses, in Grenat.
//!
//! ```ruby
//! source "https://github.com/grenat-lang/facets"   # the index: a git repository
//! facet "http_tools", "~> 0.3"                     # from the index, by version
//! facet "utils", path: "../utils"                  # a directory
//! facet "greet", git: "https://github.com/x/greet", tag: "v1.0.0"
//! facet "sheets", "~> 0.1", native: true           # trusted to ship Rust code
//! facet "texts", "~> 0.2", bridge: true            # trusted to ship a Ruby or Python bridge
//! ```

use std::path::{Path, PathBuf};

use grenat_ast::{Arg, Expr, ExprKind, Item, StrSeg};

use crate::manifest::Reference;
use crate::version::Requirement;

/// The file's name, next to `grenat.toml`.
pub const FACETFILE: &str = "Facetfile";

#[derive(Debug, Clone, PartialEq)]
pub struct Facetfile {
    /// Index repositories, in order.
    pub sources: Vec<String>,
    pub facets: Vec<FacetDecl>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct FacetDecl {
    pub name: String,
    pub requirement: Requirement,
    pub origin: Origin,
    /// `native: true`: the application trusts its Rust code, which runs
    /// outside Grenat's sandbox.
    pub native: bool,
    /// `bridge: true`: the application trusts its bridge, a process that
    /// serves Ruby or Python functions outside Grenat's sandbox.
    pub bridge: bool,
}

/// Where a facet comes from.
#[derive(Debug, Clone, PartialEq)]
pub enum Origin {
    /// The index: a version chosen among the tags of its repository.
    Index,
    /// A directory, relative to the package.
    Path(PathBuf),
    /// A repository, at a reference.
    Git { url: String, reference: Reference },
}

impl Facetfile {
    /// The `Facetfile` of the package in `dir`, if it has one.
    pub fn load(dir: &Path) -> Result<Option<Facetfile>, String> {
        let path = dir.join(FACETFILE);
        match std::fs::read_to_string(&path) {
            Ok(text) => Facetfile::parse(&text).map(Some).map_err(|e| format!("{}: {e}", path.display())),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
            Err(e) => Err(format!("cannot read {}: {e}", path.display())),
        }
    }

    pub fn parse(text: &str) -> Result<Facetfile, String> {
        let parsed = grenat_parser::parse(text);
        if let Some(d) = parsed.diagnostics.first() {
            return Err(d.message.clone());
        }
        let mut file = Facetfile { sources: Vec::new(), facets: Vec::new() };
        for item in &parsed.program.items {
            let Item::Stmt(Expr { kind: ExprKind::Call { recv: None, name, args, .. }, .. }) = item else {
                return Err("a Facetfile holds `source \"…\"` and `facet \"…\"` lines".into());
            };
            let positional: Vec<String> = args
                .iter()
                .filter_map(|a| match a {
                    Arg::Pos(e) => Some(literal(e)),
                    _ => None,
                })
                .collect::<Result<_, _>>()?;
            let named = |key: &str| -> Result<Option<String>, String> {
                args.iter()
                    .find_map(|a| match a {
                        Arg::Named { name, value: Some(e) } if name.name == key => Some(literal(e)),
                        _ => None,
                    })
                    .transpose()
            };
            match name.name.as_str() {
                "source" => match positional.as_slice() {
                    [url] => file.sources.push(url.clone()),
                    _ => return Err("`source` takes the URL of an index".into()),
                },
                "facet" => {
                    let (facet, requirement) = match positional.as_slice() {
                        [facet] => (facet.clone(), Requirement::any()),
                        [facet, requirement] => (facet.clone(), Requirement::parse(requirement)?),
                        _ => {
                            return Err(
                                "`facet` takes a name and, optionally, a version: `facet \"x\", \"~> 1.2\"`".into()
                            );
                        }
                    };
                    let origin = match (named("path")?, named("git")?) {
                        (Some(path), None) => Origin::Path(path.into()),
                        (None, Some(url)) => {
                            let reference = match (named("branch")?, named("tag")?, named("rev")?) {
                                (Some(b), None, None) => Reference::Branch(b),
                                (None, Some(t), None) => Reference::Tag(t),
                                (None, None, Some(r)) => Reference::Rev(r),
                                (None, None, None) => Reference::Default,
                                _ => return Err(format!("facet `{facet}`: give one of `branch:`, `tag:` or `rev:`")),
                            };
                            Origin::Git { url, reference }
                        }
                        (None, None) => Origin::Index,
                        _ => return Err(format!("facet `{facet}`: give either `path:` or `git:`")),
                    };
                    if file.facets.iter().any(|f| f.name == facet) {
                        return Err(format!("facet `{facet}` is listed twice"));
                    }
                    let native = flag(args, "native").map_err(|e| format!("facet `{facet}`: {e}"))?;
                    let bridge = flag(args, "bridge").map_err(|e| format!("facet `{facet}`: {e}"))?;
                    file.facets.push(FacetDecl { name: facet, requirement, origin, native, bridge });
                }
                other => return Err(format!("unknown Facetfile directive `{other}`")),
            }
        }
        Ok(file)
    }
}

/// `native: true`, `bridge: true` (or `false`): a literal boolean; `false` without it.
fn flag(args: &[Arg], key: &str) -> Result<bool, String> {
    let value = args.iter().find_map(|a| match a {
        Arg::Named { name, value } if name.name == key => Some(value.as_ref()),
        _ => None,
    });
    match value {
        None => Ok(false),
        Some(Some(Expr { kind: ExprKind::Bool(b), .. })) => Ok(*b),
        Some(_) => Err(format!("`{key}:` is `true` or `false`")),
    }
}

fn literal(e: &Expr) -> Result<String, String> {
    match &e.kind {
        ExprKind::Str(segments) => segments
            .iter()
            .map(|s| match s {
                StrSeg::Lit(t) => Ok(t.as_str()),
                StrSeg::Interp(_) => Err("a Facetfile holds literal strings, without interpolation".to_string()),
            })
            .collect(),
        _ => Err("a Facetfile holds literal strings".into()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_facetfile() {
        let file = Facetfile::parse(
            "# my app\nsource \"https://x.io/index\"\nfacet \"http_tools\", \"~> 0.3\"\nfacet \"utils\", path: \"../utils\"\nfacet \"greet\", git: \"https://x.io/greet\", tag: \"v1.0.0\"\nfacet \"any\"\n",
        )
        .unwrap();
        assert_eq!(file.sources, ["https://x.io/index"]);
        assert_eq!(file.facets[0].requirement.to_string(), "~> 0.3");
        assert_eq!(file.facets[0].origin, Origin::Index);
        assert_eq!(file.facets[1].origin, Origin::Path("../utils".into()));
        assert_eq!(
            file.facets[2].origin,
            Origin::Git { url: "https://x.io/greet".into(), reference: Reference::Tag("v1.0.0".into()) }
        );
        assert_eq!(file.facets[3].requirement, Requirement::any());
        let err = |text: &str| Facetfile::parse(text).unwrap_err();
        assert_eq!(err("facet \"a\"\nfacet \"a\"\n"), "facet `a` is listed twice");
        assert_eq!(err("gem \"rails\"\n"), "unknown Facetfile directive `gem`");
        assert_eq!(err("facet \"a\", path: \"x\", git: \"y\"\n"), "facet `a`: give either `path:` or `git:`");
        assert!(err("facet \"a\", \"latest\"\n").contains("expected a version"));
        assert_eq!(err("facet \"a\", native: \"yes\"\n"), "facet `a`: `native:` is `true` or `false`");
    }

    #[test]
    fn a_facet_is_trusted_with_native_code_explicitly() {
        let file = Facetfile::parse(
            "facet \"sheets\", \"~> 0.1\", native: true\nfacet \"plain\"\nfacet \"no\", native: false\n",
        )
        .unwrap();
        let native: Vec<bool> = file.facets.iter().map(|f| f.native).collect();
        assert_eq!(native, [true, false, false]);
        assert_eq!(file.facets[0].requirement.to_string(), "~> 0.1");
    }

    #[test]
    fn a_facet_is_trusted_with_a_bridge_explicitly() {
        let file = Facetfile::parse("facet \"texts\", bridge: true\nfacet \"plain\", native: true\n").unwrap();
        let flags: Vec<(bool, bool)> = file.facets.iter().map(|f| (f.native, f.bridge)).collect();
        assert_eq!(flags, [(false, true), (true, false)]);
        let e = Facetfile::parse("facet \"a\", bridge: 1\n").unwrap_err();
        assert_eq!(e, "facet `a`: `bridge:` is `true` or `false`");
    }
}
