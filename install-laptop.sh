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

# ---- pretty
if [ -t 1 ]; then
    B=$(printf '\033[1m') D2=$(printf '\033[2m') R0=$(printf '\033[0m')
    C_AC=$(printf '\033[38;5;141m') C_CY=$(printf '\033[36m')
    C_GR=$(printf '\033[32m') C_YL=$(printf '\033[33m') C_RD=$(printf '\033[31m')
else
    B= D2= R0= C_AC= C_CY= C_GR= C_YL= C_RD=
fi

step() { printf '%s  ›%s %s\n' "$C_AC$B" "$R0" "$*"; }
ok()   { printf '%s  ✓%s %s\n' "$C_GR" "$R0" "$*"; }
warn() { printf '%s  !%s %s\n' "$C_YL" "$R0" "$*" >&2; }
die()  { printf '%s  ✗%s %s\n' "$C_RD" "$R0" "$*" >&2; exit 1; }

banner() {
    printf '%s%s' "$B" "$C_AC"
    cat <<'EOF'

   _ __ ___  _   _ _ __ _ __ ___  _   _ _ __
  | '_ ` _ \| | | | '__| '_ ` _ \| | | | '__|
  | | | | | | |_| | |  | | | | | | |_| | |
  |_| |_| |_|\__,_|_|  |_| |_| |_|\__,_|_|
EOF
    printf '%s' "$R0"
    printf '  %slaptop installer — the daemon + ctl for your machine%s\n\n' "$D2" "$R0"
}

dl() { # dl URL DEST
    if [ -t 1 ]; then curl -fL --progress-bar "$1" -o "$2"
    else curl -fsSL "$1" -o "$2"; fi
}

banner

[ "$(uname -m)" = "x86_64" ] || \
    die "releases only ship x86_64 laptop binaries — build from source instead"

mkdir -p "$BIN"
for tool in murmurd murmurctl; do
    step "downloading $tool"
    dl "https://github.com/$REPO/releases/latest/download/$tool-$TARGET" "$BIN/$tool.new"
    chmod +x "$BIN/$tool.new"
    mv "$BIN/$tool.new" "$BIN/$tool"
    ok "$BIN/$tool"
done

case ":$PATH:" in
    *":$BIN:"*) ;;
    *) warn "$BIN is not in PATH — add it or call binaries by full path" ;;
esac

# systemd user unit (starts with the graphical session). Non-fatal on
# headless/odd setups — the daemon can run bare.
step "systemd user unit"
if "$BIN/murmurd" install >/dev/null 2>&1; then
    ok "murmurd.service — starts with your session"
else
    warn "unit install failed — the daemon can still run bare: murmurd"
fi

if [ $# -gt 0 ]; then
    echo
    exec "$BIN/murmurd" pair "$@"
fi

printf '\n  %s┌─%s %s join the family %s\n' "$C_AC" "$R0" "$B" "$R0"
printf '  %s│%s\n' "$C_AC" "$R0"
printf '  %s│%s   founder (fresh relay)    %smurmurd pair --server wss://YOUR.DOMAIN --code <bootstrap code>%s\n' "$C_AC" "$R0" "$B" "$R0"
printf '  %s│%s   invited by another node  %smurmurd pair --code <XXXX-XXXX or invite string>%s\n' "$C_AC" "$R0" "$B" "$R0"
printf '  %s│%s\n' "$C_AC" "$R0"
printf '  %s│%s   then:  %ssystemctl --user start murmurd%s   and   %smurmurd doctor%s\n' "$C_AC" "$R0" "$B" "$R0" "$B" "$R0"
printf '  %s└────────────────────────────────────────%s\n\n' "$C_AC" "$R0"
