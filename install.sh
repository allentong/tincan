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
if command -v gh >/dev/null 2>&1 && gh auth status --hostname github.com >/dev/null 2>&1; then
  gh attestation verify "$tmp/$asset" --repo "$repo" \
    --signer-workflow "$repo/.github/workflows/release.yml" >/dev/null
  echo "Verified GitHub build provenance for $asset"
else
  echo "GitHub CLI is not authenticated; verified checksum only (see README for provenance verification)."
fi
tar -xzf "$tmp/$asset" -C "$tmp"
mkdir -p "$dir"
install -m 755 "$tmp/tincan" "$dir/tincan"
echo "Installed $("$dir/tincan" --version) to $dir/tincan"
# The skill ships in the binary: put it where Claude Code, Codex and Grok look for skills.
"$dir/tincan" install-skills >/dev/null && echo "Installed the tincan skill for Claude Code, Codex and Grok"
case ":$PATH:" in *":$dir:"*) ;; *) echo "Add $dir to your PATH." ;; esac
