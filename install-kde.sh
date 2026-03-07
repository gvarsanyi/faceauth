#!/usr/bin/env bash

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"

# ---------------------------------------------------------------------------
# KDE Plasma lockscreen — non-interactive face auth (kde-fingerprint slot)
# ---------------------------------------------------------------------------
# kscreenlocker opens /etc/pam.d/kde-fingerprint in a background thread the
# moment the screen locks; success unlocks without the user typing anything.
# We only create the file when it does not already exist — if fprintd is
# configured it will have its own kde-fingerprint, which we leave untouched to
# avoid adding a 5-second face-auth delay in front of fingerprint auth.
if [[ -f /etc/pam.d/kde ]] || [[ -f /usr/lib/pam.d/kde ]]; then
    echo "==> Configuring KDE Plasma lockscreen (kde-fingerprint)..."
    if [[ -f /etc/pam.d/kde-fingerprint ]]; then
        echo "    (/etc/pam.d/kde-fingerprint already exists — leaving it untouched)"
        echo "    (face auth runs via the kde PAM service through common-auth)"
    else
        sudo install -m 644 "$SCRIPT_DIR/pam.d/kde-fingerprint" /etc/pam.d/kde-fingerprint
        echo "    (created /etc/pam.d/kde-fingerprint for non-interactive face unlock)"
    fi
fi
