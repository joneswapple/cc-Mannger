#!/usr/bin/env sh
# Installs the headless CC Manager server (ccm-server) from GitHub releases.
#
#   curl -fsSL https://raw.githubusercontent.com/joneswapple/cc-Mannger/main/scripts/install.sh | sh
#
# Environment:
#   CCM_VERSION      release tag (default: latest)
#   CCM_INSTALL_DIR  target directory (default: ~/.local/bin)
#   CCM_SERVICE=1    also install a systemd --user service (Linux)
set -eu

REPO="joneswapple/cc-Mannger"
VERSION="${CCM_VERSION:-latest}"
DIR="${CCM_INSTALL_DIR:-$HOME/.local/bin}"

os=$(uname -s)
arch=$(uname -m)
case "$os" in
  Linux) os_part="unknown-linux-gnu" ;;
  Darwin) os_part="apple-darwin" ;;
  *) echo "unsupported OS: $os (use scripts/install.ps1 on Windows)" >&2; exit 1 ;;
esac
case "$arch" in
  x86_64 | amd64) arch_part="x86_64" ;;
  arm64 | aarch64) arch_part="aarch64" ;;
  *) echo "unsupported architecture: $arch" >&2; exit 1 ;;
esac
asset="ccm-server-${arch_part}-${os_part}.tar.gz"

if [ "$VERSION" = "latest" ]; then
  url="https://github.com/$REPO/releases/latest/download/$asset"
else
  url="https://github.com/$REPO/releases/download/$VERSION/$asset"
fi

tmp=$(mktemp -d)
trap 'rm -rf "$tmp"' EXIT
echo "downloading $url"
curl -fsSL "$url" -o "$tmp/$asset"
tar -xzf "$tmp/$asset" -C "$tmp"
mkdir -p "$DIR"
install -m 0755 "$tmp/ccm-server" "$DIR/ccm-server"
echo "installed $DIR/ccm-server"

case ":$PATH:" in
  *":$DIR:"*) ;;
  *) echo "note: $DIR is not on PATH; add it to your shell profile" ;;
esac

if [ "${CCM_SERVICE:-0}" = "1" ] && [ "$os" = "Linux" ]; then
  unit_dir="$HOME/.config/systemd/user"
  mkdir -p "$unit_dir"
  cat > "$unit_dir/ccm-server.service" <<UNIT
[Unit]
Description=CC Manager (Claude account proxy)
After=network-online.target

[Service]
ExecStart=$DIR/ccm-server serve
Restart=on-failure

[Install]
WantedBy=default.target
UNIT
  systemctl --user daemon-reload
  systemctl --user enable --now ccm-server.service
  echo "started systemd user service ccm-server (admin token: journalctl --user -u ccm-server)"
else
  echo "run: ccm-server serve   (the admin token is printed on first start)"
fi
