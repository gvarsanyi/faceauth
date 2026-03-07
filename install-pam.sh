#!/usr/bin/env bash

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"

if [[ -f /etc/pam.d/common-auth-pc ]]; then
    # openSUSE / SLES: register the module via pam-config's drop-in directory
    # (/usr/lib/pam-config.d/) and enable it with pam-config --add --faceauth.
    # This survives pam-config regeneration (e.g. via YaST or package upgrades).
    # Falls back to a direct edit of common-auth-pc on older pam-config that
    # predates pam-config.d support.
    echo "==> Configuring PAM (openSUSE pam-config)..."
    sudo mkdir -p /usr/lib/pam-config.d
    sudo install -m 644 "$SCRIPT_DIR/pam-config.d/pam_faceauth" /usr/lib/pam-config.d/pam_faceauth
    if sudo pam-config --add --faceauth 2>/dev/null; then
        echo "    (registered via pam-config)"
    elif ! grep -q 'pam_faceauth.so' /etc/pam.d/common-auth-pc; then
        echo "    (pam-config.d not supported by this pam-config version — editing common-auth-pc directly)"
        sudo sed -i '0,/^auth[[:space:]]/s/^auth[[:space:]]/# faceauth\nauth\tsufficient\tpam_faceauth.so\n&/' \
            /etc/pam.d/common-auth-pc
    else
        echo "    (pam_faceauth.so already present in common-auth-pc, skipping)"
    fi
elif command -v pam-auth-update &>/dev/null && [[ -d /usr/share/pam-configs ]]; then
    # Debian / Ubuntu: install descriptor and activate via pam-auth-update
    echo "==> Configuring PAM (Debian pam-auth-update)..."
    sudo install -m 644 "$SCRIPT_DIR/pam-configs/faceauth" /usr/share/pam-configs/faceauth
    sudo pam-auth-update --enable faceauth
else
    echo ""
    echo "NOTE: Automatic PAM configuration is not supported on this distro."
    echo "To enable face authentication, add the following line to your PAM"
    echo "service file(s) (e.g. /etc/pam.d/sudo) before the pam_unix line:"
    echo ""
    echo "    auth  sufficient  pam_faceauth.so"
fi
