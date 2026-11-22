#!/bin/sh
# murmur relay installer — run as root:
#   curl -fsSL https://raw.githubusercontent.com/koloideal/murmur/main/install-server.sh | sudo sh
#
# Interactive: asks for the public domain, sets up the nginx vhost +
# certbot certificate, tunes ufw, installs the systemd service and
# prints the bootstrap code. Flags for non-interactive runs:
#   --domain D  --addr A  --email E  --no-nginx  --no-ufw  --yes
set -eu

REPO="${MURMUR_REPO:-koloideal/murmur}"
PREFIX="${PREFIX:-/usr/local/bin}"

DOMAIN="" ADDR="" EMAIL="" YES=""
NGINX_CFG=auto UFW_CFG=auto
while [ $# -gt 0 ]; do
    case "$1" in
        --domain)   DOMAIN="$2"; shift 2 ;;
        --addr)     ADDR="$2"; shift 2 ;;
        --email)    EMAIL="$2"; shift 2 ;;
        --no-nginx) NGINX_CFG=no; shift ;;
        --no-ufw)   UFW_CFG=no; shift ;;
        --yes|-y)   YES=yes; shift ;;
        *) echo "unknown flag: $1" >&2; exit 1 ;;
    esac
done

have_tty() { [ -r /dev/tty ] && [ -w /dev/tty ]; }

# ask VAR DEFAULT PROMPT — interactive unless --yes (then DEFAULT).
ask() {
    if [ -n "$YES" ] || ! have_tty; then
        echo "$1"; return
    fi
    printf "%s [%s]: " "$3" "${1:-—}" > /dev/tty
    read -r a < /dev/tty || a=""
    echo "${a:-$1}"
}

askyn() {
    def=$1
    a=$(ask "$def" "" "$2 (y/n)")
    case "$a" in
        y|Y|yes) return 0 ;;
        n|N|no)  return 1 ;;
        *)       [ "$def" = "y" ] ;;
    esac
}

[ "$(id -u)" -eq 0 ] || { echo "needs root — run: curl -fsSL ... | sudo sh" >&2; exit 1; }

# ---- questions first, so network failures don't waste the user's time
if [ -z "$DOMAIN" ] && [ "$NGINX_CFG" != "no" ]; then
    DOMAIN=$(ask "" "" "public domain for the wss endpoint (empty = plain ws, no TLS)")
fi

if [ -n "$DOMAIN" ] && [ "$NGINX_CFG" = "auto" ]; then
    if command -v nginx >/dev/null 2>&1; then
        askyn y "configure nginx vhost + certbot for $DOMAIN?" && NGINX_CFG=yes || NGINX_CFG=no
    else
        echo "note: nginx not installed — skipping vhost setup" >&2
        NGINX_CFG=no
    fi
fi

if [ "$NGINX_CFG" = "yes" ]; then
    ADDR=$(ask "127.0.0.1:8770" "" "relay listen address (localhost — nginx terminates TLS)")
else
    ADDR=$(ask "0.0.0.0:8770" "" "relay listen address")
fi

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

# ---- nginx + TLS
if [ "$NGINX_CFG" = "yes" ]; then
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

    email_flags=""
    if [ -n "$EMAIL" ]; then email_flags="--email $EMAIL"; else
        EMAIL=$(ask "" "" "email for Let's Encrypt (empty = --register-unsafely-without-email)")
        [ -n "$EMAIL" ] && email_flags="--email $EMAIL" || email_flags="--register-unsafely-without-email"
    fi
    # shellcheck disable=SC2086
    if certbot --nginx -d "$DOMAIN" --non-interactive --agree-tos $email_flags --redirect 2>&1; then
        echo "==> https://$DOMAIN ready"
    else
        echo "!! certbot failed — cert stays on :80 only; run it manually:" >&2
        echo "   certbot --nginx -d $DOMAIN" >&2
    fi
fi

# ---- firewall
if [ "$UFW_CFG" != "no" ] && command -v ufw >/dev/null 2>&1; then
    case "$ADDR" in
        127.0.0.1:*|localhost:*|\[::1\]:*) : ;;   # localhost — nothing to open
        *)
            port=${ADDR##*:}
            if askyn y "open $port/tcp in ufw (relay listens on $ADDR)?"; then
                ufw allow "$port/tcp"
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
