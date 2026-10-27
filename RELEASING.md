# Releasing

A release is a `v*` tag: `.github/workflows/release.yml` builds and publishes the binaries,
packages, extension and image. It also checks that the SDK of native facets — `grenat_ext`
and `grenat_ext_macros` — would publish (`cargo publish --dry-run`), but never publishes it:
crates.io is a manual step, with the owner's token, before the tag.

## 1. The version

Bump `[workspace.package] version` in `Cargo.toml`, and the two SDK entries of
`[workspace.dependencies]` with it (`grenat_ext_macros` is required at exactly that version:
`version = "=X.Y.Z"`). Refresh the lock files (`cargo check`, and a build of each facet's crate),
then run the full test suite.

## 2. The SDK on crates.io

Check first (nothing is uploaded; cargo 1.90 or later verifies the two together, `grenat_ext`
against the macros just packaged):

```sh
cargo package --list -p grenat_ext_macros
cargo package --list -p grenat_ext
cargo publish --dry-run -p grenat_ext_macros -p grenat_ext
```

Then publish, macros first — `grenat_ext` depends on them, and crates.io refuses a crate whose
dependencies it does not have:

```sh
cargo login                          # once: a token from https://crates.io/settings/tokens
cargo publish -p grenat_ext_macros
# wait until the index has it (cargo publish waits for it; else check
# https://crates.io/crates/grenat_ext_macros or `cargo search grenat_ext_macros`)
cargo publish -p grenat_ext
```

(`cargo publish -p grenat_ext_macros -p grenat_ext` does both, in that order, waiting in
between.) A published version cannot be replaced, only yanked: publish from the commit you are
about to tag, with a clean working tree.

The facets' crates depend on `grenat_ext = "0.1"`: a new 0.1.x is picked up by their next
build; a 0.2 needs their `Cargo.toml` updated.

## 3. The tag

```sh
git tag vX.Y.Z
git push origin main vX.Y.Z
```

# Releasing a facet

The official facets (`facets/html`, `facets/sheets`) have versions of their own, released apart
from Grenat's: the index lists them by tags with a prefix of their own
(`facets/index/facets/html.toml`: `path = "facets/html"`, `tag_prefix = "html-v"`), so a
facet's release is a tag `html-vX.Y.Z` (or `sheets-vX.Y.Z`) in this repository.

1. Bump `[package] version` in the facet's `grenat.toml` (`facets/html/grenat.toml`), and its
   crate's `version` in `facets/html/native/Cargo.toml` with it; run the full test suite
   (`crates/grenat_cli/tests/facet_html.rs` and `facet_sheets.rs` test the facets); commit.
2. Tag the commit and push the tag:

   ```sh
   git tag -a html-vX.Y.Z -m "html X.Y.Z"
   git push origin main html-vX.Y.Z
   ```

`setter install` sees the new version at once (`git ls-remote --tags`); no change to the index
is needed. A tag cannot be moved once applications have locked its commit: a fix is a new
version.

This does **not** start a Grenat release. `release.yml` runs on `tags: ["v*"]`, and GitHub's
filter patterns match the whole tag name, `*` standing for any characters except `/`: `v*`
matches only the tags that start with `v` (`v0.1.5`), never `html-v0.1.0` or `sheets-v0.1.0`.
(`ci.yml` runs on pushes to `main` and on pull requests only, never on tags.) Conversely, never
tag a facet `vX.Y.Z`: that is a Grenat release, and the index ignores it.

`setter publish` tags `v<version>` in a facet that is its own repository: it is not for these
facets, which are tagged by hand as above.

The index itself (`facets/index/`) is published as its own repository once, and again only when
an entry changes:

```sh
git subtree split --prefix facets/index -b index
git push git@github.com:itsmedit/grenat-facets.git index:main
```
