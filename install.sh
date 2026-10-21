#!/bin/sh
# Install or update sun-control from its GitHub releases.
#
#   curl -fsSL https://github.com/jordanhchin/sun-control/releases/latest/download/install.sh | sh
#
# Environment:
#   SUN_CONTROL_INSTALL_DIR  where to put the binary (default: ~/.local/bin)
#   SUN_CONTROL_VERSION      release tag to install, e.g. v0.2.0 (default: latest)
#   SUN_CONTROL_BASE_URL     download from this URL instead of GitHub (mirrors, testing)
#
# Afterwards, `sun-control update` keeps it up to date.
set -eu

REPO="jordanhchin/sun-control"
INSTALL_DIR="${SUN_CONTROL_INSTALL_DIR:-$HOME/.local/bin}"
VERSION="${SUN_CONTROL_VERSION:-latest}"

say() { printf '%s\n' "$*"; }
die() { printf 'error: %s\n' "$*" >&2; exit 1; }

[ "$(uname -s)" = "Linux" ] || die "prebuilt binaries are Linux-only; on other systems use: cargo install --git https://github.com/$REPO"
case "$(uname -m)" in
    x86_64 | amd64) arch=x86_64 ;;
    aarch64 | arm64) arch=aarch64 ;;
    *) die "no prebuilt binary for $(uname -m); use: cargo install --git https://github.com/$REPO" ;;
esac
asset="sun-control-$arch-linux"

if [ -n "${SUN_CONTROL_BASE_URL:-}" ]; then
    base="$SUN_CONTROL_BASE_URL"
elif [ "$VERSION" = latest ]; then
    base="https://github.com/$REPO/releases/latest/download"
else
    base="https://github.com/$REPO/releases/download/$VERSION"
fi

tmp=$(mktemp -d)
trap 'rm -rf "$tmp"' EXIT INT TERM

fetch() {
    if command -v curl >/dev/null 2>&1; then
        curl -fsSL "$base/$1" -o "$tmp/$1" && return 0
    elif command -v wget >/dev/null 2>&1; then
        wget -q "$base/$1" -O "$tmp/$1" && return 0
    else
        die "curl or wget is required"
    fi
    # Private repositories can't be downloaded anonymously; gh can if logged in.
    if [ -z "${SUN_CONTROL_BASE_URL:-}" ] && command -v gh >/dev/null 2>&1; then
        tag=""
        [ "$VERSION" = latest ] || tag="$VERSION"
        # shellcheck disable=SC2086 # empty $tag means "latest"
        gh release download $tag --repo "$REPO" --pattern "$1" --dir "$tmp" --clobber && return 0
    fi
    return 1
}

say "Downloading $asset ($VERSION)…"
fetch "$asset" || die "couldn't download $asset from $base"
fetch "$asset.sha256" || die "couldn't download $asset.sha256"

(cd "$tmp" && sha256sum -c "$asset.sha256" >/dev/null 2>&1) || die "checksum verification failed"
chmod 755 "$tmp/$asset"
"$tmp/$asset" --version >/dev/null 2>&1 || die "the downloaded binary doesn't run on this machine"

mkdir -p "$INSTALL_DIR"
# Copy then rename, so a running sun-control is replaced atomically.
cp "$tmp/$asset" "$INSTALL_DIR/.sun-control.new"
mv -f "$INSTALL_DIR/.sun-control.new" "$INSTALL_DIR/sun-control"

say "Installed $("$INSTALL_DIR/sun-control" --version) to $INSTALL_DIR/sun-control"
case ":$PATH:" in
    *":$INSTALL_DIR:"*) ;;
    *) say "Note: $INSTALL_DIR isn't on your PATH. Add it, e.g.: export PATH=\"$INSTALL_DIR:\$PATH\"" ;;
esac
say "Run 'sun-control update' later to get new versions."
