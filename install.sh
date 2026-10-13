#!/bin/sh
# Install the latest tincan release binary into $TINCAN_INSTALL_DIR (default ~/.local/bin).
set -eu

repo="allentong/tincan"
dir="${TINCAN_INSTALL_DIR:-$HOME/.local/bin}"

case "$(uname -s)" in
  Darwin) os=apple-darwin ;;
  Linux) os=unknown-linux-musl ;;
  *) echo "tincan: unsupported OS $(uname -s); build from source: cargo install --git https://github.com/$repo" >&2; exit 1 ;;
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
dir="$(cd "$dir" && pwd)"
install -m 755 "$tmp/tincan" "$dir/tincan"
echo "Installed $("$dir/tincan" --version) to $dir/tincan"
# The skills ship in the binary: install them for each agent found on this machine.
"$dir/tincan" install-skills | sed -n 's/.*"summary":"\([^"]*\)".*/\1/p'
case ":$PATH:" in
  *":$dir:"*) ;;
  *)
    # Put $dir on PATH for new shells, like rustup and bun do. TINCAN_NO_MODIFY_PATH=1 opts out.
    if [ -n "${TINCAN_NO_MODIFY_PATH:-}" ]; then
      echo "Add $dir to your PATH."
      exit 0
    fi
    # Quote $dir as a literal so the rc file never evaluates anything in the path.
    q="$(printf '%s' "$dir" | sed "s/'/'\\\\''/g")"
    line="export PATH='$q':\"\$PATH\""
    case "$(basename "${SHELL:-sh}")" in
      zsh) rc="${ZDOTDIR:-$HOME}/.zshrc" ;;
      bash)
        rc="$HOME/.bashrc"
        if [ "$os" = apple-darwin ]; then
          # Login shells read only the first of these that exists.
          rc="$HOME/.bash_profile"
          for f in "$HOME/.bash_profile" "$HOME/.bash_login" "$HOME/.profile"; do
            if [ -f "$f" ]; then rc="$f"; break; fi
          done
        fi ;;
      fish)
        rc="${XDG_CONFIG_HOME:-$HOME/.config}/fish/conf.d/tincan.fish"
        mkdir -p "$(dirname "$rc")"
        line="fish_add_path '$(printf '%s' "$dir" | sed -e 's/\\/\\\\/g' -e "s/'/\\\\'/g")'" ;;
      *) rc="$HOME/.profile" ;;
    esac
    if ! grep -qsF "$line" "$rc"; then
      printf '\n# tincan\n%s\n' "$line" >> "$rc"
      printf 'Added %s to PATH in %s\n' "$dir" "$rc"
    fi
    printf 'Open a new terminal (or run: %s) to use tincan.\n' "$line"
    ;;
esac
