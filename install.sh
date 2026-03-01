#!/usr/bin/env bash
# install.sh — build and install faceauth

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"

# ---------------------------------------------------------------------------
# Detect libexec directory (varies by distro)
# ---------------------------------------------------------------------------
if [[ -d /usr/libexec ]]; then
    LIBEXEC_DIR=/usr/libexec
else
    LIBEXEC_DIR=/usr/lib
fi
export FACEAUTH_LIBEXEC_DIR="$LIBEXEC_DIR"

# ---------------------------------------------------------------------------
# Require systemd
# ---------------------------------------------------------------------------
if ! command -v systemctl &>/dev/null; then
    echo "ERROR: systemd is required but 'systemctl' was not found."
    echo "faceauth-daemon must be managed as a systemd service."
    echo "On systems without systemd, install manually:"
    echo "  - run 'faceauth-daemon' as a persistent background process"
    echo "  - ensure it starts before any PAM authentication"
    exit 1
fi

# ---------------------------------------------------------------------------
# Build (runs as the current user — no sudo needed for cargo)
# ---------------------------------------------------------------------------
echo "==> Building (cargo build --release)..."
cargo build --release --manifest-path "$SCRIPT_DIR/Cargo.toml"

# ---------------------------------------------------------------------------
# Install binaries
# ---------------------------------------------------------------------------
echo "==> Installing binaries..."
sudo install -m 755 "$SCRIPT_DIR/target/release/faceauth"           /usr/bin/faceauth
sudo install -m 755 "$SCRIPT_DIR/target/release/faceauth-daemon"    "$LIBEXEC_DIR/faceauth-daemon"
sudo install -m 755 "$SCRIPT_DIR/target/release/faceauth-notify"    "$LIBEXEC_DIR/faceauth-notify"

# Detect the correct PAM module directory
if [[ -d /usr/lib64/security ]]; then
    PAM_MOD_DIR=/usr/lib64/security
elif [[ -d /usr/lib/security ]]; then
    PAM_MOD_DIR=/usr/lib/security
else
    PAM_MOD_DIR=/lib/security
fi
sudo install -m 644 "$SCRIPT_DIR/target/release/libpam_faceauth.so" "$PAM_MOD_DIR/pam_faceauth.so"

# ---------------------------------------------------------------------------
# Create system user (idempotent)
# ---------------------------------------------------------------------------
# Detect the no-login shell (path varies by distro)
if [[ -x /usr/sbin/nologin ]]; then
    NOLOGIN_SHELL=/usr/sbin/nologin
elif [[ -x /sbin/nologin ]]; then
    NOLOGIN_SHELL=/sbin/nologin
else
    NOLOGIN_SHELL=/bin/false
fi

if ! id -u faceauthd &>/dev/null; then
    echo "==> Creating faceauthd system user..."
    sudo useradd --system --no-create-home --shell "$NOLOGIN_SHELL" faceauthd
else
    echo "==> faceauthd user already exists, skipping."
fi

# ---------------------------------------------------------------------------
# Model directory
# ---------------------------------------------------------------------------
echo "==> Setting up model directory..."
sudo mkdir -p /etc/security/faceauth
sudo chown faceauthd:faceauthd /etc/security/faceauth
sudo chmod 750 /etc/security/faceauth

# ---------------------------------------------------------------------------
# systemd service
# ---------------------------------------------------------------------------
echo "==> Installing systemd service..."
sudo install -m 644 "$SCRIPT_DIR/systemd/faceauth-daemon.service" /etc/systemd/system/
if [[ "$LIBEXEC_DIR" != /usr/libexec ]]; then
    sudo sed -i "s|/usr/libexec/|$LIBEXEC_DIR/|g" /etc/systemd/system/faceauth-daemon.service
fi
sudo systemctl daemon-reload
sudo systemctl enable faceauth-daemon
sudo systemctl restart faceauth-daemon

# ---------------------------------------------------------------------------
# Locale / translations
# ---------------------------------------------------------------------------
MSGFMT_BIN=$(command -v msgfmt 2>/dev/null || true)
if [[ -n "$MSGFMT_BIN" ]]; then
    compiled=0
    for po_file in "$SCRIPT_DIR/po"/*.po; do
        [[ -f "$po_file" ]] || continue
        lang=$(basename "$po_file" .po)
        locale_dir="/usr/share/locale/$lang/LC_MESSAGES"
        sudo mkdir -p "$locale_dir"
        sudo "$MSGFMT_BIN" -o "$locale_dir/faceauth.mo" "$po_file"
        compiled=$((compiled + 1))
    done
    if [[ $compiled -gt 0 ]]; then
        echo "==> Installed translations for $compiled language(s)."
    else
        echo "==> No .po files found in po/ — skipping translations."
    fi
else
    echo "==> msgfmt not found — skipping translations (install gettext tools to enable)."
fi

# ---------------------------------------------------------------------------
# PAM integration
# ---------------------------------------------------------------------------
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

echo ""
echo "Done. faceauth-daemon is running."
echo ""
echo "NOTE: PAM modules are loaded per-session — no restart is needed for sudo,"
echo "polkit, or SSH. To enable face authentication at the SDDM login screen,"
echo "log out and back in (or restart SDDM)."
echo ""
echo "Enroll your face with:"
echo ""
echo "    faceauth add"
