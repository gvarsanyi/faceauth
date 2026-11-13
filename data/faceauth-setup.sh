#!/bin/bash
# faceauth-setup: system integration steps shared by "make install" and the
# distribution packages' maintainer scripts.
#
#   faceauth-setup configure    create the daemon user, enable the PAM
#                               module for the services in faceauth.conf
#                               (pam-auth-update on Debian, otherwise one
#                               marked line per /etc/pam.d file), reload
#   faceauth-setup unconfigure  disable the PAM module, stop the daemon
#   faceauth-setup purge        delete all enrolled face data
#
# Must run as root. Every step is idempotent.

set -euo pipefail

STATE_DIR=/var/lib/faceauth
DAEMON_USER=faceauthd
UNIT=faceauth-daemon.service
KDE_SLOT=/etc/pam.d/kde-fingerprint
SYSUSERS_CONF=@SYSUSERSDIR@/faceauth.conf
CONFIG=/etc/faceauth/faceauth.conf
# Keep in sync with Config::default() in faceauth-common/src/config.rs.
DEFAULT_SERVICES="sudo sudo-i polkit-1 kde-fingerprint"
PAM_DIR=/etc/pam.d
VENDOR_PAM_DIR=/usr/lib/pam.d
AUTH_RE='^[[:space:]]*-?auth[[:space:]]'
MARKER="# Added by faceauth-setup; removed by 'faceauth-setup unconfigure'."
MODULE_LINE="auth     sufficient     pam_faceauth.so"

log() { echo "faceauth-setup: $*"; }

have() { command -v "$1" >/dev/null 2>&1; }

create_user() {
    if have systemd-sysusers && [[ -f $SYSUSERS_CONF ]]; then
        systemd-sysusers "$SYSUSERS_CONF"
    elif ! id "$DAEMON_USER" >/dev/null 2>&1; then
        useradd --system --no-create-home --home-dir / --shell /usr/sbin/nologin \
            --groups video "$DAEMON_USER"
    fi
}

kde_slot_is_ours() {
    [[ -f $KDE_SLOT ]] && ! grep -Ev '^[[:space:]]*(#|$)' "$KDE_SLOT" | grep -qv 'pam_faceauth\|pam_deny'
}

SERVICE_RE='^[A-Za-z0-9][A-Za-z0-9_.-]{0,63}$'

# Services from the admin allow-list, with the daemon's rules
# (faceauth.conf(5)): the last "services" line wins, even if empty; without
# one, or if it has an invalid name, the defaults apply.
configured_services() {
    local line svc
    local -
    set -f # no filename expansion of the unquoted list below
    line=$(grep -E '^[[:space:]]*services[[:space:]]*=' "$CONFIG" 2>/dev/null | tail -1) || true
    [[ -n $line ]] || { echo "$DEFAULT_SERVICES"; return; }
    line=${line#*=}
    line=${line%%#*}
    for svc in ${line//,/ }; do
        if [[ ! $svc =~ $SERVICE_RE ]]; then
            log "invalid service name '$svc' in $CONFIG; using the defaults" >&2
            echo "$DEFAULT_SERVICES"
            return
        fi
    done
    echo "${line//,/ }"
}

# Insert the module before the first auth line of /etc/pam.d/SERVICE, copying
# the vendor file from /usr/lib/pam.d first if needed. Idempotent.
add_to_service() {
    local svc=$1 file=$PAM_DIR/$1 src=$PAM_DIR/$1
    [[ -e $src ]] || src=$VENDOR_PAM_DIR/$svc
    if [[ ! -f $src ]]; then
        log "PAM service $svc not found; skipped"
        return
    fi
    grep -qE '^[^#]*pam_faceauth' "$src" && return
    if ! grep -qE "$AUTH_RE" "$src"; then
        log "$src has no auth lines; skipped"
        return
    fi
    awk -v re="$AUTH_RE" -v marker="$MARKER" -v line="$MODULE_LINE" \
        '!done && $0 ~ re { print marker; print line; done = 1 } { print }' "$src" > "$file.faceauth-new"
    chmod 0644 "$file.faceauth-new"
    mv -f "$file.faceauth-new" "$file"
    log "enabled pam_faceauth in $file"
}

# Remove every marked module line; drop /etc copies that are then identical
# to the vendor file so vendor updates apply again.
remove_from_services() {
    local file
    for file in "$PAM_DIR"/*; do
        [[ -f $file ]] || continue
        grep -qxF "$MARKER" "$file" || continue
        awk -v marker="$MARKER" 'skip { skip = 0; next } $0 == marker { skip = 1; next } { print }' \
            "$file" > "$file.faceauth-new"
        if cmp -s "$file.faceauth-new" "$VENDOR_PAM_DIR/${file##*/}"; then
            rm -f "$file" "$file.faceauth-new"
            log "removed $file (vendor $VENDOR_PAM_DIR/${file##*/} applies again)"
        else
            chmod 0644 "$file.faceauth-new"
            mv -f "$file.faceauth-new" "$file"
            log "disabled pam_faceauth in $file"
        fi
    done
}

# The sudo Defaults in effect for USER, one per line ("targetpw", "!insults").
sudo_defaults() {
    sudo -l -U "$1" 2>/dev/null |
        sed -n '/^Matching Defaults entries/,/^[^[:space:]]/{/^[[:space:]]/p}' |
        tr ',' '\n' | sed 's/^[[:space:]]*//; s/[[:space:]]*$//'
}

# With targetpw/rootpw/runaspw, sudo authenticates the target account (root)
# instead of the invoking user, so face authentication never applies.
check_sudo_policy() {
    local user=${SUDO_USER:-}
    [[ -n $user && $user != root ]] && have sudo || return 0
    configured_services | tr ' ' '\n' | grep -qx 'sudo' || return 0
    if sudo_defaults "$user" | grep -qxE 'targetpw|rootpw|runaspw'; then
        log "note: sudo asks for the target user's password (targetpw/rootpw), so"
        log "face authentication cannot approve sudo for $user. To use it, make sudo"
        log "authenticate the invoking user (openSUSE: sudo-policy-wheel-auth-self)."
    fi
}

enable_pam() {
    # KDE Plasma lock screen: hands-free unlock slot, unless fprintd owns it.
    if [[ -f $PAM_DIR/kde || -f $VENDOR_PAM_DIR/kde ]] && [[ ! -e $KDE_SLOT ]]; then
        install -m 0644 @DATADIR@/faceauth/kde-fingerprint "$KDE_SLOT"
        log "created $KDE_SLOT"
    fi
    if have pam-auth-update && [[ -f /usr/share/pam-configs/faceauth ]]; then
        pam-auth-update --package --enable faceauth
        log "enabled pam_faceauth via pam-auth-update"
        return
    fi
    local svc
    for svc in $(configured_services); do
        add_to_service "$svc"
    done
    log "after changing 'services' in $CONFIG, run: faceauth-setup configure"
}

disable_pam() {
    if have pam-auth-update && [[ -f /usr/share/pam-configs/faceauth ]]; then
        pam-auth-update --package --remove faceauth || true
    fi
    remove_from_services
    if kde_slot_is_ours; then
        rm -f "$KDE_SLOT"
        log "removed $KDE_SLOT"
    fi
}

reload_services() {
    if have systemctl && [[ -d /run/systemd/system ]]; then
        systemctl daemon-reload
        systemctl reload dbus.service 2>/dev/null || systemctl reload dbus-broker.service 2>/dev/null || true
        # Pick up the new binary on the next D-Bus activation.
        systemctl try-restart "$UNIT" || true
    fi
}

[[ $EUID -eq 0 ]] || { echo "faceauth-setup: must run as root" >&2; exit 1; }

case "${1:-}" in
    configure)
        create_user
        enable_pam
        reload_services
        check_sudo_policy
        ;;
    unconfigure)
        disable_pam
        if have systemctl && [[ -d /run/systemd/system ]]; then
            systemctl stop "$UNIT" 2>/dev/null || true
        fi
        ;;
    purge)
        rm -rf "$STATE_DIR"
        log "deleted enrolled face data in $STATE_DIR"
        ;;
    *)
        echo "usage: faceauth-setup configure|unconfigure|purge" >&2
        exit 2
        ;;
esac
