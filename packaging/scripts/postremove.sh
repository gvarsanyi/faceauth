#!/bin/sh
# Delete enrolled face data on final removal.
# deb: postrm purge (kept on plain "remove"); rpm: %postun 0 (erase).
set -e
case "$1" in
    purge|0)
        rm -rf /var/lib/faceauth
        if command -v systemctl >/dev/null 2>&1; then systemctl daemon-reload || true; fi
        ;;
esac
