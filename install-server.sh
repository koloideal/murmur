#!/bin/sh
# murmur relay installer — run as root:
#   curl -fsSL https://raw.githubusercontent.com/koloideal/murmur/main/install-server.sh | sudo sh
#
# Downloads murmur-server from the latest GitHub release, installs it as a
# systemd service and prints the bootstrap code for the first laptop.
set -eu

REPO="${MURMUR_REPO:-koloideal/murmur}"
PREFIX="${PREFIX:-/usr/local/bin}"

if [ "$(id -u)" -ne 0 ]; then
    echo "needs root — run: curl -fsSL ... | sudo sh" >&2
    exit 1
fi

case "$(uname -m)" in
    x86_64)        TARGET=x86_64-unknown-linux-musl ;;
    aarch64|arm64) TARGET=aarch64-unknown-linux-musl ;;
    *) echo "unsupported arch: $(uname -m)" >&2; exit 1 ;;
esac

URL="https://github.com/$REPO/releases/latest/download/murmur-server-$TARGET"
echo "==> $URL"
tmp=$(mktemp)
trap 'rm -f "$tmp"' EXIT
curl -fsSL "$URL" -o "$tmp"
install -m 755 "$tmp" "$PREFIX/murmur-server"

# systemd unit + start (idempotent: rewrites and restarts).
"$PREFIX/murmur-server" install ${MURMUR_ADDR:+--addr "$MURMUR_ADDR"}

sleep 1
code=$(journalctl -u murmur-server -n 30 --no-pager 2>/dev/null \
    | grep -o 'bootstrap code: [0-9A-Z-]*' | tail -1 || true)

echo
echo "murmur-server is up (systemd unit: murmur-server.service)."
if [ -n "$code" ]; then
    echo "==> $code"
    echo "    on your first laptop:  murmurd pair --server wss://YOUR.DOMAIN --code ${code#*bootstrap code: }"
else
    echo "    registry already has devices — pair new ones via `murmurctl invite`."
fi
echo
echo "The relay listens on :8770 in cleartext WS — put a TLS reverse proxy in"
echo "front (nginx/caddy) and give laptops the wss:// URL. See README.md."
