# A facet index

An **index** is a git repository that tells `setter` where facets live, so that a `Facetfile`
asks for them by name and version:

```ruby
# Facetfile
source "https://github.com/itsmedit/grenat-facets"   # the index (several `source` lines: searched in order)
facet "html", "~> 0.1", native: true
```

## The format

```text
<index repository>/
  facets/
    <name>.toml        one file per facet
```

Each `facets/<name>.toml` holds up to three keys: `git`, the URL (or path) of the facet's
repository, required; and two optional ones, for a facet that shares its repository with
other code:

```toml
git = "https://github.com/someone/greet"   # a repository whose root is the facet, tagged v1.2.3…
```

```toml
git = "https://github.com/itsmedit/grenat"
path = "facets/html"     # the facet's directory in the repository (default: its root)
tag_prefix = "html-v"    # its versions are the tags html-v0.1.0, html-v0.2.0… (default: "v")
```

- `path` is relative to the repository's root, without `..` (`facets/html`, not `/facets/html`
  nor `../html`);
- `tag_prefix` is what the facet's version tags start with: non-empty, without spaces; only the
  tags with that prefix followed by a version count (`html-v0.1.0`), so the repository's own
  tags (`v0.1.5`) and those of its other facets (`sheets-v0.1.0`) are ignored. Without it, the
  versions are the tags `v1.2.3` (or `1.2.3`).

An entry that breaks these rules, or a version whose commit has no such directory, fails the
install with an error naming the entry (`the index entry facets/html.toml of <index>: …`).

At `setter install` (`crates/grenat_package/src/facets.rs`):

1. each index is cloned (shallow) into `.grenat/index/<slug>/` of the application, and pulled
   on later installs; the first index with `facets/<name>.toml` wins;
2. the facet's **versions are its repository's tags** with the entry's prefix (annotated or
   not, `git ls-remote --tags`); the highest satisfying every requirement on the facet is
   chosen (or the locked one, if it still does);
3. the tag is cloned into `.grenat/facets/<name>-<version>/`, and the facet is that clone's
   root, or its directory `path`: its `grenat.toml` must name it `<name>`, and its own
   `Facetfile`, if any, is read there (a `path:` facet in it is relative to that directory);
4. `Facetfile.lock` records `source = "index:<url>"`, the version, the commit and the facet's
   directory (`dir = ".grenat/facets/html-0.1.0/facets/html"`), where `require "html"` finds it.

A facet that is its own repository tags with `setter publish` (`v<version>` from
`grenat.toml`), pushes the tag, then adds `facets/<name>.toml` to an index once. A facet in a
directory of a larger repository is tagged by hand, `<prefix><version>`
(`git tag -a html-v0.1.0 -m "html 0.1.0"`; see `RELEASING.md` for the official ones).

A `Facetfile` can take the same directory without an index, next to `git:`:

```ruby
facet "html", git: "https://github.com/itsmedit/grenat", tag: "html-v0.1.0", path: "facets/html", native: true
```

## The official facets

`html` and `sheets` live in `facets/` of the Grenat repository, whose `v*` tags are Grenat's
own releases. Their entries here name that directory and a prefix of their own:
[`facets/html.toml`](facets/html.toml) (`path = "facets/html"`, `tag_prefix = "html-v"`) and
[`facets/sheets.toml`](facets/sheets.toml) (`facets/sheets`, `sheets-v`). Their tags
(`html-v0.1.0`, `sheets-v0.1.0`) do not start a Grenat release (`RELEASING.md`).

## Ready to push

[`facets/`](facets) here is the content of the index, to push as its own repository (the tags
must exist in the Grenat repository for `setter` to find a version):

```sh
git subtree split --prefix facets/index -b index
git push git@github.com:itsmedit/grenat-facets.git index:main
```
