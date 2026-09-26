#!/bin/sh
# Packages a release build for `target` into dist/ — bin/grenat, bin/setter,
# lib/grenat/*.a, the README and licenses, as grenat-<tag>-<target>.tar.gz and
# its SHA-256 — and checks the package as it will be installed: unpacked
# elsewhere, it runs and builds a native program.
#
#   scripts/release-package.sh aarch64-apple-darwin      (GITHUB_REF_NAME: the tag)
set -eu
target="$1"
tag="${GITHUB_REF_NAME:?the release tag, like v0.1.1}"
name="grenat-$tag-$target"

rm -rf "dist/$name" && mkdir -p "dist/$name/bin" "dist/$name/lib/grenat" package
cp target/release/grenat target/release/setter "dist/$name/bin/"
cp target/release/libgrenat_host.a target/release/libgrenat_standalone.a "dist/$name/lib/grenat/"
cp README.md LICENSE-MIT LICENSE-APACHE "dist/$name/"
tar czf "dist/$name.tar.gz" -C dist "$name"
(cd dist && { sha256sum "$name.tar.gz" 2>/dev/null || shasum -a 256 "$name.tar.gz"; } > "$name.tar.gz.sha256")
# the unpacked tree, for the Linux packages
rm -rf package && cp -r "dist/$name" package && rm -rf "dist/$name"

check="$(mktemp -d)"
tar xzf "dist/$name.tar.gz" -C "$check" --strip-components 1
printf 'def main\n  puts "hello"\nend\n' > "$check/hello.grn"
"$check/bin/grenat" --version
"$check/bin/grenat" build --native "$check/hello.grn" -o "$check/hello"
test "$("$check/hello")" = hello
echo "✓ $name.tar.gz"
