#!/bin/sh
# murmur client installer — run as your user (no sudo):
#   curl -fsSL https://raw.githubusercontent.com/koloideal/murmur/main/install-client.sh | sh
# or skip the questions and pair in one go:
#   ... | sh -s -- --server wss://relay.example --code ABCD-EFGH
# or found a serverless (lan-only, no relay) family on this machine:
#   ... | sh -s -- --serverless --name my-laptop
#
# Downloads murmurd + murmurctl from the latest release into ~/.local/bin,
# offers to put it on PATH, registers the systemd user unit, then offers
# to pair with your relay right away (Enter skips — pair manually later).
#
# MURMUR_BIN_SRC=/path/to/dir installs binaries from a local build
# instead of downloading (e.g. laptop/target/release).
set -eu

REPO="${MURMUR_REPO:-koloideal/murmur}"
BIN="${BIN_DIR:-$HOME/.local/bin}"
SRC="${MURMUR_BIN_SRC:-}"

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
    if [ -t 1 ]; then curl -fL --connect-timeout 15 --progress-bar "$1" -o "$2"
    else curl -fsSL --connect-timeout 15 "$1" -o "$2"; fi
    _rc=$?
    printf '\n'
    return $_rc
}

# remote_etag URL — last ETag header from a HEAD request, empty on failure
remote_etag() {
    curl -fsSIL --connect-timeout 10 "$1" 2>/dev/null | tr -d '\r' \
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
    # a failed/partial download must never reach DEST — drop the staging file
    if ! dl "$_url" "$_dest.new"; then rm -f "$_dest.new"; return 1; fi
    if ! install -m 755 "$_dest.new" "$_dest"; then rm -f "$_dest.new"; return 1; fi
    rm -f "$_dest.new"
    if [ -n "$_etag" ]; then printf '%s' "$_etag" > "$_etagf"; fi
    ok "$_dest"
}

# -r/-w on the device node always pass (it's 0666) — the real test is
# whether it OPENS (ENXIO without a controlling terminal). `true` not `:`:
# a failed redirect on a special builtin/compound cmd is fatal in POSIX sh,
# while `true` just returns non-zero. 2>/dev/null first so the error is muted
have_tty() { true 2>/dev/null < /dev/tty; }

v_url() {
    [ -z "$1" ] && return 0
    case "$1" in
        ws://*|wss://*|tcp://*) return 0 ;;
        *' '*|*/*) ;;
        *[a-zA-Z0-9]*) return 0 ;;
    esac
    printf "  %s✗%s expected a ws(s):// URL or the inviter's ip[:port]\n" "$C_RD" "$R0" >&2
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
        "join later:   ${B}murmurd pair --code INVITE-OR-XXXX-XXXX${R0}" \
        "              relay code → --server wss://DOMAIN" \
        "              broadcast blocked → --server INVITER-IP" \
        "or found one: ${B}murmurd init --kind serverless --name THIS-HOST${R0}" \
        "              (lan only — no relay, ever)" \
        "" \
        "then:  ${B}systemctl --user start murmurd${R0}   and   ${B}murmurd doctor${R0}"
}

banner
if [ "$(id -u)" -eq 0 ]; then
    warn "running as root — this installer is per-user; files would land in /root"
    warn "re-run as your normal user, without sudo"
    printf '\n' >&2
fi
if ! have_tty; then
    warn "no tty — questions are skipped (nothing gets paired)"
    warn "to pair non-interactively: ... | sh -s -- --server wss://HOST --code XXXX-XXXX"
    printf '\n' >&2
fi

case "$(uname -m)" in
    x86_64)        TARGET=x86_64-unknown-linux-gnu ;;
    aarch64|arm64) TARGET=aarch64-unknown-linux-gnu ;;
    *) die "no prebuilt client for $(uname -m) — build from source: cargo build --release --manifest-path laptop/Cargo.toml" ;;
esac

mkdir -p "$BIN"
if [ -n "$SRC" ]; then
    for tool in murmurd murmurctl; do
        step "$tool binary — local build"
        [ -x "$SRC/$tool" ] || die "$SRC/$tool not found — build first: cargo build --release --manifest-path laptop/Cargo.toml"
        install -m 755 "$SRC/$tool" "$BIN/$tool"
        ok "$BIN/$tool"
    done
else
    for tool in murmurd murmurctl; do
        step "$tool binary"
        dl_cached "https://github.com/$REPO/releases/latest/download/$tool-$TARGET" "$BIN/$tool"
    done
fi

# ---- shell completions — every binary prints its own via `completions`
_zfunc="$HOME/.zfunc"
_bashcomp="$HOME/.local/share/bash-completion/completions"
_fishcomp="$HOME/.config/fish/completions"

completions_for() { # completions_for TOOL
    [ -x "$BIN/$1" ] || return 0
    if [ -f "$HOME/.zshrc" ]; then
        mkdir -p "$_zfunc"
        "$BIN/$1" completions zsh > "$_zfunc/_$1" 2>/dev/null || rm -f "$_zfunc/_$1"
    fi
    mkdir -p "$_bashcomp" "$_fishcomp"
    "$BIN/$1" completions bash > "$_bashcomp/$1" 2>/dev/null || rm -f "$_bashcomp/$1"
    "$BIN/$1" completions fish > "$_fishcomp/$1.fish" 2>/dev/null || rm -f "$_fishcomp/$1.fish"
}

step "shell completions"
for tool in murmurd murmurctl; do completions_for "$tool"; done
ok "bash + fish completions installed"
if [ -f "$HOME/.zshrc" ]; then
    if grep -q "zfunc" "$HOME/.zshrc" 2>/dev/null; then
        ok "zsh completions → $_zfunc (already in fpath)"
    elif askyn y "zsh: add ~/.zfunc to fpath in .zshrc for completions?"; then
        # fpath must be set BEFORE compinit runs — a trailing fpath+= alone
        # does nothing, so append compinit too (running it twice is harmless)
        printf '\n# murmur completions\nfpath+=~/.zfunc\nautoload -Uz compinit && compinit\n' >> "$HOME/.zshrc"
        ok "fpath + compinit updated — open a new shell"
    fi
fi

# ---- PATH
case ":$PATH:" in
    *":$BIN:"*) : ;;
    *)
        if askyn y "add $BIN to PATH in your shell rc files?"; then
            line="export PATH=\"$BIN:\$PATH\"  # murmur"
            added=""
            # .bash_profile shadows .profile for login shells — write both
            for rc in "$HOME/.bash_profile" "$HOME/.profile" "$HOME/.bashrc" "$HOME/.zshrc"; do
                [ -f "$rc" ] || continue
                if ! grep -q '# murmur' "$rc"; then
                    printf '%s\n' "$line" >> "$rc"
                    ok "PATH → $rc"
                    added=yes
                fi
            done
            # fish reads none of the above — it has its own config + fish_add_path
            _fcfg="$HOME/.config/fish/config.fish"
            if [ -f "$_fcfg" ]; then
                if ! grep -q '# murmur' "$_fcfg"; then
                    printf 'fish_add_path %s  # murmur\n' "$BIN" >> "$_fcfg"
                    ok "PATH → $_fcfg"
                    added=yes
                fi
            fi
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

# ---- firewall: LAN discovery, links and pairing all need inbound —
# 17893/udp beacons, 17894/tcp links, 17895/tcp serverless pairing.
FW=""
if systemctl is-active --quiet ufw 2>/dev/null; then FW=ufw
elif systemctl is-active --quiet firewalld 2>/dev/null; then FW=firewalld
fi
if [ -n "$FW" ]; then
    if have_tty && askyn y "$FW is active — open murmur's lan ports (17893/udp + 17894-17895/tcp)?"; then
        case "$FW" in
            ufw)
                sudo ufw allow 17893/udp comment 'murmur lan discovery' \
                    && sudo ufw allow 17894/tcp comment 'murmur lan link' \
                    && sudo ufw allow 17895/tcp comment 'murmur pairing' \
                    && ok "ufw: murmur ports open" \
                    || warn "ufw rules failed — open 17893/udp + 17894-17895/tcp manually" ;;
            firewalld)
                sudo firewall-cmd --permanent --add-port=17893/udp \
                    --add-port=17894/tcp --add-port=17895/tcp \
                    && sudo firewall-cmd --reload \
                    && ok "firewalld: murmur ports open" \
                    || warn "firewalld rules failed — open 17893/udp + 17894-17895/tcp manually" ;;
        esac
    else
        warn "skipped — nothing can reach this machine on the lan:"
        warn "no phone links, no discovery, serverless pairing hangs (udp 17893 dropped)"
        warn "fix later: ${B}sudo $FW allow 17893/udp && sudo $FW allow 17894:17895/tcp${R0}"
    fi
fi

# ---- waybar island + player popup (optional, hyprland desktops)
# murmurt embeds every island asset — scripts, fonts, snippets — and
# marker-merges the waybar/hyprland configs itself. One binary install.
if command -v waybar >/dev/null 2>&1 || [ -n "${HYPRLAND_INSTANCE_SIGNATURE:-}" ]; then
    # tty-guarded: an unattended run must not touch waybar/hyprland configs
    if have_tty && askyn y "install the waybar island + player popup? (hyprland integration)"; then
        step "murmurt binary"
        _murt=""
        if [ -n "$SRC" ]; then
            if [ -x "$SRC/murmurt" ]; then
                install -m 755 "$SRC/murmurt" "$BIN/murmurt"
                _murt=yes
            else
                warn "$SRC/murmurt not found — skipping island"
            fi
        elif dl_cached "https://github.com/$REPO/releases/latest/download/murmurt-$TARGET" "$BIN/murmurt"; then
            _murt=yes
        else
            # murmurt is optional — a missing/404 release asset must not kill
            # the install before the pairing prompts below
            warn "murmurt download failed — skipping island (it may not be in the latest release yet)"
        fi
        if [ -n "$_murt" ]; then
            completions_for murmurt
            ok "$BIN/murmurt"
            step "murmurt install"
            if "$BIN/murmurt" install; then
                ok "island installed"
            else
                warn "murmurt install hit errors — check the output above"
            fi
            # Pick everything up without a re-login. `hyprctl reload`
            # merges the window rules and binds, but exec-once never
            # re-fires — so the daemons are started right here; waybar
            # reloads modules+css on SIGUSR2; swaync restarts to pick
            # up the murmur mirror rules and the island styling.
            if [ -n "${HYPRLAND_INSTANCE_SIGNATURE:-}" ] && command -v hyprctl >/dev/null 2>&1; then
                hyprctl reload >/dev/null 2>&1 || true
            fi
            if ! pgrep -f murmur-island-hover.py >/dev/null 2>&1; then
                setsid nohup "$BIN/murmur-island-hover.py" >/dev/null 2>&1 &
            fi
            if ! pgrep -f "murmur-player.py" >/dev/null 2>&1; then
                setsid nohup "$BIN/murmur-player.py" --daemon >/dev/null 2>&1 &
            fi
            pkill -SIGUSR2 -x waybar 2>/dev/null || true
            if pgrep -x swaync >/dev/null 2>&1; then
                pkill -x swaync
                sleep 1
                setsid nohup swaync >/dev/null 2>&1 &
            fi
            box "${B}island installed${R0}" \
                "" \
                "verify the runtime deps:" \
                "  ${D2}murmurt doctor${R0}" \
                "remove it cleanly:" \
                "  ${D2}murmurt uninstall${R0}"
        fi
    fi
fi

# ---- after a successful pair/init: daemon up + framed success + phone hint
# $1 = box title (default "you're in the family")
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

    box "${B}${C_GR}${1:-"you're in the family"}${R0}${FAM:+  ·  ${B}$FAM${R0}}" \
        "" \
        "the phone isn't connected yet —" \
        "invite it from this machine:" \
        "  ${B}murmurctl invite${R0}   ${D2}→ QR for the app, then answer y twice${R0}"
}

# ---- join: args beat questions beat the hint box
if [ $# -gt 0 ]; then
    # --serverless founds a lan-only family here — recognized anywhere in
    # the args, the rest (--name, --family) go straight to `murmurd init`.
    serverless=no; _nargs=$#
    for a in "$@"; do
        case "$a" in
            --serverless) serverless=yes ;;
            *) set -- "$@" "$a" ;;
        esac
    done
    shift "$_nargs"
    if [ "$serverless" = yes ]; then
        "$BIN/murmurd" init --kind serverless "$@" || die "init failed"
        post_pair "you founded a family"
        exit 0
    fi
    # sanity-check pair args before they hit murmurd
    prev=""
    for a in "$@"; do
        case "$prev" in
            --server) v_url "$a" || die "--server must be a ws(s):// URL or the inviter's ip[:port] (got: $a)" ;;
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

# ---- join: one input question, follow-ups only for what it demands.
# A full invite string carries endpoint + family kind inside it — the
# joiner never has to know the topology. A bare XXXX-XXXX short code
# needs one more bit of context: which relay (serverful) or none
# (serverless — the LAN beacon finds the inviter).
# Already paired? The whole handshake would finish just for provision()
# to refuse overwriting an existing family — skip straight to running.
if [ -f "$HOME/.config/murmur/config.toml" ]; then
    ok "already in a family (config.toml exists) — skipping pair"
    post_pair
    exit 0
fi
if have_tty && askyn y "got an invite or code from your family? (murmurctl invite on a member)"; then
    CODE=$(ask "" "" "paste the invite string or XXXX-XXXX code")
    if [ -n "$CODE" ]; then
        # case-normalize for the short-code shape check — the value
        # itself goes to murmurd untouched
        _cu=$(printf '%s' "$CODE" | tr 'a-z' 'A-Z')
        case "$_cu" in
            [A-Z0-9][A-Z0-9][A-Z0-9][A-Z0-9]-[A-Z0-9][A-Z0-9][A-Z0-9][A-Z0-9])
                SERVER=$(ask v_url "" "relay URL or inviter ip[:port] — wss://domain / ws://ip:port (empty = serverless, find on LAN)")
                if [ -n "$SERVER" ]; then
                    "$BIN/murmurd" pair --server "$SERVER" --code "$CODE" < /dev/tty || die "pairing failed"
                else
                    "$BIN/murmurd" pair --code "$CODE" < /dev/tty || die "pairing failed"
                fi ;;
            *)
                "$BIN/murmurd" pair --code "$CODE" < /dev/tty || die "pairing failed" ;;
        esac
        post_pair
        exit 0
    fi
    warn "nothing entered — nothing paired"
fi

if have_tty && askyn n "found a serverless family on this machine? (lan only — no relay, ever)"; then
    NAME=$(ask "" "$(uname -n)" "device name")
    FAMN=$(ask "" "" "family name (optional)")
    if [ -n "$FAMN" ]; then
        "$BIN/murmurd" init --kind serverless --name "$NAME" --family "$FAMN" < /dev/tty || die "init failed"
    else
        "$BIN/murmurd" init --kind serverless --name "$NAME" < /dev/tty || die "init failed"
    fi
    post_pair "you founded a family"
    exit 0
fi

hint_box
