#!/bin/sh
# Builds and tests Grenat on Linux in Docker: the whole test suite, then
# executables built by `grenat build` (with and without the interpreter,
# and optimized by LLVM).
#
#   scripts/test-linux.sh            # the machine's architecture
#   scripts/test-linux.sh amd64      # another one (emulated: slower, and
#                                    # timing assertions may fail under load)
set -eu
case "$(uname -m)" in
    arm64 | aarch64) native=arm64 ;;
    *) native=amd64 ;;
esac
arch="${1:-$native}"
platform="--platform linux/$arch"
root="$(cd "$(dirname "$0")/.." && pwd)"
docker run --rm $platform \
    -v "$root":/src:ro \
    -v "grenat-cargo:/usr/local/cargo/registry" \
    -v "grenat-target-$arch:/work/target" \
    rust:1-bookworm sh -euc '
        echo "Linux $(uname -m)"
        # clang, for release builds (LLVM)
        (apt-get update -qq && apt-get install -y -qq clang >/dev/null 2>&1) || echo "no clang: release builds not tested"
        mkdir -p /work && cd /src && tar --exclude=./target -cf - . | (cd /work && tar -xf -)
        cd /work
        # linking every test program at once exhausts a small Docker VM
        export CARGO_BUILD_JOBS=2 CARGO_PROFILE_DEV_DEBUG=0 CARGO_PROFILE_TEST_DEBUG=0
        cargo test -q --no-fail-fast 2>&1 | grep "test result" | awk "{p+=\$4; f+=\$6} END {print \"tests:\", p+0, \"passed,\", f+0, \"failed\"; exit (f > 0 || p == 0)}"
        cargo build -q --release -p grenat_cli -p grenat_host -p grenat_standalone
        ./target/release/grenat build examples/objects.grn -o /tmp/hosted
        ./target/release/grenat build --native examples/objects.grn -o /tmp/native
        ./target/release/grenat build --native --release examples/objects.grn -o /tmp/release
        [ "$(/tmp/hosted)" = "$(/tmp/native)" ] && [ "$(/tmp/native)" = "$(/tmp/release)" ] && echo "executables: identical output"
    '
