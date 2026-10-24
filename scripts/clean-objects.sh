#!/bin/sh
# Removes the object files Rust leaves behind in target/*/deps at every relink
# of a test program (about 30 per program, never deleted). They pile up by the
# thousands; macOS then checks each new executable for seconds, even minutes,
# in so crowded a directory, and the test suite takes an hour instead of
# minutes. Only build cache goes: test programs lose their debug info until
# they are built again. Run it before the whole suite (it prints what it did).
set -eu
cd "$(dirname "$0")/.."
removed=0
for deps in target/*/deps; do
    [ -d "$deps" ] || continue
    count=$(find "$deps" -maxdepth 1 -name '*.rcgu.o' | wc -l | tr -d ' ')
    find "$deps" -maxdepth 1 -name '*.rcgu.o' -delete
    removed=$((removed + count))
done
echo "removed $removed leftover object file(s) from target/*/deps"
