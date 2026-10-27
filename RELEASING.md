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
