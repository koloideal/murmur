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

have_tty() { [ -r /dev/tty ] && [ -w /dev/tty ]; }

# ask DEFAULT IGNORED PROMPT — interactive unless --yes (then DEFAULT).
ask() {
    if [ -n "$YES" ] || ! have_tty; then
        echo "$1"; return
    fi
    printf "%s [%s]: " "$3" "${1:-—}" > /dev/tty
    read -r a < /dev/tty || a=""
    echo "${a:-$1}"
}

askyn() {
    a=$(ask "$1" "" "$2 (y/n)")
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
        ufw allow "$port/tcp"
    elif command -v firewall-cmd >/dev/null 2>&1; then
        firewall-cmd --permanent --add-port="$port/tcp" && firewall-cmd --reload
    elif command -v nft >/dev/null 2>&1; then
        echo "   open $port/tcp — e.g.: nft add rule inet filter input tcp dport $port accept"
    elif command -v iptables >/dev/null 2>&1; then
        echo "   open $port/tcp — e.g.: iptables -A INPUT -p tcp --dport $port -j ACCEPT"
    else
        echo "   open $port/tcp in your firewall"
    fi
}

[ "$(id -u)" -eq 0 ] || { echo "needs root — run: curl -fsSL ... | sudo sh" >&2; exit 1; }

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
        echo "note: no nginx/caddy found — relay stays plain ws, TLS is on you" >&2
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
    *) echo "unsupported arch: $(uname -m)" >&2; exit 1 ;;
esac

URL="https://github.com/$REPO/releases/latest/download/murmur-server-$TARGET"
echo "==> $URL"
tmp=$(mktemp); trap 'rm -f "$tmp"' EXIT
curl -fsSL "$URL" -o "$tmp"
install -m 755 "$tmp" "$PREFIX/murmur-server"

# ---- proxy config + TLS
case "$PROXY" in
    nginx)
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

        if ! command -v certbot >/dev/null 2>&1; then
            if command -v apt-get >/dev/null 2>&1 && \
               askyn y "certbot not installed — install certbot + nginx plugin via apt?"; then
                apt-get update && apt-get install -y certbot python3-certbot-nginx
            fi
        fi
        if command -v certbot >/dev/null 2>&1; then
            if [ -z "$EMAIL" ]; then
                EMAIL=$(ask "" "" "email for Let's Encrypt (empty = --register-unsafely-without-email)")
            fi
            [ -n "$EMAIL" ] && ef="--email $EMAIL" || ef="--register-unsafely-without-email"
            # shellcheck disable=SC2086
            if certbot --nginx -d "$DOMAIN" --non-interactive --agree-tos $ef --redirect; then
                echo "==> https://$DOMAIN ready"
            else
                echo "!! certbot failed — run it manually: certbot --nginx -d $DOMAIN" >&2
            fi
        else
            echo "!! no certbot — get the cert yourself, then add 443 to $vh" >&2
        fi
        ;;
    caddy)
        cf="/etc/caddy/Caddyfile"
        if ! grep -q "^$DOMAIN" "$cf" 2>/dev/null; then
            cat >> "$cf" <<EOF

$DOMAIN {
    reverse_proxy $ADDR
}
EOF
        fi
        systemctl reload caddy 2>/dev/null || caddy reload --config "$cf" 2>/dev/null || true
        echo "==> caddy serves https://$DOMAIN (cert is automatic)"
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
"$PREFIX/murmur-server" install --addr "$ADDR"

sleep 1
code=$(journalctl -u murmur-server -n 30 --no-pager 2>/dev/null \
    | grep -o 'bootstrap code: [0-9A-Z-]*' | tail -1 || true)

echo
echo "=== murmur-server is up ==="
if [ -n "$DOMAIN" ]; then
    echo "public endpoint: wss://$DOMAIN/"
else
    echo "relay endpoint: ws://$ADDR  (no TLS — private nets only)"
fi
if [ -n "$code" ]; then
    short=${code##*bootstrap code: }
    echo "$code"
    echo
    echo "first laptop — install then:"
    echo "  murmurd pair --server wss://$DOMAIN --code $short"
else
    echo "registry already has devices — new ones pair via \`murmurctl invite\`"
fi
