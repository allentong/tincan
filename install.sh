#!/bin/sh
# Install the latest tincan release binary into $TINCAN_INSTALL_DIR (default ~/.local/bin).
set -eu

repo="allentong/tincan"
dir="${TINCAN_INSTALL_DIR:-$HOME/.local/bin}"

case "$(uname -s)" in
  Darwin) os=apple-darwin ;;
  Linux) os=unknown-linux-musl ;;
  *) echo "tincan: unsupported OS $(uname -s); build with cargo install --git https://github.com/$repo" >&2; exit 1 ;;
esac
case "$(uname -m)" in
  arm64|aarch64) arch=aarch64 ;;
  x86_64|amd64) arch=x86_64 ;;
  *) echo "tincan: unsupported CPU $(uname -m)" >&2; exit 1 ;;
esac

asset="tincan-$arch-$os.tar.gz"
url="https://github.com/$repo/releases/latest/download/$asset"
tmp="$(mktemp -d)"
trap 'rm -rf "$tmp"' EXIT

echo "Downloading $url"
curl -fsSL "$url" -o "$tmp/$asset"
curl -fsSL "$url.sha256" -o "$tmp/$asset.sha256"
(cd "$tmp" && if command -v sha256sum >/dev/null; then sha256sum -c "$asset.sha256"; else shasum -a 256 -c "$asset.sha256"; fi) >/dev/null
tar -xzf "$tmp/$asset" -C "$tmp"
mkdir -p "$dir"
install -m 755 "$tmp/tincan" "$dir/tincan"
echo "Installed $("$dir/tincan" --version) to $dir/tincan"
case ":$PATH:" in *":$dir:"*) ;; *) echo "Add $dir to your PATH." ;; esac
