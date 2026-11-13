#!/bin/sh
# deb: postinst configure <old-version>; rpm: %post <count>
set -e
case "$1" in
    configure|[0-9]*) /usr/libexec/faceauth-setup configure ;;
esac
