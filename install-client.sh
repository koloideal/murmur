#!/bin/sh
# murmur client installer — run as your user (no sudo):
#   curl -fsSL https://raw.githubusercontent.com/koloideal/murmur/main/install-client.sh | sh
# or skip the questions and pair in one go:
#   ... | sh -s -- --server wss://relay.example --code ABCD-EFGH
#
# Downloads murmurd + murmurctl from the latest release into ~/.local/bin,
# offers to put it on PATH, registers the systemd user unit, then offers
# to pair with your relay right away (Enter skips — pair manually later).
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

step() { printf '\n%s  ›%s %s\n' "$C_AC$B" "$R0" "$*"; }
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
    printf '  %sclient installer — murmurd + murmurctl for any linux machine%s\n' "$D2" "$R0"
}

dl() { # dl URL DEST
    if [ -t 1 ]; then curl -fL --progress-bar "$1" -o "$2"
    else curl -fsSL "$1" -o "$2"; fi
    printf '\n'
}

# remote_etag URL — last ETag header from a HEAD request, empty on failure
remote_etag() {
    curl -fsSIL "$1" 2>/dev/null | tr -d '\r' \
        | awk 'tolower($1) == "etag:" {e=$2} END {print e}'
}

# dl_cached URL DEST — skip the download when DEST's stored etag
# already matches the remote one; installs atomically otherwise.
dl_cached() {
    _url=$1 _dest=$2 _etagf="$_dest.etag"
    _etag=$(remote_etag "$_url")
    if [ -f "$_dest" ] && [ -n "$_etag" ] && \
       [ "$_etag" = "$(cat "$_etagf" 2>/dev/null)" ]; then
        ok "$_dest — already latest, skipping download"
        return 0
    fi
    dl "$_url" "$_dest.new"
    install -m 755 "$_dest.new" "$_dest" && rm -f "$_dest.new"
    if [ -n "$_etag" ]; then printf '%s' "$_etag" > "$_etagf"; fi
    ok "$_dest"
}

have_tty() { [ -r /dev/tty ] && [ -w /dev/tty ]; }

v_url() {
    [ -z "$1" ] && return 0
    case "$1" in ws://*|wss://*) return 0 ;; esac
    printf '  %s✗%s expected a ws:// or wss:// URL\n' "$C_RD" "$R0" >&2
    return 1
}

# ask VALIDATOR DEFAULT PROMPT — re-asks until valid; non-tty = DEFAULT.
ask() {
    _v=$1
    while :; do
        if ! have_tty; then echo "$2"; return; fi
        printf '\n' > /dev/tty
        printf '%s  ?%s %s %s[%s]%s\n' "$C_CY" "$R0" "$3" "$D2" "${2:-—}" "$R0" > /dev/tty
        printf '%s  ›%s ' "$C_AC" "$R0" > /dev/tty
        read -r _a < /dev/tty || _a=""
        _a=${_a:-$2}
        if [ -z "$_v" ] || "$_v" "$_a"; then echo "$_a"; return; fi
    done
}

# askyn DEFAULT PROMPT — re-asks until y/n; empty = DEFAULT.
askyn() {
    while :; do
        _a=$(ask "" "$1" "$2 (y/n)")
        case "$_a" in
            y|Y|yes) return 0 ;;
            n|N|no)  return 1 ;;
            *)       printf '  %s✗%s answer y or n (Enter = %s)\n' "$C_RD" "$R0" "$1" > /dev/tty ;;
        esac
    done
}

# vlen: visible length (strip ANSI escapes, count chars not bytes)
vlen() { printf '%s' "$1" | sed "s/$(printf '\033')\\[[0-9;]*m//g" | wc -m; }

# box LINE... — full frame, auto-width on the longest visible line
box() {
    w=0
    for _l in "$@"; do
        _n=$(vlen "$_l")
        if [ "$_n" -gt "$w" ]; then w=$_n; fi
    done
    bar=""; _i=0; while [ $_i -lt $((w + 2)) ]; do bar="${bar}─"; _i=$((_i + 1)); done
    printf '\n  %s╭%s╮%s\n' "$C_AC" "$bar" "$R0"
    for _l in "$@"; do
        printf '  %s│%s %s%*s %s│%s\n' "$C_AC" "$R0" "$_l" $((w - $(vlen "$_l"))) '' "$C_AC" "$R0"
    done
    printf '  %s╰%s╯%s\n\n' "$C_AC" "$bar" "$R0"
}

hint_box() {
    box "${B}installed${R0}" \
        "" \
        "join later:  ${B}murmurd pair --server wss://YOUR.DOMAIN --code XXXX-XXXX${R0}" \
        "             (full invite strings work too — server is inside)" \
        "" \
        "then:  ${B}systemctl --user start murmurd${R0}   and   ${B}murmurd doctor${R0}"
}

banner
[ "$(uname -m)" = "x86_64" ] || \
    die "releases only ship x86_64 client binaries — build from source instead"

mkdir -p "$BIN"
for tool in murmurd murmurctl; do
    step "$tool binary"
    dl_cached "https://github.com/$REPO/releases/latest/download/$tool-$TARGET" "$BIN/$tool"
done

# ---- PATH
case ":$PATH:" in
    *":$BIN:"*) : ;;
    *)
        if askyn y "add $BIN to PATH in your shell rc files?"; then
            line='export PATH="$HOME/.local/bin:$PATH"  # murmur'
            added=""
            for rc in "$HOME/.profile" "$HOME/.bashrc" "$HOME/.zshrc"; do
                [ -f "$rc" ] || continue
                if ! grep -q '# murmur' "$rc"; then
                    printf '%s\n' "$line" >> "$rc"
                    ok "PATH → $rc"
                    added=yes
                fi
            done
            if [ -z "$added" ]; then
                printf '%s\n' "$line" >> "$HOME/.profile"
                ok "PATH → $HOME/.profile"
            fi
            warn "open a new shell or run now: export PATH=\"$BIN:\$PATH\""
        else
            warn "skipped — call the binaries by full path or add $BIN to PATH yourself"
        fi ;;
esac

# ---- systemd user unit (starts with the graphical session)
step "systemd user unit"
if "$BIN/murmurd" install >/dev/null 2>&1; then
    ok "murmurd.service — starts with your session"
else
    warn "unit install failed — the daemon can still run bare: murmurd"
fi

# ---- after a successful pair: daemon up + framed success + phone hint
post_pair() {
    step "starting murmurd"
    if systemctl --user start murmurd 2>/dev/null; then
        ok "murmurd is running"
    else
        warn "couldn't start it here — run inside your graphical session:"
        printf '      systemctl --user start murmurd\n'
    fi

    # family name — murmurctl devices prints "family · X" once the
    # daemon is up and registered; the socket takes a beat
    FAM=""; _i=0
    while [ $_i -lt 5 ]; do
        FAM=$("$BIN/murmurctl" devices 2>/dev/null | sed -n 's/^family · //p')
        [ -n "$FAM" ] && break
        sleep 1; _i=$((_i + 1))
    done

    box "${B}${C_GR}you're in the family${R0}${FAM:+  ·  ${B}$FAM${R0}}" \
        "" \
        "the phone isn't connected yet —" \
        "invite it from this machine:" \
        "  ${B}murmurctl invite${R0}   ${D2}→ one-shot code / QR for the app${R0}"
}

# ---- join: args beat questions beat the hint box
if [ $# -gt 0 ]; then
    # sanity-check pair args before they hit murmurd
    prev=""
    for a in "$@"; do
        case "$prev" in
            --server) v_url "$a" || die "--server must be a ws:// or wss:// URL (got: $a)" ;;
            --code)
                printf '%s' "$a" | grep -Eq '^[A-Z0-9]{4}-[A-Z0-9]{4}$|^.{20,}$' || \
                    warn "--code usually looks like XXXX-XXXX or a full invite — continuing anyway"
                ;;
        esac
        prev=$a
    done
    echo
    # murmurd's SAS prompt reads stdin — hand it the real terminal,
    # not the exhausted curl pipe.
    if have_tty; then
        "$BIN/murmurd" pair "$@" < /dev/tty || die "pairing failed"
    else
        "$BIN/murmurd" pair "$@" || die "pairing failed"
    fi
    post_pair
    exit 0
fi

if have_tty && askyn n "join the family now?"; then
    SERVER=$(ask v_url "" "relay server URL — wss://domain or ws://ip:port")
    CODE=$(ask "" "" "invite code — XXXX-XXXX or a full invite string")
    pair_args=""
    if [ -n "$SERVER" ]; then pair_args="--server $SERVER"; fi
    if [ -n "$CODE" ];   then pair_args="$pair_args --code $CODE"; fi
    if [ -n "$pair_args" ]; then
        # shellcheck disable=SC2086
        "$BIN/murmurd" pair $pair_args < /dev/tty || die "pairing failed"
        post_pair
        exit 0
    fi
    warn "nothing entered — nothing paired"
fi

hint_box
