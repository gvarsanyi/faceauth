#!/bin/bash
# End-to-end check that faceauth never uses a virtual (frame-injectable)
# camera. Runs a development daemon on a private session bus.
#
# Usage: tests/virtual-camera.sh N
#   N    index of a virtual device (/dev/videoN), for example after
#        "sudo modprobe vivid n_devs=1 vid_cap_nr=42" or
#        "sudo modprobe v4l2loopback video_nr=42"
#   BIN  directory with the built binaries (default: target/debug)

set -euo pipefail

n=${1:?usage: tests/virtual-camera.sh N}
bin=$(realpath "${BIN:-target/debug}")
[[ -e /dev/video$n ]] || { echo "/dev/video$n does not exist" >&2; exit 2; }

state=$(mktemp -d)
trap 'rm -rf "$state"' EXIT
# A model bound to the virtual device, as if it had been enrolled earlier.
printf '{"camera":"/dev/video%s","encodings":[["%s"]]}' "$n" "$(printf '3f000000%.0s' {1..128})" \
    > "$state/$(id -u).enc.json"

export n bin state
# The script below is expanded by the inner shell, inside the private bus.
# shellcheck disable=SC2016
dbus-run-session -- bash -euo pipefail -c '
    fail() { echo "FAIL: $*" >&2; exit 1; }
    "$bin/faceauth-daemon" --session-bus --state-dir "$state" 2>"$state/daemon.log" &
    export FACEAUTH_BUS=session
    for _ in $(seq 50); do "$bin/faceauth" info --json >/dev/null 2>&1 && break; sleep 0.1; done

    # 1. Not offered as a camera.
    "$bin/faceauth" cameras --json | grep -q "\"index\": *$n[,}]" && fail "/dev/video$n is listed"

    # 2. Enroll on it is refused by the daemon (called directly, bypassing CLI checks).
    out=$(busctl --user call io.github.gvarsanyi.FaceAuth1 /io/github/gvarsanyi/FaceAuth1 \
        io.github.gvarsanyi.FaceAuth1.Manager Enroll si "$(id -un)" "$n" 2>&1) && fail "Enroll accepted: $out"
    echo "$out" | grep -q "not a usable camera" || fail "unexpected Enroll error: $out"

    # 3. A model already bound to it is never used.
    out=$("$bin/faceauth" test -t 1 2>&1) && fail "test succeeded: $out"
    echo "$out" | grep -q "virtual camera" || fail "unexpected test error: $out"

    echo "PASS: /dev/video$n is refused"
'
