#!/bin/sh
# deb: prerm remove|upgrade; rpm: %preun <count> (0 = erase, >0 = upgrade)
set -e
case "$1" in
    remove|0) /usr/libexec/faceauth-setup unconfigure ;;
esac
