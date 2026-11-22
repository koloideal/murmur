#!/bin/sh
# murmur relay installer — run as root:
#   curl -fsSL https://raw.githubusercontent.com/koloideal/murmur/main/install-server.sh | sudo sh
#
# Interactive: asks for the public domain, detects the proxy stack on
# the box (nginx → vhost + certbot, caddy → Caddyfile block w/ auto TLS),
# tunes whichever firewall exists (ufw / firewalld), installs the
# systemd service and prints the bootstrap code.
# Flags for non-interactive runs:
#   --domain D  --addr A  --email E  --no-proxy  --no-firewall  --yes
set -eu

REPO="${MURMUR_REPO:-koloideal/murmur}"
PREFIX="${PREFIX:-/usr/local/bin}"

DOMAIN="" ADDR="" EMAIL="" YES=""
PROXY_CFG=auto FW_CFG=auto
while [ $# -gt 0 ]; do
    case "$1" in
        --domain)      DOMAIN="$2"; shift 2 ;;
        --addr)        ADDR="$2"; shift 2 ;;
        --email)       EMAIL="$2"; shift 2 ;;
        --no-proxy)    PROXY_CFG=no; shift ;;
        --no-firewall) FW_CFG=no; shift ;;
        --yes|-y)      YES=yes; shift ;;
        *) echo "unknown flag: $1" >&2; exit 1 ;;
    esac
done

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
    printf '  %srelay installer — private device sync, self-hosted%s\n\n' "$D2" "$R0"
}

dl() { # dl URL DEST
    if [ -t 1 ]; then curl -fL --progress-bar "$1" -o "$2"
    else curl -fsSL "$1" -o "$2"; fi
}

have_tty() { [ -r /dev/tty ] && [ -w /dev/tty ]; }

# ask DEFAULT IGNORED PROMPT — interactive unless --yes (then DEFAULT).
ask() {
    if [ -n "$YES" ] || ! have_tty; then
        echo "$1"; return
    fi
    printf '%s  ?%s %s %s[%s]%s\n' "$C_CY" "$R0" "$3" "$D2" "${1:-—}" "$R0" > /dev/tty
    printf '%s  ›%s ' "$C_AC" "$R0" > /dev/tty
    read -r a < /dev/tty || a=""
    echo "${a:-$1}"
}

askyn() {
    a=$(ask "$1" "" "$2")
    case "$a" in
        y|Y|yes) return 0 ;;
        n|N|no)  return 1 ;;
        *)       [ "$1" = "y" ] ;;
    esac
}

# fw_open PORT — ufw / firewalld / print the nft rule for the rest.
fw_open() {
    port=$1
    if command -v ufw >/dev/null 2>&1; then
        ufw allow "$port/tcp" >/dev/null && ok "ufw: $port/tcp allowed"
    elif command -v firewall-cmd >/dev/null 2>&1; then
        firewall-cmd --permanent --add-port="$port/tcp" >/dev/null && \
            firewall-cmd --reload >/dev/null && ok "firewalld: $port/tcp allowed"
    elif command -v nft >/dev/null 2>&1; then
        warn "nftables: open $port/tcp — nft add rule inet filter input tcp dport $port accept"
    elif command -v iptables >/dev/null 2>&1; then
        warn "iptables: open $port/tcp — iptables -A INPUT -p tcp --dport $port -j ACCEPT"
    else
        warn "no firewall tool found — open $port/tcp manually if needed"
    fi
}

banner
[ "$(id -u)" -eq 0 ] || die "needs root — run: curl -fsSL ... | sudo sh"

# ---- questions first, so failures don't waste the user's time
if [ -z "$DOMAIN" ] && [ "$PROXY_CFG" != "no" ]; then
    DOMAIN=$(ask "" "" "public domain for the wss endpoint (empty = plain ws, no TLS)")
fi

# Proxy detection: prefer what's already installed. nginx needs certbot
# for TLS; caddy issues certs itself.
PROXY=""
if [ -n "$DOMAIN" ] && [ "$PROXY_CFG" != "no" ]; then
    if command -v nginx >/dev/null 2>&1; then found=nginx
    elif command -v caddy >/dev/null 2>&1; then found=caddy
    else found=""
    fi
    if [ -n "$found" ]; then
        askyn y "configure $found as TLS proxy for $DOMAIN?" && PROXY=$found || PROXY=none
    else
        warn "no nginx/caddy found — relay stays plain ws, TLS is on you"
        PROXY=none
    fi
fi

case "$PROXY" in
    nginx|caddy) ADDR=$(ask "127.0.0.1:8770" "" "relay listen address (localhost — proxy terminates TLS)") ;;
    *)           ADDR=$(ask "0.0.0.0:8770" "" "relay listen address") ;;
esac

# ---- binary
case "$(uname -m)" in
    x86_64)        TARGET=x86_64-unknown-linux-musl ;;
    aarch64|arm64) TARGET=aarch64-unknown-linux-musl ;;
    *) die "unsupported arch: $(uname -m)" ;;
esac

URL="https://github.com/$REPO/releases/latest/download/murmur-server-$TARGET"
step "downloading murmur-server ($TARGET)"
tmp=$(mktemp); trap 'rm -f "$tmp"' EXIT
dl "$URL" "$tmp"
install -m 755 "$tmp" "$PREFIX/murmur-server"
ok "$PREFIX/murmur-server"

# ---- proxy config + TLS
case "$PROXY" in
    nginx)
        step "nginx vhost for $DOMAIN"
        vh="/etc/nginx/sites-available/$DOMAIN"
        cat > "$vh" <<EOF
server {
    server_name $DOMAIN;
    location / {
        proxy_pass http://$ADDR;
        proxy_http_version 1.1;
        # Cloudflare strips Upgrade to origin — hardcode it (WS-only vhost).
        proxy_set_header Upgrade "websocket";
        proxy_set_header Connection "upgrade";
        proxy_read_timeout 86400s;
        proxy_send_timeout 86400s;
    }
    listen 80;
    listen [::]:80;
}
EOF
        ln -sf "$vh" "/etc/nginx/sites-enabled/$DOMAIN"
        nginx -t && systemctl reload nginx
        ok "$vh enabled"

        if ! command -v certbot >/dev/null 2>&1; then
            if command -v apt-get >/dev/null 2>&1 && \
               askyn y "certbot not installed — install certbot + nginx plugin via apt?"; then
                apt-get update -qq && apt-get install -y -qq certbot python3-certbot-nginx >/dev/null
                ok "certbot installed"
            fi
        fi
        if command -v certbot >/dev/null 2>&1; then
            if [ -z "$EMAIL" ]; then
                EMAIL=$(ask "" "" "email for Let's Encrypt (empty = --register-unsafely-without-email)")
            fi
            [ -n "$EMAIL" ] && ef="--email $EMAIL" || ef="--register-unsafely-without-email"
            # shellcheck disable=SC2086
            if certbot --nginx -d "$DOMAIN" --non-interactive --agree-tos $ef --redirect; then
                ok "https://$DOMAIN — certificate installed"
            else
                warn "certbot failed — run it manually: certbot --nginx -d $DOMAIN"
            fi
        else
            warn "no certbot — get the cert yourself, then add 443 to $vh"
        fi
        ;;
    caddy)
        step "caddy site for $DOMAIN"
        cf="/etc/caddy/Caddyfile"
        if ! grep -q "^$DOMAIN" "$cf" 2>/dev/null; then
            cat >> "$cf" <<EOF

$DOMAIN {
    reverse_proxy $ADDR
}
EOF
        fi
        systemctl reload caddy 2>/dev/null || caddy reload --config "$cf" 2>/dev/null || true
        ok "https://$DOMAIN — caddy issues the cert itself"
        ;;
esac

# ---- firewall: only what actually needs an opening
if [ "$FW_CFG" != "no" ]; then
    case "$PROXY" in
        nginx|caddy)
            if askyn y "open 80+443/tcp in the firewall (proxy needs them from outside)?"; then
                fw_open 80; fw_open 443
            fi ;;
    esac
    case "$ADDR" in
        127.0.0.1:*|localhost:*|\[::1\]:*) : ;;   # localhost — nothing to open
        *)
            port=${ADDR##*:}
            if askyn y "open $port/tcp in the firewall (relay listens on $ADDR)?"; then
                fw_open "$port"
            fi ;;
    esac
fi

# ---- systemd unit + start (idempotent)
step "systemd service"
"$PREFIX/murmur-server" install --addr "$ADDR" >/dev/null
ok "murmur-server.service enabled + started ($ADDR)"

sleep 1
code=$(journalctl -u murmur-server -n 30 --no-pager 2>/dev/null \
    | grep -o 'bootstrap code: [0-9A-Z-]*' | tail -1 || true)

if [ -n "$DOMAIN" ]; then EP="wss://$DOMAIN"; else EP="ws://$ADDR"; fi

printf '\n  %s┌─%s %s murmur-server is up %s\n' "$C_AC" "$R0" "$B" "$R0"
printf '  %s│%s\n' "$C_AC" "$R0"
printf '  %s│%s   endpoint     %s%s%s\n' "$C_AC" "$R0" "$C_CY" "$EP" "$R0"
if [ -n "$code" ]; then
    short=${code##*bootstrap code: }
    printf '  %s│%s   bootstrap    %s%s%s%s  (one-shot, founder becomes the family)\n' "$C_AC" "$R0" "$B" "$C_GR" "$short" "$R0"
    printf '  %s│%s\n' "$C_AC" "$R0"
    printf '  %s│%s   first laptop:  %smurmurd pair --server %s --code %s%s\n' "$C_AC" "$R0" "$B" "$EP" "$short" "$R0"
else
    printf '  %s│%s   family already exists — invite new devices via %smurmurctl invite%s\n' "$C_AC" "$R0" "$B" "$R0"
fi
printf '  %s└────────────────────────────────────────%s\n\n' "$C_AC" "$R0"
