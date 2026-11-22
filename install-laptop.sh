#!/bin/sh
# murmur laptop installer — run as your user (no sudo):
#   curl -fsSL https://raw.githubusercontent.com/koloideal/murmur/main/install-laptop.sh | sh
# and optionally pair in one go:
#   ... | sh -s -- --server wss://relay.example --code ABCD-EFGH
#
# Downloads murmurd + murmurctl from the latest release into ~/.local/bin,
# registers the systemd user unit, then — if given args — runs `murmurd pair`.
set -eu

REPO="${MURMUR_REPO:-koloideal/murmur}"
BIN="${BIN_DIR:-$HOME/.local/bin}"
TARGET=x86_64-unknown-linux-gnu

if [ "$(uname -m)" != "x86_64" ]; then
    echo "releases only ship x86_64 laptop binaries — build from source instead" >&2
    exit 1
fi

mkdir -p "$BIN"
for tool in murmurd murmurctl; do
    URL="https://github.com/$REPO/releases/latest/download/$tool-$TARGET"
    echo "==> $URL"
    curl -fsSL "$URL" -o "$BIN/$tool.new"
    chmod +x "$BIN/$tool.new"
    mv "$BIN/$tool.new" "$BIN/$tool"
done

case ":$PATH:" in
    *":$BIN:"*) ;;
    *) echo "note: $BIN is not in PATH" ;;
esac

# systemd user unit (starts with the graphical session). Non-fatal on
# headless/odd setups — the daemon can run bare.
"$BIN/murmurd" install || echo "hint: unit install failed — run murmurd manually"

if [ $# -gt 0 ]; then
    echo
    exec "$BIN/murmurd" pair "$@"
fi

cat <<'EOF'

join the family — pick one:
  fresh relay (founder):   murmurd pair --server wss://YOUR.DOMAIN --code <bootstrap code from the server log>
  invited by another node: murmurd pair --code <invite string or XXXX-XXXX> [--server wss://…]

then:
  systemctl --user start murmurd
  murmurd doctor
EOF
