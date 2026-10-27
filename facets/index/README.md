# A facet index

An **index** is a git repository that tells `setter` where facets live, so that a `Facetfile`
asks for them by name and version:

```ruby
# Facetfile
source "https://github.com/itsmedit/grenat-facets"   # the index (several `source` lines: searched in order)
facet "html", "~> 0.1", native: true
```

## The format (what `setter` reads today)

```text
<index repository>/
  facets/
    <name>.toml        one file per facet
```

Each `facets/<name>.toml` holds one key that `setter` reads, `git`: the URL (or path) of the
facet's repository.

```toml
git = "https://github.com/someone/greet"
```

At `setter install` (`crates/grenat_package/src/facets.rs`):

1. each index is cloned (shallow) into `.grenat/index/<slug>/` of the application, and pulled
   on later installs; the first index with `facets/<name>.toml` wins;
2. the facet's **versions are its repository's tags** (`v1.2.3`, annotated or not; others are
   ignored, `git ls-remote --tags`); the highest satisfying every requirement on the facet is
   chosen (or the locked one, if it still does);
3. the tag is cloned into `.grenat/facets/<name>-<version>/`, and **the repository's root is
   the facet**: its `grenat.toml` must name it `<name>`;
4. `Facetfile.lock` records `source = "index:<url>"`, the version and the commit.

A facet author tags with `setter publish` (`v<version>` from `grenat.toml`, in a facet that is
its own git repository), pushes the tag, then adds `facets/<name>.toml` to an index once.

## The official facets: not listable as they are

`html` and `sheets` live in `facets/` of the Grenat repository, and the format has no way to
say so:

- an entry names a repository, never a directory in it: other keys are ignored, and the
  facet would be the repository's root, where there is no `grenat.toml` — the install fails
  (a `git:` facet in a `Facetfile` has no subdirectory either);
- the versions would be the repository's tags, which are Grenat's (`v0.1.5`), not the facets'
  (`0.1.0` each); and a `v*` tag in this repository starts a Grenat release
  (`.github/workflows/release.yml`), so facets cannot be tagged there with that scheme.

Two ways out.

**Today, without a change to `setter`:** one repository per facet, split from this one
(`git subtree split --prefix facets/html -b html && git push git@github.com:itsmedit/grenat-html.git html:main`),
tagged `v0.1.0` there; the entries are then `git = "https://github.com/itsmedit/grenat-html"`.
The facet's crate already builds outside the checkout (`grenat_ext = "0.1"` from crates.io,
once published).

**The smallest change to `setter` (proposed, not implemented):** two optional keys in an entry,

```toml
git = "https://github.com/itsmedit/grenat"
path = "facets/html"     # the facet's directory in the repository (default: its root)
tag_prefix = "html-v"    # its versions are the tags html-v0.1.0, html-v0.2.0… (default: "v")
```

read by `index_entry` alongside `git` (returning the three); `fetch` would filter
`git::tags` by the prefix before `Version::parse`, clone the tag as now, and record
`dir = .grenat/facets/<name>-<version>/<path>` (the `.git` check staying on the clone's
root). `Facetfile.lock` already stores any `dir`; `setter publish` would learn the same prefix
to tag a facet inside a larger repository. About forty lines and their tests in
`grenat_package`; `git:` facets in a `Facetfile` could take the same `path:` later.

## Ready to push

[`facets/`](facets) here is the content of an index for the two official facets, in the
proposed form (`git`, `path`, `tag_prefix`), to push as its own repository once `setter` reads those keys
(and the tags exist):

```sh
git subtree split --prefix facets/index -b index
git push git@github.com:itsmedit/grenat-facets.git index:main
```

With the current `setter`, replace each entry by the facet's own repository
(`git = "https://github.com/itsmedit/grenat-html"`, nothing else), as above.
