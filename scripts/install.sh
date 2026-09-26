#!/bin/sh
# Installs Grenat on macOS or on any Linux with glibc (Ubuntu, Debian,
# Fedora, RHEL, Amazon Linux, Arch…), without a package manager:
#
#   curl -sSL https://github.com/itsmedit/grenat/releases/latest/download/install.sh | sh
#
# Options (after `sh -s --`):
#   --version v0.1.1     a given release (default: the latest)
#   --prefix DIR         where to install (default: ~/.grenat, or /usr/local as root)
#   --no-modify-path     do not add the prefix's bin/ to the shell's PATH
#   --uninstall          remove what an earlier install put in the prefix
#
# It installs bin/grenat, bin/setter and lib/grenat/ (the runtime libraries
# `grenat build` links), after checking the archive's SHA-256.
set -eu

REPO="itsmedit/grenat"
version=""
prefix=""
modify_path=1
uninstall=0

say() { printf '%s\n' "$*"; }
usage() {
    say "usage: install.sh [--version v0.1.1] [--prefix DIR] [--no-modify-path] [--uninstall]"
    say "  installs grenat, setter and lib/grenat/ into DIR (default ~/.grenat, or /usr/local as root)"
}
fail() { printf 'grenat install: %s\n' "$*" >&2; exit 1; }

while [ $# -gt 0 ]; do
    case "$1" in
        --version) version="${2:?--version needs a tag, like v0.1.1}"; shift 2 ;;
        --prefix) prefix="${2:?--prefix needs a directory}"; shift 2 ;;
        --no-modify-path) modify_path=0; shift ;;
        --uninstall) uninstall=1; shift ;;
        -h | --help) usage; exit 0 ;;
        *) fail "unknown option $1 (see --help)" ;;
    esac
done

if [ -z "$prefix" ]; then
    if [ "$(id -u)" = 0 ]; then prefix="/usr/local"; else prefix="$HOME/.grenat"; fi
fi

if [ "$uninstall" = 1 ]; then
    rm -f "$prefix/bin/grenat" "$prefix/bin/setter"
    rm -rf "$prefix/lib/grenat"
    say "removed Grenat from $prefix (a PATH line in your shell's profile, if any, stays: remove it by hand)"
    exit 0
fi

# ── What to download ────────────────────────────────────────────
case "$(uname -s)" in
    Linux) os="unknown-linux-gnu" ;;
    Darwin) os="apple-darwin" ;;
    *) fail "unsupported system $(uname -s): Grenat runs on macOS and Linux" ;;
esac
case "$(uname -m)" in
    x86_64 | amd64) arch="x86_64" ;;
    aarch64 | arm64) arch="aarch64" ;;
    *) fail "unsupported processor $(uname -m): Grenat runs on x86_64 and arm64" ;;
esac
if [ "$os" = "unknown-linux-gnu" ] && ldd --version 2>&1 | grep -qi musl; then
    fail "this Linux uses musl (Alpine?), and Grenat's binaries need glibc: use a glibc distribution, or the Docker image ghcr.io/$REPO"
fi
target="$arch-$os"

if command -v curl >/dev/null 2>&1; then
    fetch() { curl -fsSL "$1" -o "$2"; }
    latest() { curl -fsSLI -o /dev/null -w '%{url_effective}' "https://github.com/$REPO/releases/latest"; }
elif command -v wget >/dev/null 2>&1; then
    fetch() { wget -qO "$2" "$1"; }
    latest() { wget -qS --spider "https://github.com/$REPO/releases/latest" 2>&1 | sed -n 's/^ *[Ll]ocation: *//p' | tail -1; }
else
    fail "curl or wget is needed (sudo apt install curl, sudo dnf install curl…)"
fi
command -v tar >/dev/null 2>&1 || fail "tar is needed (sudo apt install tar, sudo dnf install tar…)"

if [ -z "$version" ]; then
    version="$(latest | sed 's|.*/tag/||' | tr -d '\r')"
    case "$version" in v*) ;; *) fail "cannot find the latest release of $REPO" ;; esac
fi

name="grenat-$version-$target"
base="https://github.com/$REPO/releases/download/$version"
work="$(mktemp -d)"
trap 'rm -rf "$work"' EXIT

say "Installing Grenat $version ($target) into $prefix"
fetch "$base/$name.tar.gz" "$work/$name.tar.gz" || fail "cannot download $base/$name.tar.gz"
fetch "$base/$name.tar.gz.sha256" "$work/$name.tar.gz.sha256" || fail "cannot download the checksum of $name.tar.gz"

# ── Checked, then installed ─────────────────────────────────────
expected="$(cut -d ' ' -f 1 "$work/$name.tar.gz.sha256")"
if command -v sha256sum >/dev/null 2>&1; then
    actual="$(sha256sum "$work/$name.tar.gz" | cut -d ' ' -f 1)"
else
    actual="$(shasum -a 256 "$work/$name.tar.gz" | cut -d ' ' -f 1)"
fi
[ "$expected" = "$actual" ] || fail "the archive's checksum does not match: not installed"

tar xzf "$work/$name.tar.gz" -C "$work"
mkdir -p "$prefix/bin" "$prefix/lib/grenat" || fail "cannot write to $prefix (as root, or with --prefix)"
cp "$work/$name/bin/grenat" "$work/$name/bin/setter" "$prefix/bin/"
cp "$work/$name/lib/grenat/"*.a "$prefix/lib/grenat/"
"$prefix/bin/grenat" --version >/dev/null || fail "the installed grenat does not run on this system"

# ── The PATH ────────────────────────────────────────────────────
case ":$PATH:" in
    *":$prefix/bin:"*) on_path=1 ;;
    *) on_path=0 ;;
esac
if [ "$on_path" = 0 ] && [ "$modify_path" = 1 ]; then
    line="export PATH=\"$prefix/bin:\$PATH\"  # Grenat"
    for profile in "$HOME/.bashrc" "$HOME/.zshrc" "$HOME/.profile"; do
        # the profiles of the shells in use, and ~/.profile for login shells
        case "$profile" in
            */.zshrc) [ -f "$profile" ] || [ "$(basename "${SHELL:-}")" = zsh ] || continue ;;
            */.bashrc) [ -f "$profile" ] || [ "$(basename "${SHELL:-}")" = bash ] || continue ;;
        esac
        grep -qF "$line" "$profile" 2>/dev/null || printf '\n%s\n' "$line" >> "$profile"
    done
fi

say "✓ $("$prefix/bin/grenat" --version) installed: $prefix/bin/grenat, $prefix/bin/setter"
if [ "$on_path" = 0 ]; then
    if [ "$modify_path" = 1 ]; then
        say "  open a new shell, or: export PATH=\"$prefix/bin:\$PATH\""
    else
        say "  add it to your PATH: export PATH=\"$prefix/bin:\$PATH\""
    fi
fi
if ! command -v cc >/dev/null 2>&1; then
    say "  grenat run, test and serve work now; grenat build needs a C linker:"
    say "    sudo apt install gcc      (Ubuntu, Debian)"
    say "    sudo dnf install gcc      (Fedora, RHEL, Amazon Linux)"
    say "    xcode-select --install    (macOS)"
fi
say "  next: grenat new --app hello && cd hello && grenat test"
